//! The nested host presents only when the displayed scene changes or the host asks for a redraw.
//! UI recordings, scene placement and client commits provide the damage; client pacing, animated
//! cursors, timed commits and readbacks keep their own deadlines instead of an idle frame loop.

mod demand;

use std::cell::RefCell;
use std::rc::Rc;
use std::time::{Duration, Instant};

use crate::EventSender;
use clamshell_state::{Effect, State};
use clamshell_ui::{Config, Error, Layer, Screen};
use smithay::{
    backend::{
        renderer::{Frame, Renderer, element::RenderElementStates, gles::GlesRenderer},
        winit::{self, WinitEvent, WinitGraphicsBackend},
    },
    reexports::winit::{dpi::PhysicalSize, window::WindowAttributes},
    utils::{Rectangle, Transform},
};

use crate::{
    base::Base,
    clients::Process,
    render::{self, UiFrame},
    server::Server,
};
use demand::{Redraw, Visuals};

const LAYERS: [Layer; 4] = [Layer::Screen(Screen::Top), Layer::Screen(Screen::Bottom), Layer::Overlay(Screen::Top), Layer::Overlay(Screen::Bottom)];

/// Runs the compositor in a desktop window, with the same colour pipeline as the device.
#[allow(clippy::too_many_arguments)]
pub fn run(
    state: State,
    config: Config,
    cursors: crate::CursorImages,
    start: impl FnOnce(EventSender, crate::Displays),
    on_effect: impl FnMut(&Effect) + 'static,
    identify: impl Fn(u32) -> Process + Send + Sync + 'static,
    on_capture: impl FnMut(Result<clamshell_state::ScreenshotPixels, String>) + 'static,
    quit: crate::QuitSignals,
) -> Result<(), Error> {
    let mut base = Base::new(state, config, true, identify, on_effect, quit)?;
    let server = &mut base.server;
    let profile = *server.core.state().profile();
    server.screens.millimetres = [crate::screens::shown_millimetres(None, &profile.top), crate::screens::shown_millimetres(None, &profile.bottom)];
    let attributes = WindowAttributes::default()
        .with_title("ClamShell nested")
        .with_surface_size(PhysicalSize::new(server.screens.size.0 as u32, server.screens.size.1 as u32));
    let (mut backend, events) = winit::init_from_attributes::<GlesRenderer>(attributes)?;
    server.advertise_dmabuf(backend.renderer());
    let pipeline = crate::colour::render::Pipeline::new(backend.renderer(), server)
        .map_err(|error| format!("nested compositor: the colour pipeline can't start, and nested draws only through it: {error}"))?;
    crate::colour::protocol::install(&server.dh);
    let (wake, source) = smithay::reexports::calloop::ping::make_ping()?;
    base.event_loop.handle().insert_source(source, |_, _, _| {}).map_err(|error| error.error)?;
    let host_colour = crate::colour::host::Host::new(backend.window(), wake);
    if host_colour.is_none() {
        log::info!("nested compositor: the host's colour management can't be reached (not a Wayland window), so the window is sRGB");
    }
    backend.window().set_cursor_visible(false);
    log::info!("nested output format: {:?}", backend.renderer().egl_context().pixel_format());
    // Event callbacks and the after-dispatch turn run sequentially on the same event loop.
    let drawing = Rc::new(RefCell::new(Drawing {
        pipeline,
        host_colour,
        cursor: crate::cursor::Cursor::new(cursors),
        frames: std::array::from_fn(|_| UiFrame::new()),
        elements: Vec::new(),
        states: std::array::from_fn(|_| RenderElementStates::default()),
        refresh_known: false,
        redraw: Redraw::default(),
        visuals: Visuals::default(),
        held: None,
        next_frame: Instant::now(),
        cursor_due: None,
        on_capture: Box::new(on_capture),
        timing: Default::default(),
        handle: base.event_loop.handle(),
        backend,
    }));
    let events_drawing = drawing.clone();
    base.event_loop
        .handle()
        .insert_source(events, move |event, _, server| {
            if let Err(error) = events_drawing.borrow_mut().event(event, server) {
                log::error!("nested compositor: {error}");
                server.exit = true;
            }
        })
        .map_err(|e| e.error)?;
    base.run(start, move |server, now| match drawing.borrow_mut().turn(server, now) {
        Ok(deadline) => deadline,
        Err(error) => {
            log::error!("nested compositor: {error}");
            server.exit = true;
            None
        }
    })
}

