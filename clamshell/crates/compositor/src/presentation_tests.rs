//! Presentation feedback is committed with content and completed only after submission.

use super::*;
use smithay::{
    backend::renderer::element::{RenderElementPresentationState, RenderElementState, RenderElementStates},
    reexports::wayland_protocols::wp::presentation_time::client::{wp_presentation, wp_presentation_feedback},
};
use wayland_client::protocol::{wl_output, wl_subcompositor};

impl Dispatch<wp_presentation::WpPresentation, ()> for Client {
    fn event(state: &mut Self, _: &wp_presentation::WpPresentation, event: wp_presentation::Event, _: &(), _: &Connection, _: &QueueHandle<Self>) {
        if let wp_presentation::Event::ClockId { clk_id } = event {
            state.presentation_clock = Some(clk_id);
        }
    }
}
impl Dispatch<wp_presentation_feedback::WpPresentationFeedback, u32> for Client {
    fn event(
        state: &mut Self,
        _: &wp_presentation_feedback::WpPresentationFeedback,
        event: wp_presentation_feedback::Event,
        id: &u32,
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        match event {
            wp_presentation_feedback::Event::Presented { tv_sec_hi, tv_sec_lo, tv_nsec, refresh, seq_hi, seq_lo, flags } => {
                assert_eq!(refresh, 0);
                assert_eq!((seq_hi, seq_lo), (0, 0));
                assert_eq!(flags, wayland_client::WEnum::Value(wp_presentation_feedback::Kind::empty()));
                let timestamp = Duration::new((u64::from(tv_sec_hi) << 32) | u64::from(tv_sec_lo), tv_nsec);
                state.presented.push((*id, timestamp));
            }
            wp_presentation_feedback::Event::Discarded => state.discarded.push(*id),
            wp_presentation_feedback::Event::SyncOutput { .. } => state.presentation_outputs += 1,
            _ => {}
        }
    }
}
delegate_noop!(Client: ignore wl_output::WlOutput);

#[test]
fn feedback_follows_submission_visibility_superseding_and_surface_destruction() {
    let mut display = Display::<Server>::new().unwrap();
    let config = Config { top_size: (960, 540), bottom_size: (620, 540), touch_layout: Arc::default() };
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
        let presentation: wp_presentation::WpPresentation = globals.bind(&qh, 1..=2, ()).unwrap();
        // Binding outputs makes sync_output observable as well as the terminal feedback.
        let _outputs = globals.contents().with_list(|list| {
            list.iter()
                .filter(|g| g.interface == "wl_output")
                .map(|g| globals.registry().bind::<wl_output::WlOutput, _, _>(g.name, 1, &qh, ()))
                .collect::<Vec<_>>()
        });
        let subcompositor: wl_subcompositor::WlSubcompositor = globals.bind(&qh, 1..=1, ()).unwrap();
        let surface = compositor.create_surface(&qh, ());
        let child = compositor.create_surface(&qh, ());
        let _subsurface = subcompositor.get_subsurface(&child, &surface, &qh, ());
        let xdg = shell.get_xdg_surface(&surface, &qh, ());
        let top = xdg.get_toplevel(&qh, ());
        top.set_app_id("presentation-test".into());
        surface.commit();
        let mut client = Client::default();
        queue.roundtrip(&mut client).unwrap();
        assert_eq!(client.presentation_clock, Some(1)); // CLOCK_MONOTONIC on Linux.
        let mut file = tempfile::tempfile().unwrap();
        file.write_all(&vec![0xff; 960 * 540 * 4]).unwrap();
        let pool = shm.create_pool(file.as_fd(), 960 * 540 * 4, &qh, ());
        let buffer = pool.create_buffer(0, 960, 540, 960 * 4, wl_shm::Format::Abgr8888, &qh, ());
        let child_buffer = pool.create_buffer(0, 8, 8, 32, wl_shm::Format::Abgr8888, &qh, ());
        presentation.feedback(&child, &qh, 5);
        child.attach(Some(&child_buffer), 0, 0);
        child.commit();
        presentation.feedback(&surface, &qh, 1);
        surface.attach(Some(&buffer), 0, 0);
        surface.commit();
        for phase in 0..5 {
            queue.roundtrip(&mut client).unwrap();
            report.send(phase).unwrap();
            steps.recv_timeout(Duration::from_secs(5)).unwrap();
            queue.roundtrip(&mut client).unwrap();
            match phase {
                0 => assert!(client.presented.is_empty(), "unsubmitted or occluded content has no feedback"),
                1 => {
                    assert_eq!(client.presented.len(), 2);
                    assert!(client.presented.contains(&(1, Duration::from_secs(123))));
                    assert!(client.presented.contains(&(5, Duration::from_secs(123))));
                    assert_eq!(client.presentation_outputs, 2);
                    presentation.feedback(&surface, &qh, 2);
                    surface.commit();
                    presentation.feedback(&surface, &qh, 3);
                    surface.commit();
                }
                2 => {
                    assert_eq!(client.discarded, vec![2], "a later feedback-bearing commit supersedes the first");
                    assert_eq!(client.presented.len(), 2, "Home prevents presentation");
                }
                3 => {
                    assert_eq!(client.presented.len(), 3);
                    assert_eq!(client.presented.last(), Some(&(3, Duration::from_secs(456))));
                    presentation.feedback(&surface, &qh, 4);
                    surface.commit();
                    top.destroy();
                    xdg.destroy();
                    surface.destroy();
                    child.destroy();
                }
                4 => assert_eq!(client.discarded, vec![2, 4], "surface destruction discards outstanding feedback"),
                _ => unreachable!(),
            }
        }
    });
    let until = Instant::now() + Duration::from_secs(15);
    let mut phases = 0;
    while !thread.is_finished() && Instant::now() < until {
        display.dispatch_clients(&mut server).unwrap();
        server.frame();
        if let Ok(phase) = reports.try_recv() {
            let mut rendered = RenderElementStates::default();
            if phase < 4 {
                server.windows[0].window.with_surfaces(|surface, _| {
                    rendered.states.insert(
                        surface.into(),
                        RenderElementState {
                            visible_area: 1,
                            presentation_state: RenderElementPresentationState::Rendering { reason: None },
                            needs_capture: false,
                        },
                    );
                });
            }
            match phase {
                0 => {
                    server.present_submission(&RenderElementStates::default(), Duration::from_secs(100).into());
                }
                1 => {
                    server.present_submission(&rendered, Duration::from_secs(123).into());
                    server.present_submission(&rendered, Duration::from_secs(124).into()); // A completed callback is sent once.
                }
                2 => {
                    server.core.handle(Event::Home);
                    server.frame();
                    server.present_submission(&rendered, Duration::from_secs(200).into());
                }
                3 => {
                    let id = server.windows[0].id;
                    server.core.handle(clamshell_state::WindowEvent::Activate(id).into());
                    server.frame();
                    server.present_submission(&rendered, Duration::from_secs(456).into());
                }
                4 => assert!(server.windows.is_empty()),
                _ => unreachable!(),
            }
            phases += 1;
            next.send(()).unwrap();
        }
        display.flush_clients().unwrap();
        std::thread::yield_now();
    }
    assert!(thread.is_finished(), "client did not finish");
    thread.join().unwrap();
    assert_eq!(phases, 5);
}

