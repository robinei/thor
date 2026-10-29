//! What a screen shows, and its commits: the buffers ClamShell composes into, on the screen's primary
//! plane. One frame is in flight at a time: a buffer is taken ([`Presenter::next_buffer`]), drawn and
//! committed ([`Presenter::queue`]), and shown at the next flip ([`Presenter::flipped`]), when the buffer
//! shown before is free again. Smithay's `GbmBufferedSurface` did this; ClamShell's own owns every commit
//! of the screen, so that a frame scanned out is another kind of commit rather than a second owner of the
//! screen: a screen showing only ClamShell shows a buffer of its own (see `ui_scanout`), one showing only
//! a window shows the window's (see `window_scanout`). Every commit carries the screen's other planes too,
//! its overlay and cursor (see `plane_layers`), and a change to those alone commits the frame shown again
//! ([`Presenter::show_again`]). The buffers composed into are dropped a while after the screen last composed
//! (see `crate::hold`), as when it shows ClamShell's own buffers or a window's, and made again when it
//! composes.

use smithay::backend::allocator::format::get_opaque;
use smithay::backend::allocator::gbm::{GbmAllocator, GbmBuffer};
use smithay::backend::allocator::{Format, Fourcc, Modifier, Slot, Swapchain, dmabuf::AsDmabuf, dmabuf::Dmabuf};
use smithay::backend::drm::gbm::{GbmFramebuffer, framebuffer_from_bo};
use smithay::backend::drm::{DrmDeviceFd, DrmSurface, PlaneConfig, PlaneState};
use smithay::backend::renderer::sync::SyncPoint;
use smithay::reexports::drm::control::Mode;
use smithay::utils::{Rectangle, Transform};

use clamshell_ui::Error;

pub(crate) struct Presenter {
    surface: DrmSurface,
    swapchain: Swapchain<GbmAllocator<DrmDeviceFd>>,
    /// Whether the plane takes only the opaque variant of the format (`XR30` for `AR30`).
    opaque: bool,
    /// Whether a commit can wait on the drawing's fence itself, else it is waited on first.
    fencing: bool,
    /// The buffer taken to draw into, until it is queued.
    next: Option<Slot<GbmBuffer>>,
    /// The frame committed, until its flip, and when it was committed: it is in flight until then.
    pending: Option<Shown>,
    queued_at: Option<std::time::Instant>,
    /// A frame committed whose flip was lost: it may be the one shown, so it is kept until a flip.
    lost: Option<Shown>,
    /// The frame shown.
    current: Option<Shown>,
    /// Properties the next commit of any kind sets: what the kind of commit shown before set, cleared.
    clear: Vec<crate::scanout::Property>,
    /// The next commit's properties: those cleared, then its own.
    properties: Vec<crate::scanout::Property>,
    /// What the primary plane showed in the last commit, to commit again with other planes changed.
    last: Option<Primary>,
    /// When it last composed, and whether it holds buffers to compose into.
    hold: crate::hold::Hold,
    composing_buffers: bool,
}

/// Why a commit didn't go through, by what is to be done about it.
#[derive(Debug, PartialEq)]
pub(crate) enum CommitError {
    /// The display engine refused what was asked of it: asked again, it answers the same.
    Refused(String),
    /// It was busy (a flip still in flight): the same commit may go through later.
    Busy(String),
    /// The seat is another session's.
    Away,
    /// Anything else, which may pass: the commit is tried again.
    Failed(String),
}

impl std::fmt::Display for CommitError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Refused(why) => write!(f, "the display engine refused it: {why}"),
            Self::Busy(why) => write!(f, "busy: {why}"),
            Self::Away => write!(f, "the seat is another session's"),
            Self::Failed(why) => write!(f, "{why}"),
        }
    }
}

impl std::error::Error for CommitError {}

impl CommitError {
    /// A commit's error from Smithay, by what it says: a failed test commit or `EINVAL`/`ERANGE` (a
    /// property or value the display engine doesn't take) refused; `EBUSY`, `EAGAIN` and `EINTR` busy;
    /// the device paused, or no longer ours, away.
    fn of(error: smithay::backend::drm::DrmError) -> CommitError {
        use smithay::backend::drm::DrmError;
        let why = error.to_string();
        match &error {
            DrmError::TestFailed(_) | DrmError::UnsupportedPlaneConfiguration(_) | DrmError::PlaneNotCompatible(..) => Self::Refused(why),
            DrmError::DeviceInactive | DrmError::DrmMasterFailed => Self::Away,
            DrmError::Access(access) => Self::of_errno(rustix::io::Errno::from_io_error(&access.source), why),
            _ => Self::Failed(why),
        }
    }

