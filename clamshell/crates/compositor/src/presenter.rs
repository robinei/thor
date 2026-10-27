//! What a screen shows, and its commits: the buffers ClamShell composes into, on the screen's primary
//! plane. One frame is in flight at a time: a buffer is taken ([`Presenter::next_buffer`]), drawn and
//! committed ([`Presenter::queue`]), and shown at the next flip ([`Presenter::flipped`]), when the buffer
//! shown before is free again. Smithay's `GbmBufferedSurface` did this; ClamShell's own owns every commit
//! of the screen, so that a frame scanned out (see `scanout`) is another kind of commit rather than a
//! second owner of the screen. A screen showing only ClamShell shows a buffer of its own instead (see
//! `ui_scanout`), the same way.

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
    /// The frame committed, until its flip.
    pending: Option<Shown>,
    /// The frame shown.
    current: Option<Shown>,
    /// Properties the next composed commit sets: the CRTC's colour cleared after ClamShell's own buffers.
    clear: Vec<crate::ui_scanout::Property>,
}

/// A frame committed: a buffer composed into, kept from reuse until it is off the screen, or one of
/// ClamShell's own buffers, which `ui_scanout` keeps.
enum Shown {
    Composed(#[allow(dead_code, reason = "held so that the swapchain doesn't reuse it while shown")] Slot<GbmBuffer>),
    Ui,
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
                Ok((swapchain, current, opaque)) => {
                    let fencing = !surface.is_legacy()
                        && smithay::reexports::drm::Device::get_driver_capability(&surface, smithay::reexports::drm::DriverCapability::SyncObj)
                            .is_ok_and(|v| v != 0)
                        && plane_has(&surface, "IN_FENCE_FD");
                    return Ok(Presenter {
                        surface,
                        swapchain,
                        opaque,
                        fencing,
                        next: None,
                        pending: None,
                        current: Some(Shown::Composed(current)),
                        clear: Vec::new(),
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
        let state = primary(surface, *fb.as_ref(), None, &[]);
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
    /// fence where it can, else after waiting here. It is shown at the next flip.
    pub(crate) fn queue(&mut self, sync: Option<SyncPoint>) -> Result<(), Error> {
        let slot = self.next.take().ok_or("no buffer drawn")?;
        self.swapchain.submitted(&slot);
        let fence = sync.and_then(|sync| {
            let fence = self.fencing.then(|| sync.export()).flatten();
            if fence.is_none() {
                // Without a fence to hand the commit, the drawing is waited on here; implicit sync covers
                // a wait that fails.
                let _ = sync.wait();
            }
            fence
        });
        let fb = *slot.userdata().get::<GbmFramebuffer>().ok_or("no framebuffer")?.as_ref();
        let state = primary(&self.surface, fb, fence.as_ref().map(std::os::fd::AsFd::as_fd), &self.clear);
        let committed = if self.surface.commit_pending() { self.surface.commit([state], true) } else { self.surface.page_flip([state], true) };
        committed.map_err(|e| format!("committing: {e}"))?;
        self.clear.clear();
        self.pending = Some(Shown::Composed(slot));
        Ok(())
    }

    /// Commits framebuffer `fb`, one of ClamShell's own buffers (see `ui_scanout`), with the CRTC's colour
    /// `properties`. It is shown at the next flip.
    pub(crate) fn show_ui(
        &mut self,
        fb: smithay::reexports::drm::control::framebuffer::Handle,
        properties: &[crate::ui_scanout::Property],
    ) -> Result<(), Error> {
        let state = primary(&self.surface, fb, None, properties);
        let committed = if self.surface.commit_pending() { self.surface.commit([state], true) } else { self.surface.page_flip([state], true) };
        committed.map_err(|e| format!("committing ClamShell's own buffer: {e}"))?;
        self.pending = Some(Shown::Ui);
        Ok(())
    }

    /// The next composed commit sets `properties` too: the CRTC's colour cleared after ClamShell's own
    /// buffers were shown with it.
    pub(crate) fn clear_after_ui(&mut self, properties: &[crate::ui_scanout::Property]) {
        self.clear.clear();
        self.clear.extend_from_slice(properties);
    }

    /// The frame committed is shown: the buffer shown before is free again. Whether there was one.
    pub(crate) fn flipped(&mut self) -> bool {
        match self.pending.take() {
            Some(pending) => {
                self.current = Some(pending);
                true
            }
            None => false,
        }
    }

    /// Drops the buffers' contents and any frame in flight, as after the seat was away or a flip was lost:
    /// the next frame is drawn whole.
    pub(crate) fn reset_buffers(&mut self) {
        self.swapchain.reset_buffers();
        self.next = None;
        self.pending = None;
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

/// Buffer `slot`'s framebuffer, made once and kept with it.
fn framebuffer(surface: &DrmSurface, slot: &Slot<GbmBuffer>, opaque: bool) -> Result<(), String> {
    let fb = framebuffer_from_bo(surface.device_fd(), slot, opaque).map_err(|e| format!("its framebuffer: {e}"))?;
    slot.userdata().insert_if_missing_threadsafe(|| fb);
    Ok(())
}

/// The primary plane showing framebuffer `fb` over the whole screen, after `fence`, with further
/// `properties`.
fn primary<'a>(
    surface: &DrmSurface,
    fb: smithay::reexports::drm::control::framebuffer::Handle,
    fence: Option<std::os::fd::BorrowedFd<'a>>,
    properties: &'a [crate::ui_scanout::Property],
) -> PlaneState<'a> {
    let (w, h) = surface.pending_mode().size();
    let (w, h) = (i32::from(w), i32::from(h));
    PlaneState {
        handle: surface.plane(),
        config: Some(PlaneConfig {
            src: Rectangle::from_size((w, h).into()).to_f64(),
            dst: Rectangle::from_size((w, h).into()),
            transform: Transform::Normal,
            alpha: 1.0,
            damage_clips: None,
            fb,
            fence,
            properties,
        }),
    }
}

/// Whether the screen's primary plane has property `name`.
fn plane_has(surface: &DrmSurface, name: &str) -> bool {
    use smithay::reexports::drm::control::Device;
    surface
        .get_properties(surface.plane())
        .is_ok_and(|props| props.iter().any(|(handle, _)| surface.get_property(*handle).is_ok_and(|info| info.name().to_str() == Ok(name))))
}
