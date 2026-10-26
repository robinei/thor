//! Real activation tokens delegate focus once and cannot undo Home.

use super::*;
use smithay::reexports::wayland_protocols::xdg::activation::v1::client::{xdg_activation_token_v1, xdg_activation_v1};

delegate_noop!(Client: ignore xdg_activation_v1::XdgActivationV1);
impl Dispatch<xdg_activation_token_v1::XdgActivationTokenV1, ()> for Client {
    fn event(
        state: &mut Self,
        _: &xdg_activation_token_v1::XdgActivationTokenV1,
        event: xdg_activation_token_v1::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        if let xdg_activation_token_v1::Event::Done { token } = event {
            state.activation_tokens.push(token);
        }
    }
}

/// A client with two windows, `origin` with the keyboard and `target`, and xdg-activation.
struct Activating {
    activation: xdg_activation_v1::XdgActivationV1,
    seat: wl_seat::WlSeat,
    compositor: wl_compositor::WlCompositor,
    origin: Toplevel,
    target: Toplevel,
    client: Client,
}

impl Activating {
    /// Opens the windows, and waits while the server gives `origin` the keyboard ([`Step::Shown`]).
    fn new(globals: wayland_client::globals::GlobalList, queue: &mut EventQueue<Client>, steps: &Steps<Step>) -> Activating {
        let qh = queue.handle();
        let compositor: wl_compositor::WlCompositor = globals.bind(&qh, 1..=6, ()).unwrap();
        let seat: wl_seat::WlSeat = globals.bind(&qh, 1..=7, ()).unwrap();
        let _keyboard = seat.get_keyboard(&qh, ());
        let activation: xdg_activation_v1::XdgActivationV1 = globals.bind(&qh, 1..=1, ()).unwrap();
        let mut client = Client::default();
        let origin = open_toplevel(&globals, queue, &mut client, "origin", Some((960, 540)));
        let target = open_toplevel(&globals, queue, &mut client, "target", Some((960, 540)));
        steps.step(Step::Shown);
        queue.roundtrip(&mut client).unwrap();
        assert_ne!(client.keyboard_enter, 0);
        Activating { activation, seat, compositor, origin, target, client }
    }

    /// A token asked for from `origin`, with the input `serial`, the last keyboard entry's by default.
    fn token(&mut self, queue: &mut EventQueue<Client>, serial: Option<Option<u32>>) -> String {
        let serial = serial.unwrap_or(Some(self.client.keyboard_enter));
        token(&self.activation, &self.seat, Some(&self.origin.surface), queue, &mut self.client, serial)
    }

    /// Asks for `surface` activated with `token`.
    fn activate(&mut self, queue: &mut EventQueue<Client>, token: String, surface: &wl_surface::WlSurface) {
        self.activation.activate(token, surface);
        queue.roundtrip(&mut self.client).unwrap();
    }
}

/// What the client did, for the server to check.
#[derive(Debug, PartialEq)]
enum Step {
    /// Showed its windows: the server gives `origin` the keyboard.
    Shown,
    /// Asked for an activation that does nothing.
    Refused,
    /// Holds a token it hasn't used.
    Token,
    /// Asked for `target` activated with a good token.
    Activated,
}

fn token(
    activation: &xdg_activation_v1::XdgActivationV1,
    seat: &wl_seat::WlSeat,
    source: Option<&wl_surface::WlSurface>,
    queue: &mut wayland_client::EventQueue<Client>,
    client: &mut Client,
    serial: Option<u32>,
) -> String {
    let request = activation.get_activation_token(&queue.handle(), ());
    if let Some(serial) = serial {
        request.set_serial(serial, seat);
    }
    if let Some(source) = source {
        request.set_surface(source);
    }
    request.commit();
    queue.roundtrip(client).unwrap();
    request.destroy();
    client.activation_tokens.pop().unwrap()
}

/// The windows' ids, `origin`'s and `target`'s.
fn windows(server: &Server) -> (i64, i64) {
    (server.windows[0].id, server.windows[1].id)
}

/// What every step checks: at [`Step::Shown`] `origin` is given the keyboard, and after
/// [`Step::Refused`] it still has it and is in front, and no token is left.
fn check(server: &mut Server, step: &Step) {
    let (origin, _) = windows(server);
    match step {
        Step::Shown => server.core.handle(clamshell_state::WindowEvent::Activate(origin).into()),
        Step::Refused => {
            assert_eq!(server.core.state().session().top_window().map(|window| window.id), Some(origin));
            assert_eq!(server.core.state().session().keyboard_focus(), Some(origin));
            assert_eq!(server.protocols.activation.tokens().count(), 0);
        }
        _ => {}
    }
}

