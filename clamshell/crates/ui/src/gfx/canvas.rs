//! The canvas over a pixmap: shapes and images through tiny-skia, icons and text (see [`super::text`]) as
//! cached coverage masks. All coordinates are logical pixels; the canvas scales them to the buffer.
//!
//! No heap allocation per frame once warm (a test counts): glyphs and icons are rasterized once and
//! blended straight into the pixmap as coverage masks; rounded rectangles and circles are computed per
//! pixel from their distance function, with no path at all.

use std::collections::HashMap;
use std::sync::mpsc::Sender;

use tiny_skia::{Color, LineCap, LineJoin, Paint, Path, PathBuilder, Pixmap, PixmapMut, PremultipliedColorU8, Stroke, Transform};

use super::fonts::{Face, Found, Glyph, NOTO_SANS, find_font};
use super::geom::Rect;
use super::icons::Icon;

/// What drawing keeps between frames: the font, fonts found for characters it lacks, the rasterized
/// glyphs (by character and pixel size), and a path builder whose memory is reused for every shape.
pub struct DrawCache {
    pub(super) font: Option<fontdue::Font>,
    /// Fonts for characters the built-in one lacks (a Japanese title, say), found through fontconfig
    /// when first needed; and for each such character, which of them has it (none: no font does).
    pub(super) fallbacks: Vec<(String, fontdue::Font)>,
    pub(super) fallback_for: HashMap<char, Face>,
    /// Where characters to find a font for go (a font is a few MB to read and parse); `None` finds it
    /// at once (snapshots, tests).
    pub(super) finder: Option<Sender<char>>,
    pub(super) glyphs: HashMap<(char, u32), Glyph>,
    /// Icon coverage masks, by icon and side in pixels.
    pub(super) icons: HashMap<(Icon, u32), Vec<u8>>,
    pub(super) path: Option<PathBuilder>,
}

impl DrawCache {
    pub fn load() -> DrawCache {
        let font = fontdue::Font::from_bytes(NOTO_SANS, fontdue::FontSettings::default()).map_err(|e| log::error!("the built-in font: {e}")).ok();
        DrawCache { font, fallbacks: Vec::new(), fallback_for: HashMap::new(), finder: None, glyphs: HashMap::new(), icons: HashMap::new(), path: None }
    }

    /// As [`DrawCache::load`], fonts for characters the built-in one lacks found by `finder` (see
    /// [`super::fonts::spawn_font_finder`]) and handed back through [`DrawCache::found`]; such a character isn't drawn
    /// until then.
    pub fn with_finder(finder: Sender<char>) -> DrawCache {
        DrawCache { finder: Some(finder), ..DrawCache::load() }
    }

    pub(super) fn glyph(&mut self, ch: char, px: f32) -> Option<&Glyph> {
        let key = (ch, px.to_bits());
        if !self.glyphs.contains_key(&key) {
            let font = match self.face(ch) {
                Face::Fallback(i) => self.fallbacks.get(i).map(|(_, f)| f),
                Face::Builtin => self.font.as_ref(),
                Face::Waiting => None,
            }?;
            let glyph = font.rasterize(ch, px);
            self.glyphs.insert(key, glyph);
        }
        self.glyphs.get(&key)
    }

    /// The font to draw `ch` with: the built-in one, unless it lacks `ch` and another has it.
    pub(super) fn face(&mut self, ch: char) -> Face {
        if ch.is_control() || self.font.as_ref().is_none_or(|f| f.lookup_glyph_index(ch) != 0) {
            return Face::Builtin;
        }
        if let Some(&known) = self.fallback_for.get(&ch) {
            return known;
        }
        let face = match self.fallbacks.iter().position(|(_, f)| f.lookup_glyph_index(ch) != 0) {
            Some(i) => Face::Fallback(i),
            None => match &self.finder {
                Some(finder) if finder.send(ch).is_ok() => Face::Waiting,
                Some(_) => Face::Builtin,
                None => {
                    let found = find_font(ch, &mut |path| self.fallbacks.iter().any(|(p, _)| p == path));
                    return self.found(found);
                }
            },
        };
        self.fallback_for.insert(ch, face);
        face
    }

    /// What the finder found for a character: its font now used (and the character drawn), or the
    /// built-in one's mark for a missing character if no font has it.
    pub fn found(&mut self, found: Found) -> Face {
        let Found { ch, path, font } = found;
        if let (Some(path), Some(font)) = (path.clone(), font) {
            log::info!("fallback font for {ch:?}: {path}");
            self.fallbacks.push((path, font));
        }
        let face =
            path.and_then(|path| self.fallbacks.iter().position(|(p, f)| *p == path && f.lookup_glyph_index(ch) != 0)).map_or(Face::Builtin, Face::Fallback);
        if face == Face::Builtin {
            log::debug!("no font has {ch:?}");
        }
        self.fallback_for.insert(ch, face);
        face
    }

