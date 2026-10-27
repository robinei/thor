//! The cursor: the image the client under the pointer asked for (a surface of its own or a named cursor
//! from the XCursor theme), or the default arrow over ClamShell's own view, drawn into the scene, or copied
//! onto a cursor plane where a screen has one (see `plane_layers`). It shows while the pointer moves and
//! hides three seconds after, at a touch, and while a window holds the pointer locked, so a game played by
//! touch or the pad shows none.

use std::time::{Duration, Instant};

use clamshell_ui::Error;
use smithay::{
    backend::{
        allocator::Fourcc,
        renderer::{
            ImportMem, Renderer,
            element::{Id, Kind, texture::TextureRenderElement},
            gles::{GlesRenderer, GlesTexture},
            utils::import_surface_tree,
        },
    },
    input::pointer::{CursorIcon, CursorImageStatus, CursorImageSurfaceData},
    reexports::wayland_server::Resource,
    utils::{Logical, Point, Transform},
    wayland::compositor::with_states,
};
use xcursor::{
    CursorTheme,
    parser::{Image, parse_xcursor},
};

use crate::{render::Element, server::Server};

const HIDE_AFTER: Duration = Duration::from_secs(3);

#[derive(Default)]
pub(crate) struct Activity {
    last_motion: Option<Instant>,
}
impl Activity {
    pub fn motion(&mut self, now: Instant) {
        self.last_motion = Some(now);
    }
    pub fn hide(&mut self) {
        self.last_motion = None;
    }
    pub fn visible(&self, now: Instant) -> bool {
        self.last_motion.is_some_and(|last| now.saturating_duration_since(last) < HIDE_AFTER)
    }
    pub fn deadline(&self, now: Instant) -> Option<Instant> {
        self.last_motion.map(|last| last + HIDE_AFTER).filter(|deadline| *deadline > now)
    }
}

struct Frame {
    texture: GlesTexture,
    id: Id,
    hotspot: (f64, f64),
    delay: u32,
    /// The image as the CPU has it, premultiplied RGBA, for a cursor plane: its size and pixels.
    size: (u32, u32),
    pixels: Vec<u8>,
}

/// The cursor as a cursor plane shows it: which image (to know when to copy it again), its size, and where
/// its top left is in the scene.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Shape {
    image: ShapeImage,
    pub size: (u32, u32),
    pub at: Point<i32, Logical>,
    /// The pointer's place in the image.
    pub hotspot: (i32, i32),
}

impl Shape {
    /// The same, with its top left taken relative to `origin`.
    pub fn from(&self, origin: Point<i32, Logical>) -> Shape {
        Shape { at: self.at - origin, ..self.clone() }
    }

    /// Whether it is the same image as `other`, wherever each is.
    pub fn same_image(&self, other: &Shape) -> bool {
        self.image == other.image && self.size == other.size
    }
}

#[derive(Debug, Clone, PartialEq)]
enum ShapeImage {
    /// A frame of a named cursor.
    Named(CursorIcon, usize),
    /// A client's cursor surface, at a commit.
    Surface(smithay::reexports::wayland_server::backend::ObjectId, smithay::backend::renderer::utils::CommitCounter),
}

pub(crate) struct Cursor {
    theme: CursorTheme,
    size: u32,
    cached: Vec<(CursorIcon, Vec<Frame>)>,
}

impl Cursor {
    pub fn new() -> Self {
        Self {
            theme: CursorTheme::load(&std::env::var("XCURSOR_THEME").unwrap_or_else(|_| "default".into())),
            size: std::env::var("XCURSOR_SIZE").ok().and_then(|size| size.parse().ok()).unwrap_or(24).clamp(1, 256),
            cached: Vec::new(),
        }
    }

