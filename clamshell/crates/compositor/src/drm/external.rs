//! An external display (a monitor on USB-C's DisplayPort alt mode): any connected connector that isn't one
//! of the profile's two panels. While one is connected it mirrors the top screen: the top screen's scene,
//! composed once, is encoded for it in sRGB without the panel's turn, one to one in a mode of the scene's
//! size where it has one (see [`external_mode`]), else fitted into its preferred mode. Its flips pace the
//! top screen's windows (frame callbacks, presentation feedback, the frame rate cap) and it composes the
//! scene; the top panel follows, showing the newest frame at each of its refreshes, at the external
//! display's rate where it has a mode at it, and with its own buffers, a window's and its planes off, so
//! that the picture composed holds everything, the cursor included (see [`pacing_decide`]).
//!
//! The connectors are looked at again at each udev change of the GPU, at start and when the seat is back.
//! A display connected is probed for its modes on a thread of its own, then gets a CRTC no screen uses, a
//! surface and buffers of its own; disconnected, they are dropped. One that can't be shown is left until it
//! is unplugged. One external display is shown at a time; another connected meanwhile is logged and left
//! dark.

use super::*;
use crate::colour::render::Encoding;

/// What an external display's screen has that a panel's hasn't.
pub(super) struct External {
    /// Its connector's name ("DP-1").
    pub(super) name: String,
    /// The top screen's scene encoded for it, and where on it.
    pub(super) encoding: Encoding,
}

/// What the mode choice reads of a mode.
#[derive(Debug, Clone, Copy)]
pub(super) struct ModeFacts {
    pub(super) size: (u16, u16),
    pub(super) hz: u32,
    pub(super) preferred: bool,
    pub(super) interlaced: bool,
}

impl ModeFacts {
    pub(super) fn of(mode: &smithay::reexports::drm::control::Mode) -> ModeFacts {
        use smithay::reexports::drm::control::ModeFlags;
        ModeFacts {
            size: mode.size(),
            hz: mode.vrefresh(),
            preferred: mode.mode_type().contains(ModeTypeFlags::PREFERRED),
            interlaced: mode.flags().contains(ModeFlags::INTERLACE),
        }
    }
}

/// The mode an external display shows a picture of `size` in, and whether it is of that size (shown one to
/// one): of that size and progressive, at 120 Hz if it has one, else 60, else its highest rate (120 and 60
/// being the panel's, which then follows it at the same rate), the one marked preferred among those of
/// that rate, else the first. Without one of that size, its preferred mode, else its first: the picture
/// is fitted into it. None without modes.
pub(super) fn external_mode<T>(modes: &[T], facts: impl Fn(&T) -> ModeFacts, size: (u16, u16)) -> Option<(&T, bool)> {
    let facts = &facts;
    let sized = || modes.iter().filter(move |mode| facts(mode).size == size && !facts(mode).interlaced);
    let at = |hz: u32| {
        let mut rated = sized().filter(move |mode| facts(mode).hz == hz);
        let first = rated.next()?;
        Some(std::iter::once(first).chain(rated).find(|mode| facts(mode).preferred).unwrap_or(first))
    };
    let highest = sized().map(|mode| facts(mode).hz).max();
    if let Some(mode) = at(120).or_else(|| at(60)).or_else(|| highest.and_then(at)) {
        return Some((mode, true));
    }
    let mode = modes.iter().find(|mode| facts(mode).preferred).or_else(|| modes.first())?;
    Some((mode, false))
}

/// Where a picture of size `picture` goes on a screen of size `screen`: as large as fits, its
/// proportions kept, in the middle (bars above and below, or at the sides).
pub(super) fn fit(picture: (i32, i32), screen: (i32, i32)) -> Rectangle<i32, Physical> {
    let (pw, ph) = (i64::from(picture.0.max(1)), i64::from(picture.1.max(1)));
    let (sw, sh) = (i64::from(screen.0.max(0)), i64::from(screen.1.max(0)));
    let (w, h) = if sw * ph <= sh * pw { (sw, ph * sw / pw) } else { (pw * sh / ph, sh) };
    let narrow = |v: i64| i32::try_from(v).unwrap_or(0);
    Rectangle::new((narrow((sw - w) / 2), narrow((sh - h) / 2)).into(), (narrow(w), narrow(h)).into())
}

