//! ClamShell's own screens scanned out directly, with CLAMSHELL_SCANOUT=1: while a screen shows nothing but
//! its UI (no window, overlay or cursor on it), the UI is drawn straight into a buffer the display engine
//! shows, turned to the panel's mounting (`clamshell_ui::Pixels`), and the GPU does nothing for it; with
//! both screens so, nothing at all.
//!
//! Each screen has two dumb buffers, CPU-mapped once and kept, drawn in turn: each catches up from the
//! frame it holds (`Core::draw`). They are XBGR8888, tiny-skia's red first, where the plane takes it, else
//! XRGB8888 (virtio-gpu takes nothing else), drawn blue first. The panel's colour, as the composed pass
//! encodes it, is the CRTC's while
//! the screen is scanned out: its degamma decoding sRGB, its colour matrix taking sRGB to the panel's
//! primaries, its gamma encoding for the panel; cleared when it composes again. A screen whose panel needs
//! that but whose CRTC can't do it is composed.

use std::os::fd::AsFd;
use std::ptr::NonNull;

use clamshell_state::colour::{ColourDescription, ColourPrimaries};
use clamshell_ui::{Core, Error, Layer, Pixels, Screen};
use smithay::backend::drm::DrmDeviceFd;
use smithay::reexports::drm::buffer::{DrmFourcc, DrmModifier, Handle as BufferHandle, PlanarBuffer};
use smithay::reexports::drm::control::{Device as ControlDevice, FbCmd2Flags, RawResourceHandle, crtc, framebuffer, property};

use crate::colour::math;

/// A property to set in a commit: of which object, which, to what.
pub(crate) type Property = (RawResourceHandle, property::Handle, property::RawValue);

pub(crate) struct UiScanout {
    buffers: [Dumb; 2],
    /// The buffer drawn next: not the one shown.
    next: usize,
    /// Whether the buffers hold the screen turned a quarter clockwise, as its panel is mounted.
    quarter: bool,
    /// Whether the buffers' pixels are blue first.
    bgra: bool,
    /// The screen layer's size in buffer pixels, the screen's way up.
    size: (u32, u32),
    colour: Colour,
}

impl UiScanout {
    /// The buffers for a screen whose CRTC is `crtc` at mode `mode` (the panel's way up), its primary
    /// plane taking `formats`, its UI layer `size` buffer pixels, turned a quarter clockwise if `quarter`,
    /// its panel described as `panel`; or why it can't be.
    pub(crate) fn new(
        device: &DrmDeviceFd,
        crtc: crtc::Handle,
        mode: (u16, u16),
        formats: &smithay::backend::allocator::format::FormatSet,
        size: (u32, u32),
        quarter: bool,
        panel: ColourDescription,
    ) -> Result<UiScanout, Error> {
        let turned = if quarter { (size.1, size.0) } else { size };
        let mode = (u32::from(mode.0), u32::from(mode.1));
        if turned != mode {
            return Err(format!("the UI, {turned:?} as the panel takes it, isn't the mode's {mode:?}").into());
        }
        let linear = |code| formats.iter().any(|f| f.code == code && matches!(f.modifier, DrmModifier::Linear | DrmModifier::Invalid));
        let (format, bgra) = if linear(DrmFourcc::Xbgr8888) {
            (DrmFourcc::Xbgr8888, false)
        } else if linear(DrmFourcc::Xrgb8888) {
            (DrmFourcc::Xrgb8888, true)
        } else {
            return Err("the plane takes neither XBGR8888 nor XRGB8888 linear".into());
        };
        let colour = Colour::new(device, crtc, panel)?;
        let buffers = [Dumb::new(device, mode, format)?, Dumb::new(device, mode, format)?];
        Ok(UiScanout { buffers, next: 0, quarter, bgra, size, colour })
    }

    /// Draws `screen`'s UI layer into the next buffer if that is behind, recording it first if it is stale.
    /// Returns the buffer's framebuffer to show, or none if it holds the latest frame already.
    pub(crate) fn draw(&mut self, core: &mut Core, screen: Screen) -> Option<framebuffer::Handle> {
        let layer = Layer::Screen(screen);
        if core.stale(layer) {
            core.record(layer, false);
        }
        let buffer = self.buffers.get_mut(self.next)?;
        if !core.behind(layer, buffer.held) {
            return None;
        }
        buffer.held = match (self.quarter, self.bgra) {
            (false, false) => draw_into::<false, false>(core, layer, buffer, self.size)?,
            (false, true) => draw_into::<false, true>(core, layer, buffer, self.size)?,
            (true, false) => draw_into::<true, false>(core, layer, buffer, self.size)?,
            (true, true) => draw_into::<true, true>(core, layer, buffer, self.size)?,
        };
        Some(buffer.fb)
    }