struct Drawing {
    timing: crate::frame_timing::Monitor,
    handle: smithay::reexports::calloop::LoopHandle<'static, Server>,
    pipeline: crate::colour::render::Pipeline,
    host_colour: Option<crate::colour::host::Host>,
    cursor: crate::cursor::Cursor,
    frames: [UiFrame; 4],
    elements: Vec<render::Element>,
    states: [RenderElementStates; 2],
    refresh_known: bool,
    redraw: Redraw,
    visuals: Visuals,
    held: Option<Instant>,
    next_frame: Instant,
    cursor_due: Option<Instant>,
    on_capture: Box<dyn FnMut(Result<clamshell_state::ScreenshotPixels, String>)>,
    // Keep the EGL context alive until its drawing resources have been dropped.
    backend: WinitGraphicsBackend<GlesRenderer>,
}

impl Drawing {
    fn event(&mut self, event: WinitEvent, server: &mut Server) -> Result<(), Error> {
        match event {
            WinitEvent::Resized { size, .. } => {
                self.refresh_known = false;
                server.resize((size.w, size.h));
                self.redraw.changed();
            }
            WinitEvent::Focus(focused) => {
                server.pointer.host_focused = focused;
                if !focused {
                    server.pointer.cursor_activity.hide();
                    server.touch_cancel();
                }
            }
            WinitEvent::Input(event) => {
                let whole = Rectangle::from_size((f64::from(server.screens.size.0), f64::from(server.screens.size.1)).into());
                server.input(event, crate::input::Host { absolute_cursor: true, touch_frames: false, desktop_keys: true }, |_| (whole, Transform::Normal));
            }
            WinitEvent::CloseRequested => server.core.handle(clamshell_state::Event::Quit),
            WinitEvent::Redraw => self.draw(server)?,
        }
        sync_capture(server, self.backend.window());
        Ok(())
    }

    fn colour(&mut self, server: &mut Server) -> Result<(), Error> {
        if !self.refresh_known
            && let Some(refresh) = host_refresh(self.backend.window())
        {
            self.refresh_known = true;
            if refresh != server.screens.refresh {
                log::info!("nested compositor: host refresh {} Hz", f64::from(refresh) / 1000.0);
                server.screens.refresh = refresh;
                server.resize(server.screens.size);
                self.redraw.changed();
            }
        }
        if let Some(host) = &mut self.host_colour {
            if let Some(preferred) = host.turn()? {
                log::info!("nested compositor: host preferred colour {preferred:?}");
            }
            let (peak, average) = server.colour_volume();
            if let Some(planned) = host.planned_encoding(peak, average) {
                host.describe(planned);
            }
            host.flush()?;
            if let Some(encoding) = host.encoding() {
                if self.pipeline.output_description(false) != encoding {
                    self.redraw.changed();
                }
                self.pipeline.set_outputs(self.backend.renderer(), [encoding; 2])?;
            }
            if let Some(advertised) = host.advertised() {
                server.describe_screens([advertised; 2]);
            }
        }
        Ok(())
    }

    fn turn(&mut self, server: &mut Server, now: Instant) -> Result<Option<Instant>, Error> {
        if let Some(sample) = self.timing.sample() {
            log::debug!("nested frame timing: cpu_us={} ready_us={}", sample.cpu.as_micros(), sample.ready.as_micros());
        }
        let interval = Duration::from_secs_f64(1000.0 / f64::from(server.screens.refresh.max(1000)));
        let clock: Duration = server.clock.now().into();
        let timed =
            server.release_timed_commits((clock + interval).into(), None).map(|at| now + Duration::from(at).saturating_sub(clock).saturating_sub(interval));
        // Facts and imports cannot wait for a redraw: clients may be waiting for their answers.
        server.frame();
        server.import_dmabufs(self.backend.renderer());
        self.colour(server)?;
        sync_capture(server, self.backend.window());
        if let Some(histogram) = self.pipeline.take_histogram(self.backend.renderer(), server) {
            server.report_histogram(histogram);
        }
        if self.cursor_due.is_some_and(|due| now >= due) {
            self.redraw.changed();
        }
        self.cursor_due = self.cursor.deadline(server, now);
        if self.held.is_some_and(|due| now >= due) {
            self.held = server.frame_callbacks(None, interval, now);
        }
        let before = server.core.state_revision();
        if record_ui(server) {
            self.redraw.changed();
        }
        let reported = server.core.state_revision() != before;
        if self.visuals.observe(server) || server.screenshot_requested {
            self.redraw.changed();
        }
        let ready = self.host_colour.as_ref().is_none_or(|host| host.ready());
        if ready && now >= self.next_frame && self.redraw.pending() {
            self.draw(server)?;
        }
        // Keep displayed resources without drawing again. Temporary readbacks/passes retain their
        // normal expiry deadlines, even while the window has no new frame to show.
        self.pipeline.keep_composed(now);
        let mut trims = crate::hold::Trims::default();
        self.pipeline.trim(now, &mut trims);
        for (frame, layer) in self.frames.iter_mut().zip(LAYERS) {
            if server.scene.shown().iter().any(|shown| matches!(shown, crate::scene::Shown::Software { layer: shown, .. } if *shown == layer)) {
                frame.keep(now);
            }
            frame.trim(now, &mut trims);
        }
        Ok(timed
            .into_iter()
            .chain(trims.finish())
            .chain(self.cursor_due.filter(|_| ready))
            .chain(self.pipeline.histogram_due(server, now))
            .chain(self.held)
            .chain(reported.then_some(now))
            .chain((ready && self.redraw.pending()).then_some(self.next_frame))
            .min())
    }

