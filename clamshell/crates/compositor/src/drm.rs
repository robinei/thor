//! ClamShell's compositor on the machine's own screens: a seat from libseat, the GPU through DRM and GBM,
//! input from libinput. The device profile names the connector of each screen and how its panel is
//! mounted. The scene is composed once (the colour pipeline), and each screen is shown its part of it,
//! turned as its panel is; each screen's page flips pace its frames, and their times are the presentation
//! feedback clients get.

use std::cell::RefCell;
use std::path::PathBuf;
use std::rc::Rc;
use std::time::{Duration, Instant};

use clamshell_device::{Profile, Rotation, ScreenRole, TouchSelector};
use clamshell_state::{Effect, State};
use clamshell_ui::{Config, Error, EventSender};
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
    colour::render::Pipeline,
    render::{self, ArrowFrame, UiFrame},
    server::{Process, Server},
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
    /// The scene changed in the screen's part since its last frame.
    dirty: bool,
    /// The commits its last frame callbacks answered, and when an idle screen may send the next ones.
    answered: u64,
    next_callbacks: Option<Instant>,
}

/// What drawing needs, shared by the loop's sources.
struct Drawing {
    renderer: GlesRenderer,
    pipeline: Pipeline,
    screens: Vec<Screen>,
    frames: [UiFrame; 2],
    arrows: ArrowFrame,
    cursor: crate::cursor::Cursor,
    elements: Vec<render::Element>,
    /// The scene's elements as last composed.
    states: RenderElementStates,
}

/// A connector chosen for a screen: its handle, mode and CRTC.
struct Chosen {
    connector: connector::Handle,
    mode: smithay::reexports::drm::control::Mode,
    crtc: crtc::Handle,
}

