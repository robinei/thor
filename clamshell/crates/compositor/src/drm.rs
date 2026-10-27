//! ClamShell's compositor on the machine's own screens: a seat from libseat, the GPU through DRM and GBM,
//! input from libinput. The device profile names the connector of each screen and how its panel is
//! mounted. Each screen is composed on its own (the colour pipeline, a scene each), turned as its panel
//! is; its page flips pace its frames and the composing of commits to it, independently of the other
//! screen's, and their times are the presentation feedback clients get.

use std::cell::RefCell;
use std::rc::Rc;
use std::time::{Duration, Instant};

use crate::EventSender;
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
    /// The screen's part of the scene, and its mode's size before its panel's turn.
    part: Rectangle<i32, Buffer>,
    size: Size<i32, Physical>,
    transform: Transform,
    /// How long a frame is shown.
    interval: Duration,
    /// A frame is queued and its flip not yet seen.
    pending: bool,
    /// When the frame pending was queued (see [`flip_lost`]).
    queued_at: Option<Instant>,
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
    /// Its own buffers, with CLAMSHELL_SCANOUT=1, for while it shows nothing but ClamShell (see
    /// `ui_scanout`); and whether it shows one now.
    ui: Option<UiScanout>,
    ui_shown: bool,
    /// With CLAMSHELL_SCANOUT=1, its means to show a window from the window's own buffer (see
    /// `window_scanout`), the window it shows so if it does, the window told the plane's formats, the shape
    /// the display engine last took, and why a window alone on it is composed, as last logged.
    window: Option<WindowScanout>,
    window_shown: Option<Candidate>,
    fed: Option<i64>,
    tested: Option<Shape>,
    composed_why: Option<&'static str>,
    /// The window drawn straight onto it, composed without the scene between (see `colour::render`), if
    /// it shows one alone.
    direct: Option<i64>,
}

impl Screen {
    /// Drops what its buffers hold, as after it was off or away, or a flip was lost: the next frame is
    /// drawn whole, composed or its own.
    fn reset_buffers(&mut self) {
        self.surface.reset_buffers();
        if let Some(ui) = &mut self.ui {
            ui.reset();
        }
    }