    fn draw(&mut self, server: &mut Server) -> Result<(), Error> {
        self.colour(server)?;
        if self.host_colour.as_ref().is_some_and(|host| !host.ready()) {
            self.redraw.changed();
            return Ok(());
        }
        self.timing.begin(Instant::now());
        self.redraw.submitted();
        self.next_frame = Instant::now() + Duration::from_secs_f64(1000.0 / f64::from(server.screens.refresh.max(1000)));
        server.frame();
        self.colour(server)?;
        let Self { backend, pipeline, frames, cursor, elements, states, on_capture, host_colour, timing, handle, .. } = self;
        sync_capture(server, backend.window());
        let capturing = std::mem::take(&mut server.screenshot_requested);
        for (down, states) in [false, true].into_iter().zip(states.iter_mut()) {
            render::elements(server, frames, cursor, backend.renderer(), elements, down, None, Default::default())?;
            pipeline.compose(server, backend.renderer(), down, elements, states, false)?;
        }
        if let Some(histogram) = pipeline.take_histogram(backend.renderer(), server) {
            server.report_histogram(histogram);
        }
        {
            let (renderer, mut framebuffer) = backend.bind()?;
            let sync = pipeline.present(renderer, &mut framebuffer, server.screens.size)?;
            timing.submitted(&sync, handle);
            if capturing {
                // (Read at once: the host's frame waits for the GPU anyway.)
                let captured = pipeline
                    .capture(renderer, server.screens.size)
                    .and_then(|()| pipeline.captured(renderer, true).unwrap_or_else(|| Err("no screenshot".into())));
                if let Err(error) = &captured {
                    log::warn!("compositor screenshot: {error}");
                }
                // bind() is lazy. Begin an empty frame to actually restore the
                // window EGL target after the surfaceless readback mapping.
                // No drawing is submitted by this frame; swap supplies presentation synchronization.
                drop(renderer.render(&mut framebuffer, server.screens.size.into(), Transform::Flipped180)?.finish()?);
                on_capture(captured);
            }
        }
        if let Some(host) = host_colour {
            host.queued()?;
        }
        backend.submit(Some(&[Rectangle::from_size(backend.window_size())]))?;
        for (down, states) in [false, true].into_iter().zip(states.iter()) {
            server.frame_queued(states, Some(down));
            server.frame_shown(states, Some(down));
            server.present_submission(states, server.clock.now());
        }

        // Remember only content composed before callbacks unblock any later FIFO commits.
        self.visuals.observe(server);
        self.held = server.frame_callbacks(None, Duration::from_secs_f64(1000.0 / f64::from(server.screens.refresh.max(1000))), Instant::now());
        self.cursor_due = self.cursor.deadline(server, Instant::now());
        log::debug!("nested frame submitted");
        Ok(())
    }
}

/// Record stale visible layers to discover real UI damage without a GPU draw.
fn record_ui(server: &mut Server) -> bool {
    server.place();
    let mut changed = false;
    for layer in LAYERS {
        if layer == Layer::Screen(Screen::Top) && server.scene.covers_top() {
            continue;
        }
        if server.core.stale(layer) {
            changed |= server.core.record(layer, false).is_some();
        }
    }
    server.core.frames_done();
    server.place();
    changed
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
