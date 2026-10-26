//! Real pointer constraints preserve input geometry and release on Home or lost host focus.

use super::*;
use crate::pointer::CaptureMode;
use smithay::{
    backend::input::InputTime,
    reexports::wayland_protocols::wp::{
        pointer_constraints::zv1::client::{zwp_confined_pointer_v1, zwp_locked_pointer_v1, zwp_pointer_constraints_v1},
        relative_pointer::zv1::client::{zwp_relative_pointer_manager_v1, zwp_relative_pointer_v1},
    },
};
use wayland_client::protocol::{wl_pointer, wl_region};

delegate_noop!(Client: ignore zwp_pointer_constraints_v1::ZwpPointerConstraintsV1);
delegate_noop!(Client: ignore zwp_relative_pointer_manager_v1::ZwpRelativePointerManagerV1);
delegate_noop!(Client: ignore wl_region::WlRegion);
impl Dispatch<zwp_relative_pointer_v1::ZwpRelativePointerV1, ()> for Client {
    fn event(
        state: &mut Self,
        _: &zwp_relative_pointer_v1::ZwpRelativePointerV1,
        event: zwp_relative_pointer_v1::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        if let zwp_relative_pointer_v1::Event::RelativeMotion { dx, dy, dx_unaccel, dy_unaccel, .. } = event {
            state.relative.push((dx, dy, dx_unaccel, dy_unaccel));
        }
    }
}
impl Dispatch<zwp_locked_pointer_v1::ZwpLockedPointerV1, ()> for Client {
    fn event(
        state: &mut Self,
        _: &zwp_locked_pointer_v1::ZwpLockedPointerV1,
        event: zwp_locked_pointer_v1::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        state.constraints.push(match event {
            zwp_locked_pointer_v1::Event::Locked => "locked",
            zwp_locked_pointer_v1::Event::Unlocked => "unlocked",
            _ => return,
        });
    }
}
impl Dispatch<zwp_confined_pointer_v1::ZwpConfinedPointerV1, ()> for Client {
    fn event(
        state: &mut Self,
        _: &zwp_confined_pointer_v1::ZwpConfinedPointerV1,
        event: zwp_confined_pointer_v1::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        state.constraints.push(match event {
            zwp_confined_pointer_v1::Event::Confined => "confined",
            zwp_confined_pointer_v1::Event::Unconfined => "unconfined",
            _ => return,
        });
    }
}

/// The host's pointer capture: the modes it was asked for, and whether it refuses to lock (`deny`) and has
/// refused (`rejected`).
#[derive(Default)]
struct Host {
    modes: Vec<CaptureMode>,
    deny: bool,
    rejected: bool,
}

impl Host {
    fn capture(&mut self, mode: CaptureMode) -> Result<(), &'static str> {
        if self.deny && mode == CaptureMode::Locked {
            self.rejected = true;
            return Err("host capture unavailable");
        }
        self.modes.push(mode);
        Ok(())
    }
}

/// A client's 1280×720 window under the pointer, with pointer constraints and relative motion.
struct PointerClient {
    window: Toplevel,
    pointer: wl_pointer::WlPointer,
    constraints: zwp_pointer_constraints_v1::ZwpPointerConstraintsV1,
    compositor: wl_compositor::WlCompositor,
    client: Client,
}

impl PointerClient {
    /// Opens the window, and waits while the server moves the pointer over it ([`Step::Shown`]).
    fn new(globals: wayland_client::globals::GlobalList, queue: &mut EventQueue<Client>, steps: &Steps<Step>) -> PointerClient {
        let qh = queue.handle();
        let compositor: wl_compositor::WlCompositor = globals.bind(&qh, 1..=6, ()).unwrap();
        let seat: wl_seat::WlSeat = globals.bind(&qh, 1..=7, ()).unwrap();
        let pointer = seat.get_pointer(&qh, ());
        let constraints: zwp_pointer_constraints_v1::ZwpPointerConstraintsV1 = globals.bind(&qh, 1..=1, ()).unwrap();
        let relative: zwp_relative_pointer_manager_v1::ZwpRelativePointerManagerV1 = globals.bind(&qh, 1..=1, ()).unwrap();
        let _relative = relative.get_relative_pointer(&pointer, &qh, ());
        let mut client = Client::default();
        let window = open_toplevel(&globals, queue, &mut client, "pointer-test", Some((1280, 720)));
        steps.step(Step::Shown);
        PointerClient { window, pointer, constraints, compositor, client }
    }