    /// A commit's error by its errno, if it has one.
    fn of_errno(errno: Option<rustix::io::Errno>, why: String) -> CommitError {
        use rustix::io::Errno;
        match errno {
            Some(Errno::INVAL | Errno::RANGE) => Self::Refused(why),
            Some(Errno::BUSY | Errno::AGAIN | Errno::INTR) => Self::Busy(why),
            Some(Errno::ACCESS | Errno::PERM) => Self::Away,
            _ => Self::Failed(why),
        }
    }
}

/// What the primary plane shows: which framebuffer, which part of it, where, turned how.
#[derive(Clone, Copy)]
struct Primary {
    fb: smithay::reexports::drm::control::framebuffer::Handle,
    src: Rectangle<f64, smithay::utils::Buffer>,
    dst: Rectangle<i32, smithay::utils::Physical>,
    transform: Transform,
}

/// A frame committed: a buffer composed into or a window's, each kept from reuse until it is off the
/// screen, or one of ClamShell's own buffers, which `ui_scanout` keeps.
enum Shown {
    Composed(#[allow(dead_code, reason = "held so that the swapchain doesn't reuse it while shown")] Slot<GbmBuffer>),
    Ui,
    Window(
        #[allow(dead_code, reason = "held so that the client doesn't reuse it while shown")] smithay::backend::renderer::utils::Buffer,
        #[allow(dead_code, reason = "held so that it isn't removed while shown")] std::rc::Rc<smithay::backend::drm::gbm::GbmFramebuffer>,
    ),
    /// The frame shown, committed again: it stays.
    Again,
}

impl Presenter {
    /// The screen's buffers in the first of `formats` its primary plane and the renderer (`renderer`'s
    /// formats) both take, as a test commit shows.
    pub(crate) fn new(surface: DrmSurface, allocator: GbmAllocator<DrmDeviceFd>, formats: &[Fourcc], renderer: &[Format]) -> Result<Presenter, Error> {
        let mut allocator = Some(allocator);
        let mut last = String::from("no format tried");
        for code in formats {
            let Some(chosen) = allocator.take() else { break };
            match Self::try_format(&surface, chosen, *code, renderer) {
                Ok((mut swapchain, tried, opaque)) => {
                    // Only the format was wanted of the buffer tried: it goes now, and the first frame composed
                    // makes its own.
                    drop(tried);
                    swapchain.reset_buffers();
                    let fencing = !surface.is_legacy()
                        && smithay::reexports::drm::Device::get_driver_capability(&surface, smithay::reexports::drm::DriverCapability::SyncObj)
                            .is_ok_and(|v| v != 0)
                        && plane_has(&surface, "IN_FENCE_FD");
                    if !fencing {
                        log::info!("drm: commits take no fences here: each frame's drawing is waited for before it is committed");
                    }
                    return Ok(Presenter {
                        surface,
                        swapchain,
                        opaque,
                        fencing,
                        next: None,
                        pending: None,
                        queued_at: None,
                        lost: None,
                        current: None,
                        clear: Vec::new(),
                        properties: Vec::new(),
                        last: None,
                        hold: Default::default(),
                        composing_buffers: false,
                    });
                }
                Err((back, error)) => {
                    log::info!("drm: {code} on plane {:?}: {error}", surface.plane());
                    last = format!("{code}: {error}");
                    allocator = Some(back);
                }
            }
        }
        Err(format!("no format both the plane and the renderer take ({last})").into())
    }