    /// The cursor's elements, with its hotspot `at` (where the scene places it, see `scene`), added to
    /// `out`, which is front first.
    pub fn elements(&mut self, server: &mut Server, renderer: &mut GlesRenderer, at: Point<f64, Logical>, out: &mut Vec<Element>) -> Result<(), Error> {
        let status = server.effective_cursor();
        match status {
            CursorImageStatus::Hidden => {}
            CursorImageStatus::Surface(surface) if surface.is_alive() => {
                if let Err(error) = import_surface_tree(renderer, &surface) {
                    server.reject_client(&surface, &format!("cannot import cursor buffer: {error}"));
                    server.pointer.cursor = CursorImageStatus::default_named();
                    return Ok(());
                }
                let hotspot = with_states(&surface, |states| {
                    states.data_map.get::<CursorImageSurfaceData>().and_then(|data| data.lock().ok()).map(|data| data.hotspot).unwrap_or_default()
                });
                let location = at - hotspot.to_f64();
                crate::surface::surface_tree_elements(renderer, &surface, location.to_physical(1.0).to_i32_round(), Kind::Cursor, |element| {
                    out.push(element.into());
                });
                let [top, bottom] = &server.screens.outputs;
                let output = if at.x < server.screens.top_width as f64 { top } else { bottom };
                smithay::desktop::utils::send_frames_surface_tree(&surface, output, server.started.elapsed(), Some(Duration::ZERO), |_, _| {
                    Some(output.clone())
                });
            }
            CursorImageStatus::Named(icon) => self.named(icon, server, renderer, at, out)?,
            CursorImageStatus::Surface(_) => {
                server.pointer.cursor = CursorImageStatus::default_named();
                self.named(CursorIcon::Default, server, renderer, at, out)?;
            }
        }
        Ok(())
    }

    fn named(&mut self, icon: CursorIcon, server: &Server, renderer: &mut GlesRenderer, at: Point<f64, Logical>, out: &mut Vec<Element>) -> Result<(), Error> {
        let index = self.loaded(icon, renderer)?;
        let Some((_, frames)) = self.cached.get(index) else { return Ok(()) };
        let frame = animation_frame(frames.iter().map(|frame| frame.delay), server.started.elapsed().as_millis());
        if let Some(frame) = frames.get(frame) {
            let location = at - Point::from(frame.hotspot);
            out.push(
                TextureRenderElement::from_static_texture(
                    frame.id.clone(),
                    renderer.context_id(),
                    location.to_physical(1.0),
                    frame.texture.clone(),
                    1,
                    Transform::Normal,
                    None,
                    None,
                    None,
                    None,
                    Kind::Cursor,
                )
                .into(),
            );
        }
        Ok(())
    }

    /// The cursor's shape with its hotspot `at`, for a cursor plane: none while it is hidden, or for a
    /// client's cursor the CPU can't copy (with surfaces under it, or a buffer not in shared memory), which
    /// is composed.
    pub fn shape(&mut self, server: &mut Server, renderer: &mut GlesRenderer, at: Point<f64, Logical>) -> Option<Shape> {
        match server.effective_cursor() {
            CursorImageStatus::Hidden => None,
            CursorImageStatus::Surface(surface) => {
                if !smithay::wayland::compositor::get_children(&surface).is_empty() {
                    return None;
                }
                let hotspot = with_states(&surface, |states| {
                    states.data_map.get::<CursorImageSurfaceData>().and_then(|data| data.lock().ok()).map(|data| data.hotspot).unwrap_or_default()
                });
                let (buffer, commit) =
                    smithay::backend::renderer::utils::with_renderer_surface_state(&surface, |state| (state.buffer().cloned(), state.current_commit()))?;
                let size = smithay::wayland::shm::with_buffer_contents(&*buffer?, |_, _, data| {
                    (u32::try_from(data.width).unwrap_or(0), u32::try_from(data.height).unwrap_or(0))
                })
                .ok()?;
                let at = (at - hotspot.to_f64()).to_i32_round();
                Some(Shape { image: ShapeImage::Surface(surface.id(), commit), size, at, hotspot: (hotspot.x, hotspot.y) })
            }
            CursorImageStatus::Named(icon) => {
                let index = self.loaded(icon, renderer).ok()?;
                let (_, frames) = self.cached.get(index)?;
                let n = animation_frame(frames.iter().map(|frame| frame.delay), server.started.elapsed().as_millis());
                let frame = frames.get(n)?;
                let at = (at - Point::from(frame.hotspot)).to_i32_round();
                let hotspot = (frame.hotspot.0 as i32, frame.hotspot.1 as i32);
                Some(Shape { image: ShapeImage::Named(icon, n), size: frame.size, at, hotspot })
            }
        }
    }

