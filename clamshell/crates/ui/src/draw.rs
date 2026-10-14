//! Drawing helpers: shapes and images through tiny-skia, text through fontdue with a glyph cache. All
//! coordinates are logical pixels; the canvas scales them to the buffer.
//!
//! No heap allocation per frame once warm (a test counts): glyphs and icons are rasterized once and
//! blended straight into the pixmap as coverage masks; rounded rectangles and circles are computed per
//! pixel from their distance function, with no path at all.

use std::collections::HashMap;
use std::process::Command;

use tiny_skia::{Color, LineCap, LineJoin, Paint, Path, PathBuilder, Pixmap, PremultipliedColorU8, Stroke, Transform};

use crate::icons::{self, Icon};

#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct Rect {
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
}

impl Rect {
    pub fn new(x: f32, y: f32, w: f32, h: f32) -> Self {
        Rect { x, y, w, h }
    }

    pub fn contains(&self, x: f32, y: f32) -> bool {
        x >= self.x && x < self.x + self.w && y >= self.y && y < self.y + self.h
    }

    pub fn center(&self) -> (f32, f32) {
        (self.x + self.w / 2.0, self.y + self.h / 2.0)
    }

    pub fn inset(&self, d: f32) -> Rect {
        Rect::new(self.x + d, self.y + d, self.w - 2.0 * d, self.h - 2.0 * d)
    }

    /// The top `h` of this, and the rest below it.
    pub fn split_top(&self, h: f32) -> (Rect, Rect) {
        (Rect::new(self.x, self.y, self.w, h), Rect::new(self.x, self.y + h, self.w, self.h - h))
    }

    /// Cell `i` (row by row) of a grid of `cols` by `rows` equal cells filling this, `gap` between them.
    /// A row of buttons is a grid with one row.
    pub fn cell(&self, cols: usize, rows: usize, gap: f32, i: usize) -> Rect {
        let (cols, rows) = (cols.max(1), rows.max(1));
        let w = (self.w - (cols - 1) as f32 * gap) / cols as f32;
        let h = (self.h - (rows - 1) as f32 * gap) / rows as f32;
        let (col, row) = ((i % cols) as f32, (i / cols) as f32);
        Rect::new(self.x + col * (w + gap), self.y + row * (h + gap), w, h)
    }
}

/// Which part of a text [`Canvas::text_fit`] keeps when it is too wide.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Keep {
    /// The start, with "…" where it is cut.
    Start,
    /// The end (the latest typed).
    End,
}

const ELLIPSIS: &str = "…";

/// 0xRRGGBB to an opaque colour.
pub fn rgb(c: u32) -> Color {
    Color::from_rgba8((c >> 16) as u8, (c >> 8) as u8, c as u8, 255)
}

/// 0xRRGGBB at `alpha` (0-1): what is under it shows through.
pub fn rgba(c: u32, alpha: f32) -> Color {
    Color::from_rgba8((c >> 16) as u8, (c >> 8) as u8, c as u8, (alpha.clamp(0.0, 1.0) * 255.0) as u8)
}

type Glyph = (fontdue::Metrics, Vec<u8>);

/// The UI's font, built in so text looks the same everywhere: Noto Sans Regular, unmodified (SIL Open
/// Font License 1.1, fonts/OFL.txt).
const NOTO_SANS: &[u8] = include_bytes!("../fonts/NotoSans-Regular.ttf");

/// What drawing keeps between frames: the font, fonts found for characters it lacks, the rasterized
/// glyphs (by character and pixel size), and a path builder whose memory is reused for every shape.
pub struct DrawCache {
    font: Option<fontdue::Font>,
    /// Fonts for characters the built-in one lacks (a Japanese title, say), found through fontconfig
    /// when first needed; and for each such character, which of them has it (none: no font does).
    fallbacks: Vec<(String, fontdue::Font)>,
    fallback_for: HashMap<char, Option<usize>>,
    glyphs: HashMap<(char, u32), Glyph>,
    /// Icon coverage masks, by icon and side in pixels.
    icons: HashMap<(Icon, u32), Vec<u8>>,
    path: Option<PathBuilder>,
}

impl DrawCache {
    pub fn load() -> DrawCache {
        let font = fontdue::Font::from_bytes(NOTO_SANS, fontdue::FontSettings::default())
            .map_err(|e| log::error!("the built-in font: {e}"))
            .ok();
        DrawCache { font, fallbacks: Vec::new(), fallback_for: HashMap::new(), glyphs: HashMap::new(), icons: HashMap::new(), path: None }
    }

