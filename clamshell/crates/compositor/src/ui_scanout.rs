//! ClamShell's own screens scanned out directly, with CLAMSHELL_SCANOUT=1: while a screen shows nothing but
//! its UI (no window, overlay or cursor on it), the UI is shown from buffers the display engine scans out,
//! and the GPU does nothing for it; with both screens so, nothing at all.
//!
//! Each screen has two dumb buffers, CPU-mapped and drawn in turn ([`LayerBuffers`], which the overlay's
//! plane uses too, see `plane_layers`). The UI is painted once per change into a picture in ordinary memory
//! (`Core::draw`), and each buffer copies from it what it lacks ([`copy_area`]), turned to the panel's
//! mounting. They are XBGR8888, tiny-skia's red first, where the plane takes it, else XRGB8888 (virtio-gpu
//! takes nothing else), copied blue first. The panel's colour, as the composed pass encodes it, is the CRTC's
//! while the screen is scanned out: its degamma decoding sRGB, its colour matrix taking sRGB to the panel's
//! primaries, its gamma encoding for the panel; cleared when it composes again. A screen whose panel needs
//! that but whose CRTC can't do it is composed.

use std::os::fd::AsFd;
use std::ptr::NonNull;

use clamshell_state::colour::{ColourDescription, ColourPrimaries};
use clamshell_ui::{Bounds, Core, Error, Layer, Screen};
use smithay::backend::drm::DrmDeviceFd;
use smithay::reexports::drm::buffer::{DrmFourcc, DrmModifier, Handle as BufferHandle, PlanarBuffer};
use smithay::reexports::drm::control::{Device as ControlDevice, FbCmd2Flags, RawResourceHandle, crtc, framebuffer};

use crate::colour::math;
use crate::scanout::Blobs;

pub(crate) use crate::scanout::Property;