#[test]
fn the_top_windows_new_buffers_shown_are_its_frame_rate() {
    let mut display = Display::<Server>::new().unwrap();
    let config = Config { top_size: (960, 540), bottom_size: (620, 540), touch_layout: Arc::default() };
    let mut server = Server::new(State::new(Init::default()), config, display.handle(), false, || {}, |_| Process::default(), |_| {}).unwrap();
    server.core.handle(Event::Start);
    server.frame();
    let (server_socket, client_socket) = UnixStream::pair().unwrap();
    display.handle().insert_client(server_socket, Arc::new(ClientState::default())).unwrap();
    let (report, reports) = mpsc::channel();
    let (next, steps) = mpsc::channel();
    // Ten frames with a new buffer each, then a commit without one, which is no frame.
    const STEPS: u32 = 11;
    let thread = std::thread::spawn(move || {
        let connection = Connection::from_socket(client_socket).unwrap();
        let (globals, mut queue) = registry_queue_init::<Client>(&connection).unwrap();
        let qh = queue.handle();
        let compositor: wl_compositor::WlCompositor = globals.bind(&qh, 1..=6, ()).unwrap();
        let shell: xdg_wm_base::XdgWmBase = globals.bind(&qh, 1..=6, ()).unwrap();
        let shm: wl_shm::WlShm = globals.bind(&qh, 1..=1, ()).unwrap();
        let surface = compositor.create_surface(&qh, ());
        let xdg = shell.get_xdg_surface(&surface, &qh, ());
        let top = xdg.get_toplevel(&qh, ());
        top.set_app_id("frames-test".into());
        surface.commit();
        let mut client = Client::default();
        queue.roundtrip(&mut client).unwrap();
        let mut file = tempfile::tempfile().unwrap();
        file.write_all(&vec![0xff; 960 * 540 * 4]).unwrap();
        let pool = shm.create_pool(file.as_fd(), 960 * 540 * 4, &qh, ());
        let buffer = pool.create_buffer(0, 960, 540, 960 * 4, wl_shm::Format::Abgr8888, &qh, ());
        for step in 0..STEPS {
            if step < 10 {
                surface.attach(Some(&buffer), 0, 0);
            }
            surface.commit();
            queue.roundtrip(&mut client).unwrap();
            report.send(step).unwrap();
            steps.recv_timeout(Duration::from_secs(5)).unwrap();
        }
    });
    let until = Instant::now() + Duration::from_secs(15);
    let start: Duration = server.clock.now().into();
    let mut measured = None;
    while !thread.is_finished() && Instant::now() < until {
        display.dispatch_clients(&mut server).unwrap();
        server.frame();
        if let Ok(step) = reports.try_recv() {
            let mut rendered = RenderElementStates::default();
            let root = server.windows[0].window.root().unwrap();
            let state =
                RenderElementState { visible_area: 1, presentation_state: RenderElementPresentationState::Rendering { reason: None }, needs_capture: false };
            rendered.states.insert((&root).into(), state);
            // 20 ms apart, as a 50 fps game's frames.
            server.present_submission(&rendered, (start + Duration::from_millis(20) * step).into());
            if step == STEPS - 1 {
                // Reported now rather than half a second after the last report.
                server.frames_due = None;
                server.turn();
                let metrics = server.core.state().sys().metrics();
                measured = Some((metrics.fps, metrics.frame_ms));
            }
            next.send(()).unwrap();
        }
        display.flush_clients().unwrap();
        std::thread::yield_now();
    }
    assert!(thread.is_finished(), "client did not finish");
    thread.join().unwrap();
    let (fps, frame_ms) = measured.unwrap();
    assert_eq!(fps, Some(10.0), "ten frames in the last second");
    assert!(frame_ms.is_some_and(|ms| (ms - 20.0).abs() < 0.01), "{frame_ms:?}");
}