    /// Copies `shape`'s image to the top left of a square buffer `side` pixels a side, the rest clear:
    /// `bytes`, rows of `stride` pixels, turned a quarter if `quarter`, blue first if `bgra`. A client's
    /// cursor is sent its frame callbacks, as when it is drawn.
    #[allow(clippy::too_many_arguments)]
    pub fn copy(&self, server: &Server, shape: &Shape, bytes: &mut [u8], stride: usize, quarter: bool, bgra: bool, side: u32) {
        let put = |pixel: &dyn Fn(u32, u32) -> [u8; 4], bytes: &mut [u8]| {
            for y in 0..side {
                for x in 0..side {
                    let [r, g, b, a] = if x < shape.size.0 && y < shape.size.1 { pixel(x, y) } else { [0; 4] };
                    let (bx, by) = if quarter { (side - 1 - y, x) } else { (x, y) };
                    let i = (by as usize * stride + bx as usize) * 4;
                    if let Some(px) = bytes.get_mut(i..i + 4) {
                        px.copy_from_slice(&if bgra { [b, g, r, a] } else { [r, g, b, a] });
                    }
                }
            }
        };
        match &shape.image {
            ShapeImage::Named(icon, n) => {
                let Some(frame) = self.cached.iter().find(|(cached, _)| cached == icon).and_then(|(_, frames)| frames.get(*n)) else { return };
                let width = frame.size.0 as usize;
                put(
                    &|x, y| {
                        let i = (y as usize * width + x as usize) * 4;
                        frame.pixels.get(i..i + 4).and_then(|px| px.as_array().copied()).unwrap_or([0; 4])
                    },
                    bytes,
                );
            }
            ShapeImage::Surface(..) => {
                let CursorImageStatus::Surface(surface) = &server.pointer.cursor else { return };
                let buffer = smithay::backend::renderer::utils::with_renderer_surface_state(surface, |state| state.buffer().cloned()).flatten();
                let Some(buffer) = buffer else { return };
                let copied = smithay::wayland::shm::with_buffer_contents(&buffer, |pointer, len, data| {
                    use smithay::reexports::wayland_server::protocol::wl_shm::Format;
                    // SAFETY: Smithay maps the pool `len` bytes long for the duration of this call.
                    #[allow(unsafe_code)]
                    let pool = unsafe { std::slice::from_raw_parts(pointer, len) };
                    let (offset, stride) = (usize::try_from(data.offset).unwrap_or(0), usize::try_from(data.stride).unwrap_or(0));
                    let order = match data.format {
                        // Little-endian words: blue first in memory.
                        Format::Argb8888 | Format::Xrgb8888 => Some(true),
                        Format::Abgr8888 | Format::Xbgr8888 => Some(false),
                        _ => None,
                    };
                    let opaque = matches!(data.format, Format::Xrgb8888 | Format::Xbgr8888);
                    if let Some(blue_first) = order {
                        put(
                            &|x, y| {
                                let i = offset + y as usize * stride + x as usize * 4;
                                let Some(&[c0, c1, c2, c3]) = pool.get(i..i + 4).and_then(|px| px.as_array()) else { return [0; 4] };
                                let a = if opaque { 255 } else { c3 };
                                if blue_first { [c2, c1, c0, a] } else { [c0, c1, c2, a] }
                            },
                            bytes,
                        );
                    } else {
                        log::warn!("compositor: a cursor in {:?} on its plane: shown clear", data.format);
                        put(&|_, _| [0; 4], bytes);
                    }
                });
                if let Err(error) = copied {
                    log::warn!("compositor: copying a cursor onto its plane: {error}");
                }
                let [top, bottom] = &server.screens.outputs;
                let output = if f64::from(shape.at.x) < server.screens.top_width as f64 { top } else { bottom };
                smithay::desktop::utils::send_frames_surface_tree(surface, output, server.started.elapsed(), Some(Duration::ZERO), |_, _| Some(output.clone()));
            }
        }
    }

