//! Real inhibitors lose their force when their surfaces cease to be presented.

use super::*;
use clamshell_state::WindowEvent;
use smithay::reexports::wayland_protocols::wp::idle_inhibit::zv1::client::{zwp_idle_inhibit_manager_v1, zwp_idle_inhibitor_v1};
use wayland_client::protocol::{wl_subcompositor, wl_subsurface};

delegate_noop!(Client: ignore zwp_idle_inhibit_manager_v1::ZwpIdleInhibitManagerV1);
delegate_noop!(Client: ignore zwp_idle_inhibitor_v1::ZwpIdleInhibitorV1);
delegate_noop!(Client: ignore wl_subcompositor::WlSubcompositor);
delegate_noop!(Client: ignore wl_subsurface::WlSubsurface);

#[test]
fn inhibitors_count_individually_and_only_mapped_visible_surfaces_keep_idle_off() {
    let mut display = Display::<Server>::new().unwrap();
    let config =
        Config { bottom_output: "ClamShell-Bottom".into(), top_size: (960, 540), bottom_size: (620, 540), bottom_panel: false, touch_layout: Arc::default() };
    let mut server = Server::new(State::new(Init::default()), config, display.handle(), || {}, |_| Process::default(), |_| {}).unwrap();
    server.core.handle(Event::Start);
    server.turn();
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
        let manager: zwp_idle_inhibit_manager_v1::ZwpIdleInhibitManagerV1 = globals.bind(&qh, 1..=1, ()).unwrap();
        let subcompositor: wl_subcompositor::WlSubcompositor = globals.bind(&qh, 1..=1, ()).unwrap();
        let surface = compositor.create_surface(&qh, ());
        let child = compositor.create_surface(&qh, ());
        let _subsurface = subcompositor.get_subsurface(&child, &surface, &qh, ());
        let xdg = shell.get_xdg_surface(&surface, &qh, ());
        let top = xdg.get_toplevel(&qh, ());
        top.set_app_id("video".into());
        let first = manager.create_inhibitor(&surface, &qh, ());
        let _second = manager.create_inhibitor(&child, &qh, ());
        surface.commit();
        let mut client = Client::default();
        queue.roundtrip(&mut client).unwrap();
        report.send(0).unwrap();
        steps.recv_timeout(Duration::from_secs(5)).unwrap();
        let mut file = tempfile::tempfile().unwrap();
        file.write_all(&vec![0xff; 960 * 540 * 4]).unwrap();
        let pool = shm.create_pool(file.as_fd(), 960 * 540 * 4, &qh, ());
        let buffer = pool.create_buffer(0, 960, 540, 960 * 4, wl_shm::Format::Abgr8888, &qh, ());
        child.attach(Some(&buffer), 0, 0);
        child.commit();
        surface.attach(Some(&buffer), 0, 0);
        surface.commit();
        queue.roundtrip(&mut client).unwrap();
        report.send(1).unwrap();
        steps.recv_timeout(Duration::from_secs(5)).unwrap();
        first.destroy();
        queue.roundtrip(&mut client).unwrap();
        report.send(2).unwrap();
        steps.recv_timeout(Duration::from_secs(5)).unwrap();
        surface.attach(None, 0, 0);
        surface.commit();
        queue.roundtrip(&mut client).unwrap();
        report.send(3).unwrap();
        steps.recv_timeout(Duration::from_secs(5)).unwrap();
        top.destroy();
        xdg.destroy();
        surface.destroy();
        child.destroy();
        queue.roundtrip(&mut client).unwrap();
        report.send(4).unwrap();
        steps.recv_timeout(Duration::from_secs(5)).unwrap();
    });
    let start = Instant::now();
    let future = start + Duration::from_secs(1000);
    let until = start + Duration::from_secs(15);
    let mut phases = 0;
    while !thread.is_finished() && Instant::now() < until {
        display.dispatch_clients(&mut server).unwrap();
        server.turn();
        display.flush_clients().unwrap();
        if let Ok(phase) = reports.try_recv() {
            match phase {
                0 => {
                    assert_eq!(server.idle.inhibitors.len(), 2);
                    server.update_idle(future);
                    assert_eq!(server.idle.quiet, Some(true), "an unmapped surface cannot inhibit idle");
                }
                1 => {
                    server.update_idle(future);
                    assert_eq!(server.idle.quiet, Some(false));
                    let id = server.windows[0].id;
                    server.core.handle(Event::Home);
                    server.update_idle(future);
                    assert_eq!(server.idle.quiet, Some(false), "hiding the video starts a fresh timeout");
                    server.update_idle(future + Duration::from_secs(10));
                    assert_eq!(server.idle.quiet, Some(true));
                    server.core.handle(WindowEvent::Activate(id).into());
                    server.update_idle(future + Duration::from_secs(11));
                    assert_eq!(server.idle.quiet, Some(false), "showing the video resumes inhibition");
                }
                2 => {
                    assert_eq!(server.idle.inhibitors.len(), 1);
                    server.update_idle(future);
                    assert_eq!(server.idle.quiet, Some(false), "destroying one inhibitor leaves the other effective");
                }
                3 => {
                    assert!(!server.windows[0].mapped);
                    server.update_idle(future);
                    server.update_idle(future + Duration::from_secs(10));
                    assert_eq!(server.idle.quiet, Some(true));
                }
                4 => {
                    server.update_idle(future);
                    assert!(server.idle.inhibitors.is_empty(), "dead surfaces cannot retain inhibitors");
                }
                _ => panic!("unexpected phase"),
            }
            phases += 1;
            next.send(()).unwrap();
        }
        std::thread::sleep(Duration::from_millis(1));
    }
    assert_eq!(phases, 5);
    assert!(thread.is_finished());
    thread.join().unwrap();
}
