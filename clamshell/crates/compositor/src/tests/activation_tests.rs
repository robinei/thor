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

#[test]
fn activation_tokens_require_focus_expire_are_single_use_and_cannot_undo_home() {
    let (mut display, mut server) = ServerBuilder::new().started();
    let (bad_server, bad_client) = UnixStream::pair().unwrap();
    display.handle().insert_client(bad_server, Arc::new(ClientState::default())).unwrap();
    let client = spawn_client(&mut display, ClientState::default(), move |globals, mut queue, steps| {
        let qh = queue.handle();
        let compositor: wl_compositor::WlCompositor = globals.bind(&qh, 1..=6, ()).unwrap();
        let shell: xdg_wm_base::XdgWmBase = globals.bind(&qh, 1..=6, ()).unwrap();
        let shm: wl_shm::WlShm = globals.bind(&qh, 1..=1, ()).unwrap();
        let seat: wl_seat::WlSeat = globals.bind(&qh, 1..=7, ()).unwrap();
        let _keyboard = seat.get_keyboard(&qh, ());
        let activation: xdg_activation_v1::XdgActivationV1 = globals.bind(&qh, 1..=1, ()).unwrap();
        let mut client = Client::default();
        let mut file = tempfile::tempfile().unwrap();
        file.write_all(&vec![0xff; 960 * 540 * 4]).unwrap();
        let pool = shm.create_pool(file.as_fd(), 960 * 540 * 4, &qh, ());
        let buffer = pool.create_buffer(0, 960, 540, 960 * 4, wl_shm::Format::Abgr8888, &qh, ());
        let mut surfaces = Vec::new();
        for app_id in ["origin", "target"] {
            let surface = compositor.create_surface(&qh, ());
            let xdg = shell.get_xdg_surface(&surface, &qh, ());
            let top = xdg.get_toplevel(&qh, ());
            top.set_app_id(app_id.into());
            surface.commit();
            queue.roundtrip(&mut client).unwrap();
            surface.attach(Some(&buffer), 0, 0);
            surface.commit();
            queue.roundtrip(&mut client).unwrap();
            surfaces.push((surface, xdg, top));
        }
        steps.step(0);
        queue.roundtrip(&mut client).unwrap();
        let origin = &surfaces[0].0;
        let target = &surfaces[1].0;
        assert_ne!(client.keyboard_enter, 0);
        for (phase, serial) in [(1, None), (2, Some(client.keyboard_enter.wrapping_sub(1)))] {
            let value = token(&activation, &seat, Some(origin), &mut queue, &mut client, serial);
            activation.activate(value, target);
            queue.roundtrip(&mut client).unwrap();
            steps.step(phase);
        }
        let bad = Connection::from_socket(bad_client).unwrap();
        let (bad_globals, mut bad_queue) = registry_queue_init::<Client>(&bad).unwrap();
        let bad_qh = bad_queue.handle();
        let bad_activation: xdg_activation_v1::XdgActivationV1 = bad_globals.bind(&bad_qh, 1..=1, ()).unwrap();
        let bad_seat: wl_seat::WlSeat = bad_globals.bind(&bad_qh, 1..=7, ()).unwrap();
        // A different connection cannot borrow the focused client's input serial.
        let _ = token(&bad_activation, &bad_seat, None, &mut bad_queue, &mut Client::default(), Some(client.keyboard_enter));
        steps.step(3);

        let serial = client.keyboard_enter;
        let value = token(&activation, &seat, Some(origin), &mut queue, &mut client, Some(serial));
        steps.step(4);
        activation.activate(value, target);
        queue.roundtrip(&mut client).unwrap();
        steps.step(5);
        queue.roundtrip(&mut client).unwrap();

        let serial = client.keyboard_enter;
        let value = token(&activation, &seat, Some(origin), &mut queue, &mut client, Some(serial));
        activation.activate(value.clone(), target);
        queue.roundtrip(&mut client).unwrap();
        steps.step(6);
        activation.activate(value, target);
        queue.roundtrip(&mut client).unwrap();
        steps.step(7);
        queue.roundtrip(&mut client).unwrap();

        let serial = client.keyboard_enter;
        let value = token(&activation, &seat, Some(origin), &mut queue, &mut client, Some(serial));
        steps.step(8);
        activation.activate(value, target);
        queue.roundtrip(&mut client).unwrap();
        steps.step(9);

        let serial = client.keyboard_enter;
        let value = token(&activation, &seat, Some(origin), &mut queue, &mut client, Some(serial));
        let unmapped = compositor.create_surface(&qh, ());
        activation.activate(value, &unmapped);
        queue.roundtrip(&mut client).unwrap();
        steps.step(10);
        let serial = client.keyboard_enter;
        let value = token(&activation, &seat, Some(origin), &mut queue, &mut client, Some(serial));
        activation.activate(value, target);
        queue.roundtrip(&mut client).unwrap();
        steps.step(11);
        queue.roundtrip(&mut client).unwrap();
        let serial = client.keyboard_enter;
        let value = token(&activation, &seat, Some(origin), &mut queue, &mut client, Some(serial));
        target.attach(None, 0, 0);
        target.commit();
        queue.roundtrip(&mut client).unwrap();
        activation.activate(value, target);
        queue.roundtrip(&mut client).unwrap();
        steps.step(12);
    });
    let checked = client.serve(&mut display, &mut server, Server::frame, |_, server, phase| {
        let origin = server.windows[0].id;
        let target = server.windows[1].id;
        match phase {
            0 => server.core.handle(clamshell_state::WindowEvent::Activate(origin).into()),
            1..=3 | 7 | 9 | 10 => {
                assert_eq!(server.core.state().session().top_window().map(|window| window.id), Some(origin));
                assert_eq!(server.core.state().session().keyboard_focus(), Some(origin));
                assert_eq!(server.protocols.activation.tokens().count(), 0);
            }
            4 => {
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
            5 => {
                assert!(server.core.state().session().top_window().is_none());
                assert_eq!(server.core.state().session().keyboard_focus(), None);
                server.core.handle(clamshell_state::WindowEvent::Activate(origin).into());
            }
            6 => {
                assert_eq!(server.core.state().session().top_window().map(|window| window.id), Some(target));
                assert_eq!(server.core.state().session().keyboard_focus(), Some(target));
                assert_eq!(server.protocols.activation.tokens().count(), 0);
                server.core.handle(Event::Device(DeviceKey::WindowDown));
                server.core.handle(clamshell_state::WindowEvent::Activate(origin).into());
            }
            8 => {
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
            11 => {
                assert_eq!(server.core.state().session().top_window().map(|window| window.id), Some(origin));
                assert_eq!(server.core.state().session().bottom_window().map(|window| window.id), Some(target));
                assert_eq!(server.core.state().session().keyboard_focus(), Some(target));
                assert!(server.core.state().session().below(target), "activation does not move a window between screens");
                assert_eq!(server.protocols.activation.tokens().count(), 0);
                server.core.handle(clamshell_state::WindowEvent::Activate(origin).into());
            }
            12 => {
                assert_eq!(server.core.state().session().keyboard_focus(), Some(origin));
                assert!(server.core.state().session().window(target).is_none());
                assert_eq!(server.protocols.activation.tokens().count(), 0);
            }
            _ => panic!("unexpected phase"),
        }
        server.frame();
    });
    assert_eq!(checked, 13);
}