pub(crate) struct UiScanout {
    buffers: LayerBuffers,
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
        let colour = Colour::new(device, crtc, panel)?;
        let buffers = LayerBuffers::new(device, formats, size, quarter, OPAQUE)?;
        Ok(UiScanout { buffers, colour })
    }

    /// Draws `screen`'s UI layer into the next buffer if that is behind, recording it first if it is stale.
    /// Returns the buffer's framebuffer to show, or none if it holds the latest frame already.
    pub(crate) fn draw(&mut self, core: &mut Core, screen: Screen) -> Option<framebuffer::Handle> {
        self.buffers.draw(core, Layer::Screen(screen))
    }

    /// Its buffers hold nothing any more: each is drawn whole next.
    pub(crate) fn reset(&mut self) {
        self.buffers.reset();
    }

    /// The commit showing the buffer drawn failed: it holds a frame nobody saw, and is drawn whole next.
    pub(crate) fn failed(&mut self) {
        self.buffers.failed();
    }

    /// The buffer drawn was committed: the other is drawn next.
    pub(crate) fn committed(&mut self) {
        self.buffers.committed();
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

/// The formats of an opaque picture, the first the plane takes linear: tiny-skia's red first, else blue
/// first (virtio-gpu takes nothing else).
pub(crate) const OPAQUE: &[(DrmFourcc, bool)] = &[(DrmFourcc::Xbgr8888, false), (DrmFourcc::Xrgb8888, true)];
/// The same with alpha, premultiplied as the painter draws, for a plane over another.
pub(crate) const BLENDED: &[(DrmFourcc, bool)] = &[(DrmFourcc::Abgr8888, false), (DrmFourcc::Argb8888, true)];

/// Two dumb buffers a picture of a screen's way up is shown from in turn, turned to the panel's mounting: a
/// UI layer ([`LayerBuffers::draw`]), or anything else written in ([`LayerBuffers::write`]). CPU-mapped once
/// and kept. A UI layer is painted once per change into a picture in ordinary memory, unturned, and each
/// buffer catches up by copying what it lacks of it: the painter reads what it blends over, which a display
/// buffer's memory may be slow at (write-combined, as on many display engines), and a change is painted
/// once rather than once for each buffer.
pub(crate) struct LayerBuffers {
    buffers: [Dumb; 2],
    /// The layer painted, the screen's way up, and the frame it holds.
    picture: tiny_skia::Pixmap,
    picture_held: u64,
    /// The buffer drawn next: not the one shown.
    next: usize,
    /// Whether the buffers hold the picture turned a quarter clockwise, as its panel is mounted.
    quarter: bool,
    /// Whether the buffers' pixels are blue first.
    bgra: bool,
    /// The picture's size in buffer pixels, the screen's way up.
    size: (u32, u32),
}

/// Whether a plane taking `formats` takes one of `choices` linear, as [`LayerBuffers::new`] needs.
pub(crate) fn takes(formats: &smithay::backend::allocator::format::FormatSet, choices: &[(DrmFourcc, bool)]) -> bool {
    choices.iter().any(|(code, _)| formats.iter().any(|f| f.code == *code && matches!(f.modifier, DrmModifier::Linear | DrmModifier::Invalid)))
}

impl LayerBuffers {
    /// Buffers for a picture of `size` (the screen's way up), turned a quarter clockwise if `quarter`, in the
    /// first of `choices` (a format, and whether it is blue first) the plane, taking `formats`, takes linear;
    /// or why not.
    pub(crate) fn new(
        device: &DrmDeviceFd,
        formats: &smithay::backend::allocator::format::FormatSet,
        size: (u32, u32),
        quarter: bool,
        choices: &[(DrmFourcc, bool)],
    ) -> Result<LayerBuffers, Error> {
        let linear = |code| formats.iter().any(|f| f.code == code && matches!(f.modifier, DrmModifier::Linear | DrmModifier::Invalid));
        let Some(&(format, bgra)) = choices.iter().find(|(code, _)| linear(*code)) else {
            return Err(format!("the plane takes none of {:?} linear", choices.iter().map(|(code, _)| *code).collect::<Vec<_>>()).into());
        };
        let turned = if quarter { (size.1, size.0) } else { size };
        let buffers = [Dumb::new(device, turned, format)?, Dumb::new(device, turned, format)?];
        let picture = tiny_skia::Pixmap::new(size.0, size.1).ok_or("a picture of no size")?;
        Ok(LayerBuffers { buffers, picture, picture_held: 0, next: 0, quarter, bgra, size })
    }

    /// Brings the next buffer up to `layer`'s latest frame if it is behind, recording the layer first if
    /// it is stale and painting the picture if that is behind. Returns the buffer's framebuffer to show, or
    /// none if it holds the latest frame already.
    pub(crate) fn draw(&mut self, core: &mut Core, layer: Layer) -> Option<framebuffer::Handle> {
        if core.stale(layer) {
            core.record(layer, false);
        }
        let buffer = self.buffers.get_mut(self.next)?;
        if !core.behind(layer, buffer.held) {
            return None;
        }
        if core.behind(layer, self.picture_held) {
            self.picture_held = core.draw(layer, &mut self.picture, self.picture_held).0;
        }
        let area = core.since(layer, buffer.held);
        let (picture, size, stride) = (self.picture.data(), self.size, buffer.pitch / 4);
        let copied = match (self.quarter, self.bgra) {
            (false, false) => copy_area::<false, false>(buffer.bytes(), stride, size, picture, area),
            (false, true) => copy_area::<false, true>(buffer.bytes(), stride, size, picture, area),
            (true, false) => copy_area::<true, false>(buffer.bytes(), stride, size, picture, area),
            (true, true) => copy_area::<true, true>(buffer.bytes(), stride, size, picture, area),
        };
        if !copied {
            log::warn!("ui scanout: a {size:?} picture doesn't fit its buffer ({stride} pixels a row); the screen is not updated");
            return None;
        }
        buffer.held = self.picture_held;
        Some(buffer.fb)
    }

    /// Whether the next buffer is behind `layer`'s latest frame (or the layer is stale), so that drawing it
    /// would change it.
    pub(crate) fn behind(&self, core: &Core, layer: Layer) -> bool {
        core.stale(layer) || self.buffers.get(self.next).is_some_and(|buffer| core.behind(layer, buffer.held))
    }

    /// Has `write` fill the next buffer: given its bytes, its row length in pixels, whether it is turned a
    /// quarter and whether its pixels are blue first. Returns its framebuffer.
    pub(crate) fn write(&mut self, write: impl FnOnce(&mut [u8], usize, bool, bool)) -> Option<framebuffer::Handle> {
        let (quarter, bgra) = (self.quarter, self.bgra);
        let buffer = self.buffers.get_mut(self.next)?;
        // Not a UI frame: drawn whole if it is ever drawn by a painter.
        buffer.held = 0;
        let stride = buffer.pitch / 4;
        write(buffer.bytes(), stride, quarter, bgra);
        Some(buffer.fb)
    }

    /// Its buffers hold nothing any more: each is drawn whole next.
    pub(crate) fn reset(&mut self) {
        for buffer in &mut self.buffers {
            buffer.held = 0;
        }
    }

    /// The commit showing the buffer drawn failed: it holds a frame nobody saw, and is drawn whole next.
    pub(crate) fn failed(&mut self) {
        if let Some(buffer) = self.buffers.get_mut(self.next) {
            buffer.held = 0;
        }
    }

    /// The buffer drawn was committed: the other is drawn next.
    pub(crate) fn committed(&mut self) {
        self.next ^= 1;
    }
}

/// Copies `area` of `picture`, a screen of `size` the screen's way up and red first, into `buffer`, a display
/// buffer of rows `stride` pixels long. With `QUARTER` the buffer is the screen turned a quarter clockwise
/// (a panel mounted so: the screen's top row is its right column, top to bottom), with `BGRA` its pixels
/// are blue first. Both are types, so that each layout's loop has no branch for the others; turned, it goes
/// along the buffer's rows, so that it writes in order (a display buffer's memory may be fast only so).
/// Returns false, copying nothing, if either is too short for `size`.
fn copy_area<const QUARTER: bool, const BGRA: bool>(buffer: &mut [u8], stride: usize, (w, h): (u32, u32), picture: &[u8], area: Bounds) -> bool {
    let (columns, rows) = if QUARTER { (h, w) } else { (w, h) };
    let (w, h) = (w as usize, h as usize);
    if stride < columns as usize || buffer.len() < rows as usize * stride * 4 || picture.len() < w * h * 4 {
        return false;
    }
    let area = area.intersect(Bounds::of_size((w as u32, h as u32)));
    let (x0, y0, x1, y1) = (area.x0.max(0) as usize, area.y0.max(0) as usize, area.x1.max(0) as usize, area.y1.max(0) as usize);
    let (from, to) = (picture.as_chunks::<4>().0, buffer.as_chunks_mut::<4>().0);
    let pixel = |[r, g, b, a]: [u8; 4]| if BGRA { [b, g, r, a] } else { [r, g, b, a] };
    if !QUARTER {
        for y in y0..y1 {
            let (Some(row), Some(src)) = (to.get_mut(y * stride + x0..y * stride + x1), from.get(y * w + x0..y * w + x1)) else { continue };
            if BGRA {
                row.iter_mut().zip(src).for_each(|(d, s)| *d = pixel(*s));
            } else {
                row.copy_from_slice(src);
            }
        }
        return true;
    }
    // Picture pixel (x, y) is buffer pixel (h - 1 - y, x): the screen's column x is buffer row x, bottom up.
    for x in x0..x1 {
        let Some(row) = to.get_mut(x * stride + h - y1..x * stride + h - y0) else { continue };
        for (d, y) in row.iter_mut().zip((y0..y1).rev()) {
            if let Some(s) = from.get(y * w + x) {
                *d = pixel(*s);
            }
        }
    }
    true
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

/// The CRTC's colour stages for the panel: the properties to set, their blobs, and the properties cleared.
struct Colour {
    set: Vec<Property>,
    clear: Vec<Property>,
    _blobs: Blobs,
}

impl Colour {
    /// For a panel described as `panel`: none for sRGB's, the UI's own; else degamma, matrix and gamma, if
    /// the CRTC has them.
    fn new(device: &DrmDeviceFd, crtc: crtc::Handle, panel: ColourDescription) -> Result<Colour, Error> {
        let mut colour = Colour { set: Vec::new(), clear: Vec::new(), _blobs: Blobs::new(device) };
        if panel.matches_output(ColourDescription::default()) {
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
        for (property, data) in [(degamma, decode), (ctm, matrix), (gamma, encode)] {
            let blob = colour._blobs.create(data)?;
            colour.set.push((object, property, u64::from(blob)));
            colour.clear.push((object, property, 0));
        }
        Ok(colour)
    }
}

#[cfg(test)]
#[allow(clippy::indexing_slicing)]
mod tests {
    use super::*;

    /// A picture whose every pixel says where it is: x's and y's low bytes, their high bits, and 255.
    fn picture((w, h): (u32, u32)) -> Vec<u8> {
        (0..h).flat_map(|y| (0..w).flat_map(move |x| [x as u8, y as u8, (x >> 8 << 4 | y >> 8) as u8, 255])).collect()
    }

    /// Copied into each layout of display buffer, with padded rows, a part of a picture lands where that
    /// layout puts each of its pixels and nothing else changes: the whole picture, a part inside, parts at
    /// each edge, and one partly outside the screen.
    #[test]
    fn a_picture_copied_into_a_display_buffer_is_laid_out_as_the_buffer_is() {
        let (w, h) = (300u32, 270u32);
        let source = picture((w, h));
        let areas = [
            Bounds::of_size((w, h)),
            Bounds::new(20, 30, 150, 90),
            Bounds::new(0, 0, w as i32, 1),
            Bounds::new(0, h as i32 - 1, w as i32, h as i32),
            Bounds::new(w as i32 - 1, 0, w as i32, h as i32),
            Bounds::new(-5, 200, 400, 300),
            Bounds::EMPTY,
        ];
        fn check<const Q: bool, const B: bool>(source: &[u8], (w, h): (u32, u32), area: Bounds) {
            let stride = w.max(h) as usize + 3;
            let rows = if Q { w } else { h } as usize;
            let mut buffer = vec![9u8; stride * rows * 4];
            assert!(copy_area::<Q, B>(&mut buffer, stride, (w, h), source, area));
            let inside = area.intersect(Bounds::of_size((w, h)));
            let mut copied = 0;
            for (i, px) in buffer.chunks_exact(4).enumerate() {
                let (row, column) = (i / stride, i % stride);
                // The pixel of the picture there, none in a row's padding.
                let at = if Q { (h as usize).checked_sub(column + 1).map(|y| (row, y)) } else { (column < w as usize).then_some((column, row)) };
                let shown = at.filter(|&(x, y)| (x as i32) >= inside.x0 && (x as i32) < inside.x1 && (y as i32) >= inside.y0 && (y as i32) < inside.y1);
                let want = if let Some((x, y)) = shown {
                    copied += 1;
                    let j = (y * w as usize + x) * 4;
                    let [r, g, b, a] = [source[j], source[j + 1], source[j + 2], source[j + 3]];
                    if B { [b, g, r, a] } else { [r, g, b, a] }
                } else {
                    [9; 4]
                };
                assert_eq!(px, want, "quarter {Q} bgra {B} area {area:?} at buffer ({column}, {row})");
            }
            assert_eq!(copied, inside.width().max(0) as usize * inside.height().max(0) as usize);
        }
        for area in areas {
            check::<false, false>(&source, (w, h), area);
            check::<false, true>(&source, (w, h), area);
            check::<true, false>(&source, (w, h), area);
            check::<true, true>(&source, (w, h), area);
        }
    }

    /// A buffer or picture too short for the screen is refused, nothing written.
    #[test]
    fn a_buffer_too_short_for_the_screen_takes_nothing() {
        let (w, h) = (30u32, 20u32);
        let source = picture((w, h));
        let mut buffer = vec![9u8; 30 * 20 * 4];
        let all = Bounds::of_size((w, h));
        assert!(!copy_area::<false, false>(&mut buffer, 29, (w, h), &source, all), "rows too short");
        assert!(!copy_area::<true, false>(&mut buffer, 19, (w, h), &source, all), "turned, rows too short");
        assert!(!copy_area::<false, false>(&mut buffer[4..], 30, (w, h), &source, all), "too few rows");
        assert!(!copy_area::<false, false>(&mut buffer, 30, (w, h), &source[4..], all), "the picture too short");
        assert!(buffer.iter().all(|&b| b == 9));
        assert!(copy_area::<true, false>(&mut buffer, 20, (w, h), &source, all), "turned, it fits");
    }
}
