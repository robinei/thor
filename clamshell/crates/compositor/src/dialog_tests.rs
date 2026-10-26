//! A toplevel marked a dialog by `xdg_dialog_v1` has the dialog role, also when marked after it opened.

use super::*;
use clamshell_state::session::presentation::WindowRole;
use smithay::reexports::wayland_protocols::xdg::dialog::v1::client::{xdg_dialog_v1, xdg_wm_dialog_v1};

delegate_noop!(Client: ignore xdg_wm_dialog_v1::XdgWmDialogV1);
delegate_noop!(Client: ignore xdg_dialog_v1::XdgDialogV1);

#[test]
fn a_toplevel_marked_a_dialog_has_the_dialog_role() {
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
        let dialogs: xdg_wm_dialog_v1::XdgWmDialogV1 = globals.bind(&qh, 1..=1, ()).unwrap();
        let surface = compositor.create_surface(&qh, ());
        let xdg = shell.get_xdg_surface(&surface, &qh, ());
        let top = xdg.get_toplevel(&qh, ());
        top.set_app_id("settings".into());
        surface.commit();
        let mut client = Client::default();
        queue.roundtrip(&mut client).unwrap();
        let mut file = tempfile::tempfile().unwrap();
        file.write_all(&vec![0xff; 400 * 300 * 4]).unwrap();
        let pool = shm.create_pool(file.as_fd(), 400 * 300 * 4, &qh, ());
        let buffer = pool.create_buffer(0, 400, 300, 400 * 4, wl_shm::Format::Abgr8888, &qh, ());
        surface.attach(Some(&buffer), 0, 0);
        surface.commit();
        queue.roundtrip(&mut client).unwrap();
        report.send(0).unwrap();
        steps.recv_timeout(Duration::from_secs(5)).unwrap();
        let _dialog = dialogs.get_xdg_dialog(&top, &qh, ());
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
            let id = server.windows[0].id;
            let role = server.core.state().session().window(id).unwrap().role;
            assert_eq!(role, if phase == 0 { WindowRole::Normal } else { WindowRole::Dialog });
            phases += 1;
            next.send(()).unwrap();
        }
        std::thread::sleep(Duration::from_millis(1));
    }
    assert_eq!(phases, 2);
    thread.join().unwrap();
}
