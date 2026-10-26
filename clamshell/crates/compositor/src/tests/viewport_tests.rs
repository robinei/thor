//! Buffer cropping and scaling leave presentation and input in surface coordinates.

use super::*;
use smithay::reexports::wayland_protocols::wp::viewporter::client::{wp_viewport, wp_viewporter};

delegate_noop!(Client: ignore wp_viewporter::WpViewporter);
delegate_noop!(Client: ignore wp_viewport::WpViewport);

#[test]
fn viewport_destination_drives_geometry_and_destroy_restores_buffer_size() {
    let (mut display, mut server) = ServerBuilder::new().started();
    let client = spawn_client(&mut display, ClientState::default(), |globals, mut queue, steps| {
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
        steps.step(0);
        viewport.destroy();
        surface.commit();
        queue.roundtrip(&mut client).unwrap();
        steps.step(1);
    });
    let checked = client.serve(&mut display, &mut server, Server::frame, |_, server, phase| {
        let window = &server.windows[0];
        let geometry = server.geometry(window.id).unwrap();
        let size = if phase == 0 { (600, 400) } else { (320, 240) };
        assert_eq!(geometry.native.size, size.into());
        // Input addresses the displayed logical surface, never the cropped buffer pixels.
        let hit = server.hit((480.0, 270.0).into()).unwrap();
        assert_eq!(hit.location, (size.0 as f64 / 2.0, size.1 as f64 / 2.0).into());
        assert_eq!(server.core.state().session().windows().len(), 1);
    });
    assert_eq!(checked, 2);
}
