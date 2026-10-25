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
use wayland_client::protocol::wl_region;

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

fn host(mode: CaptureMode, modes: &mut Vec<CaptureMode>, deny: bool, rejected: &mut bool) -> Result<(), &'static str> {
    if deny && mode == CaptureMode::Locked {
        *rejected = true;
        return Err("host capture unavailable");
    }
    modes.push(mode);
    Ok(())
}

#[test]
fn relative_motion_and_constraints_follow_geometry_home_regions_and_host_focus() {
    let mut display = Display::<Server>::new().unwrap();
    let config =
        Config { bottom_output: "ClamShell-Bottom".into(), top_size: (960, 540), bottom_size: (620, 540), bottom_panel: false, touch_layout: Arc::default() };
    let mut server = Server::new(State::new(Init::default()), config, display.handle(), true, || {}, |_| Process::default(), |_| {}).unwrap();
    server.core.handle(Event::Start);
    server.frame();
    let (server_socket, client_socket) = UnixStream::pair().unwrap();
    display.handle().insert_client(server_socket, Arc::new(ClientState::default())).unwrap();
    let (report, reports) = mpsc::channel();
    let (next, steps) = mpsc::channel();
    let thread = std::thread::spawn(move || {
        let connection = Connection::from_socket(client_socket).unwrap();
        let (globals, mut queue) = registry_queue_init::<Client>(&connection).unwrap();
        let qh = queue.handle();
        let compositor: wl_compositor::WlCompositor = globals.bind(&qh, 1..=6, ()).unwrap();
        let shell: xdg_wm_base::XdgWmBase = globals.bind(&qh, 1..=6, ()).unwrap();
        let shm: wl_shm::WlShm = globals.bind(&qh, 1..=1, ()).unwrap();
        let seat: wl_seat::WlSeat = globals.bind(&qh, 1..=7, ()).unwrap();
        let pointer = seat.get_pointer(&qh, ());
        let constraints: zwp_pointer_constraints_v1::ZwpPointerConstraintsV1 = globals.bind(&qh, 1..=1, ()).unwrap();
        let relative: zwp_relative_pointer_manager_v1::ZwpRelativePointerManagerV1 = globals.bind(&qh, 1..=1, ()).unwrap();
        let _relative = relative.get_relative_pointer(&pointer, &qh, ());
        let surface = compositor.create_surface(&qh, ());
        let xdg = shell.get_xdg_surface(&surface, &qh, ());
        let top = xdg.get_toplevel(&qh, ());
        top.set_app_id("pointer-test".into());
        surface.commit();
        let mut client = Client::default();
        queue.roundtrip(&mut client).unwrap();
        let mut file = tempfile::tempfile().unwrap();
        file.write_all(&vec![0xff; 1280 * 720 * 4]).unwrap();
        let pool = shm.create_pool(file.as_fd(), 1280 * 720 * 4, &qh, ());
        let buffer = pool.create_buffer(0, 1280, 720, 1280 * 4, wl_shm::Format::Abgr8888, &qh, ());
        surface.attach(Some(&buffer), 0, 0);
        surface.commit();
        queue.roundtrip(&mut client).unwrap();
        report.send(0).unwrap();
        steps.recv_timeout(Duration::from_secs(5)).unwrap();
        let lock = constraints.lock_pointer(&surface, &pointer, None, zwp_pointer_constraints_v1::Lifetime::Persistent, &qh, ());
        queue.roundtrip(&mut client).unwrap();
        queue.roundtrip(&mut client).unwrap();
        assert_eq!(client.constraints, ["locked"]);
        client.relative.clear();
        report.send(1).unwrap();
        steps.recv_timeout(Duration::from_secs(5)).unwrap();
        queue.roundtrip(&mut client).unwrap();
        assert_eq!(client.relative, vec![(12.0, -6.0, 6.0, -3.0); 100]);
        surface.attach(None, 0, 0);
        surface.commit();
        queue.roundtrip(&mut client).unwrap();
        queue.roundtrip(&mut client).unwrap();
        assert_eq!(client.constraints.last(), Some(&"unlocked"));
        report.send(2).unwrap();
        steps.recv_timeout(Duration::from_secs(5)).unwrap();
        top.set_app_id("pointer-test".into());
        surface.commit();
        queue.roundtrip(&mut client).unwrap();
        surface.attach(Some(&buffer), 0, 0);
        surface.commit();
        queue.roundtrip(&mut client).unwrap();
        report.send(3).unwrap();
        steps.recv_timeout(Duration::from_secs(5)).unwrap();
        queue.roundtrip(&mut client).unwrap();
        queue.roundtrip(&mut client).unwrap();
        assert_eq!(client.constraints.last(), Some(&"locked"), "persistent capture can resume after remapping");
        lock.destroy();
        queue.roundtrip(&mut client).unwrap();
        report.send(4).unwrap();
        steps.recv_timeout(Duration::from_secs(5)).unwrap();

        let region = compositor.create_region(&qh, ());
        region.add(0, 0, 500, 400);
        let confine = constraints.confine_pointer(&surface, &pointer, Some(&region), zwp_pointer_constraints_v1::Lifetime::Persistent, &qh, ());
        queue.roundtrip(&mut client).unwrap();
        queue.roundtrip(&mut client).unwrap();
        report.send(5).unwrap();
        steps.recv_timeout(Duration::from_secs(5)).unwrap();
        queue.roundtrip(&mut client).unwrap();
        assert_eq!(client.constraints, ["locked", "unlocked", "locked", "confined", "unconfined"]);
        report.send(6).unwrap();
        steps.recv_timeout(Duration::from_secs(5)).unwrap();
        queue.roundtrip(&mut client).unwrap();
        queue.roundtrip(&mut client).unwrap();
        assert_eq!(client.constraints.last(), Some(&"confined"));
        region.subtract(0, 0, 500, 400);
        confine.set_region(Some(&region));
        surface.commit();
        queue.roundtrip(&mut client).unwrap();
        queue.roundtrip(&mut client).unwrap();
        assert_eq!(client.constraints.last(), Some(&"unconfined"));
        report.send(7).unwrap();
        steps.recv_timeout(Duration::from_secs(5)).unwrap();
        confine.destroy();
        let once = constraints.lock_pointer(&surface, &pointer, None, zwp_pointer_constraints_v1::Lifetime::Oneshot, &qh, ());
        queue.roundtrip(&mut client).unwrap();
        queue.roundtrip(&mut client).unwrap();
        report.send(8).unwrap();
        steps.recv_timeout(Duration::from_secs(5)).unwrap();
        queue.roundtrip(&mut client).unwrap();
        assert_eq!(client.constraints.last(), Some(&"unlocked"));
        report.send(9).unwrap();
        steps.recv_timeout(Duration::from_secs(5)).unwrap();
        queue.roundtrip(&mut client).unwrap();
        assert_eq!(client.constraints.last(), Some(&"unlocked"), "oneshot does not reactivate");
        once.destroy();
        let _rejected = constraints.lock_pointer(&surface, &pointer, None, zwp_pointer_constraints_v1::Lifetime::Persistent, &qh, ());
        assert!(queue.roundtrip(&mut client).is_err(), "host capture failure disconnects its requesting client");
    });
    let until = Instant::now() + Duration::from_secs(15);
    let mut phases = 0;
    let mut modes = Vec::new();
    let mut deny = false;
    let mut rejected = false;
    while !thread.is_finished() && Instant::now() < until {
        display.dispatch_clients(&mut server).unwrap();
        server.frame();
        server.sync_pointer_capture(|mode| host(mode, &mut modes, deny, &mut rejected));
        display.flush_clients().unwrap();
        if let Ok(phase) = reports.try_recv() {
            let id = server.windows[0].id;
            match phase {
                0 => {
                    assert_eq!(server.geometry(id).unwrap().scale, smithay::utils::Scale::from(0.75));
                    server.pointer_absolute((290.0, 175.0).into(), InputTime::now());
                    server.pointer_relative((10.0, 5.0).into(), (8.0, 4.0).into(), InputTime::now());
                    assert_eq!(server.pointer, (300.0, 180.0).into(), "relative-only devices position the free cursor");
                }
                1 => {
                    assert!(server.pointer_locked());
                    let point = server.pointer;
                    server.pointer_absolute((1500.0, 400.0).into(), InputTime::now());
                    assert_eq!(server.pointer, point);
                    let revision = server.core.state_revision();
                    for _ in 0..100 {
                        server.pointer_relative((9.0, -4.5).into(), (4.5, -2.25).into(), InputTime::now());
                    }
                    assert_eq!(server.core.state_revision(), revision, "camera motion never applies state or records UI screens");
                    assert_eq!(server.pointer, point);
                    assert_eq!(server.effective_cursor(), smithay::input::pointer::CursorImageStatus::Hidden);
                }
                2 => {
                    assert_eq!(server.host_capture, CaptureMode::Free);
                    assert!(server.capture.is_none());
                    assert!(server.core.state().session().window(id).is_none());
                }
                3 => {
                    assert!(server.core.state().session().window(id).is_some());
                    server.pointer_absolute((300.0, 180.0).into(), InputTime::now());
                }
                4 => assert_eq!(server.host_capture, CaptureMode::Free),
                5 => {
                    assert_eq!(server.host_capture, CaptureMode::Confined);
                    let point = server.pointer;
                    server.pointer_absolute((900.0, 510.0).into(), InputTime::now());
                    assert_eq!(server.pointer, point);
                    server.pointer_relative((20.0, 10.0).into(), (16.0, 8.0).into(), InputTime::now());
                    assert_eq!(server.pointer, (320.0, 190.0).into());
                    server.core.handle(Event::Home);
                    server.frame();
                    assert!(server.capture.is_none());
                    assert_eq!(server.core.state().session().keyboard_focus(), None);
                }
                6 => {
                    assert_eq!(server.host_capture, CaptureMode::Free);
                    server.core.handle(clamshell_state::WindowEvent::Activate(id).into());
                    server.frame();
                    server.pointer_absolute((300.0, 180.0).into(), InputTime::now());
                }
                7 => assert_eq!(server.host_capture, CaptureMode::Free),
                8 => {
                    assert!(server.pointer_locked());
                    server.host_focused = false;
                }
                9 => {
                    assert_eq!(server.host_capture, CaptureMode::Free);
                    server.host_focused = true;
                    deny = true;
                }
                _ => panic!("unexpected phase"),
            }
            server.sync_pointer_capture(|mode| host(mode, &mut modes, deny, &mut rejected));
            phases += 1;
            next.send(()).unwrap();
        }
        std::thread::sleep(Duration::from_millis(1));
    }
    assert!(
        thread.is_finished(),
        "pointer client timed out: phases={phases}, rejected={rejected}, mode={:?}, pointer={:?}, focus={:?}, model={:?}",
        server.host_capture,
        server.pointer,
        server.seat.get_pointer().unwrap().current_focus(),
        server.core.state().session().keyboard_focus()
    );
    thread.join().unwrap();
    assert_eq!(phases, 10);
    assert!(rejected);
    assert!(!server.exit, "failure to capture a client does not end the session");
    assert_eq!(server.host_capture, CaptureMode::Free);
    server.core.handle(Event::Home);
    server.frame();
    assert_eq!(server.core.state().session().keyboard_focus(), None);
    // Each release must reach the host before a subsequent client capture can activate.
    assert_eq!(
        modes,
        [
            CaptureMode::Locked,
            CaptureMode::Free,
            CaptureMode::Locked,
            CaptureMode::Free,
            CaptureMode::Confined,
            CaptureMode::Free,
            CaptureMode::Confined,
            CaptureMode::Free,
            CaptureMode::Locked,
            CaptureMode::Free
        ]
    );
}

#[test]
fn a_host_that_cannot_release_capture_ends_the_unusable_session() {
    let display = Display::<Server>::new().unwrap();
    let config =
        Config { bottom_output: "ClamShell-Bottom".into(), top_size: (960, 540), bottom_size: (620, 540), bottom_panel: false, touch_layout: Arc::default() };
    let mut server = Server::new(State::new(Init::default()), config, display.handle(), true, || {}, |_| Process::default(), |_| {}).unwrap();
    // The client capture ended, but releasing the last applied host capture fails.
    server.host_capture = CaptureMode::Locked;
    let mut attempts = Vec::new();
    server.sync_pointer_capture(|mode| {
        attempts.push(mode);
        Err("host window lost")
    });
    assert_eq!(attempts, [CaptureMode::Free]);
    assert!(server.exit, "the user must not remain trapped in an unusable host window");
    assert!(server.capture.is_none());
}
