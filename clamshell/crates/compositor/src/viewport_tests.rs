//! Buffer cropping and scaling leave presentation and input in surface coordinates.

use super::*;
use smithay::reexports::wayland_protocols::wp::viewporter::client::{wp_viewport, wp_viewporter};

delegate_noop!(Client: ignore wp_viewporter::WpViewporter);
delegate_noop!(Client: ignore wp_viewport::WpViewport);

#[test]
fn viewport_destination_drives_geometry_and_destroy_restores_buffer_size() {
    let mut display = Display::<Server>::new().unwrap();
    let config =
        Config { bottom_output: "ClamShell-Bottom".into(), top_size: (960, 540), bottom_size: (620, 540), bottom_panel: false, touch_layout: Arc::default() };
    let mut server = Server::new(State::new(Init::default()), config, display.handle(), false, || {}, |_| Process::default(), |_| {}).unwrap();
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
        let manager: wp_viewporter::WpViewporter = globals.bind(&qh, 1..=1, ()).unwrap();
        let surface = compositor.create_surface(&qh, ());
        let xdg = shell.get_xdg_surface(&surface, &qh, ());
        let top = xdg.get_toplevel(&qh, ());
        top.set_app_id("viewport".into());
        surface.commit();
        let mut client = Client::default();
        queue.roundtrip(&mut client).unwrap();
        let mut file = tempfile::tempfile().unwrap();
        file.write_all(&vec![0xff; 640 * 480 * 4]).unwrap();
        let pool = shm.create_pool(file.as_fd(), 640 * 480 * 4, &qh, ());
        let buffer = pool.create_buffer(0, 640, 480, 640 * 4, wl_shm::Format::Abgr8888, &qh, ());
        let viewport = manager.get_viewport(&surface, &qh, ());
        surface.set_buffer_scale(2);
        viewport.set_source(10.0, 20.0, 300.0, 200.0);
        viewport.set_destination(600, 400);
        surface.attach(Some(&buffer), 0, 0);
        surface.commit();
        queue.roundtrip(&mut client).unwrap();
        report.send(0).unwrap();
        steps.recv_timeout(Duration::from_secs(5)).unwrap();
        viewport.destroy();
        surface.commit();
        queue.roundtrip(&mut client).unwrap();
        report.send(1).unwrap();
        steps.recv_timeout(Duration::from_secs(5)).unwrap();
    });
    let until = Instant::now() + Duration::from_secs(15);
    let mut phases = 0;
    while !thread.is_finished() && Instant::now() < until {
        display.dispatch_clients(&mut server).unwrap();
        server.frame();
        display.flush_clients().unwrap();
        if let Ok(phase) = reports.try_recv() {
            let window = &server.windows[0];
            let geometry = server.geometry(window.id).unwrap();
            let size = if phase == 0 { (600, 400) } else { (320, 240) };
            assert_eq!(geometry.native.size, size.into());
            // Input addresses the displayed logical surface, never the cropped buffer pixels.
            let hit = server.hit((480.0, 270.0).into()).unwrap();
            assert_eq!(hit.location, (size.0 as f64 / 2.0, size.1 as f64 / 2.0).into());
            assert_eq!(server.core.state().session().windows().len(), 1);
            phases += 1;
            next.send(()).unwrap();
        }
        std::thread::sleep(Duration::from_millis(1));
    }
    assert!(thread.is_finished(), "viewport client timed out");
    thread.join().unwrap();
    assert_eq!(phases, 2);
}
