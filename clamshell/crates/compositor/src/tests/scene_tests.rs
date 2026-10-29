//! What `Server::place` puts in the scene, with bare surfaces given buffers and added as windows the way
//! Xwayland's are, so that each is mapped without a shell.
use super::*;
use crate::scene::{Placed, Shown};
use crate::{surface::Surface, windows::ClientWindow};
use clamshell_state::session::overview::OverviewEvent;
use clamshell_state::{WindowEvent, presentation::WindowLimits, session::windows::WindowFacts};
use smithay::reexports::wayland_server::protocol::wl_surface::WlSurface;

/// A window of the test: its id, its parent, and its fixed size, which its buffer has.
struct Opened {
    id: i64,
    parent: Option<i64>,
    size: (u32, u32),
}

/// Window 1 at the top screen's size and its two dialogs, opened in that order.
const WITH_DIALOGS: [Opened; 3] = [
    Opened { id: 1, parent: None, size: (960, 540) },
    Opened { id: 2, parent: Some(1), size: (320, 240) },
    Opened { id: 3, parent: Some(1), size: (200, 100) },
];

/// Opens `windows` in order on a started server, each a surface of one client with a buffer of its size, and
/// runs `check` on the server once they are all mapped, the first is in front, and they are placed.
fn with_windows(windows: &[Opened], check: impl FnOnce(&mut Server)) -> (Display<Server>, Server) {
    let (mut display, mut server) = ServerBuilder::new().started();
    let sizes: Vec<(i32, i32)> = windows.iter().map(|w| (w.size.0 as i32, w.size.1 as i32)).collect();
    let client = spawn_client(&mut display, ClientState::default(), move |globals, mut queue, steps| {
        let qh = queue.handle();
        let compositor: wl_compositor::WlCompositor = globals.bind(&qh, 1..=6, ()).unwrap();
        let surfaces: Vec<_> = sizes
            .iter()
            .map(|&size| {
                let surface = compositor.create_surface(&qh, ());
                surface.attach(Some(&shm_buffer(&globals, &qh, size)), 0, 0);
                surface.commit();
                surface
            })
            .collect();
        queue.roundtrip(&mut Client::default()).unwrap();
        steps.step(surfaces.iter().map(|surface| surface.id().protocol_id()).collect::<Vec<_>>());
    });
    let peer = client.peer.clone();
    let mut check = Some(check);
    let checked = client.serve(&mut display, &mut server, Server::frame, |_, server, ids: Vec<u32>| {
        for (window, id) in windows.iter().zip(ids) {
            let surface: WlSurface = peer.object_from_protocol_id(&server.dh, id).unwrap();
            let facts = WindowFacts {
                id: window.id,
                app_id: "scene-test".into(),
                parent: window.parent,
                limits: WindowLimits { min: window.size, max: window.size },
                unit: Some("clamshell-run-test-1-0.service".into()),
                ..WindowFacts::default()
            };
            server.core.handle(WindowEvent::Opened(facts).into());
            server.windows.push(ClientWindow {
                id: window.id,
                window: Surface::new_x11(surface),
                mapped: true,
                opened: true,
                facts_dirty: false,
                shown: crate::windows::Shown::of(server.core.state(), window.id),
                frames: Default::default(),
            });
            server.windows_changed();
        }
        server.core.handle(WindowEvent::Activate(windows[0].id).into());
        server.sync_windows();
        server.place();
        check.take().unwrap()(server);
    });
    assert_eq!(checked, 1);
    (display, server)
}

/// A window whose surface is gone, its client ended before the window did, leaves the scene at once: a
/// surface's end changes what the windows are placed from, as its commits do.
#[test]
fn a_window_whose_surface_is_gone_leaves_the_scene() {
    let (mut display, mut server) = with_windows(&WITH_DIALOGS[..1], |server| assert!(server.scene.window(1).is_some()));
    // The client's end, and with it its surfaces'.
    display.dispatch_clients(&mut server).unwrap();
    server.place();
    assert_eq!(server.scene.window(1), None);
}

/// The windows in the scene, front first.
fn placed(server: &Server) -> Vec<Placed> {
    server.scene.windows().copied().collect()
}

#[test]
fn a_window_with_dialogs_is_placed_back_to_front_with_the_dim_behind_the_topmost_which_alone_takes_input() {
    with_windows(&WITH_DIALOGS, |server| {
        let stack: Vec<i64> = server.core.state().session().stack(false).map(|w| w.id).collect();
        assert_eq!(stack, [1, 2, 3]);
        let windows: Vec<&Shown> = server.scene.shown().iter().filter(|s| matches!(s, Shown::Window(_) | Shown::Dim { .. })).collect();
        let area = server.area(false);
        assert!(matches!(windows.as_slice(), [Shown::Window(a), Shown::Dim { area: dim }, Shown::Window(b), Shown::Window(c)]
            if (a.id, b.id, c.id) == (3, 2, 1) && *dim == area));
        let input: Vec<(i64, bool)> = placed(server).iter().map(|p| (p.id, p.input)).collect();
        assert_eq!(input, [(3, true), (2, false), (1, false)]);
        assert!(placed(server).iter().all(|p| !p.below));
    });
}