    /// Asks for the pointer locked (or `confined` to `region`) for `lifetime`, and waits for the answer.
    fn capture(
        &mut self,
        queue: &mut EventQueue<Client>,
        confined: Option<&wl_region::WlRegion>,
        lifetime: zwp_pointer_constraints_v1::Lifetime,
    ) -> Result<(), wayland_client::DispatchError> {
        let qh = queue.handle();
        match confined {
            Some(region) => drop(self.constraints.confine_pointer(&self.window.surface, &self.pointer, Some(region), lifetime, &qh, ())),
            None => drop(self.constraints.lock_pointer(&self.window.surface, &self.pointer, None, lifetime, &qh, ())),
        }
        self.roundtrips(queue)
    }

    /// Two round trips: the server's turn after a request, and its answer.
    fn roundtrips(&mut self, queue: &mut EventQueue<Client>) -> Result<(), wayland_client::DispatchError> {
        queue.roundtrip(&mut self.client)?;
        queue.roundtrip(&mut self.client).map(|_| ())
    }
}

/// What the client did, for the server to check.
#[derive(Debug, PartialEq)]
enum Step {
    /// Showed its window: the server moves the pointer there, absolutely and then relatively.
    Shown,
    /// Was given the pointer captured.
    Captured,
    /// Unmapped its window.
    Unmapped,
    /// Showed its window again.
    Remapped,
    /// Destroyed its capture, or emptied its region.
    Released,
    /// Waits for the host to refuse captures.
    Deny,
    /// Waits to be given the keyboard again, with the pointer over its window.
    Refocus,
}

/// What the host was asked for and refused, and the server's part of each turn: a frame, and the
/// pointer's capture asked of the host.
fn turn(host: &std::cell::RefCell<Host>) -> impl FnMut(&mut Server) + '_ {
    |server| {
        server.frame();
        server.sync_pointer_capture(|mode| host.borrow_mut().capture(mode));
    }
}

/// At [`Step::Shown`], moves the pointer over the window: to (290, 175), and by (10, 5) as a relative-only
/// device does.
fn shown(server: &mut Server, step: &Step) {
    if *step == Step::Shown {
        server.pointer_absolute((290.0, 175.0).into(), InputTime::now());
        server.pointer_relative((10.0, 5.0).into(), (8.0, 4.0).into(), InputTime::now());
    }
}

#[test]
fn a_relative_only_device_moves_the_free_cursor() {
    let (mut display, mut server) = ServerBuilder::new().relative_motion().started();
    let client = spawn_client(&mut display, ClientState::default(), move |globals, mut queue, steps| {
        PointerClient::new(globals, &mut queue, &steps);
    });
    let host = std::cell::RefCell::new(Host::default());
    let checked = client.serve(&mut display, &mut server, turn(&host), |_, server, step| {
        assert_eq!(server.geometry(server.windows[0].id).unwrap().scale, smithay::utils::Scale::from(0.75));
        shown(server, &step);
        assert_eq!(server.pointer.location, (300.0, 180.0).into(), "relative-only devices position the free cursor");
    });
    assert_eq!(checked, 1);
    assert!(host.into_inner().modes.is_empty());
}