/// Which screen paces the windows of a part of the scene (`down`: the second screen's), of `screens`,
/// each whether it is an external display and which part it shows: an external display showing it while
/// one does, else its panel. The one choice of who paces; everything else reads [`Screen::paces`].
pub(super) fn pacer(screens: impl Iterator<Item = (bool, bool)> + Clone, down: bool) -> Option<usize> {
    let of = |external: bool| screens.clone().position(|screen| screen == (external, down));
    of(true).or_else(|| of(false))
}

/// Sets which screens pace their windows (see [`pacer`]), and which panels are mirrored, logged when it
/// changes. A screen that starts or stops pacing shows its next frame whole.
pub(super) fn pacing_decide(drawing: &mut Drawing) {
    let kinds = || drawing.screens.iter().map(|screen| (screen.external.is_some(), screen.down()));
    let pacers = [pacer(kinds(), false), pacer(kinds(), true)];
    for (index, screen) in drawing.screens.iter_mut().enumerate() {
        let paces = pacers.get(screen.scene()).copied().flatten() == Some(index);
        if paces != screen.paces {
            screen.paces = paces;
            screen.dirty = true;
            // (What it held back is the new pacer's to send; a frame in flight still gets its feedback.)
            screen.held = None;
            screen.next_callbacks = None;
            screen.queued.states.clear();
        }
        let mirrored = screen.external.is_none() && !paces;
        if mirrored != screen.mirrored {
            screen.mirrored = mirrored;
            screen.dirty = true;
            if mirrored {
                log::info!(
                    "drm: the {} screen is mirrored on the external display, which paces its windows: its own buffers, a window's and its planes are off",
                    screen.name()
                );
            } else {
                log::info!("drm: the {} screen paces its windows again: its own buffers, a window's and its planes are back", screen.name());
            }
        }
    }
}

/// The refresh the top panel is set to: an external display's mirroring it (`mirror`) where the panel
/// has a mode at that rate (of `rates`), so that it shows each of its frames; else the one the state
/// wants (0: the panel's own).
pub(super) fn panel_hz(wanted: u16, mirror: Option<u32>, mut rates: impl Iterator<Item = u32>) -> u32 {
    mirror.filter(|hz| rates.any(|rate| rate == *hz)).unwrap_or(u32::from(wanted))
}

/// Tells clients the refresh of the screen pacing the top screen's windows, when it changes.
pub(super) fn tell_refresh(drawing: &Drawing, server: &mut Server) {
    let Some(pacer) = drawing.screens.iter().find(|screen| screen.paces && !screen.down()) else { return };
    let refresh = millihertz(&pacer.surface.pending_mode());
    if refresh != server.screens.refresh {
        server.screens.refresh = refresh;
        server.resize(server.screens.size);
    }
}

/// A connector probed off the loop's thread (see [`connectors_changed`]): its handle, name, and what the
/// probe found.
pub(super) struct Probed {
    handle: connector::Handle,
    name: String,
    info: Result<connector::Info, String>,
}

