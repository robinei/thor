//! FIFO barriers are let go only once the frame with their content is shown, and timed commits are held
//! until a frame shown at their time.

use super::*;
use smithay::{
    backend::renderer::element::{RenderElementPresentationState, RenderElementState, RenderElementStates},
    reexports::wayland_protocols::wp::{
        commit_timing::v1::client::{wp_commit_timer_v1, wp_commit_timing_manager_v1},
        fifo::v1::client::{wp_fifo_manager_v1, wp_fifo_v1},
    },
};

delegate_noop!(Client: ignore wp_fifo_manager_v1::WpFifoManagerV1);
delegate_noop!(Client: ignore wp_fifo_v1::WpFifoV1);
delegate_noop!(Client: ignore wp_commit_timing_manager_v1::WpCommitTimingManagerV1);
delegate_noop!(Client: ignore wp_commit_timer_v1::WpCommitTimerV1);

/// What the client did, for the main loop to check after each.
enum Step {
    /// A window with its first content, which the main loop shows in a frame.
    Shown,
    /// A barrier set with new content.
    Barrier,
    /// A commit timed for 1000 s on the presentation clock.
    Timed,
}

/// The window's surfaces as composed into a frame.
fn rendered(server: &Server) -> RenderElementStates {
    let mut rendered = RenderElementStates::default();
    let state = RenderElementState { visible_area: 1, presentation_state: RenderElementPresentationState::Rendering { reason: None }, needs_capture: false };
    if let Some(window) = server.windows.first() {
        window.window.with_surfaces(|surface, _| {
            rendered.states.insert(surface.into(), state);
        });
    }
    rendered
}

/// Whether the window has a FIFO barrier set.
fn barrier(server: &Server) -> bool {
    let mut set = false;
    if let Some(window) = server.windows.first() {
        window.window.with_surfaces(|_, states| {
            set |= states.cached_state.get::<smithay::wayland::fifo::FifoBarrierCachedState>().current().barrier.is_some();
        });
    }
    set
}

#[test]
fn a_barrier_goes_once_its_frame_is_shown_and_a_timed_commit_at_its_time() {
    let (mut display, mut server) = ServerBuilder::new().started();
    let client = spawn_client(&mut display, ClientState::default(), move |globals, mut queue, steps| {
        let qh = queue.handle();
        let compositor: wl_compositor::WlCompositor = globals.bind(&qh, 1..=6, ()).unwrap();
        let shell: xdg_wm_base::XdgWmBase = globals.bind(&qh, 1..=6, ()).unwrap();
        let shm: wl_shm::WlShm = globals.bind(&qh, 1..=1, ()).unwrap();
        let fifos: wp_fifo_manager_v1::WpFifoManagerV1 = globals.bind(&qh, 1..=1, ()).unwrap();
        let timing: wp_commit_timing_manager_v1::WpCommitTimingManagerV1 = globals.bind(&qh, 1..=1, ()).unwrap();
        let surface = compositor.create_surface(&qh, ());
        let xdg = shell.get_xdg_surface(&surface, &qh, ());
        let _top = xdg.get_toplevel(&qh, ());
        surface.commit();
        let mut client = Client::default();
        queue.roundtrip(&mut client).unwrap();
        let mut file = tempfile::tempfile().unwrap();
        file.write_all(&vec![0xff; 64 * 64 * 4]).unwrap();
        let pool = shm.create_pool(file.as_fd(), 64 * 64 * 4, &qh, ());
        let buffer = pool.create_buffer(0, 64, 64, 64 * 4, wl_shm::Format::Abgr8888, &qh, ());
        surface.attach(Some(&buffer), 0, 0);
        surface.commit();
        queue.roundtrip(&mut client).unwrap();
        steps.step(Step::Shown);
        let fifo = fifos.get_fifo(&surface, &qh, ());
        fifo.set_barrier();
        surface.attach(Some(&buffer), 0, 0);
        surface.damage_buffer(0, 0, 64, 64);
        surface.commit();
        queue.roundtrip(&mut client).unwrap();
        steps.step(Step::Barrier);
        let timer = timing.get_timer(&surface, &qh, ());
        timer.set_timestamp(0, 1000, 0);
        surface.attach(Some(&buffer), 0, 0);
        surface.commit();
        queue.roundtrip(&mut client).unwrap();
        steps.step(Step::Timed);
    });
    let refresh = Duration::from_millis(16);
    let checked = client.serve(&mut display, &mut server, Server::frame, |display, server, step| match step {
        Step::Shown => {
            let rendered = rendered(server);
            server.frame_queued(&rendered, None);
            server.frame_shown(&rendered, None);
        }
        Step::Barrier => {
            assert!(barrier(server), "set");
            server.frame_callbacks(None, refresh, Instant::now());
            assert!(barrier(server), "kept: the frame with its content isn't shown");
            let rendered = rendered(server);
            server.frame_queued(&rendered, None);
            server.frame_callbacks(None, refresh, Instant::now());
            assert!(barrier(server), "kept while that frame is only queued");
            server.frame_shown(&rendered, None);
            server.frame_callbacks(None, refresh, Instant::now());
            assert!(!barrier(server), "let go once it is shown");
        }
        Step::Timed => {
            let commits = server.pacing.commits;
            assert_eq!(server.release_timed_commits(Duration::from_secs(1000).into(), Some(true)), None, "the second screen's: not its window");
            display.dispatch_clients(server).unwrap();
            assert_eq!(server.pacing.commits, commits, "not let go by the other screen's frame");
            let held = server.release_timed_commits(Duration::from_secs(500).into(), Some(false));
            assert_eq!(held.map(Duration::from), Some(Duration::from_secs(1000)), "held until its time");
            display.dispatch_clients(server).unwrap();
            assert_eq!(server.pacing.commits, commits, "not applied before");
            assert_eq!(server.release_timed_commits(Duration::from_secs(1000).into(), None), None, "let go at its time");
            display.dispatch_clients(server).unwrap();
            assert!(server.pacing.commits > commits, "applied once let go");
        }
    });
    assert_eq!(checked, 3);
}

