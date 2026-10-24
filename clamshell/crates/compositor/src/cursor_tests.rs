//! Real cursor requests retain hotspots and cannot hide the pointer on the shelf.

use super::*;
use smithay::{
    backend::input::InputTime,
    input::pointer::{CursorIcon, CursorImageStatus, CursorImageSurfaceData, MotionEvent},
    reexports::wayland_protocols::wp::cursor_shape::v1::client::{wp_cursor_shape_device_v1, wp_cursor_shape_manager_v1},
    utils::SERIAL_COUNTER,
    wayland::compositor::with_states,
};
use wayland_client::protocol::wl_pointer;

delegate_noop!(Client: ignore wp_cursor_shape_manager_v1::WpCursorShapeManagerV1);
delegate_noop!(Client: ignore wp_cursor_shape_device_v1::WpCursorShapeDeviceV1);

#[test]
fn cursor_requests_keep_hotspots_ignore_old_serials_and_release_on_home() {
    let mut display = Display::<Server>::new().unwrap();
    let config =
        Config { bottom_output: "ClamShell-Bottom".into(), top_size: (960, 540), bottom_size: (620, 540), bottom_panel: false, touch_layout: Arc::default() };
    let mut server = Server::new(State::new(Init::default()), config, display.handle(), || {}, |_| Process::default(), |_| {}).unwrap();
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
        let pointer: wl_pointer::WlPointer = seat.get_pointer(&qh, ());
        let manager: wp_cursor_shape_manager_v1::WpCursorShapeManagerV1 = globals.bind(&qh, 1..=2, ()).unwrap();
        let shape = manager.get_pointer(&pointer, &qh, ());
        let surface = compositor.create_surface(&qh, ());
        let xdg = shell.get_xdg_surface(&surface, &qh, ());
        let top = xdg.get_toplevel(&qh, ());
        top.set_app_id("cursor-test".into());
        surface.commit();
        let mut client = Client::default();
        queue.roundtrip(&mut client).unwrap();
        let mut file = tempfile::tempfile().unwrap();
        file.write_all(&vec![0xff; 960 * 540 * 4]).unwrap();
        let pool = shm.create_pool(file.as_fd(), 960 * 540 * 4, &qh, ());
        let buffer = pool.create_buffer(0, 960, 540, 960 * 4, wl_shm::Format::Abgr8888, &qh, ());
        surface.attach(Some(&buffer), 0, 0);
        surface.commit();
        queue.roundtrip(&mut client).unwrap();
        report.send(0).unwrap();
        steps.recv_timeout(Duration::from_secs(5)).unwrap();
        queue.roundtrip(&mut client).unwrap();
        let serial = client.cursor_enter;
        assert_ne!(serial, 0);
        shape.set_shape(serial, wp_cursor_shape_device_v1::Shape::Text);
        queue.roundtrip(&mut client).unwrap();
        report.send(1).unwrap();
        steps.recv_timeout(Duration::from_secs(5)).unwrap();
        pointer.set_cursor(serial, None, 0, 0);
        queue.roundtrip(&mut client).unwrap();
        report.send(2).unwrap();
        steps.recv_timeout(Duration::from_secs(5)).unwrap();
        let cursor = compositor.create_surface(&qh, ());
        let image = pool.create_buffer(0, 16, 24, 64, wl_shm::Format::Abgr8888, &qh, ());
        pointer.set_cursor(serial, Some(&cursor), 3, 7);
        cursor.attach(Some(&image), 0, 0);
        cursor.commit();
        queue.roundtrip(&mut client).unwrap();
        report.send(3).unwrap();
        steps.recv_timeout(Duration::from_secs(5)).unwrap();
        pointer.set_cursor(serial.wrapping_sub(1), None, 0, 0);
        queue.roundtrip(&mut client).unwrap();
        report.send(4).unwrap();
        steps.recv_timeout(Duration::from_secs(5)).unwrap();
        cursor.destroy();
        queue.roundtrip(&mut client).unwrap();
        report.send(5).unwrap();
        steps.recv_timeout(Duration::from_secs(5)).unwrap();
    });
    let until = Instant::now() + Duration::from_secs(15);
    let mut phases = 0;
    while !thread.is_finished() && Instant::now() < until {
        display.dispatch_clients(&mut server).unwrap();
        server.frame();
        display.flush_clients().unwrap();
        if let Ok(phase) = reports.try_recv() {
            match phase {
                0 => {
                    let root = server.windows[0].window.toplevel().unwrap().wl_surface().clone();
                    server.pointer = (200.0, 200.0).into();
                    let pointer = server.seat.get_pointer().unwrap();
                    pointer.motion(
                        &mut server,
                        Some((root, (0.0, 0.0).into())),
                        &MotionEvent { location: (200.0, 200.0).into(), serial: SERIAL_COUNTER.next_serial(), time: InputTime::now() },
                    );
                    pointer.frame(&mut server);
                }
                1 => assert_eq!(server.effective_cursor(), CursorImageStatus::Named(CursorIcon::Text)),
                2 => assert_eq!(server.effective_cursor(), CursorImageStatus::Hidden),
                3 | 4 => {
                    let CursorImageStatus::Surface(surface) = server.effective_cursor() else {
                        panic!("expected the client's cursor surface");
                    };
                    let hotspot = with_states(&surface, |states| states.data_map.get::<CursorImageSurfaceData>().unwrap().lock().unwrap().hotspot);
                    assert_eq!(hotspot, (3, 7).into());
                    if phase == 4 {
                        server.core.handle(Event::Home);
                        server.frame();
                        assert_eq!(server.effective_cursor(), CursorImageStatus::default_named());
                        assert!(server.core.state().session().top_window().is_none());
                    }
                }
                5 => assert_eq!(server.effective_cursor(), CursorImageStatus::default_named()),
                _ => panic!("unexpected phase"),
            }
            phases += 1;
            next.send(()).unwrap();
        }
        std::thread::sleep(Duration::from_millis(1));
    }
    assert!(thread.is_finished());
    thread.join().unwrap();
    assert_eq!(phases, 6);
}