/// Looks at the connectors: an external display shown and no longer connected is dropped, and one
/// connected and not shown is probed for its modes, off this thread (a moment's wait on the display's
/// EDID, seconds with a bad dock), to be shown when the probe answers ([`probed`]). One that couldn't be
/// shown is left until it is unplugged. Not while the seat is away (its return looks again), nor while a
/// probe runs (its answer looks again): the kernel holds the connectors' lock for the probe, which even an
/// unprobed look would wait on.
pub(super) fn connectors_changed(drawing: &mut Drawing, server: &mut Server) {
    if drawing.away {
        return;
    }
    if drawing.probing {
        drawing.look_again = true;
        return;
    }
    let Drawing { drm, screens, .. } = drawing;
    screens.retain_mut(|screen| {
        let Some(external) = &screen.external else { return true };
        let connected = drm.get_connector(screen.connector, false).is_ok_and(|info| info.state() == connector::State::Connected);
        if !connected {
            log::info!("drm: the external display on {} disconnected: its screen dropped", external.name);
            screen.feedback.discard();
        }
        connected
    });
    let resources = match drawing.drm.resource_handles() {
        Ok(resources) => resources,
        Err(error) => return log::warn!("drm: the connectors: {error}: no external display looked for"),
    };
    let shown = drawing.screens.iter().find_map(|screen| screen.external.as_ref().map(|external| external.name.clone()));
    let mut connectors = Vec::new();
    for &handle in resources.connectors() {
        let Ok(info) = drawing.drm.get_connector(handle, false).inspect_err(|error| log::warn!("drm: a connector: {error}")) else { continue };
        let name = connector_name(&info);
        if drawing.panels.contains(&name.as_str()) {
            continue;
        }
        let connected = info.state() == connector::State::Connected;
        if let Some(shown) = shown.as_ref().filter(|shown| connected && **shown != name) {
            log::info!("drm: {name} is connected, but one external display is shown at a time: {shown} is");
        }
        connectors.push((handle, connected, name));
    }
    let states: Vec<_> = connectors.iter().map(|(handle, connected, _)| (*handle, *connected)).collect();
    let Some(handle) = to_probe(&states, shown.is_some(), &mut drawing.failed) else { return };
    let name = connectors.into_iter().find_map(|(h, _, name)| (h == handle).then_some(name)).unwrap_or_default();
    let fd = drawing.drm.device_fd().clone();
    let sender = drawing.probes.clone();
    let probe = name.clone();
    let spawned = std::thread::Builder::new().name("drm probe".into()).spawn(move || {
        let info = fd.get_connector(handle, true).map_err(|e| e.to_string());
        if sender.send(Probed { handle, name: probe, info }).is_err() {
            log::debug!("drm: a connector's probe answered after the compositor ended");
        }
    });
    match spawned {
        Ok(_) => drawing.probing = true,
        Err(error) => {
            log::warn!("drm: no thread to probe {name} on ({error}): probed here, which waits on its display");
            let info = drawing.drm.get_connector(handle, true).map_err(|e| e.to_string());
            probed(drawing, server, Probed { handle, name, info });
        }
    }
}

/// Which connector, of the ones not a panel's (each its handle and whether it is connected), to probe for
/// an external display: the first connected that isn't `failed`, while none is `shown`. A failed one no
/// longer connected is forgotten, to be tried when it is plugged in again.
pub(super) fn to_probe<H: Copy + PartialEq>(connectors: &[(H, bool)], shown: bool, failed: &mut Vec<H>) -> Option<H> {
    failed.retain(|handle| connectors.iter().any(|(h, connected)| h == handle && *connected));
    if shown {
        return None;
    }
    connectors.iter().find(|(handle, connected)| *connected && !failed.contains(handle)).map(|(handle, _)| *handle)
}

/// A connector's probe answered (see [`connectors_changed`]): the external display on it is shown, or, if
/// it can't be, left until it is unplugged; then the connectors are looked at again if they changed
/// meanwhile.
pub(super) fn probed(drawing: &mut Drawing, server: &mut Server, probed: Probed) {
    drawing.probing = false;
    if drawing.away {
        // (Its return looks again.)
        return;
    }
    let Probed { handle, name, info } = probed;
    let shown = match info {
        Ok(info) if info.state() != connector::State::Connected => {
            log::info!("drm: {name} was unplugged as it was probed");
            Ok(())
        }
        Ok(info) => connect(drawing, server, handle, &info, name.clone()),
        Err(error) => Err(format!("probing it: {error}").into()),
    };
    if let Err(error) = shown {
        log::warn!("drm: the external display on {name} isn't shown: {error}; not tried again until it is unplugged");
        drawing.failed.push(handle);
    }
    if std::mem::take(&mut drawing.look_again) {
        connectors_changed(drawing, server);
    }
}

