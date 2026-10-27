//! ClamShell's compositor on the machine's own screens: a seat from libseat, the GPU through DRM and GBM,
//! input from libinput. The device profile names the connector of each screen and how its panel is
//! mounted. The scene is composed once (the colour pipeline), and each screen is shown its part of it,
//! turned as its panel is; each screen's page flips pace its frames and the composing of commits to it,
//! and their times are the presentation feedback clients get.

use std::cell::RefCell;
use std::rc::Rc;
use std::time::{Duration, Instant};

use crate::EventSender;
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
        drm::{DrmDevice, DrmDeviceFd, DrmEvent, DrmEventMetadata, DrmEventTime, GbmBufferedSurface},
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
    surface: GbmBufferedSurface<GbmAllocator<DrmDeviceFd>, RenderElementStates>,
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
}

impl Screen {
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
    /// The scene's elements as last composed.
    states: RenderElementStates,
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

/// Runs the compositor on the machine's screens until ClamShell quits.
pub fn run(
    state: State,
    mut config: Config,
    start: impl FnOnce(EventSender, crate::Displays),
    on_effect: impl FnMut(&Effect) + 'static,
    identify: impl Fn(u32) -> Process + Send + Sync + 'static,
    on_capture: impl FnMut(Result<clamshell_state::ScreenshotPixels, String>) + 'static,
    quit: crate::QuitSignals,
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
    let mut pipeline = Pipeline::new(&mut renderer, server.screens.size)?;
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
        let surface = GbmBufferedSurface::new(surface, allocator, &[Fourcc::Xrgb2101010, Fourcc::Xrgb8888], renderer_formats.iter().copied())
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
        });
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
        states: RenderElementStates::default(),
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
                    screen.surface.reset_buffers();
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

/// A GLES renderer on the GPU's GBM device.
#[allow(unsafe_code)]
fn gles(gbm: GbmDevice<DrmDeviceFd>) -> Result<GlesRenderer, Error> {
    // SAFETY: EGL is given a GBM device that stays open as long as the display, which the context and
    // the renderer keep.
    let display = unsafe { EGLDisplay::new(gbm) }.map_err(|e| format!("drm: egl: {e}"))?;
    let context = EGLContext::new(&display).map_err(|e| format!("drm: egl context: {e}"))?;
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
    let states = match screen.surface.frame_submitted() {
        Ok(states) => states,
        Err(error) => {
            log::warn!("drm: {:?} screen: {error}", screen.role);
            None
        }
    };
    let (time, hw_clock): (Time<Monotonic>, bool) = match metadata.map(|m| m.time) {
        Some(DrmEventTime::Monotonic(at)) => (at.into(), true),
        _ => (server.clock.now(), false),
    };
    let down = screen.role == ScreenRole::Bottom;
    screen.flipped = Some(time);
    if let Some(states) = &states {
        server.frame_shown(states, Some(down));
    }
    if let Some(states) = states {
        server.present_flip(&states, time, hw_clock, down, screen.interval, metadata.map_or(0, |m| u64::from(m.sequence)));
    }
    screen.held = server.frame_callbacks(Some(down), screen.interval, Instant::now());
}

