//! ClamShell's compositor on the machine's own screens: a seat from libseat, the GPU through DRM and GBM,
//! input from libinput. The device profile names the connector of each screen and how its panel is
//! mounted. Each screen is composed on its own (the colour pipeline, a scene each), turned as its panel
//! is; its page flips pace its frames and the composing of commits to it, independently of the other
//! screen's, and their times are the presentation feedback clients get.

use std::cell::RefCell;
use std::rc::Rc;
use std::time::{Duration, Instant};

use crate::EventSender;
use crate::plane_layers::{PlaneLayers, Planed};
use crate::presenter::Presenter;
use crate::ui_scanout::UiScanout;
use crate::window_scanout::{Candidate, Shape, WindowScanout};
use clamshell_device::{DeviceProfile, Rotation, ScreenRole, TouchSelector, Touchscreen};
use clamshell_state::colour::ColourDescription;
use clamshell_state::{Effect, State};
use clamshell_ui::{Config, Error};
use smithay::{
    backend::{
        allocator::{
            Fourcc,
            gbm::{GbmAllocator, GbmBufferFlags, GbmDevice},
        },
        drm::{DrmDevice, DrmDeviceFd, DrmEvent, DrmEventMetadata, DrmEventTime},
        egl::{EGLContext, EGLDisplay},
        input::{DeviceCapability, InputEvent},
        libinput::{LibinputInputBackend, LibinputSessionInterface},
        renderer::{Bind, ImportDma, element::RenderElementStates, gles::GlesRenderer},
        session::{Event as SessionEvent, Session, libseat::LibSeatSession},
    },
    reexports::{
        drm::control::{Device as ControlDevice, ModeTypeFlags, connector, crtc},
        input::{Device as InputDevice, Libinput},
        rustix::fs::OFlags,
    },
    utils::{Buffer, DeviceFd, Logical, Monotonic, Physical, Rectangle, Size, Time, Transform},
};

use crate::{
    base::Base,
    clients::Process,
    colour::render::Pipeline,
    render::{self, UiFrame},
    server::Server,
};

/// One of the two screens: its connector's CRTC, its swapchain, and where it is in the scene.
struct Screen {
    role: ScreenRole,
    crtc: crtc::Handle,
    surface: Presenter,
    /// The elements' states in the frame queued, for its flip; refilled in place for each frame, so that
    /// queueing one allocates nothing.
    queued: RenderElementStates,
    /// The presentation feedback of the frame queued, for its flip.
    feedback: crate::pacing::Feedback,
    /// The screen's part of the scene, and its mode's size before its panel's turn.
    part: Rectangle<i32, Buffer>,
    size: Size<i32, Physical>,
    transform: Transform,
    /// How long a frame is shown.
    interval: Duration,
    /// The scene changed in the screen's part since its last frame.
    dirty: bool,
    /// The commits its last frame callbacks answered, and when an idle screen may send the next ones.
    answered: u64,
    next_callbacks: Option<Instant>,
    /// When a capped window's frame callback held back is due.
    held: Option<Instant>,
    /// Whether it is on, as the state last asked.
    on: bool,
    /// The connector's modes of the screen's size, the one it started with, and the refresh last asked
    /// for (0: the starting one).
    modes: Vec<smithay::reexports::drm::control::Mode>,
    start_mode: smithay::reexports::drm::control::Mode,
    refresh: u16,
    /// When its last frame was shown, by the flip's time.
    flipped: Option<Time<Monotonic>>,
    /// What it shows (see [`shows`]).
    shows: Shows,
    /// Its own buffers, with CLAMSHELL_SCANOUT=1, for while it shows nothing but ClamShell (see
    /// `ui_scanout`).
    ui: Option<UiScanout>,
    /// With CLAMSHELL_SCANOUT=1, its means to show a window from the window's own buffer (see
    /// `window_scanout`), the window's buffer it shows so, the window told the plane's formats, the shape
    /// the display engine last took, and why a window alone on it is composed, as last logged.
    window: Option<WindowScanout>,
    candidate: Option<Candidate>,
    fed: Option<i64>,
    tested: Option<Shape>,
    composed_why: Option<&'static str>,
    /// With CLAMSHELL_SCANOUT=1, its overlay layer and cursor on planes of their own (see `plane_layers`),
    /// and whether what they show changed since its last commit.
    layers: Option<PlaneLayers>,
    layers_dirty: bool,
    /// Whether its commits fail for a while now (busy, or failing otherwise), logged at the first.
    failing: bool,
    /// Whether turning it off fails, tried again each turn: logged at the first.
    off_failing: bool,
    /// Whether composing it fails, every turn: logged at the first.
    compose_failing: bool,
}

impl Screen {
    /// Drops what its buffers hold, as after it was off or away, or a flip was lost: the next frame is
    /// drawn whole, composed or its own.
    fn reset_buffers(&mut self) {
        self.surface.reset_buffers();
        // A frame in flight is taken as never shown.
        self.feedback.discard();
        if let Some(ui) = &mut self.ui {
            ui.reset();
        }
        if let Some(layers) = &mut self.layers {
            layers.reset();
            self.layers_dirty = true;
        }
    }

    /// Which of its layers are on their planes this turn.
    fn planed(&self) -> Planed {
        self.layers.as_ref().map(|layers| layers.planed).unwrap_or_default()
    }

    /// When a frame composed at `now` would be shown (see [`next_shown`]).
    fn next_shown(&self, now: Time<Monotonic>) -> Time<Monotonic> {
        Time::from(next_shown(self.flipped.map(Duration::from), self.interval, self.surface.in_flight(), Duration::from(now)))
    }
}

/// When a frame composed at `now` is shown on a screen showing one every `interval`, last at `flipped`:
/// the next refresh after `now`, whole refreshes on from the last flip, and a refresh later if a frame is
/// `pending` already.
fn next_shown(flipped: Option<Duration>, interval: Duration, pending: bool, now: Duration) -> Duration {
    let interval = interval.max(Duration::from_micros(1));
    let next = match flipped {
        Some(flipped) if flipped <= now => {
            let whole = u32::try_from(now.saturating_sub(flipped).as_nanos() / interval.as_nanos()).unwrap_or(u32::MAX);
            flipped + interval.saturating_mul(whole.saturating_add(1))
        }
        _ => now + interval,
    };
    if pending { next + interval } else { next }
}

/// What drawing needs, shared by the loop's sources.
struct Drawing {
    renderer: GlesRenderer,
    pipeline: Pipeline,
    screens: Vec<Screen>,
    /// The top screen's UI, the second's, and their overlays.
    frames: [UiFrame; 4],
    cursor: crate::cursor::Cursor,
    elements: Vec<render::Element>,
    /// Each screen's elements as last composed.
    states: [RenderElementStates; 2],
    /// Whether the seat is another session's (another VT shown): nothing is drawn or committed meanwhile.
    away: bool,
}

/// A connector chosen for a screen: its handle, mode, the modes it has of that size, its CRTC, and its
/// panel's size in millimetres, if it says.
struct Chosen {
    connector: connector::Handle,
    mode: smithay::reexports::drm::control::Mode,
    modes: Vec<smithay::reexports::drm::control::Mode>,
    crtc: crtc::Handle,
    millimetres: Option<(u32, u32)>,
}

