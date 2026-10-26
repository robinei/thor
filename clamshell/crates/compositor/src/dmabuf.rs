//! Linux dmabuf: clients' GPU buffers. All the renderer's formats are offered, the high-precision ones
//! among them, with the render device as feedback where it is known, and none on software EGL, which
//! accepts imports it can't sample. A client's dmabuf is imported with the renderer before the next
//! drawing (at most 64 waiting per client) and the client told whether it worked.

use std::os::unix::fs::MetadataExt;

use smithay::{
    backend::{
        allocator::{Format, dmabuf::Dmabuf},
        egl::EGLDevice,
        renderer::{ImportDma, gles::GlesRenderer},
    },
    wayland::dmabuf::{DmabufFeedbackBuilder, DmabufGlobal, DmabufHandler, DmabufState, ImportNotifier},
};

use crate::server::Server;

impl Server {
    pub fn advertise_dmabuf(&mut self, renderer: &GlesRenderer) {
        let formats = renderer.dmabuf_formats();
        if formats.iter().next().is_none() {
            log::debug!("compositor: renderer has no dmabuf formats");
            return;
        }
        let device = EGLDevice::device_for_display(renderer.egl_context().display());
        if device.as_ref().is_ok_and(EGLDevice::is_software) {
            // Software EGL exposes import formats even for native allocations it cannot sample.
            // Such an import may fail inside the driver after EGL has already accepted it.
            log::debug!("compositor: software EGL has no native dmabuf transport");
            return;
        }
        let device = device.and_then(|device| device.render_device_path()).ok().and_then(|path| std::fs::metadata(path).ok()).map(|metadata| metadata.rdev());
        self.advertise_dmabuf_formats(formats, device);
    }

    pub(super) fn advertise_dmabuf_formats(&mut self, formats: impl IntoIterator<Item = Format>, device: Option<u64>) {
        let formats: Vec<_> = formats.into_iter().collect();
        let feedback = device.and_then(|device| match DmabufFeedbackBuilder::new(device, formats.iter().copied()).build() {
            Ok(feedback) => Some(feedback),
            Err(error) => {
                log::warn!("compositor: cannot build dmabuf feedback: {error}");
                None
            }
        });
        if let Some(feedback) = feedback {
            self.protocols.dmabuf.create_global_with_default_feedback::<Self>(&self.dh, &feedback);
        } else {
            log::debug!("compositor: no render-device feedback, advertising dmabuf v3");
            self.protocols.dmabuf.create_global::<Self>(&self.dh, formats);
        }
    }

    /// Import before drawing. Returns whether any were waiting.
    pub fn import_dmabufs(&mut self, renderer: &mut GlesRenderer) -> bool {
        self.complete_dmabuf_imports(|buffer| renderer.import_dmabuf(buffer, None).map(|_| ()))
    }

    pub(super) fn complete_dmabuf_imports<E: std::fmt::Display>(&mut self, mut import: impl FnMut(&Dmabuf) -> Result<(), E>) -> bool {
        let pending = !self.pending_imports.is_empty();
        for (buffer, notifier) in self.pending_imports.drain(..) {
            match import(&buffer) {
                Ok(()) => {
                    if let Err(error) = notifier.successful::<Self>() {
                        log::debug!("compositor: dmabuf client left before import completed: {error}");
                    }
                }
                Err(error) => {
                    log::warn!("compositor: rejecting client dmabuf: {error}");
                    notifier.invalid_format();
                }
            }
        }
        pending
    }
}

impl DmabufHandler for Server {
    fn dmabuf_state(&mut self) -> &mut DmabufState {
        &mut self.protocols.dmabuf
    }

    fn dmabuf_imported(&mut self, _: &DmabufGlobal, buffer: Dmabuf, notifier: ImportNotifier) {
        // Bound each client's work per dispatch without penalising other clients.
        let client = notifier.client();
        if self.pending_imports.iter().filter(|(_, pending)| pending.client() == client).count() >= 64 {
            log::warn!("compositor: client exceeded pending dmabuf import limit");
            notifier.invalid_format();
            return;
        }
        self.pending_imports.push((buffer, notifier));
    }
}

impl Drop for Server {
    fn drop(&mut self) {
        for (_, notifier) in self.pending_imports.drain(..) {
            notifier.failed();
        }
    }
}