#[test]
fn a_token_needs_the_focused_clients_own_recent_input_serial() {
    let (mut display, mut server) = ServerBuilder::new().started();
    let (bad_server, bad_client) = UnixStream::pair().unwrap();
    display.handle().insert_client(bad_server, Arc::new(ClientState::default())).unwrap();
    let client = spawn_client(&mut display, ClientState::default(), move |globals, mut queue, steps| {
        let mut activating = Activating::new(globals, &mut queue, &steps);
        let target = activating.target.surface.clone();
        // None, and one older than the keyboard's entry.
        for serial in [None, Some(activating.client.keyboard_enter.wrapping_sub(1))] {
            let value = activating.token(&mut queue, Some(serial));
            activating.activate(&mut queue, value, &target);
            steps.step(Step::Refused);
        }
        let bad = Connection::from_socket(bad_client).unwrap();
        let (bad_globals, mut bad_queue) = registry_queue_init::<Client>(&bad).unwrap();
        let bad_qh = bad_queue.handle();
        let bad_activation: xdg_activation_v1::XdgActivationV1 = bad_globals.bind(&bad_qh, 1..=1, ()).unwrap();
        let bad_seat: wl_seat::WlSeat = bad_globals.bind(&bad_qh, 1..=7, ()).unwrap();
        // A different connection cannot borrow the focused client's input serial.
        let _ = token(&bad_activation, &bad_seat, None, &mut bad_queue, &mut Client::default(), Some(activating.client.keyboard_enter));
        steps.step(Step::Refused);
    });
    let checked = client.serve(&mut display, &mut server, Server::frame, |_, server, step| {
        check(server, &step);
        server.frame();
    });
    assert_eq!(checked, 4);
}

#[test]
fn home_voids_a_token_and_an_activation_in_flight() {
    let (mut display, mut server) = ServerBuilder::new().started();
    let client = spawn_client(&mut display, ClientState::default(), move |globals, mut queue, steps| {
        let mut activating = Activating::new(globals, &mut queue, &steps);
        let target = activating.target.surface.clone();
        let value = activating.token(&mut queue, None);
        steps.step(Step::Token);
        activating.activate(&mut queue, value, &target);
        steps.step(Step::Activated);
    });
    let checked = client.serve(&mut display, &mut server, Server::frame, |_, server, step| {
        check(server, &step);
        match step {
            Step::Token => {
                use smithay::wayland::xdg_activation::XdgActivationHandler;
                assert_eq!(server.protocols.activation.tokens().count(), 1);
                let (pending, data) = server.protocols.activation.tokens().next().map(|(token, data)| (token.clone(), data.clone())).unwrap();
                let late = server.protocols.activation.create_external_token(Some(data.clone())).0.clone();
                let surface = server.windows[1].window.toplevel().unwrap().wl_surface().clone();
                server.core.handle(Event::Home);
                // Home can settle before the seat is synchronised in this dispatch.
                // Neither token creation nor a pending activation may use the old seat focus.
                assert!(server.seat.get_keyboard().unwrap().current_focus().is_some());
                assert!(!server.token_created(pending, data.clone()));
                server.request_activation(late, data, surface);
                assert!(server.core.state().session().top_window().is_none());
                assert_eq!(server.core.state().session().keyboard_focus(), None);
                assert_eq!(server.protocols.activation.tokens().count(), 1);
                server.frame();
                assert_eq!(server.protocols.activation.tokens().count(), 0);
            }
            Step::Activated => {
                assert!(server.core.state().session().top_window().is_none());
                assert_eq!(server.core.state().session().keyboard_focus(), None);
            }
            _ => {}
        }
        server.frame();
    });
    assert_eq!(checked, 3);
}

#[test]
fn a_token_activates_its_window_once() {
    let (mut display, mut server) = ServerBuilder::new().started();
    let client = spawn_client(&mut display, ClientState::default(), move |globals, mut queue, steps| {
        let mut activating = Activating::new(globals, &mut queue, &steps);
        let target = activating.target.surface.clone();
        let value = activating.token(&mut queue, None);
        activating.activate(&mut queue, value.clone(), &target);
        steps.step(Step::Activated);
        activating.activate(&mut queue, value, &target);
        steps.step(Step::Refused);
    });
    let checked = client.serve(&mut display, &mut server, Server::frame, |_, server, step| {
        check(server, &step);
        let (origin, target) = windows(server);
        if step == Step::Activated {
            assert_eq!(server.core.state().session().top_window().map(|window| window.id), Some(target));
            assert_eq!(server.core.state().session().keyboard_focus(), Some(target));
            assert_eq!(server.protocols.activation.tokens().count(), 0);
            server.core.handle(clamshell_state::WindowEvent::Activate(origin).into());
        }
        server.frame();
    });
    assert_eq!(checked, 3);
}