/// Runs the compositor on the machine's screens until ClamShell quits. With `scanout` (CLAMSHELL_SCANOUT=1),
/// direct scanout is tried (see `crate::scanout`).
#[allow(clippy::too_many_arguments)]
pub fn run(
    state: State,
    mut config: Config,
    start: impl FnOnce(EventSender, crate::Displays),
    on_effect: impl FnMut(&Effect) + 'static,
    identify: impl Fn(u32) -> Process + Send + Sync + 'static,
    on_capture: impl FnMut(Result<clamshell_state::ScreenshotPixels, String>) + 'static,
    quit: crate::QuitSignals,
    scanout: bool,
) -> Result<(), Error> {
    let profile = *state.profile();
    let (mut session, notifier) = LibSeatSession::new().map_err(|e| format!("drm: no seat: {e}"))?;
    let seat = session.seat();
    let gpu = smithay::backend::udev::primary_gpu(&seat)?
        .or_else(|| smithay::backend::udev::all_gpus(&seat).inspect_err(|e| log::warn!("drm: listing the seat's GPUs: {e}")).ok()?.into_iter().next())
        .ok_or("drm: no GPU on the seat")?;
    log::info!("drm: {} on seat {seat}", gpu.display());
    let fd = session.open(&gpu, OFlags::RDWR | OFlags::CLOEXEC | OFlags::NOCTTY | OFlags::NONBLOCK).map_err(|e| format!("drm: {}: {e}", gpu.display()))?;
    let fd = DrmDeviceFd::new(DeviceFd::from(fd));
    // The connectors are left as they are, so each screen can keep the mode the kernel already set.
    if scanout {
        crate::plane_layers::ask_for_cursor_planes(&fd);
    }
    let (mut drm, drm_notifier) = DrmDevice::new(fd.clone(), false).map_err(|e| format!("drm: {e}"))?;
    let sync_fd = fd.clone();
    let gbm = GbmDevice::new(fd).map_err(|e| format!("drm: gbm: {e}"))?;
    let mut renderer = gles(gbm.clone())?;

    let top = choose(&drm, &profile, ScreenRole::Top, None)?;
    // Without the second screen (a broken panel, an output missing), ClamShell runs on the top screen: the
    // second is laid out as usual, but shown nowhere.
    let bottom = choose(&drm, &profile, ScreenRole::Bottom, Some(top.crtc)).inspect_err(|e| log::error!("{e}: running on the top screen alone")).ok();
    if scanout {
        let mut crtcs = vec![("the top screen", top.crtc)];
        crtcs.extend(bottom.as_ref().map(|b| ("the second screen", b.crtc)));
        crate::scanout::report(&drm, &crtcs);
    }
    let logical = |chosen: &Chosen, role: ScreenRole| {
        let (w, h) = chosen.mode.size();
        let turned = transform(profile.screen(role).rotation).transform_size(Size::<i32, Physical>::from((i32::from(w), i32::from(h))));
        Size::<i32, Logical>::from((turned.w, turned.h))
    };
    let top_size = logical(&top, ScreenRole::Top);
    let bottom_size = bottom.as_ref().map_or(top_size, |b| logical(b, ScreenRole::Bottom));
    config.top_size = (top_size.w as u32, top_size.h as u32);
    config.bottom_size = (bottom_size.w as u32, bottom_size.h as u32);

    let mut base = Base::new(state, config, true, identify, on_effect, quit)?;
    let server = &mut base.server;
    let bottom_millimetres = bottom.as_ref().and_then(|b| b.millimetres);
    server.screens.millimetres =
        [crate::screens::shown_millimetres(top.millimetres, &profile.top), crate::screens::shown_millimetres(bottom_millimetres, &profile.bottom)];
    log::info!("drm: the screens' millimetres {:?} (connector {:?}, {:?})", server.screens.millimetres, top.millimetres, bottom_millimetres);
    // No host window takes the focus away: pointer capture is always possible.
    server.pointer.host_focused = true;
    server.screens.refresh = i32::try_from(top.mode.vrefresh()).unwrap_or(60).saturating_mul(1000);
    server.resize(server.screens.size);
    server.advertise_dmabuf(&renderer);
    // Explicit sync, where the GPU can wait on a timeline point by an eventfd; without it, clients' buffers
    // are waited on by their implicit fences (see `CompositorHandler::new_surface`).
    if smithay::wayland::drm_syncobj::supports_syncobj_eventfd(&sync_fd) {
        server.protocols.syncobj = Some(smithay::wayland::drm_syncobj::DrmSyncobjState::new::<Server>(&server.dh, sync_fd));
        log::info!("drm: explicit sync offered");
    } else {
        log::info!("drm: no syncobj eventfd on {}: no explicit sync, buffers' implicit fences only", gpu.display());
    }
    let mut pipeline = Pipeline::new(&mut renderer, server)?;
    crate::colour::protocol::install(&server.dh);
    // Each screen encoded for its panel, and described to clients as it, from the device profile.
    let panels = [ColourDescription::from(profile.top.colour), ColourDescription::from(profile.bottom.colour)];
    pipeline.set_outputs(&mut renderer, panels)?;
    server.describe_screens(panels);
    log::info!("drm: panels' colour: top {:?}, second {:?}", panels[0], panels[1]);

    let renderer_formats = renderer.dmabuf_formats();
    let mut screens = Vec::new();
    let shown = [Some((ScreenRole::Top, &top, 0)), bottom.as_ref().map(|b| (ScreenRole::Bottom, b, top_size.w))];
    for (role, chosen, x) in shown.into_iter().flatten() {
        let surface = drm.create_surface(chosen.crtc, chosen.mode, &[chosen.connector]).map_err(|e| format!("drm: {role:?} screen: {e}"))?;
        // A screen turned on or off, or to another refresh, is a modeset, which on the panels' power
        // sequences takes tens to hundreds of ms: it doesn't wait for it, so neither do the other screen's
        // frames and the input. A commit made before it is done is busy and tried again.
        surface.set_nonblocking_modesets(true);
        let allocator = GbmAllocator::new(gbm.clone(), GbmBufferFlags::RENDERING | GbmBufferFlags::SCANOUT);
        // Ten bits a channel where the screen takes them, for HDR later; else eight.
        let renderer_formats: Vec<_> = renderer_formats.iter().copied().collect();
        let surface = Presenter::new(surface, allocator, &[Fourcc::Xrgb2101010, Fourcc::Xrgb8888], &renderer_formats)
            .map_err(|e| format!("drm: {role:?} screen: {e}"))?;
        log::info!("drm: {role:?} screen on {} at {:?}, {}", profile.screen(role).connector, chosen.mode.size(), surface.format());
        let (w, h) = chosen.mode.size();
        let size = Size::from((i32::from(w), i32::from(h)));
        let width = if role == ScreenRole::Top { top_size.w } else { bottom_size.w };
        let part = Rectangle::new((x, 0).into(), (width, server.screens.size.1).into());
        let interval = Duration::from_micros(1_000_000 / u64::from(chosen.mode.vrefresh().max(1)));
        screens.push(Screen {
            role,
            crtc: chosen.crtc,
            surface,
            part,
            size,
            transform: transform(profile.screen(role).rotation),
            interval,
            dirty: true,
            answered: 0,
            next_callbacks: None,
            held: None,
            on: true,
            modes: chosen.modes.clone(),
            start_mode: chosen.mode,
            refresh: 0,
            flipped: None,
            queued: RenderElementStates::default(),
            feedback: Default::default(),
            shows: Shows::Composed,
            ui: None,
            window: None,
            candidate: None,
            fed: None,
            tested: None,
            composed_why: None,
            layers: None,
            layers_dirty: false,
            failing: false,
            off_failing: false,
            compose_failing: false,
        });
    }
    // With CLAMSHELL_SCANOUT=1, each screen's own buffers, for while it shows only ClamShell.
    if scanout {
        for (screen, panel) in screens.iter_mut().zip(panels) {
            let layer = ui_screen(screen.role);
            let ((w, h), scale) = server.core.size(layer);
            // Turned as the composed frame turns it: a quarter clockwise in Smithay's projection for
            // `Transform::_90`, a screen pixel (x, y) landing in column h - 1 - y, row x.
            let quarter = profile.screen(screen.role).rotation == Rotation::Clockwise90;
            let mode = screen.surface.pending_mode().size();
            let formats = &screen.surface.surface().plane_info().formats;
            match UiScanout::new(drm.device_fd(), screen.crtc, mode, formats, (w * scale, h * scale), quarter, panel) {
                Ok(ui) => {
                    log::info!("ui scanout: the {:?} screen can show ClamShell alone from its own buffers", screen.role);
                    screen.ui = Some(ui);
                }
                Err(error) => log::warn!("ui scanout: the {:?} screen is composed: {error}", screen.role),
            }
        }
        // And each screen's means to show a window from its own buffer, with the feedback a window alone on
        // it gets: the plane's formats the renderer takes too (to compose them if refused), preferred.
        let main = crate::dmabuf::render_device(&renderer);
        let node = smithay::backend::drm::DrmNode::from_file(drm.device_fd());
        for screen in &mut screens {
            let formats = screen.surface.surface().plane_info().formats.clone();
            let preferred: Vec<_> = formats.iter().copied().filter(|f| renderer_formats.contains(f)).collect();
            let feedback = match (main, &node) {
                (Some(main), Ok(node)) => smithay::wayland::dmabuf::DmabufFeedbackBuilder::new(main, renderer_formats.iter().copied())
                    .add_preference_tranche(
                        node.dev_id(),
                        smithay::reexports::wayland_protocols::wp::linux_dmabuf::zv1::server::zwp_linux_dmabuf_feedback_v1::TrancheFlags::Scanout,
                        preferred,
                        4..=u32::MAX,
                    )
                    .build()
                    .map_err(|e| e.to_string()),
                (None, _) => Err("no render device".into()),
                (_, Err(e)) => Err(format!("the display device: {e}")),
            };
            let feedback =
                feedback.inspect_err(|error| log::warn!("window scanout: the {:?} screen tells no window the plane's formats: {error}", screen.role)).ok();
            let plane = screen.surface.surface().plane();
            screen.window = Some(WindowScanout::new(drm.device_fd(), &gbm, plane, formats, feedback));
        }
        // And each screen's overlay and cursor planes, each plane for one screen only.
        let mut taken = Vec::new();
        for screen in &mut screens {
            let ((w, h), scale) = server.core.size(ui_screen(screen.role));
            let quarter = profile.screen(screen.role).rotation == Rotation::Clockwise90;
            let role = format!("{:?}", screen.role);
            screen.layers = Some(PlaneLayers::new(&drm, screen.crtc, (w * scale, h * scale), quarter, &mut taken, &role));
        }
    }

    // The top screen's rates, for the refresh setting.
    let mut rates = clamshell_state::presentation::modes::DisplayModes::default();
    for mode in &top.modes {
        rates.add(i32::try_from(mode.vrefresh()).unwrap_or(0).saturating_mul(1000));
    }
    rates.set_start(i32::try_from(top.mode.vrefresh()).unwrap_or(0).saturating_mul(1000));
    server.core.handle(clamshell_state::Reading::DisplayModes(rates).into());

    let drawing = Rc::new(RefCell::new(Drawing {
        renderer,
        pipeline,
        screens,
        frames: std::array::from_fn(|_| UiFrame::new()),
        cursor: crate::cursor::Cursor::new(),
        elements: Vec::new(),
        states: [RenderElementStates::default(), RenderElementStates::default()],
        away: false,
    }));
    let on_capture = Rc::new(RefCell::new(on_capture));

    // Page flips: the frame shown, its time to the clients, and the next frame.
    let flips = drawing.clone();
    let capture = on_capture.clone();
    base.event_loop
        .handle()
        .insert_source(drm_notifier, move |event, metadata, server| match event {
            DrmEvent::VBlank(crtc) => {
                let drawing = &mut *flips.borrow_mut();
                let Some(index) = drawing.screens.iter().position(|s| s.crtc == crtc) else {
                    return;
                };
                flipped(drawing, server, index, metadata.as_ref());
                step(drawing, server, &mut *capture.borrow_mut(), Instant::now());
            }
            DrmEvent::Error(error) => log::warn!("drm: {error}"),
        })
        .map_err(|e| e.error)?;

    // Input, each absolute device placed on its screen by the profile.
    let mut libinput = Libinput::new_with_udev::<LibinputSessionInterface<LibSeatSession>>(session.clone().into());
    libinput.udev_assign_seat(&seat).map_err(|()| format!("drm: libinput on seat {seat}"))?;
    let areas = Areas { profile, top: top_size, bottom: bottom_size };
    let switcher = Rc::new(RefCell::new(session.clone()));
    let vt = switcher.clone();
    base.event_loop
        .handle()
        .insert_source(LibinputInputBackend::new(libinput.clone()), move |event, _, server| {
            if let InputEvent::DeviceAdded { device } = &event {
                log::info!("drm: input {} ({}) on the {:?} screen", device.name(), identifier(device), areas.role(device));
                if identifier(device) == TOUCHPAD {
                    tap(&mut device.clone());
                }
            }
            server.input(event, crate::input::Host { absolute_cursor: false, touch_frames: true }, |device| areas.of(device));
            if let Some(n) = server.vt_switch.take()
                && let Err(error) = vt.borrow_mut().change_vt(n)
            {
                log::warn!("drm: switching to VT {n}: {error}");
            }
        })
        .map_err(|e| e.error)?;

    // The seat: paused when another VT is shown, resumed when this one is back.
    let resumed = drawing.clone();
    let paused = drawing.clone();
    let capture = on_capture.clone();
    base.event_loop
        .handle()
        .insert_source(notifier, move |event, _, server| match event {
            SessionEvent::PauseSession => {
                log::info!("drm: seat paused");
                libinput.suspend();
                drm.pause();
                paused.borrow_mut().away = true;
            }
            SessionEvent::ActivateSession => {
                log::info!("drm: seat resumed");
                if let Err(()) = libinput.resume() {
                    log::warn!("drm: libinput did not resume");
                }
                // Every screen off and its state forgotten, as the console left them: one that is on is set
                // up whole by its next commit, one that is off stays so.
                if let Err(error) = drm.activate(true) {
                    log::warn!("drm: {error}");
                }
                let drawing = &mut *resumed.borrow_mut();
                drawing.away = false;
                for screen in &mut drawing.screens {
                    screen.reset_buffers();
                    if !screen.on {
                        screen.surface.off();
                        if let Some(layers) = &mut screen.layers {
                            layers.screen_off();
                        }
                    }
                }
                drawing.pipeline.redraw_all();
                step(drawing, server, &mut *capture.borrow_mut(), Instant::now());
            }
        })
        .map_err(|e| e.error)?;

    drop(switcher);
    base.run(start, move |server, now| step(&mut drawing.borrow_mut(), server, &mut *on_capture.borrow_mut(), now))
}