    /// The swapchain in format `code`, its first buffer committed in a test, and whether the plane takes
    /// only its opaque variant; or why not, with the allocator back.
    #[allow(clippy::type_complexity)]
    fn try_format(
        surface: &DrmSurface,
        allocator: GbmAllocator<DrmDeviceFd>,
        code: Fourcc,
        renderer: &[Format],
    ) -> Result<(Swapchain<GbmAllocator<DrmDeviceFd>>, Slot<GbmBuffer>, bool), (GbmAllocator<DrmDeviceFd>, String)> {
        let opaque_code = get_opaque(code).unwrap_or(code);
        let plane: Vec<Format> = surface.plane_info().formats.iter().copied().filter(|f| f.code == code || f.code == opaque_code).collect();
        let drawn: Vec<Modifier> = renderer.iter().filter(|f| f.code == code).map(|f| f.modifier).collect();
        if plane.is_empty() {
            return Err((allocator, "the plane doesn't take it".into()));
        }
        if drawn.is_empty() {
            return Err((allocator, "the renderer doesn't draw it".into()));
        }
        // As Smithay does: a plane or renderer that takes only implicit modifiers with the other's linear
        // forces implicit, so that it works if likely linear.
        let implicit_only = |formats: &[Modifier]| formats.len() == 1 && formats.first() == Some(&Modifier::Invalid);
        let plane_modifiers: Vec<Modifier> = plane.iter().map(|f| f.modifier).collect();
        let modifiers = if (implicit_only(&plane_modifiers) && drawn.contains(&Modifier::Linear))
            || (implicit_only(&drawn) && plane_modifiers.contains(&Modifier::Linear))
        {
            vec![Modifier::Invalid]
        } else {
            plane_modifiers.iter().copied().filter(|m| drawn.contains(m)).collect()
        };
        let (w, h) = surface.pending_mode().size();
        let mut swapchain = Swapchain::new(allocator, u32::from(w), u32::from(h), code, modifiers);
        let buffer = match swapchain.acquire() {
            Ok(Some(buffer)) => buffer,
            Ok(None) => return Err((swapchain.allocator, "no free buffer".into())),
            Err(error) => return Err((swapchain.allocator, format!("allocating: {error}"))),
        };
        let opaque = !plane.iter().any(|f| f.code == code);
        if let Err(error) = framebuffer(surface, &buffer, opaque) {
            return Err((swapchain.allocator, error));
        }
        let Some(fb) = buffer.userdata().get::<GbmFramebuffer>() else { return Err((swapchain.allocator, "no framebuffer".into())) };
        let (src, dst) = whole(surface);
        let state = primary(surface, *fb.as_ref(), src, dst, Transform::Normal, None, &[]);
        if let Err(error) = surface.test_state([state], true) {
            return Err((swapchain.allocator, format!("the test commit: {error}")));
        }
        log::info!("drm: plane {:?} in {code} {:?}", surface.plane(), buffer.modifier());
        Ok((swapchain, buffer, opaque))
    }

    pub(crate) fn surface(&self) -> &DrmSurface {
        &self.surface
    }

    pub(crate) fn format(&self) -> Fourcc {
        self.swapchain.format()
    }

    /// The buffer to draw the next frame into: the same until it is queued.
    pub(crate) fn next_buffer(&mut self) -> Result<Dmabuf, Error> {
        self.hold.used(std::time::Instant::now());
        self.composing_buffers = true;
        if self.next.is_none() {
            let slot = self.swapchain.acquire().map_err(|e| format!("allocating: {e}"))?.ok_or("no free buffer: a flip not seen")?;
            if slot.userdata().get::<GbmFramebuffer>().is_none() {
                framebuffer(&self.surface, &slot, self.opaque)?;
            }
            self.next = Some(slot);
        }
        let slot = self.next.as_ref().ok_or("no buffer")?;
        Ok(slot.export().map_err(|e| format!("exporting the buffer: {e}"))?)
    }

    /// Commits the buffer drawn into, once `sync` says the drawing is done: by the commit waiting on its
    /// fence where it can, else after waiting here, with the other planes `layers`. It is shown at the next
    /// flip.
    pub(crate) fn queue<'l>(&mut self, sync: Option<SyncPoint>, layers: impl Iterator<Item = PlaneState<'l>> + Clone) -> Result<(), CommitError> {
        let slot = self.next.take().ok_or(CommitError::Failed("no buffer drawn".into()))?;
        self.swapchain.submitted(&slot);
        let fence = sync.and_then(|sync| {
            let fence = self.fencing.then(|| sync.export()).flatten();
            if fence.is_none() {
                // Without a fence to hand the commit, the drawing is waited on here; implicit sync covers
                // a wait that fails.
                if let Err(error) = sync.wait() {
                    log::debug!("drm: waiting for the drawing failed, left to implicit sync: {error:?}");
                }
            }
            fence
        });
        let fb = *slot.userdata().get::<GbmFramebuffer>().ok_or(CommitError::Failed("no framebuffer".into()))?.as_ref();
        let (src, dst) = whole(&self.surface);
        self.commit(fb, src, dst, Transform::Normal, fence.as_ref().map(std::os::fd::AsFd::as_fd), &[], layers, false)?;
        self.pending = Some(Shown::Composed(slot));
        Ok(())
    }