    /// Its buffers hold nothing any more: each is drawn whole next.
    pub(crate) fn reset(&mut self) {
        for buffer in &mut self.buffers {
            buffer.held = 0;
        }
    }

    /// The buffer drawn was committed: the other is drawn next.
    pub(crate) fn committed(&mut self) {
        self.next ^= 1;
    }

    /// The CRTC's colour properties while it is scanned out.
    pub(crate) fn colour(&self) -> &[Property] {
        &self.colour.set
    }

    /// Those properties cleared, for its first composed commit after.
    pub(crate) fn clear(&self) -> &[Property] {
        &self.colour.clear
    }
}

/// Draws `layer` into `buffer`, a screen of `size` laid out as `Q` and `B` say (see [`Pixels`]); returns the
/// frame it now holds.
fn draw_into<const Q: bool, const B: bool>(core: &mut Core, layer: Layer, buffer: &mut Dumb, size: (u32, u32)) -> Option<u64> {
    let (stride, held) = (buffer.pitch / 4, buffer.held);
    let pixels = Pixels::<Q, B>::new(buffer.bytes(), size, stride)?;
    Some(core.draw(layer, pixels, held).0)
}

/// A dumb buffer, mapped for the CPU, with its framebuffer and the UI frame it holds.
struct Dumb {
    device: DrmDeviceFd,
    handle: u32,
    fb: framebuffer::Handle,
    map: NonNull<u8>,
    len: usize,
    /// Bytes a row.
    pitch: usize,
    held: u64,
}

impl Dumb {
    #[allow(unsafe_code)]
    fn new(device: &DrmDeviceFd, (w, h): (u32, u32), format: DrmFourcc) -> Result<Dumb, Error> {
        let fd = device.as_fd();
        let made = drm_ffi::mode::dumbbuffer::create(fd, w, h, 32, 0).map_err(|e| format!("a dumb buffer: {e}"))?;
        let handle = made.handle;
        let destroy = || {
            let _ = drm_ffi::mode::dumbbuffer::destroy(fd, handle);
        };
        let len = usize::try_from(made.size).map_err(|e| format!("a dumb buffer's size: {e}"))?;
        let pitch = made.pitch as usize;
        let planar = Planar { size: (w, h), format, pitch: made.pitch, handle };
        let fb = match device.add_planar_framebuffer(&planar, FbCmd2Flags::empty()) {
            Ok(fb) => fb,
            Err(e) => {
                destroy();
                return Err(format!("a dumb buffer's framebuffer: {e}").into());
            }
        };
        let mapped = drm_ffi::mode::dumbbuffer::map(fd, handle, 0, 0).and_then(|info| {
            use rustix::mm::{MapFlags, ProtFlags, mmap};
            // SAFETY: a shared mapping of the dumb buffer, `len` bytes as the kernel made it, kept until
            // `Drop` unmaps it before the buffer is destroyed; nothing else maps it.
            unsafe { mmap(std::ptr::null_mut(), len, ProtFlags::READ | ProtFlags::WRITE, MapFlags::SHARED, fd, info.offset) }.map_err(std::io::Error::from)
        });
        let map = match mapped.ok().and_then(|p| NonNull::new(p.cast::<u8>())) {
            Some(map) => map,
            None => {
                let _ = device.destroy_framebuffer(fb);
                destroy();
                return Err("mapping a dumb buffer".into());
            }
        };
        let mut dumb = Dumb { device: device.clone(), handle, fb, map, len, pitch, held: 0 };
        // Black to begin with: the first frame draws all of it anyway.
        dumb.bytes().fill(0);
        Ok(dumb)
    }

    #[allow(unsafe_code)]
    fn bytes(&mut self) -> &mut [u8] {
        // SAFETY: the mapping is `len` bytes, valid until drop, and borrowed mutably through `self` alone.
        unsafe { std::slice::from_raw_parts_mut(self.map.as_ptr(), self.len) }
    }
}

impl Drop for Dumb {
    #[allow(unsafe_code)]
    fn drop(&mut self) {
        // SAFETY: the mapping made in `new`, unmapped once.
        if let Err(e) = unsafe { rustix::mm::munmap(self.map.as_ptr().cast(), self.len) } {
            log::warn!("ui scanout: unmapping a buffer: {e}");
        }
        if let Err(e) = self.device.destroy_framebuffer(self.fb) {
            log::warn!("ui scanout: a buffer's framebuffer: {e}");
        }
        if let Err(e) = drm_ffi::mode::dumbbuffer::destroy(self.device.as_fd(), self.handle) {
            log::warn!("ui scanout: a dumb buffer: {e}");
        }
    }
}