/// The UI's screen for `role`.
fn ui_screen(role: ScreenRole) -> clamshell_ui::Screen {
    match role {
        ScreenRole::Top => clamshell_ui::Screen::Top,
        ScreenRole::Bottom => clamshell_ui::Screen::Bottom,
    }
}

/// Whether the screen, the bottom one if `down`, shows nothing but ClamShell's own view in the scene: no
/// window, overlay, dim layer or cursor on it (see `ui_scanout`).
fn ui_only(server: &Server, down: bool, planed: Planed) -> bool {
    use crate::scene::Shown;
    let on_it = |x: f64| server.screens.below(x) == down;
    let screen = if down { clamshell_ui::Screen::Bottom } else { clamshell_ui::Screen::Top };
    server.scene.shown().iter().all(|shown| match shown {
        Shown::Window(placed) => placed.below != down,
        Shown::Dim { area } => !on_it(f64::from(area.loc.x)),
        Shown::Cursor { at } => planed.cursor || !on_it(at.x),
        Shown::Software { layer, .. } => planed.overlay || *layer != clamshell_ui::Layer::Overlay(screen),
    })
}

/// A GLES renderer on the GPU's GBM device, its context at high priority: where the GPU preempts (the
/// Thor's Adreno, by its kernel's priority rings), ClamShell's frames go ahead of a game's work rather
/// than queueing behind it. Where the driver doesn't offer it, the context is an ordinary one; Smithay
/// logs the priority granted either way.
#[allow(unsafe_code)]
fn gles(gbm: GbmDevice<DrmDeviceFd>) -> Result<GlesRenderer, Error> {
    // SAFETY: EGL is given a GBM device that stays open as long as the display, which the context and
    // the renderer keep.
    let display = unsafe { EGLDisplay::new(gbm) }.map_err(|e| format!("drm: egl: {e}"))?;
    let context =
        EGLContext::new_with_priority(&display, smithay::backend::egl::context::ContextPriority::High).map_err(|e| format!("drm: egl context: {e}"))?;
    // SAFETY: the context was just made on this thread and only this renderer uses it.
    unsafe { GlesRenderer::new(context) }.map_err(|e| format!("drm: renderer: {e}").into())
}

/// The rotation that turns a panel mounted `rotation` the right way up.
fn transform(rotation: Rotation) -> Transform {
    match rotation {
        Rotation::Normal => Transform::Normal,
        Rotation::Clockwise90 => Transform::_90,
    }
}

/// The connector the profile names for `role`, its mode, and a CRTC it can use other than `taken`.
fn choose(drm: &DrmDevice, profile: &DeviceProfile, role: ScreenRole, taken: Option<crtc::Handle>) -> Result<Chosen, Error> {
    let name = profile.screen(role).connector;
    let resources = drm.resource_handles().map_err(|e| format!("drm: {e}"))?;
    for &handle in resources.connectors() {
        let Ok(info) = drm.get_connector(handle, false) else {
            continue;
        };
        if format!("{}-{}", info.interface().as_str(), info.interface_id()) != name {
            continue;
        }
        if info.state() != connector::State::Connected {
            return Err(format!("drm: {name}, the {role:?} screen, is not connected").into());
        }
        // The mode the kernel set (its command line's video=, or the panel's own), else the preferred one.
        let current = info
            .current_encoder()
            .and_then(|e| drm.get_encoder(e).inspect_err(|error| log::warn!("drm: {name}: its encoder: {error}")).ok()?.crtc())
            .filter(|c| Some(*c) != taken)
            .and_then(|c| Some((c, drm.get_crtc(c).inspect_err(|error| log::warn!("drm: {name}: its CRTC: {error}")).ok()?.mode()?)));
        let same_size = |mode: smithay::reexports::drm::control::Mode| info.modes().iter().filter(|m| m.size() == mode.size()).copied().collect();
        let millimetres = info.size();
        if let Some((crtc, mode)) = current {
            return Ok(Chosen { connector: handle, mode, modes: same_size(mode), crtc, millimetres });
        }
        let mode = info
            .modes()
            .iter()
            .find(|m| m.mode_type().contains(ModeTypeFlags::PREFERRED))
            .or_else(|| info.modes().first())
            .copied()
            .ok_or_else(|| format!("drm: {name} has no modes"))?;
        let crtc = info
            .encoders()
            .iter()
            .filter_map(|&e| drm.get_encoder(e).inspect_err(|error| log::warn!("drm: {name}: an encoder: {error}")).ok())
            .flat_map(|e| resources.filter_crtcs(e.possible_crtcs()))
            .find(|c| Some(*c) != taken)
            .ok_or_else(|| format!("drm: no CRTC for {name}"))?;
        return Ok(Chosen { connector: handle, mode, modes: same_size(mode), crtc, millimetres });
    }
    Err(format!("drm: no connector {name} for the {role:?} screen").into())
}

/// ClamShell's virtual touchpad (the touchpad tab, the keyboard while it points), as libinput names it.
const TOUCHPAD: &str = "4660:22136:clamshell_touchpad";

/// On ClamShell's touchpad a tap clicks, two fingers tapped right-click, and a tap then a held finger
/// drags: libinput leaves tapping off by default.
fn tap(device: &mut smithay::reexports::input::Device) {
    let mut set = true;
    for (what, done) in [("tap", device.config_tap_set_enabled(true)), ("tap and drag", device.config_tap_set_drag_enabled(true))] {
        if let Err(error) = done {
            log::warn!("drm: ClamShell's touchpad: {what}: {error:?}");
            set = false;
        }
    }
    if set {
        log::info!("drm: ClamShell's touchpad taps and drags");
    }
}

/// The frame queued on screen `index` is shown: its time is the presentation feedback, and the clients
/// shown there get their frame callbacks.
fn flipped(drawing: &mut Drawing, server: &mut Server, index: usize, metadata: Option<&DrmEventMetadata>) {
    let Some(screen) = drawing.screens.get_mut(index) else {
        return;
    };
    let submitted = screen.surface.flipped();
    if screen.surface.shows_composed() && screen.ui.as_ref().is_some_and(|ui| ui.refused) {
        log::info!("ui scanout: the {:?} screen's own buffers, refused, dropped", screen.role);
        screen.ui = None;
    }
    let (time, hw_clock): (Time<Monotonic>, bool) = match metadata.map(|m| m.time) {
        Some(DrmEventTime::Monotonic(at)) => (at.into(), true),
        _ => (server.clock.now(), false),
    };
    let down = screen.role == ScreenRole::Bottom;
    screen.flipped = Some(time);
    if submitted {
        server.frame_shown(&screen.queued, Some(down));
        server.present_flip(&mut screen.feedback, time, hw_clock, screen.interval, metadata.map_or(0, |m| u64::from(m.sequence)));
    }
    screen.held = server.frame_callbacks(Some(down), screen.interval, Instant::now());
}

