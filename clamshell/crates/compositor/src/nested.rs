//! A desktop window containing two logical outputs. It never takes control of the host session.

use std::os::fd::AsFd;
use std::time::{Duration, Instant};

use clamshell_state::{Effect, Event, State};
use clamshell_ui::{Config, Error, EventSender};
use smithay::{
    backend::{
        renderer::{damage::OutputDamageTracker, gles::GlesRenderer},
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
) -> Result<(), Error> {
    let mut event_loop: EventLoop<Server> = EventLoop::try_new()?;
    let mut display = Display::<Server>::new()?;
    let (wake, woken) = ping::make_ping()?;
    event_loop.handle().insert_source(woken, |_, _, _| {}).map_err(|e| e.error)?;
    let mut server = Server::new(state, config, display.handle(), move || wake.ping(), identify, on_effect)?;
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

    let attributes = WindowAttributes::default().with_title("ClamShell nested").with_surface_size(LogicalSize::new(1580.0, 540.0));
    let (mut backend, events) = winit::init_from_attributes::<GlesRenderer>(attributes)?;
    log::info!("nested output format: {:?}", backend.renderer().egl_context().pixel_format());
    let mut frames = [UiFrame::new((960, 540))?, UiFrame::new((620, 540))?];
    let mut arrows = ArrowFrame::new((960, 540))?;
    let mut elements = Vec::new();
    let mut submitted = false;
    let mut damage = OutputDamageTracker::new(backend.window_size(), 1.0, Transform::Flipped180);
    event_loop
        .handle()
        .insert_source(events, move |event, _, server| {
            let result: Result<(), Error> = (|| {
                match event {
                    WinitEvent::Resized { size, .. } => {
                        server.resize((size.w, size.h));
                        damage = OutputDamageTracker::new(size, 1.0, Transform::Flipped180);
                    }
                    WinitEvent::Focus(false) => server.touch_cancel(),
                    WinitEvent::Input(event) => server.input(event),
                    WinitEvent::CloseRequested => server.exit = true,
                    WinitEvent::Redraw => {
                        server.frame();
                        let age = if submitted { backend.buffer_age().unwrap_or(0) } else { 0 };
                        {
                            let (renderer, mut framebuffer) = backend.bind()?;
                            render::elements(server, &mut frames, &mut arrows, renderer, &mut elements)?;
                            damage.render_output(renderer, &mut framebuffer, age, &elements, [0.0, 0.0, 0.0, 1.0])?;
                        }
                        backend.submit(Some(&[Rectangle::from_size(backend.window_size())]))?;
                        submitted = true;
                        for client in server.windows.iter().filter(|w| w.mapped && server.visible(w.id)) {
                            let [top, bottom] = &server.outputs;
                            let output = if server.core.state().session().window(client.id).is_some_and(|w| w.down) { bottom } else { top };
                            client.window.send_frame(output, server.started.elapsed(), Some(Duration::ZERO), |_, _| Some(output.clone()));
                        }
                        backend.window().request_redraw();
                    }
                    _ => {}
                }
                Ok(())
            })();
            if let Err(error) = result {
                log::error!("nested compositor: {error}");
                server.exit = true;
            }
        })
        .map_err(|e| e.error)?;
    let runtime = std::env::var_os("XDG_RUNTIME_DIR").ok_or("nested compositor: XDG_RUNTIME_DIR is missing")?;
    start(sender, std::path::PathBuf::from(runtime).join(&server.socket));
    server.core.handle(Event::Start);
    while !server.exit {
        let wait = server.core.deadline().map(|at| at.saturating_duration_since(Instant::now()));
        event_loop.dispatch(wait, &mut server)?;
        server.turn();
        server.dh.flush_clients()?;
    }
    Ok(())
}