    /// Commits framebuffer `fb`, one of ClamShell's own buffers (see `ui_scanout`), with the CRTC's colour
    /// `properties` and the other planes `layers`. It is shown at the next flip.
    pub(crate) fn show_ui<'l>(
        &mut self,
        fb: smithay::reexports::drm::control::framebuffer::Handle,
        properties: &[crate::scanout::Property],
        layers: impl Iterator<Item = PlaneState<'l>> + Clone,
    ) -> Result<(), CommitError> {
        let (src, dst) = whole(&self.surface);
        self.commit(fb, src, dst, Transform::Normal, None, properties, layers, false)?;
        self.pending = Some(Shown::Ui);
        Ok(())
    }

    /// Commits a window's `buffer`, its framebuffer `fb`, its part `src` shown at `dst` on the panel turned
    /// by `transform`, with the plane's colour `properties` and the other planes `layers`; first in a test
    /// commit if `test`. It is shown at the next flip, and the buffer held until it is off the screen.
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn show_window<'l>(
        &mut self,
        buffer: smithay::backend::renderer::utils::Buffer,
        fb: std::rc::Rc<smithay::backend::drm::gbm::GbmFramebuffer>,
        src: Rectangle<f64, smithay::utils::Buffer>,
        dst: Rectangle<i32, smithay::utils::Physical>,
        transform: Transform,
        properties: &[crate::scanout::Property],
        layers: impl Iterator<Item = PlaneState<'l>> + Clone,
        test: bool,
    ) -> Result<(), CommitError> {
        self.commit(*(*fb).as_ref(), src, dst, transform, None, properties, layers, test)?;
        self.pending = Some(Shown::Window(buffer, fb));
        Ok(())
    }

    /// Commits the frame shown again, with the other planes `layers` changed (the cursor moved, the overlay
    /// redrawn). It stays shown.
    pub(crate) fn show_again<'l>(&mut self, layers: impl Iterator<Item = PlaneState<'l>> + Clone) -> Result<(), CommitError> {
        let Primary { fb, src, dst, transform } = self.last.ok_or(CommitError::Failed("nothing shown to commit again".into()))?;
        self.commit(fb, src, dst, transform, None, &[], layers, false)?;
        self.pending = Some(Shown::Again);
        Ok(())
    }

    /// The commit after this kind's sets `properties` too: what this kind set, cleared.
    pub(crate) fn clear_after(&mut self, properties: &[crate::scanout::Property]) {
        self.clear.extend_from_slice(properties);
    }

    /// Commits `fb` on the primary plane, `src` of it shown at `dst`, turned by `transform`, after `fence`,
    /// with the properties cleared and then `properties`, and the other planes `layers`; first in a test
    /// commit if `test`.
    #[allow(clippy::too_many_arguments)]
    fn commit<'l>(
        &mut self,
        fb: smithay::reexports::drm::control::framebuffer::Handle,
        src: Rectangle<f64, smithay::utils::Buffer>,
        dst: Rectangle<i32, smithay::utils::Physical>,
        transform: Transform,
        fence: Option<std::os::fd::BorrowedFd<'_>>,
        properties: &[crate::scanout::Property],
        layers: impl Iterator<Item = PlaneState<'l>> + Clone,
        test: bool,
    ) -> Result<(), CommitError> {
        self.properties.clear();
        self.properties.extend_from_slice(&self.clear);
        self.properties.extend_from_slice(properties);
        let state = primary(&self.surface, fb, src, dst, transform, fence, &self.properties);
        let states = || std::iter::once(state.clone()).chain(layers.clone().map(narrowed));
        if test {
            // (A mode change waiting, as after the refresh changed or the screen came on, goes in the test too.)
            let modeset = self.surface.commit_pending();
            self.surface.test_state(states(), modeset).map_err(CommitError::of)?;
        }
        let committed = if self.surface.commit_pending() { self.surface.commit(states(), true) } else { self.surface.page_flip(states(), true) };
        committed.map_err(CommitError::of)?;
        self.clear.clear();
        self.last = Some(Primary { fb, src, dst, transform });
        self.queued_at = Some(std::time::Instant::now());
        Ok(())
    }

    /// The frame committed is shown: the buffer shown before is free again. Whether there was one. A flip
    /// with none committed is a lost frame's, late, as is one of the frame shown again after it (the lost
    /// frame committed again): that frame is the one shown.
    pub(crate) fn flipped(&mut self) -> bool {
        self.queued_at = None;
        let lost = self.lost.take();
        match self.pending.take() {
            Some(Shown::Again) => {
                if let Some(lost) = lost {
                    self.current = Some(lost);
                }
                true
            }
            Some(pending) => {
                self.current = Some(pending);
                true
            }
            None => {
                if let Some(lost) = lost {
                    self.current = Some(lost);
                }
                false
            }
        }
    }

    /// Whether a frame committed waits for its flip.
    pub(crate) fn in_flight(&self) -> bool {
        self.queued_at.is_some()
    }

    /// When the frame in flight is taken as having lost its flip (see [`FLIP_LOST`]).
    pub(crate) fn flip_due(&self) -> Option<std::time::Instant> {
        self.queued_at.map(|at| at + FLIP_LOST)
    }

    /// Whether the frame shown is one composed (not ClamShell's own buffer or a window's).
    pub(crate) fn shows_composed(&self) -> bool {
        matches!(self.current, Some(Shown::Composed(_)))
    }

    /// Drops the buffers composed into if it hasn't composed for a while by `now` and none of them is shown or
    /// on its way, else tells `trims` when they will be.
    pub(crate) fn trim(&mut self, now: std::time::Instant, trims: &mut crate::hold::Trims) {
        if !self.composing_buffers {
            return;
        }
        let in_use = |shown: &Option<Shown>| matches!(shown, Some(Shown::Composed(_)));
        // (A buffer taken but not queued, its drawing failed, isn't in use: it is only kept for the next.)
        if in_use(&self.pending) || in_use(&self.lost) || in_use(&self.current) {
            // In use: kept until a while after it no longer is.
            self.hold.used(now);
            return;
        }
        if self.hold.expired(now) {
            log::info!("drm: plane {:?}'s buffers to compose into dropped", self.surface.plane());
            self.swapchain.reset_buffers();
            self.next = None;
            self.composing_buffers = false;
            trims.dropped();
            return;
        }
        trims.held(&self.hold);
    }

    /// Drops the buffers' contents and any frame in flight, as after the seat was away or a flip was lost:
    /// the next frame is drawn whole.
    pub(crate) fn reset_buffers(&mut self) {
        self.queued_at = None;
        self.swapchain.reset_buffers();
        self.composing_buffers = false;
        self.next = None;
        if let Some(pending) = self.pending.take().filter(|pending| !matches!(pending, Shown::Again)) {
            self.lost = Some(pending);
        }
    }

    /// The screen was turned off: its plane shows none of its buffers any more, so they may be dropped.
    pub(crate) fn off(&mut self) {
        self.queued_at = None;
        self.last = None;
        self.next = None;
        self.pending = None;
        self.lost = None;
        self.current = None;
    }

    pub(crate) fn pending_mode(&self) -> Mode {
        self.surface.pending_mode()
    }

    /// Changes the screen's mode at the next commit, its buffers to its size.
    pub(crate) fn use_mode(&mut self, mode: Mode) -> Result<(), Error> {
        self.surface.use_mode(mode).map_err(|e| format!("{e}"))?;
        let (w, h) = mode.size();
        self.swapchain.resize(u32::from(w), u32::from(h));
        self.next = None;
        Ok(())
    }
}