#[test]
fn a_locked_pointer_stays_put_and_its_motion_goes_to_the_window() {
    let (mut display, mut server) = ServerBuilder::new().relative_motion().started();
    let client = spawn_client(&mut display, ClientState::default(), move |globals, mut queue, steps| {
        let mut pointer = PointerClient::new(globals, &mut queue, &steps);
        pointer.capture(&mut queue, None, zwp_pointer_constraints_v1::Lifetime::Persistent).unwrap();
        assert_eq!(pointer.client.constraints, ["locked"]);
        pointer.client.relative.clear();
        steps.step(Step::Captured);
        queue.roundtrip(&mut pointer.client).unwrap();
        assert_eq!(pointer.client.relative, vec![(12.0, -6.0, 6.0, -3.0); 100]);
    });
    let host = std::cell::RefCell::new(Host::default());
    let checked = client.serve(&mut display, &mut server, turn(&host), |_, server, step| {
        shown(server, &step);
        if step == Step::Captured {
            assert!(server.pointer_locked());
            let point = server.pointer.location;
            server.pointer_absolute((1500.0, 400.0).into(), InputTime::now());
            assert_eq!(server.pointer.location, point);
            let revision = server.core.state_revision();
            for _ in 0..100 {
                server.pointer_relative((9.0, -4.5).into(), (4.5, -2.25).into(), InputTime::now());
            }
            assert_eq!(server.core.state_revision(), revision, "camera motion never applies state or records UI screens");
            assert_eq!(server.pointer.location, point);
            assert_eq!(server.effective_cursor(), smithay::input::pointer::CursorImageStatus::Hidden);
        }
        server.sync_pointer_capture(|mode| host.borrow_mut().capture(mode));
    });
    assert_eq!(checked, 2);
    assert_eq!(host.into_inner().modes, [CaptureMode::Locked]);
}

#[test]
fn unmapping_ends_a_lock_and_a_persistent_lock_resumes_once_mapped_again() {
    let (mut display, mut server) = ServerBuilder::new().relative_motion().started();
    let client = spawn_client(&mut display, ClientState::default(), move |globals, mut queue, steps| {
        let mut pointer = PointerClient::new(globals, &mut queue, &steps);
        pointer.capture(&mut queue, None, zwp_pointer_constraints_v1::Lifetime::Persistent).unwrap();
        steps.step(Step::Captured);
        pointer.window.unmap(&mut queue, &mut pointer.client);
        queue.roundtrip(&mut pointer.client).unwrap();
        assert_eq!(pointer.client.constraints.last(), Some(&"unlocked"));
        steps.step(Step::Unmapped);
        pointer.window.remap(&mut queue, &mut pointer.client);
        steps.step(Step::Remapped);
        pointer.roundtrips(&mut queue).unwrap();
        assert_eq!(pointer.client.constraints.last(), Some(&"locked"), "persistent capture can resume after remapping");
    });
    let host = std::cell::RefCell::new(Host::default());
    let checked = client.serve(&mut display, &mut server, turn(&host), |_, server, step| {
        let id = server.windows[0].id;
        shown(server, &step);
        match step {
            Step::Unmapped => {
                assert_eq!(server.pointer.host_capture, CaptureMode::Free);
                assert!(server.pointer.capture.is_none());
                assert!(server.core.state().session().window(id).is_none());
            }
            Step::Remapped => {
                assert!(server.core.state().session().window(id).is_some());
                server.pointer_absolute((300.0, 180.0).into(), InputTime::now());
            }
            _ => {}
        }
        server.sync_pointer_capture(|mode| host.borrow_mut().capture(mode));
    });
    assert_eq!(checked, 4);
    // Each release reaches the host before a later capture.
    assert_eq!(host.into_inner().modes, [CaptureMode::Locked, CaptureMode::Free, CaptureMode::Locked]);
}

#[test]
fn a_lock_destroyed_frees_the_hosts_pointer() {
    let (mut display, mut server) = ServerBuilder::new().relative_motion().started();
    let client = spawn_client(&mut display, ClientState::default(), move |globals, mut queue, steps| {
        let mut pointer = PointerClient::new(globals, &mut queue, &steps);
        let lock = pointer.constraints.lock_pointer(
            &pointer.window.surface,
            &pointer.pointer,
            None,
            zwp_pointer_constraints_v1::Lifetime::Persistent,
            &queue.handle(),
            (),
        );
        pointer.roundtrips(&mut queue).unwrap();
        steps.step(Step::Captured);
        lock.destroy();
        queue.roundtrip(&mut pointer.client).unwrap();
        steps.step(Step::Released);
    });
    let host = std::cell::RefCell::new(Host::default());
    let checked = client.serve(&mut display, &mut server, turn(&host), |_, server, step| {
        shown(server, &step);
        match step {
            Step::Captured => assert_eq!(server.pointer.host_capture, CaptureMode::Locked),
            Step::Released => assert_eq!(server.pointer.host_capture, CaptureMode::Free),
            _ => {}
        }
        server.sync_pointer_capture(|mode| host.borrow_mut().capture(mode));
    });
    assert_eq!(checked, 3);
    assert_eq!(host.into_inner().modes, [CaptureMode::Locked, CaptureMode::Free]);
}