    fn glyph(&mut self, ch: char, px: f32) -> Option<&Glyph> {
        let key = (ch, px.to_bits());
        if !self.glyphs.contains_key(&key) {
            let font = match self.fallback(ch) {
                Some(i) => self.fallbacks.get(i).map(|(_, f)| f),
                None => self.font.as_ref(),
            }?;
            let glyph = font.rasterize(ch, px);
            self.glyphs.insert(key, glyph);
        }
        self.glyphs.get(&key)
    }

    /// The fallback font to draw `ch` with, if the built-in font lacks it and another has it.
    fn fallback(&mut self, ch: char) -> Option<usize> {
        if ch.is_control() || self.font.as_ref().is_none_or(|f| f.lookup_glyph_index(ch) != 0) {
            return None;
        }
        if let Some(&known) = self.fallback_for.get(&ch) {
            return known;
        }
        let found = self.find_fallback(ch);
        if found.is_none() {
            log::debug!("no font has {ch:?}");
        }
        self.fallback_for.insert(ch, found);
        found
    }

    /// Asks fontconfig for a sans-serif font with `ch`, loading it once.
    fn find_fallback(&mut self, ch: char) -> Option<usize> {
        if let Some(i) = self.fallbacks.iter().position(|(_, f)| f.lookup_glyph_index(ch) != 0) {
            return Some(i);
        }
        let pattern = format!("sans-serif:charset={:x}", u32::from(ch));
        let out = Command::new("fc-match").args(["--format=%{file}", &pattern]).output().ok()?;
        let path = String::from_utf8(out.stdout).ok().filter(|p| out.status.success() && !p.is_empty())?;
        if self.fallbacks.iter().any(|(p, _)| *p == path) {
            return None;
        }
        let bytes = std::fs::read(&path).map_err(|e| log::warn!("font {path}: {e}")).ok()?;
        let font = fontdue::Font::from_bytes(bytes, fontdue::FontSettings::default())
            .map_err(|e| log::warn!("font {path}: {e}"))
            .ok()
            .filter(|f| f.lookup_glyph_index(ch) != 0)?;
        log::info!("fallback font for {ch:?}: {path}");
        self.fallbacks.push((path, font));
        Some(self.fallbacks.len() - 1)
    }

    fn ascent(&self, px: f32) -> f32 {
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
    pixmap: &'a mut Pixmap,
    scale: f32,
    /// Whole buffer pixels everything is moved by (burn-in orbit).
    offset: (f32, f32),
    cache: &'a mut DrawCache,
}

impl<'a> Canvas<'a> {
    pub fn new(pixmap: &'a mut Pixmap, scale: f32, cache: &'a mut DrawCache) -> Self {
        Canvas { pixmap, scale, offset: (0.0, 0.0), cache }
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
    fn px(&self, x: f32) -> f32 {
        x * self.scale + self.offset.0
    }

    fn py(&self, y: f32) -> f32 {
        y * self.scale + self.offset.1
    }

    fn transform(&self) -> Transform {
        Transform::from_row(self.scale, 0.0, 0.0, self.scale, self.offset.0, self.offset.1)
    }

    fn paint(color: Color) -> Paint<'static> {
        let mut paint = Paint::default();
        paint.set_color(color);
        paint.anti_alias = true;
        paint
    }

    /// A vertical line `px` buffer pixels wide, centred on `x` and snapped to whole pixels (sharp).
    pub fn vline(&mut self, x: f32, y0: f32, y1: f32, px: u32, color: Color) {
        let left = (self.px(x) - px as f32 / 2.0).round();
        let (top, bottom) = (self.py(y0).round(), self.py(y1).round());
        if let Some(rect) = tiny_skia::Rect::from_ltrb(left, top, left + px as f32, bottom) {
            self.pixmap.fill_rect(rect, &Self::paint(color), Transform::identity(), None);
        }
    }

    /// A horizontal line `px` buffer pixels high, centred on `y` and snapped to whole pixels.
    pub fn hline(&mut self, x0: f32, x1: f32, y: f32, px: u32, color: Color) {
        let top = (self.py(y) - px as f32 / 2.0).round();
        let (left, right) = (self.px(x0).round(), self.px(x1).round());
        if let Some(rect) = tiny_skia::Rect::from_ltrb(left, top, right, top + px as f32) {
            self.pixmap.fill_rect(rect, &Self::paint(color), Transform::identity(), None);
        }
    }