#[test]
fn an_expired_token_is_refused_and_dropped() {
    let (mut display, mut server) = ServerBuilder::new().started();
    let client = spawn_client(&mut display, ClientState::default(), move |globals, mut queue, steps| {
        let mut activating = Activating::new(globals, &mut queue, &steps);
        let target = activating.target.surface.clone();
        let value = activating.token(&mut queue, None);
        steps.step(Step::Token);
        activating.activate(&mut queue, value, &target);
        steps.step(Step::Refused);
    });
    let checked = client.serve(&mut display, &mut server, Server::frame, |_, server, step| {
        check(server, &step);
        let (origin, _) = windows(server);
        if step == Step::Token {
            assert_eq!(server.protocols.activation.tokens().count(), 1);
            use smithay::wayland::xdg_activation::XdgActivationHandler;
            let mut data = server.protocols.activation.tokens().next().unwrap().1.clone();
            data.timestamp = Instant::now().checked_sub(Duration::from_secs(5)).unwrap();
            let expired = server.protocols.activation.create_external_token(Some(data.clone())).0.clone();
            let surface = server.windows[1].window.toplevel().unwrap().wl_surface().clone();
            server.request_activation(expired, data, surface);
            assert_eq!(server.core.state().session().keyboard_focus(), Some(origin));
            assert_eq!(server.protocols.activation.tokens().count(), 1);
            server.expire_activation_tokens(Instant::now() + Duration::from_secs(5));
            assert_eq!(server.protocols.activation.tokens().count(), 0);
        }
        server.frame();
    });
    assert_eq!(checked, 3);
}

#[test]
fn a_token_for_a_surface_of_no_window_does_nothing() {
    let (mut display, mut server) = ServerBuilder::new().started();
    let client = spawn_client(&mut display, ClientState::default(), move |globals, mut queue, steps| {
        let mut activating = Activating::new(globals, &mut queue, &steps);
        let value = activating.token(&mut queue, None);
        let unmapped = activating.compositor.create_surface(&queue.handle(), ());
        activating.activate(&mut queue, value, &unmapped);
        steps.step(Step::Refused);
    });
    let checked = client.serve(&mut display, &mut server, Server::frame, |_, server, step| {
        check(server, &step);
        server.frame();
    });
    assert_eq!(checked, 2);
}

#[test]
fn activating_a_window_below_gives_it_the_keyboard_and_leaves_it_below() {
    let (mut display, mut server) = ServerBuilder::new().started();
    let client = spawn_client(&mut display, ClientState::default(), move |globals, mut queue, steps| {
        let mut activating = Activating::new(globals, &mut queue, &steps);
        let target = activating.target.surface.clone();
        let value = activating.token(&mut queue, None);
        activating.activate(&mut queue, value, &target);
        steps.step(Step::Activated);
    });
    let checked = client.serve(&mut display, &mut server, Server::frame, |_, server, step| {
        check(server, &step);
        let (origin, target) = windows(server);
        match step {
            // `target` goes down to the second screen, and `origin` has the keyboard again.
            Step::Shown => {
                server.core.handle(clamshell_state::WindowEvent::Activate(target).into());
                server.core.handle(Event::HardwareKey(HardwareKey::WindowDown));
                server.core.handle(clamshell_state::WindowEvent::Activate(origin).into());
            }
            Step::Activated => {
                assert_eq!(server.core.state().session().top_window().map(|window| window.id), Some(origin));
                assert_eq!(server.core.state().session().bottom_window().map(|window| window.id), Some(target));
                assert_eq!(server.core.state().session().keyboard_focus(), Some(target));
                assert!(server.core.state().session().below(target), "activation does not move a window between screens");
                assert_eq!(server.protocols.activation.tokens().count(), 0);
            }
            _ => {}
        }
        server.frame();
    });
    assert_eq!(checked, 2);
}

#[test]
fn a_token_for_a_window_unmapped_meanwhile_does_nothing() {
    let (mut display, mut server) = ServerBuilder::new().started();
    let client = spawn_client(&mut display, ClientState::default(), move |globals, mut queue, steps| {
        let mut activating = Activating::new(globals, &mut queue, &steps);
        let target = activating.target.surface.clone();
        let value = activating.token(&mut queue, None);
        activating.target.unmap(&mut queue, &mut activating.client);
        activating.activate(&mut queue, value, &target);
        steps.step(Step::Activated);
    });
    let checked = client.serve(&mut display, &mut server, Server::frame, |_, server, step| {
        check(server, &step);
        let (origin, target) = windows(server);
        if step == Step::Activated {
            assert_eq!(server.core.state().session().keyboard_focus(), Some(origin));
            assert!(server.core.state().session().window(target).is_none());
            assert_eq!(server.protocols.activation.tokens().count(), 0);
        }
        server.frame();
    });
    assert_eq!(checked, 2);
}
