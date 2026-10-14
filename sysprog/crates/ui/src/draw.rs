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
}

/// 0xRRGGBB to an opaque colour.
pub fn rgb(c: u32) -> Color {
    Color::from_rgba8((c >> 16) as u8, (c >> 8) as u8, c as u8, 255)
}

type Glyph = (fontdue::Metrics, Vec<u8>);

/// What drawing keeps between frames: the font, its rasterized glyphs (by character and pixel size), and
/// a path builder whose memory is reused for every shape.
pub struct DrawCache {
    font: Option<fontdue::Font>,
    glyphs: HashMap<(char, u32), Glyph>,
    /// Icon coverage masks, by icon and side in pixels.
    icons: HashMap<(Icon, u32), Vec<u8>>,
    path: Option<PathBuilder>,
}

impl DrawCache {
    /// The system's default sans-serif font (fontconfig). Without one, text is skipped, nothing else.
    pub fn load() -> DrawCache {
        let path = Command::new("fc-match")
            .args(["--format=%{file}", "sans-serif"])
            .output()
            .ok()
            .filter(|out| out.status.success())
            .and_then(|out| String::from_utf8(out.stdout).ok());
        let font = path.and_then(|path| {
            let bytes = std::fs::read(&path).map_err(|e| log::warn!("font {path}: {e}")).ok()?;
            fontdue::Font::from_bytes(bytes, fontdue::FontSettings::default())
                .map_err(|e| log::warn!("font {path}: {e}"))
                .ok()
        });
        if font.is_none() {
            log::warn!("no font found (fc-match sans-serif); drawing without text");
        }
        DrawCache { font, glyphs: HashMap::new(), icons: HashMap::new(), path: None }
    }

    fn glyph(&mut self, ch: char, px: f32) -> Option<&Glyph> {
        let font = self.font.as_ref()?;
        Some(self.glyphs.entry((ch, px.to_bits())).or_insert_with(|| font.rasterize(ch, px)))
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