    pub fn fill(&mut self, r: Rect, color: Color) {
        if let Some(rect) = tiny_skia::Rect::from_xywh(r.x, r.y, r.w, r.h) {
            self.pixmap.fill_rect(rect, &Self::paint(color), self.transform(), None);
        }
    }

    fn stroke_path(&mut self, path: Path, stroke: &Stroke, color: Color) {
        self.pixmap.stroke_path(&path, &Self::paint(color), stroke, Transform::identity(), None);
        self.cache.recycle(path);
    }

    /// A rounded rectangle, filled.
    pub fn rounded(&mut self, r: Rect, radius: f32, color: Color) {
        let shape = RoundedRect::new(r, radius, self.scale, self.offset);
        self.shade(shape.bounds(1.0), color, |x, y| (0.5 - shape.distance(x, y)).clamp(0.0, 1.0));
    }

    /// A rounded rectangle's outline, `width` wide, centred on its edge.
    pub fn outline(&mut self, r: Rect, radius: f32, width: f32, color: Color) {
        let shape = RoundedRect::new(r, radius, self.scale, self.offset);
        let half = width * self.scale / 2.0;
        self.shade(shape.bounds(half + 1.0), color, |x, y| (half + 0.5 - shape.distance(x, y).abs()).clamp(0.0, 1.0));
    }

    pub fn circle(&mut self, cx: f32, cy: f32, r: f32, color: Color) {
        let (cx, cy, r) = (self.px(cx), self.py(cy), r * self.scale);
        let bounds = (cx - r - 1.0, cy - r - 1.0, cx + r + 1.0, cy + r + 1.0);
        self.shade(bounds, color, |x, y| (0.5 - ((x - cx).hypot(y - cy) - r)).clamp(0.0, 1.0));
    }

    /// Blends `color` over the pixels in `bounds` (x0, y0, x1, y1 in buffer pixels) with the coverage
    /// `coverage` gives for each pixel centre.
    fn shade(&mut self, (x0, y0, x1, y1): (f32, f32, f32, f32), color: Color, coverage: impl Fn(f32, f32) -> f32) {
        let (pw, ph) = (self.pixmap.width() as i32, self.pixmap.height() as i32);
        let (x0, y0) = ((x0.floor() as i32).max(0), (y0.floor() as i32).max(0));
        let (x1, y1) = ((x1.ceil() as i32).min(pw), (y1.ceil() as i32).min(ph));
        let src = premultiplied(color);
        let data = self.pixmap.data_mut();
        for y in y0..y1 {
            for x in x0..x1 {
                let k = (coverage(x as f32 + 0.5, y as f32 + 0.5) * 255.0) as u32;
                let i = ((y * pw + x) * 4) as usize;
                if k > 0
                    && let Some(px) = data.get_mut(i..i + 4)
                {
                    blend(px, src, k);
                }
            }
        }
    }

    /// Draws `icon` centred in `r`, as large as fits. Rendered once per size, then blended as a mask.
    pub fn icon(&mut self, icon: Icon, r: Rect, color: Color) {
        let side = (r.w.min(r.h) * self.scale).round().max(1.0) as u32;
        let key = (icon, side);
        if !self.cache.icons.contains_key(&key) {
            let Some(mut mask) = Pixmap::new(side, side) else {
                return;
            };
            {
                let logical = side as f32 / self.scale;
                let mut c = Canvas { pixmap: &mut mask, scale: self.scale, offset: (0.0, 0.0), cache: &mut *self.cache };
                icons::draw(&mut c, icon, Rect::new(0.0, 0.0, logical, logical), Color::WHITE);
            }
            let coverage = mask.data().chunks_exact(4).filter_map(|px| px.get(3).copied()).collect();
            self.cache.icons.insert(key, coverage);
        }
        let (cx, cy) = r.center();
        let x = (self.px(cx) - side as f32 / 2.0).round() as i32;
        let y = (self.py(cy) - side as f32 / 2.0).round() as i32;
        if let Some(coverage) = self.cache.icons.get(&key) {
            blend_mask(self.pixmap, x, y, side as usize, coverage, color.premultiply().to_color_u8());
        }
    }