/// Runs the compositor on the machine's screens until ClamShell quits.
pub fn run(
    state: State,
    mut config: Config,
    start: impl FnOnce(EventSender, PathBuf),
    on_effect: impl FnMut(&Effect) + 'static,
    identify: impl Fn(u32) -> Process + 'static,
    on_capture: impl FnMut(Result<crate::Capture, String>) + 'static,
) -> Result<(), Error> {
    let profile = *state.profile();
    let (mut session, notifier) = LibSeatSession::new().map_err(|e| format!("drm: no seat: {e}"))?;
    let seat = session.seat();
    let gpu = smithay::backend::udev::primary_gpu(&seat)?
        .or_else(|| smithay::backend::udev::all_gpus(&seat).ok()?.into_iter().next())
        .ok_or("drm: no GPU on the seat")?;
    log::info!("drm: {} on seat {seat}", gpu.display());
    let fd = session.open(&gpu, OFlags::RDWR | OFlags::CLOEXEC | OFlags::NOCTTY | OFlags::NONBLOCK).map_err(|e| format!("drm: {}: {e}", gpu.display()))?;
    let fd = DrmDeviceFd::new(DeviceFd::from(fd));
    // The connectors are left as they are, so each screen can keep the mode the kernel already set.
    let (mut drm, drm_notifier) = DrmDevice::new(fd.clone(), false).map_err(|e| format!("drm: {e}"))?;
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

    let mut base = Base::new(state, config, true, identify, on_effect)?;
    let server = &mut base.server;
    // No host window takes the focus away: pointer capture is always possible.
    server.host_focused = true;
    server.refresh = i32::try_from(top.mode.vrefresh()).unwrap_or(60).saturating_mul(1000);
    server.resize(server.size);
    server.advertise_dmabuf(&renderer);
    let pipeline = Pipeline::new(&mut renderer, server.size)?;
    crate::colour::protocol::install(&server.dh);

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
        let part = Rectangle::new((x, 0).into(), (width, server.size.1).into());
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
            dirty: true,
            answered: 0,
            next_callbacks: None,
        });
    }

    let top_ui = server.core.size(clamshell_ui::Screen::Top).0;
    let bottom_ui = server.core.size(clamshell_ui::Screen::Bottom).0;
    let drawing = Rc::new(RefCell::new(Drawing {
        renderer,
        pipeline,
        screens,
        frames: [UiFrame::new(top_ui)?, UiFrame::new(bottom_ui)?],
        arrows: ArrowFrame::new(top_ui)?,
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
fn choose(drm: &DrmDevice, profile: &Profile, role: ScreenRole, taken: Option<crtc::Handle>) -> Result<Chosen, Error> {
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
            .and_then(|e| drm.get_encoder(e).ok()?.crtc())
            .filter(|c| Some(*c) != taken)
            .and_then(|c| Some((c, drm.get_crtc(c).ok()?.mode()?)));
        if let Some((crtc, mode)) = current {
            return Ok(Chosen { connector: handle, mode, crtc });
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
            .filter_map(|&e| drm.get_encoder(e).ok())
            .flat_map(|e| resources.filter_crtcs(e.possible_crtcs()))
            .find(|c| Some(*c) != taken)
            .ok_or_else(|| format!("drm: no CRTC for {name}"))?;
        return Ok(Chosen { connector: handle, mode, crtc });
    }
    Err(format!("drm: no connector {name} for the {role:?} screen").into())
}

/// The frame queued on screen `index` is shown: its time is the presentation feedback, and the clients
/// shown there get their frame callbacks.
fn flipped(drawing: &mut Drawing, server: &mut Server, index: usize, metadata: Option<&DrmEventMetadata>) {
    let Some(screen) = drawing.screens.get_mut(index) else {
        return;
    };
    screen.pending = false;
    let states = match screen.surface.frame_submitted() {
        Ok(states) => states,
        Err(error) => {
            log::warn!("drm: {:?} screen: {error}", screen.role);
            None
        }
    };
    let time: Time<Monotonic> = match metadata.map(|m| m.time) {
        Some(DrmEventTime::Monotonic(at)) => at.into(),
        _ => server.clock.now(),
    };
    let down = screen.role == ScreenRole::Bottom;
    if let Some(states) = states {
        server.present_flip(&states, time, down, screen.interval, metadata.map_or(0, |m| u64::from(m.sequence)));
    }
    callbacks(server, down);
}

/// Frame callbacks for the windows shown on the top screen or the second (`down`).
fn callbacks(server: &Server, down: bool) {
    let [top, bottom] = &server.outputs;
    let output = if down { bottom } else { top };
    for client in server.windows.iter().filter(|w| w.mapped && server.visible(w.id)) {
        if server.core.state().session().window(client.id).is_some_and(|w| w.down) == down {
            client.window.send_frame(output, server.started.elapsed(), Some(Duration::ZERO), |_, _| Some(output.clone()));
        }
    }
}

/// Composes what changed in the scene and shows it on each screen whose part changed and that has no
/// flip pending; nothing changed means no frame. Clients that committed without a visible change get
/// their frame callbacks, at most once a refresh, so that an idle screen never spins. Returns when those
/// are next due.
fn step(drawing: &mut Drawing, server: &mut Server, on_capture: &mut impl FnMut(Result<crate::Capture, String>), now: Instant) -> Option<Instant> {
    if let Err(error) = compose(drawing, server, on_capture) {
        log::warn!("drm: drawing: {error}");
    }
    let mut due: Option<Instant> = None;
    for index in 0..drawing.screens.len() {
        let Some(screen) = drawing.screens.get_mut(index) else {
            continue;
        };
        if screen.pending {
            continue;
        }
        if screen.dirty {
            if let Err(error) = present(drawing, server, index) {
                log::warn!("drm: drawing: {error}");
            }
            continue;
        }
        if server.commits != screen.answered {
            match screen.next_callbacks {
                Some(at) if at > now => due = Some(due.map_or(at, |d| d.min(at))),
                _ => {
                    screen.answered = server.commits;
                    screen.next_callbacks = Some(now + screen.interval);
                    callbacks(server, screen.role == ScreenRole::Bottom);
                }
            }
        }
    }
    due
}

/// Draws what changed in the scene, marking the screens whose part it is in.
fn compose(drawing: &mut Drawing, server: &mut Server, on_capture: &mut impl FnMut(Result<crate::Capture, String>)) -> Result<(), Error> {
    server.frame();
    server.sync_pointer_capture(|_| Ok::<(), std::convert::Infallible>(()));
    server.import_dmabufs(&mut drawing.renderer);
    let Drawing { renderer, pipeline, screens, frames, arrows, cursor, elements, states } = drawing;
    render::elements(server, frames, arrows, renderer, elements)?;
    cursor.elements(server, renderer, elements)?;
    let (composed, changed) = pipeline.compose(server, renderer, elements)?;
    *states = composed;
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
        pipeline.present_part(renderer, &mut target, screen.part, screen.size, screen.transform)?
    };
    screen.surface.queue_buffer(Some(sync), None, states.clone()).map_err(|e| format!("{:?} screen: {e}", screen.role))?;
    screen.pending = true;
    screen.dirty = false;
    screen.answered = server.commits;
    Ok(())
}

/// Where each absolute input device lies in the scene: on the screen the profile places it, else, for a
/// touchscreen, the profile's catch-all; anything else (a tablet nobody named) covers the top screen.
#[derive(Clone, Copy)]
struct Areas {
    profile: Profile,
    top: Size<i32, Logical>,
    bottom: Size<i32, Logical>,
}

impl Areas {
    fn role(&self, device: &InputDevice) -> ScreenRole {
        let id = identifier(device);
        let named = self.profile.touchscreens.iter().find(|t| matches!(t.selector, TouchSelector::Named(n) if n == id));
        let touch = device.has_capability(DeviceCapability::Touch.into());
        named
            .or_else(|| touch.then(|| self.profile.touchscreens.iter().find(|t| t.selector == TouchSelector::All)).flatten())
            .map_or(ScreenRole::Top, |t| t.screen)
    }

    fn of(&self, device: &InputDevice) -> Rectangle<f64, Logical> {
        match self.role(device) {
            ScreenRole::Top => Rectangle::from_size(self.top.to_f64()),
            ScreenRole::Bottom => Rectangle::new((f64::from(self.top.w), 0.0).into(), self.bottom.to_f64()),
        }
    }
}

/// A device's identifier as sway writes it and the profile names it: vendor, product and name.
fn identifier(device: &InputDevice) -> String {
    format!("{}:{}:{}", device.id_vendor(), device.id_product(), device.name().replace(' ', "_"))
}