/// Composes each screen that is free and composed for (see [`composes`]) and shows it if it changed;
/// nothing changed means no frame. A screen waiting for its flip is composed at the flip, with whatever
/// came meanwhile, and the other screen doesn't wait for it. Clients that committed without a visible
/// change get their frame callbacks, at most once a refresh, so that an idle screen never spins. Returns
/// when those are next due.
fn step(
    drawing: &mut Drawing,
    server: &mut Server,
    on_capture: &mut impl FnMut(Result<clamshell_state::ScreenshotPixels, String>),
    now: Instant,
) -> Option<Instant> {
    if drawing.away {
        // The clients' work goes on, their frames held back; the screens wait for the seat (see the
        // session's events).
        server.frame();
        server.import_dmabufs(&mut drawing.renderer);
        if std::mem::take(&mut server.screenshot_requested) {
            on_capture(Err("the screens are another session's".into()));
        }
        return None;
    }
    outputs(drawing, server);
    // The commits of a screen's windows timed for no later than its next frame go ahead before it is
    // composed; the loop wakes a refresh of that screen before the next one held is due, to compose it in
    // time, or, within a refresh of it already, at the next refresh, when the frame shown then is composed
    // (not at once again). Each screen by its own refresh, which the two needn't share.
    let clock = server.clock.now();
    let mut timed = None;
    for screen in &drawing.screens {
        let next = screen.next_shown(clock);
        let held = server.release_timed_commits(next, Some(screen.role == ScreenRole::Bottom));
        timed = earlier(timed, held.map(|at| timed_wake(now, Duration::from(clock), Duration::from(next), Duration::from(at), screen.interval)));
    }
    // The frame's own work (the X11 windows' changes to the state) and the buffers' imports, which their
    // clients wait for, every turn.
    server.frame();
    server.sync_pointer_capture(|_| Ok::<(), std::convert::Infallible>(()));
    server.import_dmabufs(&mut drawing.renderer);
    // What goes on the screens' overlay and cursor planes, which the scene then leaves out (see
    // `plane_layers`). A screen showing only ClamShell shows its own buffers, and isn't composed for (see
    // `ui_scanout`); one showing one window alone may show the window's (see `window_scanout`), or draw it
    // straight onto itself.
    record_overlays(server);
    layers_decide(drawing, server);
    shows_decide(drawing, server);
    // Each screen composes on its own: when it is free (not waiting for a flip, which composes it with
    // what came meanwhile), unless it shows its own buffers or a window's; for a screenshot, all of them.
    let capture = std::mem::take(&mut server.screenshot_requested);
    for index in 0..drawing.screens.len() {
        // A failure is warned of at the first, and while it goes on, at debug: it would be every turn.
        match compose(drawing, server, index, capture) {
            Ok(()) => {
                if let Some(screen) = drawing.screens.get_mut(index) {
                    screen.compose_failing = false;
                }
            }
            Err(error) => match drawing.screens.get_mut(index) {
                Some(screen) if !screen.compose_failing => {
                    screen.compose_failing = true;
                    log::warn!("drm: drawing: {error}");
                }
                _ => log::debug!("drm: drawing: {error}"),
            },
        }
    }
    if let Some(histogram) = drawing.pipeline.take_histogram(&mut drawing.renderer, server) {
        server.report_histogram(histogram);
    }
    // A screenshot is drawn now and read once the GPU is done, at a later turn: reading it at once would
    // wait for the GPU here.
    if capture && let Err(error) = drawing.pipeline.capture(&mut drawing.renderer, server.screens.size) {
        on_capture(Err(error));
    }
    if let Some(captured) = drawing.pipeline.captured(&mut drawing.renderer, false) {
        on_capture(captured);
    }
    let composed = server.pacing.commits;
    // Buffers not used for a while are dropped (see `crate::hold`), and the loop wakes when the next would be.
    let mut trims = crate::hold::Trims::default();
    drawing.pipeline.trim(now, &mut trims);
    for frame in &mut drawing.frames {
        frame.trim(now, &mut trims);
    }
    for screen in &mut drawing.screens {
        screen.surface.trim(now, &mut trims);
        if let Some(layers) = &mut screen.layers {
            layers.trim(now, &mut trims);
        }
    }
    let mut due: Option<Instant> = earlier(earlier(timed, trims.finish()), drawing.pipeline.capture_due(now));
    for screen in &mut drawing.screens {
        if screen.surface.flip_due().is_some_and(|lost| now >= lost) {
            log::warn!("drm: {:?} screen: no flip {} ms after its frame: drawn again", screen.role, crate::presenter::FLIP_LOST.as_millis());
            screen.reset_buffers();
            screen.dirty = true;
        }
        due = earlier(due, screen.surface.flip_due());
    }
    for index in 0..drawing.screens.len() {
        let Some(screen) = drawing.screens.get_mut(index) else {
            continue;
        };
        let down = screen.role == ScreenRole::Bottom;
        // A frame waits for its flip: a move of the cursor alone goes at once.
        if screen.surface.in_flight()
            && screen.layers_dirty
            && let Some(layers) = &mut screen.layers
            && layers.move_now(screen.surface.surface())
        {
            screen.layers_dirty = false;
        }
        let turn = screen_turn(Timing::of(screen, server.pacing.commits), now);
        if turn.send_held {
            screen.held = server.frame_callbacks(Some(down), screen.interval, now);
            due = earlier(due, screen.held);
        }
        due = earlier(due, turn.wake);
        // A commit that failed wakes the loop for the next try (see `failed`).
        if screen.shows == Shows::Own {
            if screen.on && !screen.surface.in_flight() {
                due = earlier(due, present_ui(screen, server, &mut drawing.pipeline, &drawing.cursor));
            }
        } else if turn.present && matches!(screen.shows, Shows::Window(_)) {
            due = earlier(due, present_window(screen, server, &mut drawing.pipeline, &drawing.cursor));
        } else if turn.present {
            due = earlier(due, present(drawing, server, index));
        } else if screen.layers_dirty && screen.on && !screen.surface.in_flight() {
            // Only the overlay or the cursor changed: the frame shown, committed again with them.
            due = earlier(due, present_layers(screen, server, &mut drawing.pipeline, &drawing.cursor));
        } else if turn.answer {
            screen.answered = server.pacing.commits;
            screen.next_callbacks = Some(now + screen.interval);
            screen.held = server.frame_callbacks(Some(down), screen.interval, now);
            // A callback held back here is due later too (a FIFO client's empty commit, under a cap).
            due = earlier(due, screen.held);
        }
    }
    drawing.pipeline.let_go();
    after_compose(due, composed, server.pacing.commits, now)
}

/// When the loop wakes next, `due` as the screens' turns say, with `composed` the commit count composed
/// and `commits` the count now: commits let through after composing (a FIFO barrier the cap let go) are
/// composed at once, not at the next wake-up for something else.
fn after_compose(due: Option<Instant>, composed: u64, commits: u64, now: Instant) -> Option<Instant> {
    if commits != composed { Some(now) } else { due }
}

/// When the loop wakes for a commit timed for `at` (the clock's times since boot), its screen's next
/// frame `next` and its refresh `interval`, given the loop's `now` and the clock's `clock`: a refresh before
/// it is due, to compose it in time; or, within a refresh of it already, at the next frame, when the one
/// shown then is composed (not at once again, which would turn the loop until then).
fn timed_wake(now: Instant, clock: Duration, next: Duration, at: Duration, interval: Duration) -> Instant {
    let wait = at.saturating_sub(clock).saturating_sub(interval);
    now + if wait.is_zero() { next.saturating_sub(clock) } else { wait }
}

/// The item whose rate (`rate`) is nearest `hz`, the first of those as near.
fn nearest<T>(items: &[T], rate: impl Fn(&T) -> u32, hz: u32) -> Option<&T> {
    items.iter().min_by_key(|item| rate(item).abs_diff(hz))
}

/// The earlier of two times, either or both absent.
fn earlier(a: Option<Instant>, b: Option<Instant>) -> Option<Instant> {
    match (a, b) {
        (Some(a), Some(b)) => Some(a.min(b)),
        (a, b) => a.or(b),
    }
}

/// What decides a screen's turn in [`step`].
#[derive(Debug, Clone, Copy)]
struct Timing {
    /// A frame is queued, its flip not yet come.
    pending: bool,
    /// Its part of the scene changed since its last frame.
    dirty: bool,
    on: bool,
    /// When frame callbacks held back (under a frame rate cap) are due.
    held: Option<Instant>,
    /// Whether the clients' commits have been answered with their frame callbacks.
    answered: bool,
    /// The soonest an idle screen answers commits again (at most once a refresh).
    next_callbacks: Option<Instant>,
}

impl Timing {
    /// `screen`'s, with the server's commit count `commits`.
    fn of(screen: &Screen, commits: u64) -> Timing {
        Timing {
            pending: screen.surface.in_flight(),
            dirty: screen.dirty,
            on: screen.on,
            held: screen.held,
            answered: screen.answered == commits,
            next_callbacks: screen.next_callbacks,
        }
    }
}

/// What a screen's turn does.
#[derive(Debug, Default, PartialEq, Eq)]
struct Turn {
    /// Send the frame callbacks held back, now due.
    send_held: bool,
    /// Show a frame.
    present: bool,
    /// Answer the clients' commits with their frame callbacks, the screen being idle.
    answer: bool,
    /// When to wake for this screen, unless something comes before.
    wake: Option<Instant>,
}

/// A screen's turn: nothing while a frame is queued (its flip sends what is held: a wake now would only
/// turn the loop until it); the held callbacks once due; a frame if its part changed and it is on; else,
/// with commits not answered, their callbacks at most once a refresh.
fn screen_turn(timing: Timing, now: Instant) -> Turn {
    let mut turn = Turn::default();
    if timing.pending {
        return turn;
    }
    match timing.held {
        Some(at) if at <= now => turn.send_held = true,
        held => turn.wake = held,
    }
    if timing.dirty && timing.on {
        turn.present = true;
    } else if !timing.answered {
        match timing.next_callbacks {
            Some(at) if at > now => turn.wake = earlier(turn.wake, Some(at)),
            _ => turn.answer = true,
        }
    }
    turn
}

/// What of a screen's state its power changes.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
struct Power {
    on: bool,
    dirty: bool,
}

/// A screen turned on or off as `wanted`, if that changes it: turned on it is to be drawn whole, since its
/// last frame is gone. (Either way no frame is in flight any more: see `outputs`.)
fn switched(power: Power, wanted: bool) -> Option<Power> {
    (power.on != wanted).then_some(Power { on: wanted, dirty: power.dirty || wanted })
}

/// The mode for a refresh rate of `hz` asked for: `start` for 0 (the screen's own), else the one of
/// `modes` whose rate is nearest (see [`nearest`]), or `start` if there are none.
fn mode_for<'a, T>(modes: &'a [T], start: &'a T, rate: impl Fn(&T) -> u32, hz: u32) -> &'a T {
    if hz == 0 { start } else { nearest(modes, rate, hz).unwrap_or(start) }
}