    /// Connected lines through `points`, round caps and joins; `close` joins the last to the first. Uses
    /// tiny-skia (allocates): for drawing icons into their cache only.
    pub fn polyline(&mut self, points: &[(f32, f32)], width: f32, color: Color, close: bool) {
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
            let stroke =
                Stroke { width: width * self.scale, line_cap: LineCap::Round, line_join: LineJoin::Round, ..Stroke::default() };
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
        let (pw, ph) = (self.pixmap.width() as i32, self.pixmap.height() as i32);
        let iw = image.width() as usize;
        let data = self.pixmap.data_mut();
        for (row, src_row) in image.data().chunks_exact(iw * 4).enumerate() {
            let y = y0 + row as i32;
            if y < 0 || y >= ph {
                continue;
            }
            for (col, src) in src_row.chunks_exact(4).enumerate() {
                let x = x0 + col as i32;
                if x < 0 || x >= pw {
                    continue;
                }
                let i = ((y * pw + x) * 4) as usize;
                if let (Some(dst), [r, g, b, a]) = (data.get_mut(i..i + 4), src) {
                    let mut px = [*r, *g, *b, *a].map(u32::from);
                    if let Some(level) = grey {
                        let luma = (px[0] * 54 + px[1] * 183 + px[2] * 19) / 256 * level / 256;
                        px = [luma, luma, luma, px[3]];
                    }
                    if *a == 255 && grey.is_none() {
                        dst.copy_from_slice(src);
                    } else if *a > 0 {
                        blend(dst, px, 255);
                    }
                }
            }
        }
    }

    /// Draws one line of text with its top at `y`; returns its width.
    pub fn text(&mut self, x: f32, y: f32, size: f32, color: Color, text: &str) -> f32 {
        let px = size * self.scale;
        let baseline = (self.py(y) + self.cache.ascent(px)).round();
        let start = self.px(x);
        let mut pen = start;
        let color = color.premultiply().to_color_u8();
        for ch in text.chars() {
            let Some((metrics, coverage)) = self.cache.glyph(ch, px) else {
                return 0.0;
            };
            let gx = (pen + metrics.xmin as f32).round() as i32;
            let gy = (baseline - metrics.height as f32 - metrics.ymin as f32) as i32;
            blend_mask(self.pixmap, gx, gy, metrics.width, coverage, color);
            pen += metrics.advance_width;
        }
        (pen - start) / self.scale
    }

    /// Draws one line of text within `width`: cut to fit as `keep` says if it is wider. Returns the width
    /// drawn.
    pub fn text_fit(&mut self, (x, y, width): (f32, f32, f32), size: f32, color: Color, text: &str, keep: Keep) -> f32 {
        if self.text_width(size, text) <= width {
            return self.text(x, y, size, color, text);
        }
        let (start, end) = self.fit(size, text, width, keep);
        let part = text.get(start..end).unwrap_or("");
        let drawn = self.text(x, y, size, color, part);
        match keep {
            Keep::Start => drawn + self.text(x + drawn, y, size, color, ELLIPSIS),
            Keep::End => drawn,
        }
    }

    /// The byte range of `text` that `text_fit` draws when it is too wide.
    fn fit(&mut self, size: f32, text: &str, width: f32, keep: Keep) -> (usize, usize) {
        let mut used = 0.0;
        match keep {
            Keep::Start => {
                let room = width - self.text_width(size, ELLIPSIS);
                let mut end = 0;
                for (i, ch) in text.char_indices() {
                    used += self.char_width(size, ch);
                    if used > room {
                        break;
                    }
                    end = i + ch.len_utf8();
                }
                (0, text.get(..end).unwrap_or("").trim_end().len())
            }
            Keep::End => {
                let mut start = text.len();
                for (i, ch) in text.char_indices().rev() {
                    used += self.char_width(size, ch);
                    if used > width {
                        break;
                    }
                    start = i;
                }
                (start, text.len())
            }
        }
    }

    fn char_width(&mut self, size: f32, ch: char) -> f32 {
        let px = size * self.scale;
        self.cache.glyph(ch, px).map_or(0.0, |(m, _)| m.advance_width) / self.scale
    }

