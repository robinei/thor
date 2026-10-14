//! Drawing helpers: shapes and images through tiny-skia, text through fontdue with a glyph cache. All
//! coordinates are logical pixels; the canvas scales them to the buffer.

use std::collections::HashMap;
use std::process::Command;

use tiny_skia::{Color, FillRule, Paint, Path, PathBuilder, Pixmap, PixmapPaint, Stroke, Transform};

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

/// One UI font and its rasterized glyphs, keyed by character and pixel size.
pub struct Fonts {
    font: Option<fontdue::Font>,
    cache: HashMap<(char, u32), Glyph>,
}

impl Fonts {
    /// The system's default sans-serif font (fontconfig). Without one, text is skipped, nothing else.
    pub fn load() -> Fonts {
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
        Fonts { font, cache: HashMap::new() }
    }

    fn glyph(&mut self, ch: char, px: f32) -> Option<&Glyph> {
        let font = self.font.as_ref()?;
        Some(self.cache.entry((ch, px.to_bits())).or_insert_with(|| font.rasterize(ch, px)))
    }

    fn ascent(&self, px: f32) -> f32 {
        self.font.as_ref().and_then(|f| f.horizontal_line_metrics(px)).map_or(px * 0.8, |m| m.ascent)
    }
}

pub struct Canvas<'a> {
    pixmap: &'a mut Pixmap,
    scale: f32,
    fonts: &'a mut Fonts,
}

impl<'a> Canvas<'a> {
    pub fn new(pixmap: &'a mut Pixmap, scale: f32, fonts: &'a mut Fonts) -> Self {
        Canvas { pixmap, scale, fonts }
    }

    fn transform(&self) -> Transform {
        Transform::from_scale(self.scale, self.scale)
    }

    fn paint(color: Color) -> Paint<'static> {
        let mut paint = Paint::default();
        paint.set_color(color);
        paint.anti_alias = true;
        paint
    }

    pub fn fill(&mut self, r: Rect, color: Color) {
        if let Some(rect) = tiny_skia::Rect::from_xywh(r.x, r.y, r.w, r.h) {
            self.pixmap.fill_rect(rect, &Self::paint(color), self.transform(), None);
        }
    }

    pub fn rounded(&mut self, r: Rect, radius: f32, color: Color) {
        if let Some(path) = rounded_path(r, radius) {
            self.pixmap.fill_path(&path, &Self::paint(color), FillRule::Winding, self.transform(), None);
        }
    }

    pub fn outline(&mut self, r: Rect, radius: f32, width: f32, color: Color) {
        if let Some(path) = rounded_path(r, radius) {
            let stroke = Stroke { width, ..Stroke::default() };
            self.pixmap.stroke_path(&path, &Self::paint(color), &stroke, self.transform(), None);
        }
    }

    /// Draws one line of text with its top at `y`; returns its width.
    pub fn text(&mut self, x: f32, y: f32, size: f32, color: Color, text: &str) -> f32 {
        let px = size * self.scale;
        let baseline = (y * self.scale + self.fonts.ascent(px)).round();
        let start = x * self.scale;
        let mut pen = start;
        for ch in text.chars() {
            let Some((metrics, coverage)) = self.fonts.glyph(ch, px) else {
                return 0.0;
            };
            let advance = metrics.advance_width;
            if let Some(glyph) = tint(metrics, coverage, color) {
                let gx = (pen + metrics.xmin as f32).round() as i32;
                let gy = (baseline - metrics.height as f32 - metrics.ymin as f32) as i32;
                self.pixmap.draw_pixmap(gx, gy, glyph.as_ref(), &PixmapPaint::default(), Transform::identity(), None);
            }
            pen += advance;
        }
        (pen - start) / self.scale
    }

    pub fn text_width(&mut self, size: f32, text: &str) -> f32 {
        let px = size * self.scale;
        let mut width = 0.0;
        for ch in text.chars() {
            if let Some((metrics, _)) = self.fonts.glyph(ch, px) {
                width += metrics.advance_width;
            }
        }
        width / self.scale
    }
}

/// A glyph's coverage as a premultiplied pixmap in `color`.
fn tint(metrics: &fontdue::Metrics, coverage: &[u8], color: Color) -> Option<Pixmap> {
    let mut glyph = Pixmap::new(u32::try_from(metrics.width).ok()?, u32::try_from(metrics.height).ok()?)?;
    let c = color.premultiply();
    for (px, &cov) in glyph.data_mut().chunks_exact_mut(4).zip(coverage) {
        let k = f32::from(cov) / 255.0;
        if let [r, g, b, a] = px {
            *r = (c.red() * k * 255.0) as u8;
            *g = (c.green() * k * 255.0) as u8;
            *b = (c.blue() * k * 255.0) as u8;
            *a = (c.alpha() * k * 255.0) as u8;
        }
    }
    Some(glyph)
}

fn rounded_path(r: Rect, radius: f32) -> Option<Path> {
    let rad = radius.min(r.w / 2.0).min(r.h / 2.0).max(0.0);
    let (x0, y0, x1, y1) = (r.x, r.y, r.x + r.w, r.y + r.h);
    let mut pb = PathBuilder::new();
    pb.move_to(x0 + rad, y0);
    pb.line_to(x1 - rad, y0);
    pb.quad_to(x1, y0, x1, y0 + rad);
    pb.line_to(x1, y1 - rad);
    pb.quad_to(x1, y1, x1 - rad, y1);
    pb.line_to(x0 + rad, y1);
    pb.quad_to(x0, y1, x0, y1 - rad);
    pb.line_to(x0, y0 + rad);
    pb.quad_to(x0, y0, x0 + rad, y0);
    pb.close();
    pb.finish()
}