    pub(super) fn ascent(&self, px: f32) -> f32 {
        self.font.as_ref().and_then(|f| f.horizontal_line_metrics(px)).map_or(px * 0.8, |m| m.ascent)
    }

    /// An empty path builder, reusing the last path's memory.
    fn builder(&mut self) -> PathBuilder {
        self.path.take().unwrap_or_default()
    }

    /// Gives a finished path's memory back for the next shape.
    fn recycle(&mut self, path: Path) {
        self.path = Some(path.clear());
    }
}

pub struct Canvas<'a> {
    pub(super) pixmap: PixmapMut<'a>,
    pub(super) scale: f32,
    /// Whole buffer pixels everything is moved by (burn-in orbit).
    pub(super) offset: (f32, f32),
    /// Where drawing reaches, in buffer pixels (x0, y0, x1, y1): the pixmap, or a scrolled list's area.
    pub(super) clip: (i32, i32, i32, i32),
    pub(super) cache: &'a mut DrawCache,
}

impl<'a> Canvas<'a> {
    pub fn new(pixmap: PixmapMut<'a>, scale: f32, cache: &'a mut DrawCache) -> Self {
        let clip = (0, 0, pixmap.width() as i32, pixmap.height() as i32);
        Canvas { pixmap, scale, offset: (0.0, 0.0), clip, cache }
    }

    /// Drawing kept within `r` from now on (`None`: the whole pixmap again).
    pub fn clip(&mut self, r: Option<Rect>) {
        let (pw, ph) = (self.pixmap.width() as i32, self.pixmap.height() as i32);
        self.clip = match r {
            Some(r) => (
                (self.px(r.x).floor() as i32).clamp(0, pw),
                (self.py(r.y).floor() as i32).clamp(0, ph),
                (self.px(r.x + r.w).ceil() as i32).clamp(0, pw),
                (self.py(r.y + r.h).ceil() as i32).clamp(0, ph),
            ),
            None => (0, 0, pw, ph),
        };
    }

    /// Buffer pixels per logical pixel.
    pub fn scale(&self) -> f32 {
        self.scale
    }

    /// Moves everything drawn by whole buffer pixels.
    pub fn with_offset(mut self, (dx, dy): (i32, i32)) -> Self {
        self.offset = (dx as f32, dy as f32);
        self
    }

    /// Logical x and y to buffer pixels.
    pub(super) fn px(&self, x: f32) -> f32 {
        x * self.scale + self.offset.0
    }

    pub(super) fn py(&self, y: f32) -> f32 {
        y * self.scale + self.offset.1
    }