/// Composes what changed in the scene when a screen can show it (see [`compose_now`]), and shows it on
/// each screen whose part changed and that has no flip pending; nothing changed means no frame. Clients that committed without a visible change get
/// their frame callbacks, at most once a refresh, so that an idle screen never spins. Returns when those
/// are next due.
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
    // The scene is composed when a screen can show it (see `compose_now`). Otherwise the frame's own work
    // (the X11 windows' changes to the state) goes on, and so do the buffers' imports, which their clients
    // wait for.
    let power = |role| drawing.screens.iter().find(|s| s.role == role).map_or(Power::default(), |s| Power { on: s.on, dirty: s.dirty, pending: s.pending });
    if compose_now([power(ScreenRole::Top), power(ScreenRole::Bottom)], server.pacing.committed, server.screenshot_requested) {
        if let Err(error) = compose(drawing, server, on_capture) {
            log::warn!("drm: drawing: {error}");
        }
    } else {
        server.frame();
        server.import_dmabufs(&mut drawing.renderer);
    }
    let composed = server.pacing.commits;
    let mut due: Option<Instant> = timed.flatten();
    for screen in &mut drawing.screens {
        if flip_lost(screen.pending, screen.queued_at, now) {
            log::warn!("drm: {:?} screen: no flip {} ms after its frame: drawn again", screen.role, FLIP_LOST.as_millis());
            screen.surface.reset_buffers();
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
        if turn.present {
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

/// Whether the scene is composed this turn, the screens being `screens` (the top's, the second's) and
/// `committed` what committed since it was last composed: always for a screenshot, never with both
/// screens off, and otherwise unless every commit is a window's on a screen with a frame queued. Such a
/// screen shows nothing new before its flip, and an uncapped game commits faster than the screen
/// refreshes: its commits until the flip are composed once, at the flip, rather than each in turn. A
/// commit on a screen that is off holds nothing back, since no flip comes there. A turn without commits
/// composes whatever else changed, and the rest of the scene waits at most a refresh for a flip.
fn compose_now(screens: [Power; 2], committed: crate::pacing::Committed, screenshot: bool) -> bool {
    if screenshot {
        return true;
    }
    if !screens.iter().any(|screen| screen.on) {
        return false;
    }
    let mut on_screens = screens.iter().zip(committed.screens).filter(|(screen, committed)| *committed && screen.on).map(|(screen, _)| screen);
    let held = !committed.elsewhere && on_screens.clone().next().is_some() && on_screens.all(|screen| screen.pending);
    !held
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
                screen.surface.reset_buffers();
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

/// Draws what changed in the scene, marking the screens whose part it is in.
fn compose(drawing: &mut Drawing, server: &mut Server, on_capture: &mut impl FnMut(Result<clamshell_state::ScreenshotPixels, String>)) -> Result<(), Error> {
    server.pacing.committed = Default::default();
    server.frame();
    server.sync_pointer_capture(|_| Ok::<(), std::convert::Infallible>(()));
    server.import_dmabufs(&mut drawing.renderer);
    let Drawing { renderer, pipeline, screens, frames, cursor, elements, states } = drawing;
    render::elements(server, frames, cursor, renderer, elements)?;
    let (composed, changed) = pipeline.compose(server, renderer, elements)?;
    *states = composed;
    if let Some(histogram) = pipeline.take_histogram() {
        server.report_histogram(histogram);
    }
    if std::mem::take(&mut server.screenshot_requested) {
        on_capture(pipeline.capture(renderer));
    }
    if let Some(changed) = changed {
        for screen in screens.iter_mut() {
            let part = Rectangle::<i32, Physical>::new((screen.part.loc.x, screen.part.loc.y).into(), (screen.part.size.w, screen.part.size.h).into());
            screen.dirty |= part.overlaps(changed);
        }
    }
    Ok(())
}

/// Shows screen `index` its part of the scene.
fn present(drawing: &mut Drawing, server: &Server, index: usize) -> Result<(), Error> {
    let Drawing { renderer, pipeline, screens, states, .. } = drawing;
    let Some(screen) = screens.get_mut(index) else {
        return Ok(());
    };
    let (mut buffer, _age) = screen.surface.next_buffer().map_err(|e| format!("{:?} screen: {e}", screen.role))?;
    let sync = {
        let mut target = renderer.bind(&mut buffer)?;
        pipeline.present_part(renderer, &mut target, screen.role == ScreenRole::Bottom, screen.part, screen.size, screen.transform)?
    };
    screen.surface.queue_buffer(Some(sync), None, states.clone()).map_err(|e| format!("{:?} screen: {e}", screen.role))?;
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
    fn commits_on_a_screen_with_a_frame_queued_wait_for_its_flip() {
        use crate::pacing::Committed;
        let free = Power { on: true, dirty: false, pending: false };
        let queued = Power { pending: true, ..free };
        let off = Power::default();
        let top = Committed { screens: [true, false], elsewhere: false };
        assert!(!compose_now([queued, free], top, false), "the top screen's flip shows it no sooner");
        assert!(compose_now([free, queued], top, false), "the top screen shows it at its next refresh");
        assert!(compose_now([queued, free], Committed { elsewhere: true, ..top }, false), "a popup or a cursor: at once");
        assert!(!compose_now([queued, queued], Committed { screens: [true, true], elsewhere: false }, false));
        assert!(compose_now([queued, free], Committed { screens: [true, true], elsewhere: false }, false), "the second screen shows its part");
        assert!(compose_now([queued, free], Committed::default(), false), "no commits: whatever else changed, as before");
        assert!(compose_now([queued, off], Committed { screens: [false, true], elsewhere: false }, false), "no flip comes on a screen off");
        assert!(!compose_now([off, off], top, false), "both off: nothing shows it");
        assert!(compose_now([off, off], top, true), "a screenshot");
        assert!(compose_now([queued, queued], top, true));
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