/// A capped window's timed commit goes ahead at once, whatever its time: its cap paces it, and Mesa,
/// which takes a frame shown later than it asked for as missed, would move its times on further each
/// frame the cap holds back (see [`Server::release_timed_commits`]).
#[test]
fn a_capped_windows_timed_commit_goes_ahead_at_once() {
    let mut settings = clamshell_state::settings::Settings::default();
    settings.set(None, clamshell_state::settings::Setting::FpsCap, Some(30));
    let state = State::new(Init { settings, device_session: true, ..Init::default() });
    let (mut display, mut server) = ServerBuilder::new().state(state).started();
    let process = Process { unit: Some("app-game.scope".into()), pids: vec![100], game: None };
    let client = spawn_client(&mut display, ClientState { process: Some(process), ..ClientState::default() }, move |globals, mut queue, steps| {
        let qh = queue.handle();
        let compositor: wl_compositor::WlCompositor = globals.bind(&qh, 1..=6, ()).unwrap();
        let shell: xdg_wm_base::XdgWmBase = globals.bind(&qh, 1..=6, ()).unwrap();
        let shm: wl_shm::WlShm = globals.bind(&qh, 1..=1, ()).unwrap();
        let timing: wp_commit_timing_manager_v1::WpCommitTimingManagerV1 = globals.bind(&qh, 1..=1, ()).unwrap();
        let surface = compositor.create_surface(&qh, ());
        let xdg = shell.get_xdg_surface(&surface, &qh, ());
        let top = xdg.get_toplevel(&qh, ());
        top.set_app_id("game".into());
        surface.commit();
        let mut client = Client::default();
        queue.roundtrip(&mut client).unwrap();
        let mut file = tempfile::tempfile().unwrap();
        file.write_all(&vec![0xff; 64 * 64 * 4]).unwrap();
        let pool = shm.create_pool(file.as_fd(), 64 * 64 * 4, &qh, ());
        let buffer = pool.create_buffer(0, 64, 64, 64 * 4, wl_shm::Format::Abgr8888, &qh, ());
        surface.attach(Some(&buffer), 0, 0);
        surface.commit();
        queue.roundtrip(&mut client).unwrap();
        steps.step(Step::Shown);
        let timer = timing.get_timer(&surface, &qh, ());
        timer.set_timestamp(0, 1000, 0);
        surface.attach(Some(&buffer), 0, 0);
        surface.commit();
        queue.roundtrip(&mut client).unwrap();
        steps.step(Step::Timed);
    });
    let checked = client.serve(&mut display, &mut server, Server::frame, |display, server, step| match step {
        Step::Shown => assert_eq!(server.windows.first().map(|w| w.shown.fps_cap), Some(Some(30)), "capped"),
        Step::Timed => {
            let commits = server.pacing.commits;
            assert_eq!(server.release_timed_commits(Duration::from_secs(500).into(), None), None, "nothing held");
            display.dispatch_clients(server).unwrap();
            assert!(server.pacing.commits > commits, "applied before its time");
        }
        Step::Barrier => panic!("no barrier"),
    });
    assert_eq!(checked, 2);
}