    pub fn text_width(&mut self, size: f32, text: &str) -> f32 {
        let px = size * self.scale;
        let mut width = 0.0;
        for ch in text.chars() {
            if let Some((metrics, _)) = self.cache.glyph(ch, px) {
                width += metrics.advance_width;
            }
        }
        width / self.scale
    }
}

/// Blends a glyph's coverage (`width` bytes per row) in `color` onto the pixmap at (x, y), source-over,
/// premultiplied; clipped to the pixmap.
fn blend_mask(pixmap: &mut Pixmap, x: i32, y: i32, width: usize, coverage: &[u8], color: PremultipliedColorU8) {
    if width == 0 {
        return;
    }
    let (pw, ph) = (pixmap.width() as i32, pixmap.height() as i32);
    let src = [color.red(), color.green(), color.blue(), color.alpha()].map(u32::from);
    let data = pixmap.data_mut();
    for (gy, row) in coverage.chunks_exact(width).enumerate() {
        let ty = y + gy as i32;
        if ty < 0 || ty >= ph {
            continue;
        }
        for (gx, &cov) in row.iter().enumerate() {
            let tx = x + gx as i32;
            if cov == 0 || tx < 0 || tx >= pw {
                continue;
            }
            let i = ((ty * pw + tx) * 4) as usize;
            if let Some(px) = data.get_mut(i..i + 4) {
                blend(px, src, u32::from(cov));
            }
        }
    }
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

fn premultiplied(color: Color) -> [u32; 4] {
    let c = color.premultiply().to_color_u8();
    [c.red(), c.green(), c.blue(), c.alpha()].map(u32::from)
}

/// Source-over of premultiplied `src` at coverage `k` (0-255) onto one RGBA pixel.
fn blend(px: &mut [u8], src: [u32; 4], k: u32) {
    let inverse = 255 - src[3] * k / 255;
    for (d, s) in px.iter_mut().zip(src) {
        *d = (s * k / 255 + u32::from(*d) * inverse / 255) as u8;
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    #[test]
    fn cells_fill_the_rect_with_gaps_between() {
        let r = Rect::new(10.0, 20.0, 310.0, 100.0);
        assert_eq!(r.cell(3, 2, 5.0, 0), Rect::new(10.0, 20.0, 100.0, 47.5));
        assert_eq!(r.cell(3, 2, 5.0, 5), Rect::new(220.0, 72.5, 100.0, 47.5));
        let (top, rest) = r.split_top(30.0);
        assert_eq!((top.h, rest.y, rest.h), (30.0, 50.0, 70.0));
    }

    #[test]
    fn the_built_in_font_covers_the_ui_and_others_fill_in() {
        let mut cache = DrawCache::load();
        for ch in "Aa0…·°–€éøÅßΩЖ".chars() {
            assert_eq!(cache.fallback(ch), None, "{ch:?} is in the built-in font");
            assert!(cache.glyph(ch, 20.0).is_some_and(|(m, _)| m.width > 0), "{ch:?} draws");
        }
        // A kana: from a system font, when fontconfig knows one (not every machine has one).
        let offered = Command::new("fc-match").args(["--format=%{file}", "sans-serif:charset=3042"]).output();
        if offered.is_ok_and(|o| o.status.success() && !o.stdout.is_empty()) {
            let i = cache.fallback('あ');
            assert!(i.is_some(), "fontconfig offers a font with あ, so it is used");
            assert!(cache.glyph('あ', 20.0).is_some_and(|(m, _)| m.width > 0));
        }
    }

    #[test]
    fn text_is_cut_to_fit() {
        let mut cache = DrawCache::load();
        let mut pixmap = Pixmap::new(400, 50).unwrap();
        let mut c = Canvas::new(&mut pixmap, 1.0, &mut cache);
        let text = "A Link to the Past";
        let full = c.text_width(20.0, text);
        let width = full * 0.6;
        let (start, end) = c.fit(20.0, text, width, Keep::Start);
        assert_eq!(start, 0);
        let head = text.get(..end).unwrap();
        assert!(c.text_width(20.0, head) + c.text_width(20.0, ELLIPSIS) <= width && !head.ends_with(' '), "{head:?}");
        let (start, _) = c.fit(20.0, text, width, Keep::End);
        let tail = text.get(start..).unwrap();
        assert!(c.text_width(20.0, tail) <= width && tail.ends_with("Past"), "{tail:?}");
        assert!(c.text_fit((0.0, 0.0, width), 20.0, rgb(0xffffff), text, Keep::Start) <= width + 0.5);
    }
}