#[test]
fn a_confined_pointer_stays_in_its_region_until_home() {
    let (mut display, mut server) = ServerBuilder::new().relative_motion().started();
    let client = spawn_client(&mut display, ClientState::default(), move |globals, mut queue, steps| {
        let mut pointer = PointerClient::new(globals, &mut queue, &steps);
        let region = pointer.compositor.create_region(&queue.handle(), ());
        region.add(0, 0, 500, 400);
        pointer.capture(&mut queue, Some(&region), zwp_pointer_constraints_v1::Lifetime::Persistent).unwrap();
        steps.step(Step::Captured);
        queue.roundtrip(&mut pointer.client).unwrap();
        assert_eq!(pointer.client.constraints, ["confined", "unconfined"]);
    });
    let host = std::cell::RefCell::new(Host::default());
    let checked = client.serve(&mut display, &mut server, turn(&host), |_, server, step| {
        shown(server, &step);
        if step == Step::Captured {
            assert_eq!(server.pointer.host_capture, CaptureMode::Confined);
            let point = server.pointer.location;
            server.pointer_absolute((900.0, 510.0).into(), InputTime::now());
            assert_eq!(server.pointer.location, point);
            server.pointer_relative((20.0, 10.0).into(), (16.0, 8.0).into(), InputTime::now());
            assert_eq!(server.pointer.location, (320.0, 190.0).into());
            server.core.handle(Event::Home);
            server.frame();
            assert!(server.pointer.capture.is_none());
            assert_eq!(server.core.state().session().keyboard_focus(), None);
        }
        server.sync_pointer_capture(|mode| host.borrow_mut().capture(mode));
    });
    assert_eq!(checked, 2);
    assert_eq!(host.into_inner().modes, [CaptureMode::Confined, CaptureMode::Free]);
}

#[test]
fn a_confinement_resumes_with_the_keyboard_and_ends_when_its_region_leaves_the_pointer() {
    let (mut display, mut server) = ServerBuilder::new().relative_motion().started();
    let client = spawn_client(&mut display, ClientState::default(), move |globals, mut queue, steps| {
        let mut pointer = PointerClient::new(globals, &mut queue, &steps);
        let region = pointer.compositor.create_region(&queue.handle(), ());
        region.add(0, 0, 500, 400);
        let confine = pointer.constraints.confine_pointer(
            &pointer.window.surface,
            &pointer.pointer,
            Some(&region),
            zwp_pointer_constraints_v1::Lifetime::Persistent,
            &queue.handle(),
            (),
        );
        pointer.roundtrips(&mut queue).unwrap();
        steps.step(Step::Captured);
        queue.roundtrip(&mut pointer.client).unwrap();
        assert_eq!(pointer.client.constraints, ["confined", "unconfined"]);
        steps.step(Step::Refocus);
        pointer.roundtrips(&mut queue).unwrap();
        assert_eq!(pointer.client.constraints.last(), Some(&"confined"));
        region.subtract(0, 0, 500, 400);
        confine.set_region(Some(&region));
        pointer.window.surface.commit();
        pointer.roundtrips(&mut queue).unwrap();
        assert_eq!(pointer.client.constraints.last(), Some(&"unconfined"));
        steps.step(Step::Released);
    });
    let host = std::cell::RefCell::new(Host::default());
    let checked = client.serve(&mut display, &mut server, turn(&host), |_, server, step| {
        let id = server.windows[0].id;
        shown(server, &step);
        match step {
            Step::Captured => {
                server.core.handle(Event::Home);
                server.frame();
            }
            Step::Refocus => {
                assert_eq!(server.pointer.host_capture, CaptureMode::Free);
                server.core.handle(clamshell_state::WindowEvent::Activate(id).into());
                server.frame();
                server.pointer_absolute((300.0, 180.0).into(), InputTime::now());
            }
            Step::Released => assert_eq!(server.pointer.host_capture, CaptureMode::Free),
            _ => {}
        }
        server.sync_pointer_capture(|mode| host.borrow_mut().capture(mode));
    });
    assert_eq!(checked, 4);
    assert_eq!(host.into_inner().modes, [CaptureMode::Confined, CaptureMode::Free, CaptureMode::Confined, CaptureMode::Free]);
}