/// A dumb buffer as a framebuffer takes it.
struct Planar {
    size: (u32, u32),
    format: DrmFourcc,
    pitch: u32,
    handle: u32,
}

impl PlanarBuffer for Planar {
    fn size(&self) -> (u32, u32) {
        self.size
    }
    fn format(&self) -> DrmFourcc {
        self.format
    }
    fn modifier(&self) -> Option<DrmModifier> {
        None
    }
    fn pitches(&self) -> [u32; 4] {
        [self.pitch, 0, 0, 0]
    }
    fn handles(&self) -> [Option<BufferHandle>; 4] {
        [smithay::reexports::drm::control::from_u32(self.handle), None, None, None]
    }
    fn offsets(&self) -> [u32; 4] {
        [0; 4]
    }
}

/// The CRTC's colour stages for the panel: the properties to set, their blobs (destroyed with it), and the
/// properties cleared.
struct Colour {
    device: DrmDeviceFd,
    set: Vec<Property>,
    clear: Vec<Property>,
    blobs: Vec<u32>,
}

impl Colour {
    /// For a panel described as `panel`: none for sRGB's, the UI's own; else degamma, matrix and gamma, if
    /// the CRTC has them.
    fn new(device: &DrmDeviceFd, crtc: crtc::Handle, panel: ColourDescription) -> Result<Colour, Error> {
        let mut colour = Colour { device: device.clone(), set: Vec::new(), clear: Vec::new(), blobs: Vec::new() };
        if panel == ColourDescription::default() {
            return Ok(colour);
        }
        let props = crate::scanout::properties_of(device, u32::from(crtc), drm_ffi::DRM_MODE_OBJECT_CRTC);
        let find = |name: &str| props.iter().find(|(n, ..)| n == name).map(|(_, handle, value)| (*handle, *value));
        let (Some((degamma, _)), Some((_, degamma_size)), Some((ctm, _)), Some((gamma, _)), Some((_, gamma_size))) =
            (find("DEGAMMA_LUT"), find("DEGAMMA_LUT_SIZE"), find("CTM"), find("GAMMA_LUT"), find("GAMMA_LUT_SIZE"))
        else {
            return Err("the CRTC has no degamma, colour matrix and gamma for the panel's colour".into());
        };
        let srgb = ColourDescription::default();
        let table = |n: u64, f: &dyn Fn(f64) -> f64| -> Vec<u8> {
            (0..n.max(2))
                .flat_map(|i| {
                    let v = (f(i as f64 / (n.max(2) - 1) as f64).clamp(0.0, 1.0) * 65535.0).round() as u16;
                    [v, v, v, 0]
                })
                .flat_map(u16::to_ne_bytes)
                .collect()
        };
        let decode = table(degamma_size, &|x| math::decode(x, srgb).unwrap_or(x));
        let encode = table(gamma_size, &|x| math::encode(x, panel).unwrap_or(x));
        let matrix = math::matrix(ColourPrimaries::Srgb, panel.primaries).ok_or("no matrix to the panel's primaries")?;
        // S31.32 sign-magnitude, row by row.
        let matrix: Vec<u8> = matrix
            .iter()
            .flatten()
            .map(|v| {
                let magnitude = (v.abs() * 4_294_967_296.0).round() as u64 & !(1 << 63);
                if *v < 0.0 { magnitude | 1 << 63 } else { magnitude }
            })
            .flat_map(u64::to_ne_bytes)
            .collect();
        let object: RawResourceHandle = crtc.into();
        for (property, mut data) in [(degamma, decode), (ctm, matrix), (gamma, encode)] {
            let blob = drm_ffi::mode::create_property_blob(device.as_fd(), &mut data).map_err(|e| format!("a colour blob: {e}"))?.blob_id;
            colour.blobs.push(blob);
            colour.set.push((object, property, u64::from(blob)));
            colour.clear.push((object, property, 0));
        }
        Ok(colour)
    }
}

impl Drop for Colour {
    fn drop(&mut self) {
        for blob in &self.blobs {
            if let Err(e) = self.device.destroy_property_blob(u64::from(*blob)) {
                log::warn!("ui scanout: a colour blob: {e}");
            }
        }
    }
}