/// Turns each screen off or on, and the top screen to the refresh, as the state last asked.
fn outputs(drawing: &mut Drawing, server: &mut Server) {
    for screen in &mut drawing.screens {
        let index = usize::from(screen.role == ScreenRole::Bottom);
        let wanted = server.screens.on.get(index).copied().unwrap_or(true);
        if let Some(power) = switched(Power { on: screen.on, dirty: screen.dirty }, wanted) {
            if wanted {
                log::info!("drm: {:?} screen on", screen.role);
                Power { on: screen.on, dirty: screen.dirty } = power;
                // The next frame turns it back on.
                screen.reset_buffers();
            } else {
                // Off only once the screen is: a failure leaves it on, and it is tried again next turn
                // rather than left lit with nothing composed on it.
                match screen.surface.surface().clear() {
                    Ok(()) => {
                        log::info!("drm: {:?} screen off", screen.role);
                        Power { on: screen.on, dirty: screen.dirty } = power;
                        screen.off_failing = false;
                        screen.surface.off();
                        screen.feedback.discard();
                        if let Some(layers) = &mut screen.layers {
                            layers.screen_off();
                        }
                    }
                    Err(error) if !screen.off_failing => {
                        screen.off_failing = true;
                        log::warn!("drm: turning the {:?} screen off: {error}: trying again", screen.role);
                    }
                    Err(error) => log::debug!("drm: turning the {:?} screen off: {error}", screen.role),
                }
            }
        }
        if screen.role == ScreenRole::Top && server.screens.refresh_wanted != screen.refresh {
            screen.refresh = server.screens.refresh_wanted;
            let hz = u32::from(screen.refresh);
            let mode = *mode_for(&screen.modes, &screen.start_mode, |m| m.vrefresh(), hz);
            if hz != 0 && mode.vrefresh() != hz {
                log::info!("drm: top screen: no mode at {hz} Hz, the nearest at {} Hz", mode.vrefresh());
            }
            if mode != screen.surface.pending_mode() {
                log::info!("drm: top screen at {} Hz", mode.vrefresh());
                match screen.surface.use_mode(mode) {
                    Ok(()) => {
                        screen.interval = Duration::from_micros(1_000_000 / u64::from(mode.vrefresh().max(1)));
                        screen.dirty = true;
                        server.screens.refresh = i32::try_from(mode.vrefresh()).unwrap_or(60).saturating_mul(1000);
                        server.resize(server.screens.size);
                    }
                    Err(error) => log::warn!("drm: top screen at {} Hz: {error}", mode.vrefresh()),
                }
            }
        }
    }
}

/// Composes screen `index` if it is composed now (see [`composes`]), or for a screenshot (`capture`),
/// marking it changed if its picture did.
fn compose(drawing: &mut Drawing, server: &mut Server, index: usize, capture: bool) -> Result<(), Error> {
    let Drawing { renderer, pipeline, screens, frames, cursor, elements, states, .. } = drawing;
    let (Some(screen), Some(states)) = (screens.get_mut(index), states.get_mut(index)) else { return Ok(()) };
    let shows_own = matches!(screen.shows, Shows::Own | Shows::Window(_));
    if !composes(screen.on, screen.surface.in_flight(), shows_own, capture) {
        return Ok(());
    }
    let down = screen.role == ScreenRole::Bottom;
    let skip = match screen.shows {
        Shows::Window(id) => Some(id),
        _ => None,
    };
    render::elements(server, frames, cursor, renderer, elements, down, skip, screen.planed())?;
    screen.dirty |= pipeline.compose(server, renderer, down, elements, states, matches!(screen.shows, Shows::Straight(_)))?;
    Ok(())
}

/// Whether a screen composes this turn: on, not waiting for a flip, and not showing its own buffers or a
/// window's; or for a screenshot, whatever it shows.
fn composes(on: bool, pending: bool, shows_own: bool, capture: bool) -> bool {
    capture || (on && !pending && !shows_own)
}

/// Decides for each screen with overlay and cursor planes what goes on them this turn, the cursor if its
/// hotspot is on the screen; a change to what they show has the screen commit (see `plane_layers`).
fn layers_decide(drawing: &mut Drawing, server: &mut Server) {
    let Drawing { screens, cursor, renderer, .. } = drawing;
    let at = server.scene.shown().iter().find_map(|shown| match shown {
        crate::scene::Shown::Cursor { at } => Some(*at),
        _ => None,
    });
    // (Not for a screen that is off: nothing is planed there until it is on.)
    for screen in screens.iter_mut().filter(|screen| screen.on) {
        let Some(layers) = &mut screen.layers else { continue };
        let down = screen.role == ScreenRole::Bottom;
        let at = at.filter(|at| server.screens.below(at.x) == down);
        let origin = smithay::utils::Point::<i32, Logical>::from((screen.part.loc.x, screen.part.loc.y));
        screen.layers_dirty |= layers.decide(server, ui_screen(screen.role), cursor, renderer, at, origin);
    }
}

/// The screen's commit with its planes went through: they show as placed.
fn layers_committed(screen: &mut Screen) {
    if let Some(layers) = &mut screen.layers {
        layers.committed();
    }
    screen.layers_dirty = false;
}

/// The screen's commit with its planes failed: they are off from now on, composed, and the screen is
/// committed again.
fn layers_failed(screen: &mut Screen, error: &impl std::fmt::Display) {
    if let Some(layers) = &mut screen.layers {
        layers.fail(&format!("a commit with them failed: {error}").into(), &format!("{:?}", screen.role));
    }
    screen.dirty = true;
    screen.layers_dirty = true;
}

/// Commits the frame the screen shows again, with its overlay and cursor planes as they now are. Returns
/// when to try again if it failed.
fn present_layers(screen: &mut Screen, server: &mut Server, pipeline: &mut Pipeline, cursor: &crate::cursor::Cursor) -> Option<Instant> {
    let with_layers = prepare_layers(screen, server, cursor);
    let states = screen.layers.iter().flat_map(PlaneLayers::states);
    match screen.surface.show_again(states) {
        Ok(()) => {
            committed(screen);
            None
        }
        Err(error) => failed(screen, pipeline, Commit::Again, with_layers, error),
    }
}

/// What a commit showed on the primary plane, for what its failure means.
#[derive(Debug, Clone, Copy)]
enum Commit {
    /// A frame composed.
    Composed,
    /// The screen's own buffer, drawn anew for it if `drew`.
    Own { drew: bool },
    /// Window `id`'s buffer (the one decided on, see [`shows_decide`]).
    Window { id: i64 },
    /// The frame shown, again, with the planes changed.
    Again,
}

/// What is done about a commit that failed, by its error (see `CommitError`), and whether the overlay and
/// cursor planes showed something in it.
#[derive(Debug, PartialEq)]
enum Remedy {
    /// The seat is away: nothing, until it is back.
    Wait,
    /// Tried again at the next refresh, as it was.
    Retry,
    /// The planes are off from now on: composed, at once.
    PlanesOff,
    /// What the primary plane showed is refused: the screen's own buffers, or the window's shape; composed
    /// at once.
    Refuse,
}

impl Commit {
    /// Whether the screen has something else to show if it is refused: composing, for the screen's own
    /// buffer or a window's; nothing, for a frame composed or the frame shown again.
    fn refusable(self) -> bool {
        matches!(self, Commit::Own { .. } | Commit::Window { .. })
    }
}

/// The remedy for a commit that failed with `error`, `with_layers` showing, `refusable` or not (see
/// [`Commit::refusable`]).
fn remedy(refusable: bool, with_layers: bool, error: &crate::presenter::CommitError) -> Remedy {
    use crate::presenter::CommitError;
    match error {
        CommitError::Away => Remedy::Wait,
        CommitError::Busy(_) | CommitError::Failed(_) => Remedy::Retry,
        CommitError::Refused(_) if with_layers => Remedy::PlanesOff,
        CommitError::Refused(_) if refusable => Remedy::Refuse,
        // Nothing else to show: tried again.
        CommitError::Refused(_) => Remedy::Retry,
    }
}

/// Applies the remedy for the screen's commit `kind` that failed with `error`, `with_layers` showing.
/// Returns when to try again.
fn failed(screen: &mut Screen, pipeline: &mut Pipeline, kind: Commit, with_layers: bool, error: crate::presenter::CommitError) -> Option<Instant> {
    if let (Commit::Own { drew: true }, Some(ui)) = (kind, &mut screen.ui) {
        ui.failed();
    }
    let role = screen.role;
    match remedy(kind.refusable(), with_layers, &error) {
        Remedy::Wait => None,
        Remedy::Retry => {
            if !screen.failing {
                log::warn!("drm: the {role:?} screen's commit failed ({error}): tried again at each refresh until one goes through");
                screen.failing = true;
            }
            // (The screen's own buffer, not drawn anew, is committed again as the frame shown is.)
            match kind {
                Commit::Again | Commit::Own { drew: false } => screen.layers_dirty = true,
                Commit::Own { drew: true } | Commit::Window { .. } | Commit::Composed => screen.dirty = true,
            }
            Some(Instant::now() + screen.interval)
        }
        Remedy::PlanesOff => {
            layers_failed(screen, &error);
            Some(Instant::now())
        }
        Remedy::Refuse => {
            match kind {
                Commit::Own { .. } => {
                    log::warn!("ui scanout: the {role:?} screen: {error}: composed from now on");
                    if let Some(ui) = &mut screen.ui {
                        ui.refused = true;
                    }
                }
                Commit::Window { id } => {
                    log::info!(
                        "window scanout: the {role:?} screen can't show window {id} from its own buffer: {error}: composed until its buffers' shape changes"
                    );
                    if let Some(scanout) = &mut screen.window {
                        scanout.refused = screen.candidate.as_ref().map(Candidate::shape);
                    }
                }
                Commit::Composed | Commit::Again => {}
            }
            // Composed from the next turn, which decides again (see `shows_decide`) without it.
            show(screen, pipeline, Shows::Composed);
            Some(Instant::now())
        }
    }
}

/// Readies the screen's overlay and cursor planes for its next commit (see `PlaneLayers::prepare`).
/// Returns whether they show something in it, so that a refusal of it may be theirs.
fn prepare_layers(screen: &mut Screen, server: &mut Server, cursor: &crate::cursor::Cursor) -> bool {
    if let Some(layers) = &mut screen.layers {
        let failed = layers.failed();
        layers.prepare(server, ui_screen(screen.role), cursor);
        if layers.failed() && !failed {
            // Off from now on: composed, from the next frame.
            screen.dirty = true;
            screen.layers_dirty = true;
        }
    }
    screen.layers.as_ref().is_some_and(PlaneLayers::shows)
}