    fn paint(color: Color) -> Paint<'static> {
        let mut paint = Paint::default();
        paint.set_color(color);
        paint.anti_alias = true;
        paint
    }

    /// A vertical line `px` buffer pixels wide, centred on `x` and snapped to whole pixels (sharp).
    pub fn vline(&mut self, x: f32, y0: f32, y1: f32, px: u32, color: impl Into<Color>) {
        let left = (self.px(x) - px as f32 / 2.0).round();
        let (top, bottom) = (self.py(y0).round(), self.py(y1).round());
        if let Some(rect) = self.clipped(left, top, left + px as f32, bottom) {
            self.pixmap.fill_rect(rect, &Self::paint(color.into()), Transform::identity(), None);
        }
    }

    /// A horizontal line `px` buffer pixels high, centred on `y` and snapped to whole pixels.
    pub fn hline(&mut self, x0: f32, x1: f32, y: f32, px: u32, color: impl Into<Color>) {
        let top = (self.py(y) - px as f32 / 2.0).round();
        let (left, right) = (self.px(x0).round(), self.px(x1).round());
        if let Some(rect) = self.clipped(left, top, right, top + px as f32) {
            self.pixmap.fill_rect(rect, &Self::paint(color.into()), Transform::identity(), None);
        }
    }

    pub fn fill(&mut self, r: Rect, color: impl Into<Color>) {
        // In buffer pixels, filled without a transform: tiny-skia fills a transformed rect as a path,
        // which allocates.
        let (x0, y0) = (self.px(r.x), self.py(r.y));
        if let Some(rect) = self.clipped(x0, y0, x0 + r.w * self.scale, y0 + r.h * self.scale) {
            self.pixmap.fill_rect(rect, &Self::paint(color.into()), Transform::identity(), None);
        }
    }

    /// A rectangle in buffer pixels cut to the clip; `None` if nothing is left.
    fn clipped(&self, x0: f32, y0: f32, x1: f32, y1: f32) -> Option<tiny_skia::Rect> {
        let (cx0, cy0, cx1, cy1) = self.clip;
        tiny_skia::Rect::from_ltrb(x0.max(cx0 as f32), y0.max(cy0 as f32), x1.min(cx1 as f32), y1.min(cy1 as f32))
    }

    fn stroke_path(&mut self, path: Path, stroke: &Stroke, color: Color) {
        self.pixmap.stroke_path(&path, &Self::paint(color), stroke, Transform::identity(), None);
        self.cache.recycle(path);
    }

    /// A rounded rectangle, filled.
    pub fn rounded(&mut self, r: Rect, radius: f32, color: impl Into<Color>) {
        let shape = RoundedRect::new(r, radius, self.scale, self.offset);
        self.shade(shape.bounds(1.0), color.into(), Runs::One, |x, y| (0.5 - shape.distance(x, y)).clamp(0.0, 1.0));
    }

    /// A rounded rectangle's outline, `width` wide, centred on its edge.
    pub fn outline(&mut self, r: Rect, radius: f32, width: f32, color: impl Into<Color>) {
        let shape = RoundedRect::new(r, radius, self.scale, self.offset);
        let half = width * self.scale / 2.0;
        self.shade(shape.bounds(half + 1.0), color.into(), Runs::Any, |x, y| (half + 0.5 - shape.distance(x, y).abs()).clamp(0.0, 1.0));
    }

    pub fn circle(&mut self, cx: f32, cy: f32, r: f32, color: impl Into<Color>) {
        let (cx, cy, r) = (self.px(cx), self.py(cy), r * self.scale);
        let bounds = (cx - r - 1.0, cy - r - 1.0, cx + r + 1.0, cy + r + 1.0);
        self.shade(bounds, color.into(), Runs::One, |x, y| (0.5 - ((x - cx).hypot(y - cy) - r)).clamp(0.0, 1.0));
    }

    /// Blends `color` over the pixels in `bounds` (x0, y0, x1, y1 in buffer pixels) with the coverage
    /// `coverage` gives for each pixel centre. With [`Runs::One`], a row is asked from either end only
    /// up to its first fully covered pixel; the pixels between are filled.
    fn shade(&mut self, (x0, y0, x1, y1): (f32, f32, f32, f32), color: Color, runs: Runs, coverage: impl Fn(f32, f32) -> f32) {
        let pw = self.pixmap.width() as i32;
        let (cx0, cy0, cx1, cy1) = self.clip;
        let (x0, y0) = ((x0.floor() as i32).max(cx0), (y0.floor() as i32).max(cy0));
        let (x1, y1) = ((x1.ceil() as i32).min(cx1), (y1.ceil() as i32).min(cy1));
        let src = premultiplied(color);
        let data = self.pixmap.data_mut();
        let k_at = |x: i32, y: i32| (coverage(x as f32 + 0.5, y as f32 + 0.5) * 255.0) as u32;
        let at = |x: i32, y: i32| ((y * pw + x) * 4) as usize;
        for y in y0..y1 {
            let (mut from, mut to) = (x0, x1);
            if runs == Runs::One {
                while from < to
                    && let k = k_at(from, y)
                    && k < 255
                {
                    blend_at(data, at(from, y), src, k);
                    from += 1;
                }
                while to > from
                    && let k = k_at(to - 1, y)
                    && k < 255
                {
                    blend_at(data, at(to - 1, y), src, k);
                    to -= 1;
                }
                if src[3] == 255 {
                    let opaque = src.map(|c| c as u8);
                    for px in data.get_mut(at(from, y)..at(to, y)).into_iter().flat_map(|run| run.chunks_exact_mut(4)) {
                        px.copy_from_slice(&opaque);
                    }
                    continue;
                }
            }
            for x in from..to {
                blend_at(data, at(x, y), src, k_at(x, y));
            }
        }
    }

    /// A straight line from (x0, y0) to (x1, y1), `width` wide with round ends; any angle, antialiased,
    /// computed per pixel (no path).
    pub fn line(&mut self, (x0, y0): (f32, f32), (x1, y1): (f32, f32), width: f32, color: impl Into<Color>) {
        let (ax, ay, bx, by) = (self.px(x0), self.py(y0), self.px(x1), self.py(y1));
        let half = width * self.scale / 2.0;
        let (dx, dy) = (bx - ax, by - ay);
        let len2 = (dx * dx + dy * dy).max(1e-6);
        let bounds = (ax.min(bx) - half - 1.0, ay.min(by) - half - 1.0, ax.max(bx) + half + 1.0, ay.max(by) + half + 1.0);
        self.shade(bounds, color.into(), Runs::One, |x, y| {
            let t = (((x - ax) * dx + (y - ay) * dy) / len2).clamp(0.0, 1.0);
            let d = (x - ax - t * dx).hypot(y - ay - t * dy);
            (half + 0.5 - d).clamp(0.0, 1.0)
        });
    }

    /// Connected lines through `points`, round caps and joins. Uses tiny-skia (allocates): for drawing
    /// icons into their cache only.
    pub fn polyline(&mut self, points: &[(f32, f32)], width: f32, color: Color) {
        self.stroke_points(points, width, color, false);
    }

    /// [`Canvas::polyline`] with the last point joined back to the first.
    pub fn polygon(&mut self, points: &[(f32, f32)], width: f32, color: Color) {
        self.stroke_points(points, width, color, true);
    }

    fn stroke_points(&mut self, points: &[(f32, f32)], width: f32, color: Color, close: bool) {
        let mut iter = points.iter();
        let Some(&(x, y)) = iter.next() else {
            return;
        };
        let mut pb = self.cache.builder();
        pb.move_to(self.px(x), self.py(y));
        for &(x, y) in iter {
            pb.line_to(self.px(x), self.py(y));
        }
        if close {
            pb.close();
        }
        if let Some(path) = pb.finish() {
            let stroke = Stroke { width: width * self.scale, line_cap: LineCap::Round, line_join: LineJoin::Round, ..Stroke::default() };
            self.stroke_path(path, &stroke, color);
        }
    }

    /// Draws `image` (already at its pixel size) with its top left at (x, y), blending its alpha.
    pub fn image(&mut self, image: &Pixmap, x: f32, y: f32) {
        self.blit(image, x, y, None);
    }

    /// Draws `image` in grey at `level` (0..1) of its brightness: an idle app's icon.
    pub fn image_grey(&mut self, image: &Pixmap, x: f32, y: f32, level: f32) {
        self.blit(image, x, y, Some((level.clamp(0.0, 1.0) * 256.0) as u32));
    }

    /// Blends `image` (premultiplied) with its top left at (x, y); `grey`: as luminance scaled by this
    /// (of 256).
    fn blit(&mut self, image: &Pixmap, x: f32, y: f32, grey: Option<u32>) {
        let (x0, y0) = (self.px(x).round() as i32, self.py(y).round() as i32);
        let pw = self.pixmap.width() as i32;
        let clip = self.clip;
        let iw = image.width() as usize;
        let Some((skip, x, len)) = span(clip, x0, iw) else {
            return;
        };
        let data = self.pixmap.data_mut();
        for (row, src_row) in image.data().chunks_exact(iw * 4).enumerate() {
            let y = y0 + row as i32;
            let i = ((y * pw + x) * 4) as usize;
            let (Some(dst), Some(src)) =
                ((clip.1..clip.3).contains(&y).then(|| data.get_mut(i..i + len * 4)).flatten(), src_row.get(skip * 4..(skip + len) * 4))
            else {
                continue;
            };
            match grey {
                // (Box art and screenshots: rows without transparency copied whole.)
                None if src.as_chunks::<4>().0.iter().fold(255, |all, px| all & px[3]) == 255 => dst.copy_from_slice(src),
                None => {
                    for (d, s) in dst.as_chunks_mut::<4>().0.iter_mut().zip(src.as_chunks::<4>().0) {
                        over(d, s.map(u32::from));
                    }
                }
                Some(level) => {
                    for (d, s) in dst.as_chunks_mut::<4>().0.iter_mut().zip(src.as_chunks::<4>().0) {
                        let luma = (u32::from(s[0]) * 54 + u32::from(s[1]) * 183 + u32::from(s[2]) * 19) / 256 * level / 256;
                        over(d, [luma, luma, luma, u32::from(s[3])]);
                    }
                }
            }
        }
    }
}