#[test]
fn the_overview_places_each_windows_stack_in_its_cell_once_without_input() {
    const WINDOWS: [Opened; 3] = [
        Opened { id: 1, parent: None, size: (960, 540) },
        Opened { id: 2, parent: None, size: (960, 540) },
        Opened { id: 3, parent: Some(1), size: (320, 240) },
    ];
    with_windows(&WINDOWS, |server| {
        server.core.handle(WindowEvent::Activate(2).into());
        server.core.handle(Event::Overview(OverviewEvent::Toggle));
        assert!(server.core.state().session().overview().is_some());
        server.place();
        let placed = placed(server);
        let mut ids: Vec<i64> = placed.iter().map(|p| p.id).collect();
        ids.sort_unstable();
        assert_eq!(ids, [1, 2, 3], "each window is placed once");
        assert!(placed.iter().all(|p| !p.input), "the overview's windows are shown to be chosen");
        for p in &placed {
            let root = server.core.state().session().root_of(p.id);
            let cell = server.overview_cell(root).unwrap();
            assert!(cell.contains_rect(p.geometry.viewport), "window {} at {:?} is outside its cell {cell:?}", p.id, p.geometry.viewport);
        }
        // Window 1's dialog over it, with the dim between them over the screen as brought into the cell.
        let shown = server.scene.shown();
        let dialog = shown.iter().position(|s| matches!(s, Shown::Window(p) if p.id == 3)).unwrap();
        let Some(&[Shown::Dim { area }, Shown::Window(window)]) = shown.get(dialog + 1..dialog + 3) else { panic!("{shown:?}") };
        assert_eq!((window.id, area), (1, window.geometry.viewport));
    });
}

#[test]
fn the_cursor_is_placed_in_front_of_everything_while_it_shows() {
    with_windows(&WITH_DIALOGS, |server| {
        assert!(!server.scene.shown().iter().any(|s| matches!(s, Shown::Cursor { .. })), "hidden until the pointer moves");
        server.pointer.location = (100.0, 100.0).into();
        server.pointer.cursor_activity.motion(std::time::Instant::now());
        server.place();
        assert_eq!(server.scene.shown().first(), Some(&Shown::Cursor { at: (100.0, 100.0).into() }));
        assert_eq!(server.scene.shown().iter().filter(|s| matches!(s, Shown::Cursor { .. })).count(), 1);
    });
}

#[test]
fn a_window_alone_on_its_screen_is_the_lone_window_until_a_dialog_shows_over_it() {
    const WINDOWS: [Opened; 2] = [Opened { id: 1, parent: None, size: (960, 540) }, Opened { id: 2, parent: Some(1), size: (320, 240) }];
    with_windows(&WINDOWS[..1], |server| {
        let planed = crate::plane_layers::Planed { overlay: false, cursor: false };
        let area = server.area(false);
        assert_eq!(server.lone_window(false, area, planed).map(|p| p.id), Some(1));
    });
    with_windows(&WINDOWS, |server| {
        let planed = crate::plane_layers::Planed { overlay: false, cursor: false };
        let area = server.area(false);
        assert_eq!(server.lone_window(false, area, planed), None);
    });
}

/// What is touched on the main screen is what is drawn there: ClamShell's view while the window in front has
/// nothing to show yet, the window once it does.
#[test]
fn the_main_screens_view_takes_touches_until_a_window_is_drawn_over_it() {
    with_windows(&WITH_DIALOGS[..1], |server| {
        let point = (100.0, 100.0).into();
        assert!(server.scene.covers_top() && server.ui_at(point).is_none(), "the window drawn");
        server.windows[0].mapped = false;
        server.windows_changed();
        server.place();
        assert!(server.core.state().session().top_window().is_some(), "still in front");
        assert!(!server.scene.covers_top());
        assert_eq!(server.ui_at(point).map(|(screen, _)| screen), Some(clamshell_ui::Screen::Top), "the view drawn and touched");
    });
}

/// The scene is placed again only when what it is placed from changed: asked for again with nothing
/// changed, it is as it was; a commit, a state event, an overlay drawn anew and a resize each place it once.
#[test]
fn placing_again_with_nothing_changed_does_nothing() {
    const WINDOWS: [Opened; 2] = [Opened { id: 1, parent: None, size: (960, 540) }, Opened { id: 2, parent: None, size: (960, 540) }];
    with_windows(&WINDOWS, |server| {
        server.core.handle(WindowEvent::Activate(2).into());
        server.place();
        let placements = server.scene.placements;
        for _ in 0..3 {
            server.place();
        }
        assert_eq!(server.scene.placements, placements, "nothing changed");
        let root = server.windows[0].window.root().unwrap();
        smithay::wayland::compositor::CompositorHandler::commit(server, &root);
        server.place();
        server.place();
        assert_eq!(server.scene.placements, placements + 1, "a commit");
        server.core.handle(Event::Overview(OverviewEvent::Toggle));
        server.place();
        server.place();
        assert_eq!(server.scene.placements, placements + 2, "a state event");
        let overlay = server.core.overlay_area(clamshell_ui::Screen::Top);
        server.core.record(clamshell_ui::Layer::Overlay(clamshell_ui::Screen::Top), false);
        assert_ne!(server.core.overlay_area(clamshell_ui::Screen::Top), overlay, "the overview's cells drawn on the overlay");
        server.place();
        server.place();
        assert_eq!(server.scene.placements, placements + 3, "an overlay drawn anew");
        // (The keyboard's focus follows the overview, which the state is told.)
        server.sync_windows();
        let placements = server.scene.placements;
        server.resize((1600, 540));
        server.place();
        assert_eq!(server.scene.placements, placements + 1, "a resize");
    });
}
