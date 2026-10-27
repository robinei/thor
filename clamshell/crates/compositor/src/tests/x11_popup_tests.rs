//! Bare Xwayland-style surfaces exercise the shared popup transform and touch path without a GPU.
use super::*;
use crate::{
    surface::{Surface, X11Popup},
    windows::ClientWindow,
};
use clamshell_state::{WindowEvent, presentation::WindowLimits, session::windows::WindowFacts};
use smithay::reexports::wayland_server::protocol::wl_surface::WlSurface;
use smithay::{backend::input::InputTime, utils::Point};

#[test]
fn x11_popups_share_parent_scaling_keep_tooltips_passive_and_cancel_dismissed_contacts() {
    let (mut display, mut server) = ServerBuilder::new().started();
    let client = spawn_client(&mut display, ClientState::default(), move |globals, mut queue, steps| {
        let qh = queue.handle();
        let compositor: wl_compositor::WlCompositor = globals.bind(&qh, 1..=6, ()).unwrap();
        let shm: wl_shm::WlShm = globals.bind(&qh, 1..=1, ()).unwrap();
        let seat: wl_seat::WlSeat = globals.bind(&qh, 1..=7, ()).unwrap();
        let _touch = seat.get_touch(&qh, ());
        let mut file = tempfile::tempfile().unwrap();
        file.write_all(&vec![0xff; 1280 * 720 * 4]).unwrap();
        let pool = shm.create_pool(file.as_fd(), 1280 * 720 * 4, &qh, ());
        let root_buffer = pool.create_buffer(0, 1280, 720, 1280 * 4, wl_shm::Format::Abgr8888, &qh, ());
        let popup_buffer = pool.create_buffer(0, 160, 100, 160 * 4, wl_shm::Format::Abgr8888, &qh, ());
        let mut surfaces = Vec::new();
        for i in 0..4 {
            let surface = compositor.create_surface(&qh, ());
            surface.attach(Some(if i == 0 { &root_buffer } else { &popup_buffer }), 0, 0);
            surface.commit();
            surfaces.push(surface);
        }
        let mut client = Client::default();
        queue.roundtrip(&mut client).unwrap();
        steps.step(surfaces.iter().map(|surface| surface.id().protocol_id()).collect::<Vec<_>>());
        queue.roundtrip(&mut client).unwrap();
        assert_eq!(
            client.touches,
            vec![
                TouchRecord::Down(0, surfaces[1].id().protocol_id(), (20.0, 20.0)),
                TouchRecord::Up(0),
                TouchRecord::Frame,
                TouchRecord::Down(1, surfaces[2].id().protocol_id(), (10.0, 20.0)),
                TouchRecord::Up(1),
                TouchRecord::Frame,
                TouchRecord::Down(2, surfaces[1].id().protocol_id(), (20.0, 20.0)),
                TouchRecord::Frame,
                TouchRecord::Motion(2, (20.0, 20.0)),
                TouchRecord::Cancel,
            ]
        );
    });
    let peer = client.peer.clone();
    let checked = client.serve(&mut display, &mut server, Server::frame, |_, server, ids: Vec<u32>| {
        let root = peer.object_from_protocol_id::<smithay::reexports::wayland_server::protocol::wl_surface::WlSurface>(&server.dh, ids[0]).unwrap();
        let first: WlSurface = peer.object_from_protocol_id(&server.dh, ids[1]).unwrap();
        let second: WlSurface = peer.object_from_protocol_id(&server.dh, ids[2]).unwrap();
        let tooltip: WlSurface = peer.object_from_protocol_id(&server.dh, ids[3]).unwrap();
        server.core.handle(
            WindowEvent::Opened(WindowFacts {
                id: 1,
                app_id: "popup-test".into(),
                limits: WindowLimits { min: (1280, 720), max: (1280, 720) },
                ..WindowFacts::default()
            })
            .into(),
        );
        server.windows.push(ClientWindow {
            id: 1,
            window: Surface::new_x11(root.clone()),
            mapped: true,
            opened: true,
            title_dirty: false,
            shown: crate::windows::Shown::of(server.core.state(), 1),
            frames: Default::default(),
        });
        server.place();
        let geometry = server.geometry(1).unwrap();
        assert_eq!(geometry.scale, smithay::utils::Scale::from(0.75));
        if let Surface::X11(window) = &mut server.windows[0].window {
            window.popups.extend([
                X11Popup { id: 101, surface: first.clone(), location: (100, 80).into(), geometry, input: true },
                X11Popup { id: 102, surface: second.clone(), location: (140, 100).into(), geometry, input: true },
                X11Popup { id: 103, surface: tooltip, location: (100, 80).into(), geometry, input: false },
            ]);
        }
        server.sync_windows();
        assert_eq!(server.geometry(1).unwrap(), geometry, "popup extents do not change parent sizing");
        assert!(server.has_x11_menu());
        let time = InputTime::from_millis(10);
        let point = |x, y| geometry.origin + Point::from((x * geometry.scale.x, y * geometry.scale.y));
        let main_point = point(120.0, 100.0);
        let sub_point = point(150.0, 120.0);
        assert_eq!(server.hit(main_point).unwrap().surface, first);
        assert_eq!(server.hit(sub_point).unwrap().surface, second);
        for (slot, at) in [(0, main_point), (1, sub_point)] {
            server.touch_down(Some(slot).into(), at, time);
            server.touch_up(Some(slot).into(), time);
            server.touch_frame();
        }
        server.touch_down(Some(2).into(), main_point, time);
        server.touch_frame();
        server.dismiss_x11_menus_outside(Some(&root));
        server.sync_windows();
        assert!(server.touch.contacts.iter().all(Option::is_none), "a detached popup cannot retain a touch contact");
        assert!(!server.has_x11_menu(), "tooltips do not capture input");
        assert_eq!(server.hit(main_point).unwrap().surface, root, "a tooltip passes input to its parent");
        assert_eq!(server.x11.popup_dismiss.len(), 2);
        assert!(server.x11.popup_dismiss.contains(&101) && server.x11.popup_dismiss.contains(&102));
        server.core.handle(Event::Home);
        server.frame();
        assert!(server.windows[0].window.x11_popups().is_empty());
        assert!(server.x11.popup_dismiss.contains(&103), "hiding a parent dismisses its tooltip too");
        assert_eq!(server.core.state().session().windows().len(), 1, "only the parent ever enters State");
    });
    assert_eq!(checked, 1);
}
