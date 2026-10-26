//! A toplevel marked a dialog by `xdg_dialog_v1` has the dialog role, also when marked after it opened.

use super::*;
use clamshell_state::session::presentation::WindowRole;
use smithay::reexports::wayland_protocols::xdg::dialog::v1::client::{xdg_dialog_v1, xdg_wm_dialog_v1};

delegate_noop!(Client: ignore xdg_wm_dialog_v1::XdgWmDialogV1);
delegate_noop!(Client: ignore xdg_dialog_v1::XdgDialogV1);

#[test]
fn a_toplevel_marked_a_dialog_has_the_dialog_role() {
    let (mut display, mut server) = ServerBuilder::new().started();
    let client = spawn_client(&mut display, ClientState::default(), |globals, mut queue, steps| {
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
        steps.step(0);
        let _dialog = dialogs.get_xdg_dialog(&top, &qh, ());
        queue.roundtrip(&mut client).unwrap();
        steps.step(1);
    });
    let checked = client.serve(&mut display, &mut server, Server::frame, |_, server, phase| {
        let id = server.windows[0].id;
        let role = server.core.state().session().window(id).unwrap().role;
        assert_eq!(role, if phase == 0 { WindowRole::Normal } else { WindowRole::Dialog });
    });
    assert_eq!(checked, 2);
}