/// The screen's commit went through: its planes show as placed, and its flip is awaited.
fn committed(screen: &mut Screen) {
    if std::mem::take(&mut screen.failing) {
        log::info!("drm: the {:?} screen's commits go through again", screen.role);
    }
    layers_committed(screen);
}

/// Records the screens' overlays that are stale, and places the scene again if any was: what an overlay
/// shows decides whether it goes on a plane, a screen shows its own buffers, or a window is shown from its
/// own, all of which follow. (Composing would record it too, but only after those decisions, a turn late.)
fn record_overlays(server: &mut Server) {
    let mut recorded = false;
    for screen in [clamshell_ui::Screen::Top, clamshell_ui::Screen::Bottom] {
        let overlay = clamshell_ui::Layer::Overlay(screen);
        if server.core.stale(overlay) {
            server.core.record(overlay, false);
            recorded = true;
        }
    }
    if recorded {
        server.place();
    }
}

/// What a screen shows.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Shows {
    /// A frame composed from the scene.
    Composed,
    /// Its own buffers, with ClamShell's view (see `ui_scanout`).
    Own,
    /// Window `id` from the window's own buffer (see `window_scanout`).
    Window(i64),
    /// Window `id` drawn straight onto it, composed without the scene between (see `colour::render`).
    Straight(i64),
}

impl std::fmt::Display for Shows {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Shows::Composed => write!(f, "composes its frames"),
            Shows::Own => write!(f, "shows its own buffers"),
            Shows::Window(id) => write!(f, "shows window {id} from its own buffer"),
            Shows::Straight(id) => write!(f, "draws window {id} straight onto it while it is one element"),
        }
    }
}

/// What a screen's showing is decided from (see [`shows`]).
#[derive(Debug, Clone, Copy)]
struct Facts {
    on: bool,
    /// It has its own buffers, not refused, and nothing but ClamShell's view is on it (but the overlay and
    /// cursor on their planes).
    own: bool,
    /// The window alone on it (see `Server::lone_window`), if one is.
    lone: Option<i64>,
    /// Whether that window's buffer can be shown as it is: else why not. None where the screen can't
    /// show a window's buffer.
    buffer: Option<Result<(), &'static str>>,
    /// A screenshot is taken of the scene this turn.
    screenshot: bool,
}

/// What a screen shows by its `facts`: its own buffers while it can; the window alone on it from its own
/// buffer while that can be, else drawn straight; else a frame composed. For a screenshot a lone window
/// is composed, the scene being what it is taken of.
fn shows(facts: Facts) -> Shows {
    match facts.lone {
        _ if facts.own => Shows::Own,
        Some(_) if facts.screenshot => Shows::Composed,
        Some(id) if facts.on && facts.buffer == Some(Ok(())) => Shows::Window(id),
        Some(id) => Shows::Straight(id),
        None => Shows::Composed,
    }
}

/// What a change of what a screen shows takes.
#[derive(Debug, Default, PartialEq, Eq)]
struct Change {
    /// The next commit clears the CRTC's colour its own buffers set.
    clear_own: bool,
    /// Its own buffers are drawn and committed whole, though the view didn't change: the screen shows
    /// something else.
    reset_own: bool,
    /// The next commit clears the plane's colour a window's buffer set.
    clear_window: bool,
    /// The scenes are drawn whole: they left the window out.
    redraw: bool,
    /// The screen shows a new frame.
    dirty: bool,
}

/// What going `from` showing one thing `to` another takes.
fn change(from: Shows, to: Shows) -> Change {
    let mut change = Change::default();
    if from == to {
        return change;
    }
    match from {
        Shows::Own => (change.clear_own, change.dirty) = (true, true),
        Shows::Window(_) if !matches!(to, Shows::Window(_)) => (change.clear_window, change.redraw, change.dirty) = (true, true, true),
        _ => {}
    }
    match to {
        Shows::Own => change.reset_own = true,
        Shows::Window(_) => change.dirty = true,
        _ => {}
    }
    change
}

/// Has the screen show `to`, with what the change takes, logged.
fn show(screen: &mut Screen, pipeline: &mut Pipeline, to: Shows) {
    let change = change(screen.shows, to);
    if change.clear_own
        && let Some(ui) = &screen.ui
    {
        screen.surface.clear_after(ui.clear());
    }
    if change.reset_own
        && let Some(ui) = &mut screen.ui
    {
        ui.reset();
    }
    if change.clear_window
        && let Some(scanout) = &screen.window
    {
        screen.surface.clear_after(scanout.clear());
    }
    if change.redraw {
        pipeline.redraw_all();
    }
    screen.dirty |= change.dirty;
    if to != screen.shows {
        log::info!("drm: the {:?} screen {to}", screen.role);
    }
    screen.shows = to;
    if !matches!(to, Shows::Window(_)) {
        screen.candidate = None;
    }
}

/// Decides for each screen what it shows (see [`shows`]), from what is on it this turn. The window alone on
/// a screen that can show a window's buffer is told the plane's formats, and told the renderer's again
/// once it isn't; one that can't be shown so is composed, with the reason logged. A window shown from its
/// own buffer is committed again when its buffer changes.
fn shows_decide(drawing: &mut Drawing, server: &mut Server) {
    let Drawing { screens, pipeline, .. } = drawing;
    for screen in screens.iter_mut() {
        let down = screen.role == ScreenRole::Bottom;
        let planed = screen.planed();
        let area = Rectangle::new((screen.part.loc.x, screen.part.loc.y).into(), (screen.part.size.w, screen.part.size.h).into());
        let own = screen.ui.as_ref().is_some_and(|ui| !ui.refused) && ui_only(server, down, planed);
        let lone = server.lone_window(down, area, planed).map(|placed| (placed.id, placed.geometry));
        let mut candidate = None;
        let mut buffer = None;
        if let Some(scanout) = &screen.window {
            let alone = lone.filter(|_| !own && screen.on);
            if alone.map(|(id, _)| id) != screen.fed {
                if let (Some(id), Some(default)) = (screen.fed, &server.protocols.dmabuf_feedback) {
                    server.dmabuf_feedback(id, default);
                }
                if let (Some((id, _)), Some(feedback)) = (alone, &scanout.feedback) {
                    server.dmabuf_feedback(id, feedback);
                }
                screen.fed = alone.map(|(id, _)| id);
            }
            let chosen = alone.map(|(id, geometry)| match server.candidate(id, geometry, down, area, pipeline.output_description(down)) {
                Ok(candidate) if candidate.colour.is_some() && !scanout.has_pipeline() => Err("its colour needs a colour pipeline the plane hasn't"),
                Ok(candidate) if scanout.refused == Some(candidate.shape()) => Err("the display engine refused its buffer"),
                chosen => chosen,
            });
            let why = chosen.as_ref().and_then(|chosen| chosen.as_ref().err().copied());
            if why != screen.composed_why {
                if let (Some(why), Some((id, _))) = (why, alone) {
                    log::info!("window scanout: window {id}, alone on the {:?} screen, is composed: {why}", screen.role);
                }
                screen.composed_why = why;
            }
            buffer = Some(chosen.as_ref().map_or(Err("no window alone"), |chosen| chosen.as_ref().map(|_| ()).map_err(|why| *why)));
            candidate = chosen.and_then(Result::ok);
        }
        let facts = Facts { on: screen.on, own, lone: lone.map(|(id, _)| id), buffer, screenshot: server.screenshot_requested };
        let to = shows(facts);
        if let Shows::Window(_) = to
            && screen.candidate.as_ref().zip(candidate.as_ref()).is_none_or(|(shown, now)| shown.commit != now.commit || shown.shape() != now.shape())
        {
            // A new buffer to show.
            screen.dirty = true;
        }
        show(screen, pipeline, to);
        if let Shows::Window(_) = to {
            screen.candidate = candidate;
        }
    }
}

/// Shows the window decided on (see [`shows_decide`]) from its own buffer, in a test commit
/// first when its shape is new. Returns when to try again if it failed (see [`failed`]): refused, the
/// window is composed until its buffers' shape changes, and the scene, which left it out, is drawn whole.
fn present_window(screen: &mut Screen, server: &mut Server, pipeline: &mut Pipeline, cursor: &crate::cursor::Cursor) -> Option<Instant> {
    let with_layers = prepare_layers(screen, server, cursor);
    let (Some(scanout), Some(candidate)) = (&mut screen.window, &screen.candidate) else { return None };
    let shape = candidate.shape();
    let test = screen.tested != Some(shape);
    // The window's place turned as the composed frame is, its buffer the other way, as the plane takes it.
    let logical = screen.transform.transform_size(screen.size);
    let dst = screen.transform.transform_rect_in(candidate.dst.to_physical(1), &logical);
    // A framebuffer or colour the window's buffer can't have is the window's alone, whatever the planes show.
    let (shown, with_layers) = match scanout.framebuffer(&candidate.dmabuf).and_then(|fb| Ok((fb, scanout.colour(candidate.colour)?))) {
        Ok((fb, properties)) => {
            let states = screen.layers.iter().flat_map(PlaneLayers::states);
            (screen.surface.show_window(candidate.buffer.clone(), fb, candidate.src, dst, screen.transform.invert(), properties, states, test), with_layers)
        }
        Err(error) => (Err(crate::presenter::CommitError::Refused(error.to_string())), false),
    };
    let down = screen.role == ScreenRole::Bottom;
    match shown {
        Ok(()) => {
            screen.tested = Some(shape);
            let state = smithay::backend::renderer::element::RenderElementState {
                visible_area: usize::try_from(dst.size.w).unwrap_or(0) * usize::try_from(dst.size.h).unwrap_or(0),
                presentation_state: smithay::backend::renderer::element::RenderElementPresentationState::ZeroCopy,
                needs_capture: false,
            };
            screen.queued.states.clear();
            screen.queued.states.insert(smithay::backend::renderer::element::Id::from_wayland_resource(&candidate.surface), state);
            server.frame_queued(&screen.queued, Some(down));
            server.take_feedback(&screen.queued, Some(down), &mut screen.feedback);
            committed(screen);
            screen.dirty = false;
            screen.answered = server.pacing.commits;
            None
        }
        Err(error) => {
            let id = candidate.id;
            failed(screen, pipeline, Commit::Window { id }, with_layers, error)
        }
    }
}