/// How long a frame committed waits for its flip before it is taken as lost (the driver never answered):
/// the screen would otherwise never show another, with the loop still turning for the watchdog.
pub(crate) const FLIP_LOST: std::time::Duration = std::time::Duration::from_secs(1);

/// Buffer `slot`'s framebuffer, made once and kept with it.
fn framebuffer(surface: &DrmSurface, slot: &Slot<GbmBuffer>, opaque: bool) -> Result<(), String> {
    let fb = framebuffer_from_bo(surface.device_fd(), slot, opaque).map_err(|e| format!("its framebuffer: {e}"))?;
    slot.userdata().insert_if_missing_threadsafe(|| fb);
    Ok(())
}

/// A plane's state, its borrows taken as shorter, to go in one commit with the primary plane's.
fn narrowed<'a: 'b, 'b>(state: PlaneState<'a>) -> PlaneState<'b> {
    state
}

/// A buffer of the screen's mode shown whole: all of it, over all the screen.
fn whole(surface: &DrmSurface) -> (Rectangle<f64, smithay::utils::Buffer>, Rectangle<i32, smithay::utils::Physical>) {
    let (w, h) = surface.pending_mode().size();
    let (w, h) = (i32::from(w), i32::from(h));
    (Rectangle::from_size((w, h).into()).to_f64(), Rectangle::from_size((w, h).into()))
}