/// Blends a glyph's coverage (`width` bytes per row) in `color` onto the pixmap at (x, y), source-over,
/// premultiplied; within `clip` (x0, y0, x1, y1, inside the pixmap).
pub(super) fn blend_mask(pixmap: &mut PixmapMut, clip: (i32, i32, i32, i32), x: i32, y: i32, width: usize, coverage: &[u8], color: PremultipliedColorU8) {
    if width == 0 {
        return;
    }
    let Some((skip, x, len)) = span(clip, x, width) else {
        return;
    };
    let pw = pixmap.width() as i32;
    let src = [color.red(), color.green(), color.blue(), color.alpha()].map(u32::from);
    let data = pixmap.data_mut();
    for (gy, row) in coverage.chunks_exact(width).enumerate() {
        let ty = y + gy as i32;
        let i = ((ty * pw + x) * 4) as usize;
        let (Some(dst), Some(cov)) = ((clip.1..clip.3).contains(&ty).then(|| data.get_mut(i..i + len * 4)).flatten(), row.get(skip..skip + len)) else {
            continue;
        };
        for (px, &k) in dst.as_chunks_mut::<4>().0.iter_mut().zip(cov) {
            blend(px, src, u32::from(k));
        }
    }
}

/// The part of a row `width` pixels long from `x` inside `clip`'s columns: how many pixels it skips,
/// where it starts, how long it is.
fn span((cx0, _, cx1, _): (i32, i32, i32, i32), x: i32, width: usize) -> Option<(usize, i32, usize)> {
    let (from, to) = (x.max(cx0), (x + width as i32).min(cx1));
    (to > from).then(|| ((from - x) as usize, from, (to - from) as usize))
}