/// Shows the screen's own buffer drawn with what changed in ClamShell's view, if anything did. Returns when
/// to try again if it failed (see [`failed`]).
fn present_ui(screen: &mut Screen, server: &mut Server, pipeline: &mut Pipeline, cursor: &crate::cursor::Cursor) -> Option<Instant> {
    let ui = screen.ui.as_mut()?;
    let fb = ui.draw(&mut server.core, ui_screen(screen.role));
    if fb.is_none() && !screen.layers_dirty {
        return None;
    }
    server.core.frames_done();
    let with_layers = prepare_layers(screen, server, cursor);
    let ui = screen.ui.as_mut()?;
    let states = screen.layers.iter().flat_map(PlaneLayers::states);
    let shown = match fb {
        Some(fb) => screen.surface.show_ui(fb, ui.colour(), states),
        None => screen.surface.show_again(states),
    };
    match shown {
        Ok(()) => {
            if fb.is_some() {
                ui.committed();
            }
            committed(screen);
            None
        }
        Err(error) => failed(screen, pipeline, Commit::Own { drew: fb.is_some() }, with_layers, error),
    }
}

/// Shows screen `index` its part of the scene. Returns when to try again if it failed (see [`failed`]):
/// drawing it failing too.
fn present(drawing: &mut Drawing, server: &mut Server, index: usize) -> Option<Instant> {
    use crate::presenter::CommitError;
    let Drawing { renderer, pipeline, screens, states, cursor, .. } = drawing;
    let (screen, states) = (screens.get_mut(index)?, states.get(index)?);
    let with_layers = prepare_layers(screen, server, cursor);
    let drawn = screen.surface.next_buffer().and_then(|mut buffer| {
        let mut target = renderer.bind(&mut buffer)?;
        pipeline.present_screen(renderer, &mut target, screen.role == ScreenRole::Bottom, screen.size, screen.transform)
    });
    let sync = match drawn {
        Ok(sync) => sync,
        Err(error) => return failed(screen, pipeline, Commit::Composed, false, CommitError::Failed(format!("drawing: {error}"))),
    };
    // (Not clone_from: a map of another capacity would be allocated anew.)
    screen.queued.states.clear();
    screen.queued.states.extend(states.states.iter().map(|(id, state)| (id.clone(), *state)));
    let layers = screen.layers.iter().flat_map(PlaneLayers::states);
    if let Err(error) = screen.surface.queue(Some(sync), layers) {
        return failed(screen, pipeline, Commit::Composed, with_layers, error);
    }
    committed(screen);
    server.frame_queued(states, Some(screen.role == ScreenRole::Bottom));
    server.take_feedback(states, Some(screen.role == ScreenRole::Bottom), &mut screen.feedback);
    screen.dirty = false;
    screen.answered = server.pacing.commits;
    None
}

/// Where each absolute input device lies in the scene: on the screen the profile places it, else, for a
/// touchscreen, the profile's catch-all; anything else (a tablet nobody named) covers the top screen.
#[derive(Clone, Copy)]
struct Areas {
    profile: DeviceProfile,
    top: Size<i32, Logical>,
    bottom: Size<i32, Logical>,
}

impl Areas {
    fn role(&self, device: &InputDevice) -> ScreenRole {
        let touch = device.has_capability(DeviceCapability::Touch.into());
        screen_of(self.profile.touchscreens, touch, |identifier| names(identifier, device))
    }

    /// The device's screen's area in the scene, and how its panel is turned.
    fn of(&self, device: &InputDevice) -> (Rectangle<f64, Logical>, Transform) {
        let role = self.role(device);
        (area(role, self.top, self.bottom), transform(self.profile.screen(role).rotation))
    }
}

/// The screen of an absolute input device: the one of the profile's `touchscreens` that names it (by
/// `named`, see [`names`]), else, for a touchscreen (`touch`), the catch-all's, else the top screen.
fn screen_of(touchscreens: &[Touchscreen], touch: bool, named: impl Fn(&str) -> bool) -> ScreenRole {
    let by_name = touchscreens.iter().find(|t| matches!(t.selector, TouchSelector::Named(n) if named(n)));
    by_name.or_else(|| touch.then(|| touchscreens.iter().find(|t| t.selector == TouchSelector::All)).flatten()).map_or(ScreenRole::Top, |t| t.screen)
}

/// Screen `role`'s area in the scene, the screens being `top` and `bottom` in size: the top screen's at
/// the origin, the bottom one's to its right.
fn area(role: ScreenRole, top: Size<i32, Logical>, bottom: Size<i32, Logical>) -> Rectangle<f64, Logical> {
    match role {
        ScreenRole::Top => Rectangle::from_size(top.to_f64()),
        ScreenRole::Bottom => Rectangle::new((f64::from(top.w), 0.0).into(), bottom.to_f64()),
    }
}

/// A device's identifier as the profile names it: vendor, product and name, with spaces as underscores.
fn identifier(device: &InputDevice) -> String {
    format!("{}:{}:{}", device.id_vendor(), device.id_product(), device.name().replace(' ', "_"))
}

/// Whether `identifier` (as [`identifier`] makes it) names `device`, compared without allocating: this is
/// asked for every absolute input event.
fn names(identifier: &str, device: &InputDevice) -> bool {
    names_ids(identifier, device.id_vendor(), device.id_product(), &device.name())
}

/// [`names`], for a device's vendor, product and name.
fn names_ids(identifier: &str, vendor: u32, product: u32, name: &str) -> bool {
    let mut parts = identifier.splitn(3, ':');
    let number = |part: Option<&str>| part.and_then(|p| p.parse::<u32>().ok());
    let (ours, theirs) = ((number(parts.next()), number(parts.next())), (Some(vendor), Some(product)));
    let named = parts.next().unwrap_or("");
    ours == theirs && named.len() == name.len() && named.bytes().zip(name.bytes()).all(|(a, b)| a == b || (a == b'_' && b == b' '))
}

#[cfg(test)]
mod tests {
    /// A screen shows its own buffers while it can, else the window alone on it from the window's buffer
    /// while that can be (not for a screenshot, nor while off), else drawn straight, else composed.
    #[test]
    fn a_screen_shows_its_own_buffers_a_lone_windows_or_a_frame_composed() {
        use super::{Facts, Shows, shows};
        let none = Facts { on: true, own: false, lone: None, buffer: None, screenshot: false };
        let lone = Facts { lone: Some(7), buffer: Some(Ok(())), ..none };
        assert_eq!(shows(none), Shows::Composed);
        assert_eq!(shows(Facts { own: true, ..none }), Shows::Own);
        assert_eq!(shows(Facts { own: true, screenshot: true, ..none }), Shows::Own, "a screenshot composes it anyway");
        assert_eq!(shows(lone), Shows::Window(7));
        assert_eq!(shows(Facts { buffer: Some(Err("refused")), ..lone }), Shows::Straight(7));
        assert_eq!(shows(Facts { buffer: None, ..lone }), Shows::Straight(7), "a screen without window scanout");
        assert_eq!(shows(Facts { on: false, ..lone }), Shows::Straight(7));
        assert_eq!(shows(Facts { screenshot: true, ..lone }), Shows::Composed, "the scene, for the screenshot");
    }

    /// Leaving its own buffers clears their colour and composes; going to them commits them whole; leaving
    /// a window's buffer clears its colour and draws the scenes whole, which left the window out.
    #[test]
    fn a_change_of_what_a_screen_shows_takes_what_it_left_and_where_it_goes() {
        use super::{Change, Shows, change};
        assert_eq!(change(Shows::Own, Shows::Own), Change::default());
        assert_eq!(change(Shows::Composed, Shows::Own), Change { reset_own: true, ..Change::default() });
        assert_eq!(change(Shows::Own, Shows::Composed), Change { clear_own: true, dirty: true, ..Change::default() });
        assert_eq!(change(Shows::Own, Shows::Window(7)), Change { clear_own: true, dirty: true, ..Change::default() });
        assert_eq!(change(Shows::Window(7), Shows::Straight(7)), Change { clear_window: true, redraw: true, dirty: true, ..Change::default() });
        assert_eq!(change(Shows::Window(7), Shows::Own), Change { clear_window: true, redraw: true, dirty: true, reset_own: true, ..Change::default() });
        assert_eq!(change(Shows::Window(7), Shows::Window(8)), Change { dirty: true, ..Change::default() }, "another window's buffer");
        assert_eq!(change(Shows::Straight(7), Shows::Composed), Change::default(), "composing sees to that itself");
    }

    /// A failed commit's remedy: away waits, busy or failing tries again, a refusal with the planes showing
    /// turns them off, and one without them refuses what the primary plane showed if the screen can
    /// compose instead.
    #[test]
    fn a_failed_commit_is_remedied_by_what_failed_and_what_it_showed() {
        use super::{Remedy, remedy};
        use crate::presenter::CommitError::*;
        let refused = || Refused(String::new());
        assert_eq!(remedy(true, true, &Away), Remedy::Wait);
        assert_eq!(remedy(true, true, &Busy(String::new())), Remedy::Retry);
        assert_eq!(remedy(true, false, &Failed(String::new())), Remedy::Retry);
        assert_eq!(remedy(false, true, &refused()), Remedy::PlanesOff);
        assert_eq!(remedy(true, true, &refused()), Remedy::PlanesOff, "the planes first, being what is tried");
        assert_eq!(remedy(true, false, &refused()), Remedy::Refuse);
        assert_eq!(remedy(false, false, &refused()), Remedy::Retry, "composed, with nothing else to show");
    }

    /// A commit of the screen's own buffer or a window's can be refused, the screen composing instead; a
    /// frame composed, or the frame shown committed again, has nothing else to show.
    #[test]
    fn only_a_commit_of_a_buffer_not_composed_is_refusable() {
        use super::Commit;
        assert!(Commit::Own { drew: true }.refusable());
        assert!(Commit::Own { drew: false }.refusable());
        assert!(Commit::Window { id: 7 }.refusable());
        assert!(!Commit::Composed.refusable());
        assert!(!Commit::Again.refusable());
    }