/// The primary plane showing `src` of framebuffer `fb` at `dst`, turned by `transform`, after `fence`,
/// with further `properties`.
fn primary<'a>(
    surface: &DrmSurface,
    fb: smithay::reexports::drm::control::framebuffer::Handle,
    src: Rectangle<f64, smithay::utils::Buffer>,
    dst: Rectangle<i32, smithay::utils::Physical>,
    transform: Transform,
    fence: Option<std::os::fd::BorrowedFd<'a>>,
    properties: &'a [crate::scanout::Property],
) -> PlaneState<'a> {
    PlaneState { handle: surface.plane(), config: Some(PlaneConfig { src, dst, transform, alpha: 1.0, damage_clips: None, fb, fence, properties }) }
}

/// Whether the screen's primary plane has property `name`.
fn plane_has(surface: &DrmSurface, name: &str) -> bool {
    use smithay::reexports::drm::control::Device;
    surface
        .get_properties(surface.plane())
        .is_ok_and(|props| props.iter().any(|(handle, _)| surface.get_property(*handle).is_ok_and(|info| info.name().to_str() == Ok(name))))
}

#[cfg(test)]
mod tests {
    use super::*;
    use rustix::io::Errno;

    /// A commit's error says what to do: a refusal isn't asked again, the rest is.
    #[test]
    fn commit_errors_are_sorted_by_what_to_do_about_them() {
        use smithay::backend::drm::DrmError;
        let crtc = smithay::reexports::drm::control::crtc::Handle::from(
            smithay::reexports::drm::control::RawResourceHandle::new(1).unwrap_or(std::num::NonZeroU32::MIN),
        );
        assert!(matches!(CommitError::of(DrmError::TestFailed(crtc)), CommitError::Refused(_)));
        assert_eq!(CommitError::of(DrmError::DeviceInactive), CommitError::Away);
        assert_eq!(CommitError::of(DrmError::DrmMasterFailed), CommitError::Away);
        let plane = smithay::reexports::drm::control::plane::Handle::from(
            smithay::reexports::drm::control::RawResourceHandle::new(2).unwrap_or(std::num::NonZeroU32::MIN),
        );
        assert!(matches!(CommitError::of(DrmError::UnsupportedPlaneConfiguration(plane)), CommitError::Refused(_)));
        assert!(matches!(CommitError::of(DrmError::PlaneNotCompatible(crtc, plane)), CommitError::Refused(_)));
        assert!(matches!(CommitError::of(DrmError::NoPlane), CommitError::Failed(_)));
        let why = || String::new();
        assert_eq!(CommitError::of_errno(Some(Errno::INVAL), why()), CommitError::Refused(why()));
        assert_eq!(CommitError::of_errno(Some(Errno::RANGE), why()), CommitError::Refused(why()));
        assert_eq!(CommitError::of_errno(Some(Errno::BUSY), why()), CommitError::Busy(why()));
        assert_eq!(CommitError::of_errno(Some(Errno::AGAIN), why()), CommitError::Busy(why()));
        assert_eq!(CommitError::of_errno(Some(Errno::INTR), why()), CommitError::Busy(why()));
        assert_eq!(CommitError::of_errno(Some(Errno::ACCESS), why()), CommitError::Away);
        assert_eq!(CommitError::of_errno(Some(Errno::PERM), why()), CommitError::Away);
        assert_eq!(CommitError::of_errno(Some(Errno::NOMEM), why()), CommitError::Failed(why()));
        assert_eq!(CommitError::of_errno(None, why()), CommitError::Failed(why()));
    }
}
