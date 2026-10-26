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
    let mut display = Display::<Server>::new().unwrap();
    let config = Config { top_size: (960, 540), bottom_size: (620, 540), touch_layout: Arc::default() };
    let mut server = Server::new(State::new(Init::default()), config, display.handle(), false, || {}, |_| Process::default(), |_| {}).unwrap();
    server.core.handle(Event::Start);
    server.frame();
    let (server_socket, client_socket) = UnixStream::pair().unwrap();
    display.handle().insert_client(server_socket, Arc::new(ClientState::default())).unwrap();
    let (report, reports) = mpsc::channel();
    let (next, steps) = mpsc::channel::<()>();
    let thread = std::thread::spawn(move || {
        let connection = Connection::from_socket(client_socket).unwrap();
        let (globals, mut queue) = registry_queue_init::<Client>(&connection).unwrap();
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
        report.send(Step::Shown).unwrap();
        steps.recv_timeout(Duration::from_secs(5)).unwrap();
        let fifo = fifos.get_fifo(&surface, &qh, ());
        fifo.set_barrier();
        surface.attach(Some(&buffer), 0, 0);
        surface.damage_buffer(0, 0, 64, 64);
        surface.commit();
        queue.roundtrip(&mut client).unwrap();
        report.send(Step::Barrier).unwrap();
        steps.recv_timeout(Duration::from_secs(5)).unwrap();
        let timer = timing.get_timer(&surface, &qh, ());
        timer.set_timestamp(0, 1000, 0);
        surface.attach(Some(&buffer), 0, 0);
        surface.commit();
        queue.roundtrip(&mut client).unwrap();
        report.send(Step::Timed).unwrap();
        steps.recv_timeout(Duration::from_secs(5)).unwrap();
    });
    let until = Instant::now() + Duration::from_secs(15);
    let mut steps_seen = 0;
    let refresh = Duration::from_millis(16);
    while !thread.is_finished() && Instant::now() < until {
        display.dispatch_clients(&mut server).unwrap();
        server.frame();
        if let Ok(step) = reports.try_recv() {
            match step {
                Step::Shown => {
                    let rendered = rendered(&server);
                    server.frame_queued(&rendered, None);
                    server.frame_shown(&rendered, None);
                }
                Step::Barrier => {
                    assert!(barrier(&server), "set");
                    server.frame_callbacks(None, refresh, Instant::now());
                    assert!(barrier(&server), "kept: the frame with its content isn't shown");
                    let rendered = rendered(&server);
                    server.frame_queued(&rendered, None);
                    server.frame_callbacks(None, refresh, Instant::now());
                    assert!(barrier(&server), "kept while that frame is only queued");
                    server.frame_shown(&rendered, None);
                    server.frame_callbacks(None, refresh, Instant::now());
                    assert!(!barrier(&server), "let go once it is shown");
                }
                Step::Timed => {
                    let commits = server.commits;
                    let held = server.release_timed_commits(Duration::from_secs(500).into());
                    assert_eq!(held.map(Duration::from), Some(Duration::from_secs(1000)), "held until its time");
                    display.dispatch_clients(&mut server).unwrap();
                    assert_eq!(server.commits, commits, "not applied before");
                    assert_eq!(server.release_timed_commits(Duration::from_secs(1000).into()), None, "let go at its time");
                    display.dispatch_clients(&mut server).unwrap();
                    assert!(server.commits > commits, "applied once let go");
                }
            }
            steps_seen += 1;
            next.send(()).unwrap();
        }
        display.flush_clients().unwrap();
        std::thread::sleep(Duration::from_millis(1));
    }
    assert_eq!(steps_seen, 3);
    thread.join().unwrap();
}