    use super::{Timing, Turn, screen_turn, timed_wake};
    use std::time::{Duration, Instant};

    #[test]
    fn the_mode_nearest_the_rate_asked_for() {
        let rates = [60, 90, 120];
        assert_eq!(super::nearest(&rates, |r| *r, 120), Some(&120));
        assert_eq!(super::nearest(&rates, |r| *r, 100), Some(&90), "the nearer");
        assert_eq!(super::nearest(&rates, |r| *r, 75), Some(&60), "as near as 90: the first");
        assert_eq!(super::nearest(&[] as &[u32], |r| *r, 60), None);
    }

    const IDLE: Timing = Timing { pending: false, dirty: false, on: true, held: None, answered: true, next_callbacks: None };

    #[test]
    fn a_queued_frame_waits_for_its_flip_without_turning_the_loop() {
        let now = Instant::now();
        let held = Timing { pending: true, held: Some(now), dirty: true, answered: false, ..IDLE };
        assert_eq!(screen_turn(held, now), Turn::default(), "no callbacks, no frame, no wake: the flip sends them");
    }

    #[test]
    fn held_callbacks_go_once_due_and_wake_the_loop_until_then() {
        let now = Instant::now();
        let later = now + Duration::from_millis(5);
        assert_eq!(screen_turn(Timing { held: Some(now), ..IDLE }, now), Turn { send_held: true, ..Turn::default() });
        assert_eq!(screen_turn(Timing { held: Some(later), ..IDLE }, now), Turn { wake: Some(later), ..Turn::default() });
    }

    #[test]
    fn a_changed_screen_shows_a_frame_if_on_and_an_idle_one_answers_at_most_once_a_refresh() {
        let now = Instant::now();
        let next = now + Duration::from_millis(8);
        assert_eq!(screen_turn(Timing { dirty: true, answered: false, ..IDLE }, now), Turn { present: true, ..Turn::default() });
        assert_eq!(screen_turn(Timing { dirty: true, on: false, answered: false, ..IDLE }, now), Turn { answer: true, ..Turn::default() }, "off: answered");
        assert_eq!(screen_turn(Timing { answered: false, next_callbacks: Some(next), ..IDLE }, now), Turn { wake: Some(next), ..Turn::default() });
        assert_eq!(screen_turn(Timing { answered: false, next_callbacks: Some(now), ..IDLE }, now), Turn { answer: true, ..Turn::default() });
        assert_eq!(screen_turn(IDLE, now), Turn::default(), "nothing to answer");
    }

    /// Held callbacks due later and commits answered later still: the loop wakes for the earlier.
    #[test]
    fn a_screen_wakes_the_loop_for_the_earlier_of_its_held_callbacks_and_its_next_answer() {
        let now = Instant::now();
        let (soon, later) = (now + Duration::from_millis(3), now + Duration::from_millis(8));
        let idle = Timing { answered: false, ..IDLE };
        assert_eq!(screen_turn(Timing { held: Some(soon), next_callbacks: Some(later), ..idle }, now), Turn { wake: Some(soon), ..Turn::default() });
        assert_eq!(screen_turn(Timing { held: Some(later), next_callbacks: Some(soon), ..idle }, now), Turn { wake: Some(soon), ..Turn::default() });
    }

    #[test]
    fn the_earlier_of_two_times_is_either_present() {
        let now = Instant::now();
        let later = now + Duration::from_millis(5);
        assert_eq!(super::earlier(Some(now), Some(later)), Some(now));
        assert_eq!(super::earlier(Some(later), Some(now)), Some(now));
        assert_eq!(super::earlier(None, Some(later)), Some(later));
        assert_eq!(super::earlier(Some(later), None), Some(later));
        assert_eq!(super::earlier(None, None), None);
    }

    #[test]
    fn a_timed_commit_wakes_the_loop_a_refresh_before_it_or_at_the_next_frame() {
        let now = Instant::now();
        let ms = Duration::from_millis;
        // The clock at 100 ms, the next frame at 108, a refresh of 8 ms.
        assert_eq!(timed_wake(now, ms(100), ms(108), ms(130), ms(8)), now + ms(22), "a refresh before it");
        assert_eq!(timed_wake(now, ms(100), ms(108), ms(110), ms(8)), now + ms(2), "a refresh before it, soon");
        assert_eq!(timed_wake(now, ms(100), ms(108), ms(105), ms(8)), now + ms(8), "within a refresh of it: the next frame, not now");
    }

    #[test]
    fn a_device_is_named_by_its_ids_and_its_name_with_underscores() {
        use super::names_ids;
        assert!(names_ids("0:0:top_touchscreen", 0, 0, "top touchscreen"));
        assert!(names_ids("1575:3:QEMU_Virtio_Tablet", 1575, 3, "QEMU Virtio Tablet"));
        assert!(!names_ids("1575:3:QEMU_Virtio_Tablet", 1575, 4, "QEMU Virtio Tablet"), "another product");
        assert!(!names_ids("0:0:top_touchscreen", 0, 0, "top touchscreens"));
        assert!(!names_ids("0:0:top touchscreen", 0, 0, "top_touchscreen"), "an underscore in the name stays one");
        assert!(!names_ids("nonsense", 0, 0, ""));
    }

    use super::*;

    #[test]
    fn commits_let_through_after_composing_are_composed_at_once() {
        let now = Instant::now();
        let later = Some(now + Duration::from_millis(8));
        assert_eq!(after_compose(later, 5, 5, now), later, "none: as the screens say");
        assert_eq!(after_compose(None, 5, 5, now), None);
        assert_eq!(after_compose(later, 5, 6, now), Some(now), "one let through: now");
        assert_eq!(after_compose(None, 5, 6, now), Some(now));
    }

    #[test]
    fn a_screen_composes_when_free_and_composed_or_for_a_screenshot() {
        assert!(composes(true, false, false, false), "on and free");
        assert!(!composes(true, true, false, false), "a frame queued: at its flip");
        assert!(!composes(false, false, false, false), "off");
        assert!(!composes(true, false, true, false), "showing its own buffers or a window's");
        assert!(composes(true, true, true, true), "a screenshot: whatever it shows");
        assert!(composes(false, false, false, true));
    }

    #[test]
    fn a_screen_turned_on_is_drawn_whole() {
        let off = Power { on: false, dirty: false };
        assert_eq!(switched(off, true), Some(Power { on: true, dirty: true }), "on: drawn whole");
        let on = Power { on: true, dirty: true };
        assert_eq!(switched(on, false), Some(Power { on: false, dirty: true }));
        assert_eq!(switched(on, true), None, "unchanged");
        assert_eq!(switched(off, false), None);
    }

    #[test]
    fn the_mode_for_a_refresh_is_the_nearest_or_the_screens_own() {
        let (modes, start) = ([60, 90, 120], 120);
        assert_eq!(mode_for(&modes, &start, |r| *r, 0), &120, "0: the screen's own");
        assert_eq!(mode_for(&modes, &start, |r| *r, 60), &60);
        assert_eq!(mode_for(&modes, &start, |r| *r, 50), &60, "the nearest");
        assert_eq!(mode_for(&modes, &start, |r| *r, 100), &90);
        assert_eq!(mode_for(&[], &start, |r| *r, 60), &120, "none of its size: its own");
    }

    #[test]
    fn an_input_device_is_on_the_screen_the_profile_names_it_on() {
        let thor = DeviceProfile::thor().touchscreens;
        let named = |identifier: &'static str| move |n: &str| n == identifier;
        assert_eq!(screen_of(thor, true, named("0:0:top_touchscreen")), ScreenRole::Top);
        assert_eq!(screen_of(thor, true, named("0:0:bottom_touchscreen")), ScreenRole::Bottom);
        assert_eq!(screen_of(thor, true, named("1:2:a_stray_touchscreen")), ScreenRole::Top, "unnamed, no catch-all: the top screen");
        let catch_all = [Touchscreen { selector: TouchSelector::All, screen: ScreenRole::Bottom }];
        assert_eq!(screen_of(&catch_all, true, named("1:2:a_touchscreen")), ScreenRole::Bottom, "a touchscreen: the catch-all's");
        assert_eq!(screen_of(&catch_all, false, named("1:2:a_tablet")), ScreenRole::Top, "a tablet nobody named: the top screen");
        let both = [Touchscreen { selector: TouchSelector::Named("1:2:pen"), screen: ScreenRole::Top }, catch_all[0]];
        assert_eq!(screen_of(&both, true, named("1:2:pen")), ScreenRole::Top, "named before the catch-all");
        assert_eq!(screen_of(&[], true, named("0:0:x")), ScreenRole::Top);
    }

    #[test]
    fn the_bottom_screens_area_is_right_of_the_top_screens() {
        let (top, bottom) = (Size::from((1920, 1080)), Size::from((1240, 1080)));
        assert_eq!(area(ScreenRole::Top, top, bottom), Rectangle::from_size(Size::from((1920.0, 1080.0))));
        assert_eq!(area(ScreenRole::Bottom, top, bottom), Rectangle::new((1920.0, 0.0).into(), Size::from((1240.0, 1080.0))));
    }

    #[test]
    fn a_frame_is_shown_at_the_next_refresh_after_the_last_flip() {
        let ms = Duration::from_millis;
        assert_eq!(next_shown(Some(ms(100)), ms(10), false, ms(123)), ms(130));
        assert_eq!(next_shown(Some(ms(100)), ms(10), false, ms(130)), ms(140), "at a refresh: the one after");
        assert_eq!(next_shown(Some(ms(100)), ms(10), true, ms(123)), ms(140), "a frame queued: a refresh later");
        assert_eq!(next_shown(None, ms(10), false, ms(123)), ms(133), "no flip yet: a refresh on");
        let us = Duration::from_micros;
        assert_eq!(next_shown(Some(ms(100)), Duration::ZERO, false, ms(123)), ms(123) + us(1), "no refresh known: taken as a microsecond");
    }
}
