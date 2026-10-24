//! A desktop window containing two logical outputs. It never takes control of the host session.

use std::os::fd::AsFd;
use std::time::{Duration, Instant};

use clamshell_state::{Effect, Event, State};
use clamshell_ui::{Config, Error, EventSender};
use smithay::{
    backend::{
        renderer::{Frame, Renderer, damage::OutputDamageTracker, gles::GlesRenderer},
        winit::{self, WinitEvent},
    },
    reexports::{
        calloop::{EventLoop, Interest, Mode, PostAction, channel, generic::Generic, ping},
        wayland_server::Display,
        winit::{dpi::LogicalSize, window::WindowAttributes},
    },
    utils::{Rectangle, Transform},
    wayland::socket::ListeningSocketSource,
};

use crate::{
    render::{self, ArrowFrame, UiFrame},
    server::{Process, Server, insert_client},
};

/// Runs the experimental nested compositor. Launch environments are scoped to its socket; the
/// process environment and the parent's compositor are left alone.
pub fn run(
    state: State,
    config: Config,
    start: impl FnOnce(EventSender, std::path::PathBuf),
    on_effect: impl FnMut(&Effect) + 'static,
    identify: impl Fn(u32) -> Process + 'static,
    mut on_capture: impl FnMut(Result<crate::Capture, String>) + 'static,
) -> Result<(), Error> {
    let mut event_loop: EventLoop<Server> = EventLoop::try_new()?;
    let mut display = Display::<Server>::new()?;
    let (wake, woken) = ping::make_ping()?;
    event_loop.handle().insert_source(woken, |_, _, _| {}).map_err(|e| e.error)?;
    let mut server = Server::new(state, config, display.handle(), false, move || wake.ping(), identify, on_effect)?;
    let socket = ListeningSocketSource::new_auto()?;
    server.socket = socket.socket_name().to_string_lossy().into_owned();
    log::info!("nested compositor: WAYLAND_DISPLAY={}", server.socket);
    event_loop.handle().insert_source(socket, |stream, _, server| insert_client(&mut server.dh, stream)).map_err(|e| e.error)?;
    // The duplicated descriptor is only a readiness source. The closure owns the display, so dispatch
    // does not need an unsafe mutable borrow of calloop's Generic storage.
    let fd = display.as_fd().try_clone_to_owned()?;
    event_loop
        .handle()
        .insert_source(Generic::new(fd, Interest::READ, Mode::Level), move |_, _, server| {
            display.dispatch_clients(server)?;
            Ok(PostAction::Continue)
        })
        .map_err(|e| e.error)?;
    let (sender, receiver) = EventSender::channel();
    event_loop
        .handle()
        .insert_source(receiver, |event, _, server| {
            if let channel::Event::Msg(event) = event {
                server.core.handle(event);
            }
        })
        .map_err(|e| e.error)?;

    let attributes =
        WindowAttributes::default().with_title("ClamShell nested").with_surface_size(LogicalSize::new(f64::from(server.size.0), f64::from(server.size.1)));
    let (mut backend, events) = winit::init_from_attributes::<GlesRenderer>(attributes)?;
    server.advertise_dmabuf(backend.renderer());
    backend.window().set_cursor_visible(false);
    let mut cursor = crate::cursor::Cursor::new();
    log::info!("nested output format: {:?}", backend.renderer().egl_context().pixel_format());
    let mut frames = [UiFrame::new((960, 540))?, UiFrame::new((620, 540))?];
    let mut arrows = ArrowFrame::new((960, 540))?;
    let mut elements = Vec::new();
    let mut submitted = false;
    let mut refresh_known = false;
    let mut damage = OutputDamageTracker::new(backend.window_size(), 1.0, Transform::Flipped180);
    event_loop
        .handle()
        .insert_source(events, move |event, _, server| {
            let result: Result<(), Error> = (|| {
                match event {
                    WinitEvent::Resized { size, .. } => {
                        refresh_known = false;
                        server.resize((size.w, size.h));
                        damage = OutputDamageTracker::new(size, 1.0, Transform::Flipped180);
                    }
                    WinitEvent::Focus(focused) => {
                        server.host_focused = focused;
                        if !focused {
                            server.cursor_activity.hide();
                            server.touch_cancel();
                        }
                    }
                    WinitEvent::Input(event) => server.input(event),
                    WinitEvent::CloseRequested => server.exit = true,
                    WinitEvent::Redraw => {
                        // The host tells which monitor shows the window only after it is shown, so ask
                        // at each frame until it does, and again after a resize.
                        if !refresh_known && let Some(refresh) = host_refresh(backend.window()) {
                            refresh_known = true;
                            if refresh != server.refresh {
                                log::info!("nested compositor: host refresh {} Hz", f64::from(refresh) / 1000.0);
                                server.refresh = refresh;
                                server.resize(server.size);
                            }
                        }
                        server.frame();
                        sync_capture(server, backend.window());
                        let imported = server.import_dmabufs(backend.renderer());
                        // Imports may make EGL surfaceless. Force a rendered frame to restore the
                        // window target even when the scene has not otherwise changed.
                        let age = if submitted && !imported { backend.buffer_age().unwrap_or(0) } else { 0 };
                        let capturing = std::mem::take(&mut server.screenshot_requested);
                        let states;
                        {
                            let (renderer, mut framebuffer) = backend.bind()?;
                            render::elements(server, &mut frames, &mut arrows, renderer, &mut elements)?;
                            cursor.elements(server, renderer, &mut elements)?;
                            states = damage.render_output(renderer, &mut framebuffer, age, &elements, [0.0, 0.0, 0.0, 1.0])?.states;
                            if capturing {
                                let captured = render::capture(renderer, &framebuffer, server.size);
                                if let Err(error) = &captured {
                                    log::warn!("compositor screenshot: {error}");
                                }
                                // bind() is lazy. Begin an empty frame to actually restore the
                                // window EGL target after the surfaceless readback mapping.
                                // No drawing is submitted by this frame; swap supplies presentation synchronization.
                                drop(renderer.render(&mut framebuffer, server.size.into(), Transform::Flipped180)?.finish()?);
                                on_capture(captured);
                            }
                        }
                        backend.submit(Some(&[Rectangle::from_size(backend.window_size())]))?;
                        submitted = true;
                        server.present_submission(&states, server.clock.now());
                        for client in server.windows.iter().filter(|w| w.mapped && server.visible(w.id)) {
                            let [top, bottom] = &server.outputs;
                            let output = if server.core.state().session().window(client.id).is_some_and(|w| w.down) { bottom } else { top };
                            client.window.send_frame(output, server.started.elapsed(), Some(Duration::ZERO), |_, _| Some(output.clone()));
                        }
                        backend.window().request_redraw();
                    }
                }
                sync_capture(server, backend.window());
                Ok(())
            })();
            if let Err(error) = result {
                log::error!("nested compositor: {error}");
                server.exit = true;
            }
        })
        .map_err(|e| e.error)?;
    let mut x11 = crate::x11::Supervisor::new(event_loop.handle())?;
    server.x11_display = Some(x11.display());
    let runtime = std::env::var_os("XDG_RUNTIME_DIR").ok_or("nested compositor: XDG_RUNTIME_DIR is missing")?;
    start(sender, std::path::PathBuf::from(runtime).join(&server.socket));
    server.core.handle(Event::Start);
    while !server.exit {
        let now = Instant::now();
        x11.turn(&mut server, now);
        let wait = server
            .core
            .deadline()
            .into_iter()
            .chain(server.cursor_activity.deadline(now))
            .chain(x11.deadline())
            .min()
            .map(|at| at.saturating_duration_since(now));
        event_loop.dispatch(wait, &mut server)?;
        server.turn();
        server.dh.flush_clients()?;
    }
    Ok(())
}

/// The refresh of the monitor showing the window, in mHz, once the host says.
fn host_refresh(window: &dyn smithay::reexports::winit::window::Window) -> Option<i32> {
    let refresh = window.current_monitor()?.current_video_mode()?.refresh_rate_millihertz()?;
    i32::try_from(refresh.get()).ok()
}

fn sync_capture(server: &mut Server, window: &dyn smithay::reexports::winit::window::Window) {
    use crate::pointer::CaptureMode;
    use smithay::reexports::winit::window::CursorGrabMode;
    server.sync_pointer_capture(|mode| {
        window.set_cursor_grab(match mode {
            CaptureMode::Free => CursorGrabMode::None,
            CaptureMode::Locked => CursorGrabMode::Locked,
            CaptureMode::Confined => CursorGrabMode::Confined,
        })
    });
}