    /// Named cursor `icon`'s frames, loaded (and uploaded) the first time: their index in the cache.
    fn loaded(&mut self, icon: CursorIcon, renderer: &mut GlesRenderer) -> Result<usize, Error> {
        let index = if let Some(index) = self.cached.iter().position(|(cached, _)| *cached == icon) {
            index
        } else {
            let images = self
                .theme
                .load_icon(icon.name())
                .or_else(|| self.theme.load_icon("default"))
                .and_then(|path| std::fs::read(path).ok())
                .and_then(|bytes| parse_xcursor(&bytes))
                .filter(|images| !images.is_empty());
            let images = images.unwrap_or_else(|| {
                log::warn!("compositor: no usable cursor theme image for {}, using the built-in pointer", icon.name());
                vec![fallback()]
            });
            let nearest = images.iter().min_by_key(|image| image.size.abs_diff(self.size)).map(|image| image.size);
            let mut frames = Vec::new();
            for image in images.into_iter().filter(|image| Some(image.size) == nearest) {
                let size = (i32::try_from(image.width)?, i32::try_from(image.height)?).into();
                let texture = renderer.import_memory(&image.pixels_rgba, Fourcc::Abgr8888, size, false)?;
                let hotspot = (image.xhot as f64, image.yhot as f64);
                let size = (image.width, image.height);
                frames.push(Frame { id: Id::new(), texture, hotspot, delay: image.delay, size, pixels: image.pixels_rgba });
            }
            let index = self.cached.len();
            self.cached.push((icon, frames));
            index
        };
        Ok(index)
    }
}

fn animation_frame(delays: impl Iterator<Item = u32> + Clone, millis: u128) -> usize {
    let total: u128 = delays.clone().map(u128::from).sum();
    if total == 0 {
        return 0;
    }
    let mut elapsed = millis % total;
    for (index, delay) in delays.enumerate() {
        if elapsed < u128::from(delay) {
            return index;
        }
        elapsed -= u128::from(delay);
    }
    0
}

fn fallback() -> Image {
    let mut pixels = Vec::with_capacity(16 * 24 * 4);
    for y in 0..24 {
        for x in 0..16 {
            let inside = x <= y / 2 && y < 22;
            let edge = x == 0 || x == y / 2 || y == 21;
            let shade = if edge { 24 } else { 180 };
            let shade = if inside { shade } else { 0 };
            pixels.extend_from_slice(&[shade, shade, shade, if inside { 255 } else { 0 }]);
        }
    }
    Image { size: 24, width: 16, height: 24, xhot: 0, yhot: 0, delay: 0, pixels_rgba: pixels, pixels_argb: Vec::new() }
}

impl Server {
    pub(crate) fn effective_cursor(&self) -> CursorImageStatus {
        if self.pointer_locked() {
            return CursorImageStatus::Hidden;
        }
        let client = self.seat.get_pointer().is_some_and(|pointer| pointer.current_focus().is_some())
            && (self.hit(self.pointer.location).is_some() || self.pointer.window.is_some_and(|window| self.visible(window)));
        match &self.pointer.cursor {
            CursorImageStatus::Surface(surface) if !surface.is_alive() => CursorImageStatus::default_named(),
            image if client => image.clone(),
            _ => CursorImageStatus::default_named(),
        }
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    #[test]
    fn motion_shows_the_cursor_and_touch_or_three_seconds_without_motion_hides_it() {
        let start = Instant::now();
        let mut activity = Activity::default();
        assert!(!activity.visible(start));
        assert_eq!(activity.deadline(start), None);
        activity.motion(start);
        assert!(activity.visible((start + HIDE_AFTER).checked_sub(Duration::from_nanos(1)).unwrap()));
        assert_eq!(activity.deadline(start), Some(start + HIDE_AFTER));
        assert!(!activity.visible(start + HIDE_AFTER));
        assert_eq!(activity.deadline(start + HIDE_AFTER), None);
        activity.motion(start + HIDE_AFTER);
        activity.motion(start + HIDE_AFTER + Duration::from_secs(1));
        assert!(activity.visible(start + HIDE_AFTER * 2));
        activity.hide();
        assert!(!activity.visible(start + HIDE_AFTER * 2));
        assert_eq!(activity.deadline(start + HIDE_AFTER * 2), None);
        activity.motion(start + HIDE_AFTER * 2);
        assert!(activity.visible(start + HIDE_AFTER * 2));
    }

    #[test]
    fn cursor_animation_wraps_and_zero_delay_images_do_not_stall_it() {
        let frame = |time| animation_frame([0, 20, 30].into_iter(), time);
        assert_eq!(frame(0), 1);
        assert_eq!(frame(19), 1);
        assert_eq!(frame(20), 2);
        assert_eq!(frame(49), 2);
        assert_eq!(frame(50), 1);
        assert_eq!(animation_frame([0, 0].into_iter(), u128::MAX), 0);
        assert_eq!(animation_frame(std::iter::empty(), 10), 0);
        let image = fallback();
        assert_eq!(image.pixels_rgba.len(), (image.width * image.height * 4) as usize);
    }
}