use wayland_client::protocol::{wl_callback, wl_subcompositor};
delegate_noop!(Client: ignore wl_callback::WlCallback);

/// Root and child callbacks share one window pacing turn, including when only the child waits.
#[test]
fn callbacks_anywhere_in_the_window_share_its_cap() {
    let mut settings = clamshell_state::settings::Settings::default();
    settings.set(None, clamshell_state::settings::Setting::FpsCap, Some(30));
    let state = State::new(Init { settings, device_session: true, ..Init::default() });
    let (mut display, mut server) = ServerBuilder::new().state(state).started();
    let process = Process { unit: Some("app-game.scope".into()), pids: vec![100], game: None };
    let client = spawn_client(&mut display, ClientState { process: Some(process), ..ClientState::default() }, move |globals, mut queue, steps| {
        let qh = queue.handle();
        let mut client = Client::default();
        let top = open_toplevel(&globals, &mut queue, &mut client, "game", Some((960, 540)));
        let compositor: wl_compositor::WlCompositor = globals.bind(&qh, 1..=6, ()).unwrap();
        let subs: wl_subcompositor::WlSubcompositor = globals.bind(&qh, 1..=1, ()).unwrap();
        let child = compositor.create_surface(&qh, ());
        let sub = subs.get_subsurface(&child, &top.surface, &qh, ());
        sub.set_desync();
        child.attach(Some(&top.buffer), 0, 0);
        child.commit();
        top.surface.commit();
        for (root, picture) in [(false, true), (true, false), (true, true), (false, false)] {
            if root {
                top.surface.frame(&qh, ());
            }
            if picture {
                child.frame(&qh, ());
            }
            child.commit();
            top.surface.commit();
            queue.roundtrip(&mut client).unwrap();
            steps.step((root, picture));
        }
    });
    let checked = client.serve(&mut display, &mut server, Server::frame, |_, server, (root, child)| {
        let now = Instant::now();
        let refresh = Duration::from_millis(16);
        let window = &mut server.windows[0];
        assert_eq!(window.shown.fps_cap, Some(30));
        window.frames = crate::frames::Frames::default();
        window.frames.pace(Some(30), refresh, now).unwrap();
        let count = |server: &Server| {
            let mut count = 0;
            server.windows[0].window.with_surfaces(|_, states| {
                count += states.cached_state.get::<smithay::wayland::compositor::SurfaceAttributes>().current().frame_callbacks.len();
            });
            count
        };
        assert_eq!(count(server), usize::from(root) + usize::from(child));
        let held = server.frame_callbacks(None, refresh, now);
        assert_eq!(held.is_some(), root || child);
        assert_eq!(count(server), usize::from(root) + usize::from(child), "held callbacks remain pending");
        if let Some(due) = held {
            assert_eq!(server.frame_callbacks(None, refresh, due), None);
            assert_eq!(count(server), 0, "one turn releases both surfaces");
            assert!(server.windows[0].frames.pace(Some(30), refresh, due).is_err(), "only one pacing turn was consumed");
        }
    });
    assert_eq!(checked, 4);
}