/// Gives the external display on connector `handle` (`name`), probed (`info`, the modes of the display now
/// connected, which the connector's last list, of one connected before, may not be), its screen,
/// mirroring the top screen.
fn connect(drawing: &mut Drawing, server: &mut Server, handle: connector::Handle, info: &connector::Info, name: String) -> Result<(), Error> {
    let resources = drawing.drm.resource_handles().map_err(|e| e.to_string())?;
    let picture = (server.screens.top_width, server.screens.size.1);
    let size = (u16::try_from(picture.0).unwrap_or(0), u16::try_from(picture.1).unwrap_or(0));
    let (mode, one_to_one) = external_mode(info.modes(), ModeFacts::of, size).ok_or("it has no modes")?;
    let mode = *mode;
    let (w, h) = mode.size();
    if !one_to_one {
        log::info!("drm: {name} has no {}x{} mode: the top screen's picture fitted into its {w}x{h}, with bars", size.0, size.1);
    }
    let taken: Vec<crtc::Handle> = drawing.screens.iter().map(|screen| screen.crtc).collect();
    let crtc = match current_crtc(&drawing.drm, info, &name, &taken) {
        Some(crtc) => crtc,
        None => free_crtc(&drawing.drm, &resources, info, &name, &taken)?,
    };
    let surface = presenter(&mut drawing.drm, &drawing.gbm, &drawing.renderer, crtc, mode, handle)?;
    let dst = fit(picture, (i32::from(w), i32::from(h)));
    let encoding = drawing.pipeline.encoding(&mut drawing.renderer, false, ColourDescription::default(), dst)?;
    log::info!("drm: an external display on {name} at {w}x{h} {} Hz, {}: it mirrors the top screen and paces it", mode.vrefresh(), surface.format());
    let part = Rectangle::new((0, 0).into(), picture.into());
    let external = External { name, encoding };
    let mut screen = Screen::new(ScreenRole::Top, handle, crtc, surface, mode, vec![mode], part, Transform::Normal, Some(external));
    screen.on = server.screens.on.first().copied().unwrap_or(true);
    if !screen.on {
        // (Off as the top screen is: its first commit turns it on.)
        screen.dirty = false;
    }
    drawing.screens.push(screen);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mode(w: u16, h: u16, hz: u32) -> ModeFacts {
        ModeFacts { size: (w, h), hz, preferred: false, interlaced: false }
    }

    fn chosen(modes: &[ModeFacts]) -> Option<(usize, bool)> {
        external_mode(modes, |m| *m, (1920, 1080)).map(|(m, one)| (modes.iter().position(|x| std::ptr::eq(x, m)).unwrap_or(99), one))
    }

    /// 1080p at 120 Hz, else 60, else its highest rate, progressive, the one marked preferred among equals.
    #[test]
    fn an_external_display_shows_1080p_at_120_else_60_else_its_highest() {
        let preferred = ModeFacts { preferred: true, ..mode(3840, 2160, 60) };
        assert_eq!(chosen(&[preferred, mode(1920, 1080, 60), mode(1920, 1080, 120), mode(1920, 1080, 144)]), Some((2, true)));
        assert_eq!(chosen(&[preferred, mode(1920, 1080, 50), mode(1920, 1080, 60), mode(1920, 1080, 144)]), Some((2, true)), "60 before the highest");
        assert_eq!(chosen(&[preferred, mode(1920, 1080, 50), mode(1920, 1080, 75)]), Some((2, true)), "neither: the highest");
        let interlaced = ModeFacts { interlaced: true, ..mode(1920, 1080, 120) };
        assert_eq!(chosen(&[interlaced, mode(1920, 1080, 60)]), Some((1, true)), "not interlaced");
        let marked = ModeFacts { preferred: true, ..mode(1920, 1080, 60) };
        assert_eq!(chosen(&[mode(1920, 1080, 60), marked, mode(1920, 1080, 60)]), Some((1, true)), "the preferred of a rate");
        assert_eq!(chosen(&[mode(1920, 1080, 60), mode(1920, 1080, 60)]), Some((0, true)), "else the first");
    }

    /// Without 1080p: its preferred mode, else its first, the picture fitted into it.
    #[test]
    fn without_1080p_an_external_display_shows_its_preferred_mode_fitted() {
        let preferred = ModeFacts { preferred: true, ..mode(1280, 1024, 60) };
        assert_eq!(chosen(&[mode(1024, 768, 60), preferred]), Some((1, false)));
        assert_eq!(chosen(&[mode(1024, 768, 60), mode(800, 600, 60)]), Some((0, false)));
        assert_eq!(chosen(&[ModeFacts { interlaced: true, preferred: true, ..mode(1920, 1080, 60) }]), Some((0, false)), "interlaced only");
        assert_eq!(chosen(&[]), None);
        // Fitted: bars above and below on a squarer screen, at the sides on a wider one, none at its size.
        assert_eq!(fit((1920, 1080), (1280, 1024)), Rectangle::new((0, 152).into(), (1280, 720).into()));
        assert_eq!(fit((1920, 1080), (2560, 1080)), Rectangle::new((320, 0).into(), (1920, 1080).into()));
        assert_eq!(fit((1920, 1080), (3840, 2160)), Rectangle::new((0, 0).into(), (3840, 2160).into()));
        assert_eq!(fit((1920, 1080), (1920, 1080)), Rectangle::new((0, 0).into(), (1920, 1080).into()));
    }

    /// An external display showing a part paces its windows while connected; without one, its panel does.
    #[test]
    fn an_external_display_paces_the_top_screen_while_connected_else_the_top_panel_does() {
        let panels = [(false, false), (false, true)];
        assert_eq!(pacer(panels.iter().copied(), false), Some(0), "disconnected: the top panel");
        assert_eq!(pacer(panels.iter().copied(), true), Some(1));
        let connected = [(false, false), (false, true), (true, false)];
        assert_eq!(pacer(connected.iter().copied(), false), Some(2), "connected: the external display");
        assert_eq!(pacer(connected.iter().copied(), true), Some(1), "the second screen paces itself still");
        assert_eq!(pacer([(false, false)].iter().copied(), true), None, "no second screen");
    }

    /// A connected display is probed once none is shown; one that couldn't be shown is left until it is
    /// unplugged, then probed again when plugged in.
    #[test]
    fn a_display_that_failed_is_probed_again_only_once_unplugged() {
        let mut failed = Vec::new();
        assert_eq!(to_probe(&[(1, false), (2, true)], false, &mut failed), Some(2), "the connected one");
        assert_eq!(to_probe(&[(1, false), (2, true)], true, &mut failed), None, "one shown already");
        assert_eq!(to_probe::<u32>(&[], false, &mut failed), None);
        failed.push(2);
        assert_eq!(to_probe(&[(1, false), (2, true)], false, &mut failed), None, "failed: not again at every change");
        assert_eq!(to_probe(&[(1, true), (2, true)], false, &mut failed), Some(1), "another one, though");
        assert_eq!(to_probe(&[(1, false), (2, false)], false, &mut failed), None);
        assert!(failed.is_empty(), "unplugged: forgotten");
        assert_eq!(to_probe(&[(1, false), (2, true)], false, &mut failed), Some(2), "plugged in again: probed");
    }

    /// The top panel follows the external display's rate where it has a mode at it, else the state's.
    #[test]
    fn the_top_panel_takes_the_external_displays_rate_where_it_has_it() {
        let rates = [60, 120];
        assert_eq!(panel_hz(0, Some(120), rates.iter().copied()), 120);
        assert_eq!(panel_hz(120, Some(60), rates.iter().copied()), 60, "over the state's");
        assert_eq!(panel_hz(90, Some(75), rates.iter().copied()), 90, "a rate the panel hasn't: the state's");
        assert_eq!(panel_hz(0, None, rates.iter().copied()), 0, "none connected: the state's");
    }
}