    /// When a frame composed at `now` would be shown (see [`next_shown`]).
    fn next_shown(&self, now: Time<Monotonic>) -> Time<Monotonic> {
        Time::from(next_shown(self.flipped.map(Duration::from), self.interval, self.pending, Duration::from(now)))
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
    let (mut drm, drm_notifier) = DrmDevice::new(fd.clone(), false).map_err(|e| format!("drm: {e}"))?;
    let sync_fd = fd.clone();
    let gbm = GbmDevice::new(fd).map_err(|e| format!("drm: gbm: {e}"))?;
    let mut renderer = gles(gbm.clone())?;

    let top = choose(&drm, &profile, ScreenRole::Top, None)?;
    let bottom = choose(&drm, &profile, ScreenRole::Bottom, Some(top.crtc))?;
    if scanout {
        crate::scanout::report(&drm, &[("the top screen", top.crtc), ("the second screen", bottom.crtc)]);
    }
    let logical = |chosen: &Chosen, role: ScreenRole| {
        let (w, h) = chosen.mode.size();
        let turned = transform(profile.screen(role).rotation).transform_size(Size::<i32, Physical>::from((i32::from(w), i32::from(h))));
        Size::<i32, Logical>::from((turned.w, turned.h))
    };
    let (top_size, bottom_size) = (logical(&top, ScreenRole::Top), logical(&bottom, ScreenRole::Bottom));
    config.top_size = (top_size.w as u32, top_size.h as u32);
    config.bottom_size = (bottom_size.w as u32, bottom_size.h as u32);

    let mut base = Base::new(state, config, true, identify, on_effect, quit)?;
    let server = &mut base.server;
    server.screens.millimetres =
        [crate::screens::shown_millimetres(top.millimetres, &profile.top), crate::screens::shown_millimetres(bottom.millimetres, &profile.bottom)];
    log::info!("drm: the screens' millimetres {:?} (connector {:?}, {:?})", server.screens.millimetres, top.millimetres, bottom.millimetres);
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
    for (role, chosen, x) in [(ScreenRole::Top, &top, 0), (ScreenRole::Bottom, &bottom, top_size.w)] {
        let surface = drm.create_surface(chosen.crtc, chosen.mode, &[chosen.connector]).map_err(|e| format!("drm: {role:?} screen: {e}"))?;
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
            pending: false,
            queued_at: None,
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
            ui: None,
            ui_shown: false,
            window: None,
            window_shown: None,
            fed: None,
            tested: None,
            composed_why: None,
            direct: None,
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
    }

    // The top screen's rates, for the refresh setting.
    let mut rates = clamshell_state::presentation::modes::DisplayModes::default();
    for mode in &top.modes {
        rates.add(i32::try_from(mode.vrefresh()).unwrap_or(0).saturating_mul(1000));
    }
    rates.set_start(i32::try_from(top.mode.vrefresh()).unwrap_or(0).saturating_mul(1000));
    server.core.handle(clamshell_state::Reading::DisplayModes(rates).into());

    let top_ui = server.core.size(clamshell_ui::Screen::Top).0;
    let bottom_ui = server.core.size(clamshell_ui::Screen::Bottom).0;
    let drawing = Rc::new(RefCell::new(Drawing {
        renderer,
        pipeline,
        screens,
        frames: [UiFrame::new(top_ui)?, UiFrame::new(bottom_ui)?, UiFrame::new(top_ui)?, UiFrame::new(bottom_ui)?],
        cursor: crate::cursor::Cursor::new(),
        elements: Vec::new(),
        states: [RenderElementStates::default(), RenderElementStates::default()],
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
    let capture = on_capture.clone();
    base.event_loop
        .handle()
        .insert_source(notifier, move |event, _, server| match event {
            SessionEvent::PauseSession => {
                log::info!("drm: seat paused");
                libinput.suspend();
                drm.pause();
            }
            SessionEvent::ActivateSession => {
                log::info!("drm: seat resumed");
                if let Err(()) = libinput.resume() {
                    log::warn!("drm: libinput did not resume");
                }
                if let Err(error) = drm.activate(false) {
                    log::warn!("drm: {error}");
                }
                let drawing = &mut *resumed.borrow_mut();
                for screen in &mut drawing.screens {
                    screen.reset_buffers();
                    screen.pending = false;
                    screen.queued_at = None;
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
fn ui_only(server: &Server, down: bool) -> bool {
    use crate::scene::Shown;
    let edge = server.screens.top_width;
    let on_it = |x: i32| (x >= edge) == down;
    let screen = if down { clamshell_ui::Screen::Bottom } else { clamshell_ui::Screen::Top };
    server.scene.shown().iter().all(|shown| match shown {
        Shown::Window(placed) => placed.below != down,
        Shown::Dim { area } => !on_it(area.loc.x),
        Shown::Cursor { at } => !on_it(at.x as i32),
        Shown::Software { layer, .. } => *layer != clamshell_ui::Layer::Overlay(screen),
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
    screen.pending = false;
    screen.queued_at = None;
    let submitted = screen.surface.flipped();
    let (time, hw_clock): (Time<Monotonic>, bool) = match metadata.map(|m| m.time) {
        Some(DrmEventTime::Monotonic(at)) => (at.into(), true),
        _ => (server.clock.now(), false),
    };
    let down = screen.role == ScreenRole::Bottom;
    screen.flipped = Some(time);
    if submitted {
        server.frame_shown(&screen.queued, Some(down));
        server.present_flip(&screen.queued, time, hw_clock, down, screen.interval, metadata.map_or(0, |m| u64::from(m.sequence)));
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
    outputs(drawing, server);
    // Commits timed for no later than the top screen's next frame go ahead before it is composed; the
    // loop wakes a refresh before the next one held is due, to compose it in time, or, within a refresh
    // of it already, at the next refresh, when the frame shown then is composed (not at once again).
    let timed = drawing.screens.iter().find(|s| s.role == ScreenRole::Top).map(|top| {
        let clock = server.clock.now();
        let next = top.next_shown(clock);
        let held = server.release_timed_commits(next);
        held.map(|at| timed_wake(now, Duration::from(clock), Duration::from(next), Duration::from(at), top.interval))
    });
    // The frame's own work (the X11 windows' changes to the state) and the buffers' imports, which their
    // clients wait for, every turn.
    server.frame();
    server.sync_pointer_capture(|_| Ok::<(), std::convert::Infallible>(()));
    server.import_dmabufs(&mut drawing.renderer);
    // A screen showing only ClamShell shows its own buffers, and isn't composed for (see `ui_scanout`); one
    // showing one window alone may show the window's (see `window_scanout`), or draw it straight onto
    // itself.
    ui_scanout_decide(drawing, server);
    window_scanout_decide(drawing, server);
    direct_decide(drawing, server);
    // Each screen composes on its own: when it is free (not waiting for a flip, which composes it with
    // what came meanwhile), unless it shows its own buffers or a window's; for a screenshot, all of them.
    let capture = std::mem::take(&mut server.screenshot_requested);
    for index in 0..drawing.screens.len() {
        if let Err(error) = compose(drawing, server, index, capture) {
            log::warn!("drm: drawing: {error}");
        }
    }
    if let Some(histogram) = drawing.pipeline.take_histogram() {
        server.report_histogram(histogram);
    }
    if capture {
        on_capture(drawing.pipeline.capture(&mut drawing.renderer, server.screens.size));
    }
    let composed = server.pacing.commits;
    let mut due: Option<Instant> = timed.flatten();
    for screen in &mut drawing.screens {
        if flip_lost(screen.pending, screen.queued_at, now) {
            log::warn!("drm: {:?} screen: no flip {} ms after its frame: drawn again", screen.role, FLIP_LOST.as_millis());
            screen.reset_buffers();
            (screen.pending, screen.queued_at, screen.dirty) = (false, None, true);
        }
        due = earlier(due, screen.queued_at.filter(|_| screen.pending).map(|at| at + FLIP_LOST));
    }
    for index in 0..drawing.screens.len() {
        let Some(screen) = drawing.screens.get_mut(index) else {
            continue;
        };
        let down = screen.role == ScreenRole::Bottom;
        let turn = screen_turn(Timing::of(screen, server.pacing.commits), now);
        if turn.send_held {
            screen.held = server.frame_callbacks(Some(down), screen.interval, now);
            due = earlier(due, screen.held);
        }
        due = earlier(due, turn.wake);
        if screen.ui_shown {
            if screen.on && !screen.pending {
                present_ui(screen, server);
            }
        } else if turn.present && screen.window_shown.is_some() {
            if !present_window(screen, server, &mut drawing.pipeline) {
                // Refused: composed at once.
                due = Some(now);
            }
        } else if turn.present {
            if let Err(error) = present(drawing, server, index) {
                log::warn!("drm: drawing: {error}");
            }
        } else if turn.answer {
            screen.answered = server.pacing.commits;
            screen.next_callbacks = Some(now + screen.interval);
            screen.held = server.frame_callbacks(Some(down), screen.interval, now);
            // A callback held back here is due later too (a FIFO client's empty commit, under a cap).
            due = earlier(due, screen.held);
        }
    }
    after_compose(due, composed, server.pacing.commits, now)
}

/// How long a queued frame waits for its flip before it is taken as lost (the driver never answered):
/// the screen would otherwise never present again, with the loop still turning for the watchdog.
const FLIP_LOST: Duration = Duration::from_secs(1);

/// Whether the frame `pending` since `queued` lost its flip by `now`.
fn flip_lost(pending: bool, queued: Option<Instant>, now: Instant) -> bool {
    pending && queued.is_some_and(|at| now >= at + FLIP_LOST)
}

/// When the loop wakes next, `due` as the screens' turns say, with `composed` the commit count composed
/// and `commits` the count now: commits let through after composing (a FIFO barrier the cap let go) are
/// composed at once, not at the next wake-up for something else.
fn after_compose(due: Option<Instant>, composed: u64, commits: u64, now: Instant) -> Option<Instant> {
    if commits != composed { Some(now) } else { due }
}

/// When the loop wakes for a commit timed for `at` (the clock's times since boot), the top screen's next
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
            pending: screen.pending,
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
    pending: bool,
}

/// A screen turned on or off as `wanted`, if that changes it: no frame is queued on it any more (turned
/// off, its flip won't come; turned on, it starts afresh), and turned on it is to be drawn whole, since
/// its last frame is gone.
fn switched(power: Power, wanted: bool) -> Option<Power> {
    (power.on != wanted).then_some(Power { on: wanted, dirty: power.dirty || wanted, pending: false })
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
        if let Some(power) = switched(Power { on: screen.on, dirty: screen.dirty, pending: screen.pending }, wanted) {
            log::info!("drm: {:?} screen {}", screen.role, if wanted { "on" } else { "off" });
            Power { on: screen.on, dirty: screen.dirty, pending: screen.pending } = power;
            if wanted {
                // The next frame turns it back on.
                screen.reset_buffers();
            } else if let Err(error) = screen.surface.surface().clear() {
                log::warn!("drm: turning the {:?} screen off: {error}", screen.role);
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
    let Drawing { renderer, pipeline, screens, frames, cursor, elements, states } = drawing;
    let (Some(screen), Some(states)) = (screens.get_mut(index), states.get_mut(index)) else { return Ok(()) };
    let shows_own = screen.ui_shown || screen.window_shown.is_some();
    if !composes(screen.on, screen.pending, shows_own, capture) {
        return Ok(());
    }
    let down = screen.role == ScreenRole::Bottom;
    let skip = screen.window_shown.as_ref().map(|shown| shown.id);
    render::elements(server, frames, cursor, renderer, elements, down, skip)?;
    screen.dirty |= pipeline.compose(server, renderer, down, elements, states, screen.direct.is_some())?;
    Ok(())
}

/// Whether a screen composes this turn: on, not waiting for a flip, and not showing its own buffers or a
/// window's; or for a screenshot, whatever it shows.
fn composes(on: bool, pending: bool, shows_own: bool, capture: bool) -> bool {
    capture || (on && !pending && !shows_own)
}

/// Decides for each screen with its own buffers whether it shows them: while the scene has nothing but
/// ClamShell's view on it. Overlays waiting to be recorded are first, as a notice coming up has the screen
/// composed. A screen going back to composing clears the CRTC's colour in its next commit and is drawn
/// whole.
fn ui_scanout_decide(drawing: &mut Drawing, server: &mut Server) {
    if drawing.screens.iter().all(|s| s.ui.is_none()) {
        return;
    }
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
    for screen in &mut drawing.screens {
        let Some(ui) = &screen.ui else { continue };
        let now = ui_only(server, screen.role == ScreenRole::Bottom);
        if screen.ui_shown && !now {
            screen.surface.clear_after(ui.clear());
            screen.dirty = true;
        }
        if now != screen.ui_shown {
            log::info!("ui scanout: the {:?} screen is {}", screen.role, if now { "scanned out" } else { "composed" });
        }
        screen.ui_shown = now;
    }
}

/// Decides for each screen that can show a window from the window's own buffer whether it does: while the
/// window is all there is on it, its buffer is one the plane can show as composing would, and the display
/// engine hasn't refused that (see `window_scanout`). The window alone on a screen is told the plane's
/// formats, and told the renderer's again once it isn't. A screenshot, taken of the scene composed, has
/// the window composed for it. A screen going back to composing clears the plane's colour in its next
/// commit, and the scene, which left the window out, is drawn whole.
fn window_scanout_decide(drawing: &mut Drawing, server: &mut Server) {
    let Drawing { screens, pipeline, .. } = drawing;
    for screen in screens.iter_mut() {
        let Some(scanout) = &mut screen.window else { continue };
        let down = screen.role == ScreenRole::Bottom;
        let area = Rectangle::new((screen.part.loc.x, screen.part.loc.y).into(), (screen.part.size.w, screen.part.size.h).into());
        let found = if screen.ui_shown || !screen.on { None } else { server.scanout_window(down, area, pipeline.output_description(down)) };
        let alone = found.as_ref().map(|(id, _)| *id);
        if alone != screen.fed {
            if let (Some(id), Some(default)) = (screen.fed, &server.protocols.dmabuf_feedback) {
                server.dmabuf_feedback(id, default);
            }
            if let (Some(id), Some(feedback)) = (alone, &scanout.feedback) {
                server.dmabuf_feedback(id, feedback);
            }
            screen.fed = alone;
        }
        let chosen = match found {
            Some((_, Ok(candidate))) if candidate.colour.is_some() && !scanout.has_pipeline() => Err("its colour needs a colour pipeline the plane hasn't"),
            Some((_, Ok(candidate))) if scanout.refused == Some(candidate.shape()) => Err("the display engine refused its buffer"),
            Some((_, Ok(candidate))) => Ok(Some(candidate)),
            Some((_, Err(why))) => Err(why),
            None => Ok(None),
        };
        let why = chosen.as_ref().err().copied();
        if why != screen.composed_why {
            if let (Some(why), Some(id)) = (why, alone) {
                log::info!("window scanout: window {id}, alone on the {:?} screen, is composed: {why}", screen.role);
            }
            screen.composed_why = why;
        }
        let now = chosen.ok().flatten().filter(|_| !server.screenshot_requested);
        match (&now, &screen.window_shown) {
            (Some(candidate), None) => log::info!("window scanout: the {:?} screen shows window {} from its own buffer", screen.role, candidate.id),
            (None, Some(shown)) => {
                log::info!("window scanout: the {:?} screen no longer shows window {} from its own buffer", screen.role, shown.id);
                screen.surface.clear_after(scanout.clear());
                screen.dirty = true;
                pipeline.redraw_all();
            }
            _ => {}
        }
        if let Some(candidate) = &now
            && screen.window_shown.as_ref().is_none_or(|shown| shown.commit != candidate.commit || shown.shape() != candidate.shape())
        {
            screen.dirty = true;
        }
        screen.window_shown = now;
    }
}

/// Decides for each screen whether a window alone on it is drawn straight onto it, composed without the
/// scene between (see `colour::render`): unless it is shown from its own buffer, or a screenshot wants the
/// scene.
fn direct_decide(drawing: &mut Drawing, server: &Server) {
    for screen in &mut drawing.screens {
        let down = screen.role == ScreenRole::Bottom;
        let area = Rectangle::new((screen.part.loc.x, screen.part.loc.y).into(), (screen.part.size.w, screen.part.size.h).into());
        let lone = server.lone_window(down, area).map(|placed| placed.id);
        let direct = lone.filter(|_| !server.screenshot_requested && !screen.ui_shown && screen.window_shown.is_none());
        if direct != screen.direct {
            match direct {
                Some(id) => log::info!("drm: the {:?} screen draws window {id} straight onto it", screen.role),
                None => log::info!("drm: the {:?} screen composes through the scene again", screen.role),
            }
            screen.direct = direct;
        }
    }
}

/// Shows the window decided on (see [`window_scanout_decide`]) from its own buffer, in a test commit
/// first when its shape is new. Returns whether it is: refused, the window is composed until its buffer
/// changes, and the scene, which left it out, is drawn whole.
fn present_window(screen: &mut Screen, server: &mut Server, pipeline: &mut Pipeline) -> bool {
    let (Some(scanout), Some(candidate)) = (&mut screen.window, &screen.window_shown) else { return true };
    let shape = candidate.shape();
    let test = screen.tested != Some(shape);
    // The window's place turned as the composed frame is, its buffer the other way, as the plane takes it.
    let logical = screen.transform.transform_size(screen.size);
    let dst = screen.transform.transform_rect_in(candidate.dst.to_physical(1), &logical);
    let surface = &mut screen.surface;
    let shown = scanout.framebuffer(&candidate.dmabuf).and_then(|fb| {
        let properties = scanout.colour(candidate.colour)?;
        surface.show_window(candidate.buffer.clone(), fb, candidate.src, dst, screen.transform.invert(), properties, test)
    });
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
            screen.pending = true;
            screen.queued_at = Some(Instant::now());
            screen.dirty = false;
            screen.answered = server.pacing.commits;
            true
        }
        Err(error) => {
            log::info!(
                "window scanout: the {:?} screen can't show window {} from its own buffer: {error}: composed until its buffer changes",
                screen.role,
                candidate.id
            );
            scanout.refused = Some(shape);
            screen.surface.clear_after(scanout.clear());
            screen.window_shown = None;
            screen.dirty = true;
            pipeline.redraw_all();
            false
        }
    }
}

/// Shows the screen's own buffer drawn with what changed in ClamShell's view, if anything did.
fn present_ui(screen: &mut Screen, server: &mut Server) {
    let Some(ui) = &mut screen.ui else { return };
    let Some(fb) = ui.draw(&mut server.core, ui_screen(screen.role)) else {
        return;
    };
    server.core.frames_done();
    match screen.surface.show_ui(fb, ui.colour()) {
        Ok(()) => {
            ui.committed();
            screen.pending = true;
            screen.queued_at = Some(Instant::now());
        }
        Err(error) => {
            // Composed instead from now on, with the reason.
            log::warn!("ui scanout: the {:?} screen: {error}: composed from now on", screen.role);
            screen.surface.clear_after(ui.clear());
            screen.ui = None;
            screen.ui_shown = false;
            screen.dirty = true;
        }
    }
}

/// Shows screen `index` its part of the scene.
fn present(drawing: &mut Drawing, server: &Server, index: usize) -> Result<(), Error> {
    let Drawing { renderer, pipeline, screens, states, .. } = drawing;
    let (Some(screen), Some(states)) = (screens.get_mut(index), states.get(index)) else {
        return Ok(());
    };
    let mut buffer = screen.surface.next_buffer().map_err(|e| format!("{:?} screen: {e}", screen.role))?;
    let sync = {
        let mut target = renderer.bind(&mut buffer)?;
        pipeline.present_screen(renderer, &mut target, screen.role == ScreenRole::Bottom, screen.size, screen.transform)?
    };
    screen.queued.states.clone_from(&states.states);
    screen.surface.queue(Some(sync)).map_err(|e| format!("{:?} screen: {e}", screen.role))?;
    server.frame_queued(states, Some(screen.role == ScreenRole::Bottom));
    screen.pending = true;
    screen.queued_at = Some(Instant::now());
    screen.dirty = false;
    screen.answered = server.pacing.commits;
    Ok(())
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

    #[test]
    fn a_frame_without_its_flip_for_a_second_is_lost() {
        let now = Instant::now();
        assert!(!flip_lost(true, Some(now), now + FLIP_LOST.saturating_sub(Duration::from_millis(1))));
        assert!(flip_lost(true, Some(now), now + FLIP_LOST));
        assert!(!flip_lost(false, Some(now), now + FLIP_LOST), "flipped");
        assert!(!flip_lost(true, None, now + FLIP_LOST));
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
    fn a_screen_turned_on_is_drawn_whole_and_one_switched_has_no_frame_queued() {
        let off = Power { on: false, dirty: false, pending: true };
        assert_eq!(switched(off, true), Some(Power { on: true, dirty: true, pending: false }), "on: drawn whole");
        let on = Power { on: true, dirty: true, pending: true };
        assert_eq!(switched(on, false), Some(Power { on: false, dirty: true, pending: false }), "off: its flip won't come");
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
    }
}