#[test]
fn a_oneshot_lock_ends_with_the_hosts_focus_and_does_not_come_back() {
    let (mut display, mut server) = ServerBuilder::new().relative_motion().started();
    let client = spawn_client(&mut display, ClientState::default(), move |globals, mut queue, steps| {
        let mut pointer = PointerClient::new(globals, &mut queue, &steps);
        pointer.capture(&mut queue, None, zwp_pointer_constraints_v1::Lifetime::Oneshot).unwrap();
        steps.step(Step::Captured);
        queue.roundtrip(&mut pointer.client).unwrap();
        assert_eq!(pointer.client.constraints.last(), Some(&"unlocked"));
        steps.step(Step::Released);
        queue.roundtrip(&mut pointer.client).unwrap();
        assert_eq!(pointer.client.constraints.last(), Some(&"unlocked"), "oneshot does not reactivate");
    });
    let host = std::cell::RefCell::new(Host::default());
    let checked = client.serve(&mut display, &mut server, turn(&host), |_, server, step| {
        shown(server, &step);
        match step {
            Step::Captured => {
                assert!(server.pointer_locked());
                server.pointer.host_focused = false;
            }
            Step::Released => {
                assert_eq!(server.pointer.host_capture, CaptureMode::Free);
                server.pointer.host_focused = true;
            }
            _ => {}
        }
        server.sync_pointer_capture(|mode| host.borrow_mut().capture(mode));
    });
    assert_eq!(checked, 3);
    assert_eq!(host.into_inner().modes, [CaptureMode::Locked, CaptureMode::Free]);
}

#[test]
fn a_capture_the_host_refuses_disconnects_its_client_not_the_session() {
    let (mut display, mut server) = ServerBuilder::new().relative_motion().started();
    let client = spawn_client(&mut display, ClientState::default(), move |globals, mut queue, steps| {
        let mut pointer = PointerClient::new(globals, &mut queue, &steps);
        steps.step(Step::Deny);
        let captured = pointer.capture(&mut queue, None, zwp_pointer_constraints_v1::Lifetime::Persistent);
        assert!(captured.is_err(), "host capture failure disconnects its requesting client");
    });
    let host = std::cell::RefCell::new(Host::default());
    let checked = client.serve(&mut display, &mut server, turn(&host), |_, server, step| {
        shown(server, &step);
        if step == Step::Deny {
            host.borrow_mut().deny = true;
        }
    });
    assert_eq!(checked, 2);
    assert!(host.into_inner().rejected);
    assert!(!server.exit, "failure to capture a client does not end the session");
    assert_eq!(server.pointer.host_capture, CaptureMode::Free);
    server.core.handle(Event::Home);
    server.frame();
    assert_eq!(server.core.state().session().keyboard_focus(), None);
}

#[test]
fn a_host_that_cannot_release_capture_ends_the_unusable_session() {
    let (_display, mut server) = ServerBuilder::new().relative_motion().build();
    // The client capture ended, but releasing the last applied host capture fails.
    server.pointer.host_capture = CaptureMode::Locked;
    let mut attempts = Vec::new();
    server.sync_pointer_capture(|mode| {
        attempts.push(mode);
        Err("host window lost")
    });
    assert_eq!(attempts, [CaptureMode::Free]);
    assert!(server.exit, "the user must not remain trapped in an unusable host window");
    assert!(server.pointer.capture.is_none());
}