/// A rounded rectangle in buffer pixels, for its signed distance (negative inside).
struct RoundedRect {
    cx: f32,
    cy: f32,
    /// Half size minus the radius: the inner rectangle the corners are centred on.
    hx: f32,
    hy: f32,
    radius: f32,
}

impl RoundedRect {
    fn new(r: Rect, radius: f32, scale: f32, (dx, dy): (f32, f32)) -> Self {
        let (w, h) = (r.w * scale, r.h * scale);
        let radius = (radius * scale).min(w / 2.0).min(h / 2.0).max(0.0);
        let (cx, cy) = (r.x * scale + dx + w / 2.0, r.y * scale + dy + h / 2.0);
        RoundedRect { cx, cy, hx: w / 2.0 - radius, hy: h / 2.0 - radius, radius }
    }

    fn distance(&self, x: f32, y: f32) -> f32 {
        let qx = (x - self.cx).abs() - self.hx;
        let qy = (y - self.cy).abs() - self.hy;
        qx.max(0.0).hypot(qy.max(0.0)) + qx.max(qy).min(0.0) - self.radius
    }

    /// Pixel bounds, grown by `margin`.
    fn bounds(&self, margin: f32) -> (f32, f32, f32, f32) {
        let (ex, ey) = (self.hx + self.radius + margin, self.hy + self.radius + margin);
        (self.cx - ex, self.cy - ey, self.cx + ex, self.cy + ey)
    }
}

/// How a shape covers each row of pixels: in one run (filled and convex: a rounded rectangle, a
/// circle, a line), or any way (an outline).
#[derive(Clone, Copy, PartialEq, Eq)]
enum Runs {
    One,
    Any,
}

fn premultiplied(color: Color) -> [u32; 4] {
    let c = color.premultiply().to_color_u8();
    [c.red(), c.green(), c.blue(), c.alpha()].map(u32::from)
}

/// Source-over of premultiplied `src` at coverage `k` (0-255) onto one RGBA pixel.
/// Premultiplied `src` over the pixel `px` (as [`blend`] fully covered).
fn over(px: &mut [u8; 4], src: [u32; 4]) {
    let inverse = 255 - src[3] as u16;
    for (d, s) in px.iter_mut().zip(src) {
        *d = (s as u16 + div255(u16::from(*d) * inverse)) as u8;
    }
}

/// [`blend`] at byte `i` of `data`, if covered and there.
fn blend_at(data: &mut [u8], i: usize, src: [u32; 4], k: u32) {
    if k > 0
        && let Some(px) = data.get_mut(i..i + 4).and_then(|px| px.as_mut_array())
    {
        blend(px, src, k);
    }
}

fn blend(px: &mut [u8; 4], src: [u32; 4], k: u32) {
    let k = k as u16;
    let inverse = 255 - div255(src[3] as u16 * k);
    for (d, s) in px.iter_mut().zip(src) {
        *d = (div255(s as u16 * k) + div255(u16::from(*d) * inverse)) as u8;
    }
}

/// `x / 255` for `x` up to 255 * 255, in shifts (which vectorise; the same for every such `x`).
fn div255(x: u16) -> u16 {
    (x + 1 + (x >> 8)) >> 8
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    #[test]
    fn div255_divides() {
        for x in 0..=255 * 255 {
            assert_eq!(div255(x), x / 255, "{x}");
        }
    }
}
