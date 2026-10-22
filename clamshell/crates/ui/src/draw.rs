//! Drawing helpers: shapes and images through tiny-skia, text through fontdue with a glyph cache. All
//! coordinates are logical pixels; the canvas scales them to the buffer.
//!
//! No heap allocation per frame once warm (a test counts): glyphs and icons are rasterized once and
//! blended straight into the pixmap as coverage masks; rounded rectangles and circles are computed per
//! pixel from their distance function, with no path at all.

use std::collections::HashMap;
use std::process::Command;
use std::sync::mpsc::{Receiver, Sender};

use tiny_skia::{Color, LineCap, LineJoin, Paint, Path, PathBuilder, Pixmap, PixmapMut, PremultipliedColorU8, Stroke, Transform};

use crate::icons::{self, Icon};

/// A rectangle in logical pixels: its top left corner and size.
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

    pub fn right(&self) -> f32 {
        self.x + self.w
    }

    pub fn bottom(&self) -> f32 {
        self.y + self.h
    }

    pub fn contains(&self, x: f32, y: f32) -> bool {
        x >= self.x && x < self.x + self.w && y >= self.y && y < self.y + self.h
    }

    pub fn center(&self) -> (f32, f32) {
        (self.x + self.w / 2.0, self.y + self.h / 2.0)
    }

    /// Smaller by `d` on every side (negative: larger).
    pub fn inset(&self, d: f32) -> Rect {
        Rect::new(self.x + d, self.y + d, self.w - 2.0 * d, self.h - 2.0 * d)
    }

    /// The part of this inside `other` (empty, with no size, if none).
    pub fn intersect(&self, other: Rect) -> Rect {
        let (x0, y0) = (self.x.max(other.x), self.y.max(other.y));
        let (x1, y1) = (self.right().min(other.right()), self.bottom().min(other.bottom()));
        Rect::new(x0, y0, (x1 - x0).max(0.0), (y1 - y0).max(0.0))
    }

    /// Wider by `dx` on each side (into the margins, as a list's rows reach for their pressed look).
    pub fn widened(&self, dx: f32) -> Rect {
        Rect::new(self.x - dx, self.y, self.w + 2.0 * dx, self.h)
    }

    /// Moved by `dx`, `dy`.
    pub fn shifted(&self, dx: f32, dy: f32) -> Rect {
        Rect::new(self.x + dx, self.y + dy, self.w, self.h)
    }

    /// A `w` by `h` rectangle centred in this one.
    pub fn centered(&self, w: f32, h: f32) -> Rect {
        Rect::new(self.x + (self.w - w) / 2.0, self.y + (self.h - h) / 2.0, w, h)
    }

    /// A square of `side` centred in this one (an icon's place).
    pub fn square_center(&self, side: f32) -> Rect {
        self.centered(side, side)
    }

    /// The first line of text of `size` in this: its top, its width, one [`LINE`] high.
    pub fn top_line(&self, size: f32) -> Rect {
        Rect::new(self.x, self.y, self.w, size * LINE)
    }

    /// Cuts the top `h` off this; returns it.
    pub fn take_top(&mut self, h: f32) -> Rect {
        let top = Rect::new(self.x, self.y, self.w, h);
        self.y += h;
        self.h -= h;
        top
    }

    /// Cuts the bottom `h` off this; returns it.
    pub fn take_bottom(&mut self, h: f32) -> Rect {
        self.h -= h;
        Rect::new(self.x, self.y + self.h, self.w, h)
    }

    /// Cuts the left `w` off this; returns it.
    pub fn take_left(&mut self, w: f32) -> Rect {
        let left = Rect::new(self.x, self.y, w, self.h);
        self.x += w;
        self.w -= w;
        left
    }

    /// Cuts the right `w` off this; returns it.
    pub fn take_right(&mut self, w: f32) -> Rect {
        self.w -= w;
        Rect::new(self.x + self.w, self.y, w, self.h)
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

/// Lines stacked down an area: each [`Column::take`] takes the next `h` below the last.
#[derive(Debug, Clone, Copy)]
pub struct Column {
    area: Rect,
    y: f32,
}

impl Column {
    pub fn new(area: Rect) -> Self {
        Column { area, y: area.y }
    }

    /// Where the next line would start.
    pub fn y(&self) -> f32 {
        self.y
    }

    /// The bottom of the area.
    pub fn end(&self) -> f32 {
        self.area.y + self.area.h
    }

    /// Whether `h` more fits.
    pub fn fits(&self, h: f32) -> bool {
        self.y + h <= self.end()
    }

    /// The next `h`, whether it fits or not.
    pub fn take(&mut self, h: f32) -> Rect {
        let r = Rect::new(self.area.x, self.y, self.area.w, h);
        self.y += h;
        r
    }

    /// The next `h`, if it fits (rows taken while there is room).
    pub fn next(&mut self, h: f32) -> Option<Rect> {
        if self.y + h > self.end() + 0.01 {
            return None;
        }
        Some(self.take(h))
    }

    /// Leaves `h` empty.
    pub fn skip(&mut self, h: f32) {
        self.y += h;
    }

    /// What is left below the lines taken.
    pub fn rest(&self) -> Rect {
        Rect::new(self.area.x, self.y, self.area.w, self.end() - self.y)
    }
}

/// Where a line of text starts and how much room it has to the right (for text that runs on over
/// several lines).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Span {
    pub x: f32,
    pub y: f32,
    pub w: f32,
}

/// Where text or a picture sits in the room it has, along one axis.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Align {
    Start,
    Center,
    End,
}

/// A line of text's height, of its size (Noto Sans' ascent and descent, rounded up).
pub const LINE: f32 = 1.2;

/// A colour as 0xRRGGBB, opaque.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Rgb(pub u32);

impl Rgb {
    /// At `alpha` (0-1): what is under it shows through.
    pub fn alpha(self, alpha: f32) -> Color {
        let c = self.0;
        Color::from_rgba8((c >> 16) as u8, (c >> 8) as u8, c as u8, (alpha.clamp(0.0, 1.0) * 255.0) as u8)
    }
}

impl From<Rgb> for Color {
    fn from(c: Rgb) -> Color {
        rgb(c.0)
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

/// The distance between wrapped lines, of the text's size (a little more than [`LINE`], for reading).
pub const WRAP_LINE: f32 = 1.3;

/// 0xRRGGBB to an opaque colour.
pub fn rgb(c: u32) -> Color {
    Color::from_rgba8((c >> 16) as u8, (c >> 8) as u8, c as u8, 255)
}

type Glyph = (fontdue::Metrics, Vec<u8>);

/// Which font draws a character.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Face {
    Builtin,
    /// A font found for characters the built-in one lacks (`DrawCache`'s fallbacks).
    Fallback(usize),
    /// Being looked for: not drawn yet.
    Waiting,
}

/// A font found for a character: fontconfig's choice (`path`), and the font read from it unless it
/// was read before or lacks the character.
pub struct Found {
    pub ch: char,
    pub path: Option<String>,
    pub font: Option<fontdue::Font>,
}

/// Asks fontconfig for a sans-serif font with `ch`, and reads it unless `loaded` says it was.
fn find_font(ch: char, loaded: &mut dyn FnMut(&str) -> bool) -> Found {
    let pattern = format!("sans-serif:charset={:x}", u32::from(ch));
    let path = Command::new("fc-match")
        .args(["--format=%{file}", &pattern])
        .output()
        .ok()
        .filter(|out| out.status.success())
        .and_then(|out| String::from_utf8(out.stdout).ok())
        .filter(|p| !p.is_empty());
    let font = path.as_deref().filter(|p| !loaded(p)).and_then(|path| {
        let bytes = std::fs::read(path).map_err(|e| log::warn!("font {path}: {e}")).ok()?;
        let font = fontdue::Font::from_bytes(bytes, fontdue::FontSettings::default()).map_err(|e| log::warn!("font {path}: {e}")).ok()?;
        // (Kept only for a character it has: fontconfig's best may lack it.)
        (font.lookup_glyph_index(ch) != 0).then_some(font)
    });
    Found { ch, path, font }
}

/// The font finding thread: characters in, the fonts found for them out through `done`.
pub fn spawn_font_finder(requests: Receiver<char>, done: impl Fn(Found) -> bool + Send + 'static) -> std::io::Result<()> {
    std::thread::Builder::new().name("fonts".into()).spawn(move || {
        let mut read: Vec<String> = Vec::new();
        for ch in requests {
            let found = find_font(ch, &mut |path| read.iter().any(|p| p == path));
            if found.font.is_some() {
                read.extend(found.path.clone());
            }
            if !done(found) {
                return;
            }
        }
    })?;
    Ok(())
}

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
    fallback_for: HashMap<char, Face>,
    /// Where characters to find a font for go (a font is a few MB to read and parse); `None` finds it
    /// at once (snapshots, tests).
    finder: Option<Sender<char>>,
    glyphs: HashMap<(char, u32), Glyph>,
    /// Icon coverage masks, by icon and side in pixels.
    icons: HashMap<(Icon, u32), Vec<u8>>,
    path: Option<PathBuilder>,
}

impl DrawCache {
    pub fn load() -> DrawCache {
        let font = fontdue::Font::from_bytes(NOTO_SANS, fontdue::FontSettings::default()).map_err(|e| log::error!("the built-in font: {e}")).ok();
        DrawCache { font, fallbacks: Vec::new(), fallback_for: HashMap::new(), finder: None, glyphs: HashMap::new(), icons: HashMap::new(), path: None }
    }

    /// As [`DrawCache::load`], fonts for characters the built-in one lacks found by `finder` (see
    /// [`spawn_font_finder`]) and handed back through [`DrawCache::found`]; such a character isn't drawn
    /// until then.
    pub fn with_finder(finder: Sender<char>) -> DrawCache {
        DrawCache { finder: Some(finder), ..DrawCache::load() }
    }

    fn glyph(&mut self, ch: char, px: f32) -> Option<&Glyph> {
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
    fn face(&mut self, ch: char) -> Face {
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
    pixmap: PixmapMut<'a>,
    scale: f32,
    /// Whole buffer pixels everything is moved by (burn-in orbit).
    offset: (f32, f32),
    /// Where drawing reaches, in buffer pixels (x0, y0, x1, y1): the pixmap, or a scrolled list's area.
    clip: (i32, i32, i32, i32),
    cache: &'a mut DrawCache,
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
    fn px(&self, x: f32) -> f32 {
        x * self.scale + self.offset.0
    }

    fn py(&self, y: f32) -> f32 {
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

    /// Draws `icon` centred in `r`, as large as fits. Rendered once per size, then blended as a mask.
    pub fn icon(&mut self, icon: Icon, r: Rect, color: impl Into<Color>) {
        let side = (r.w.min(r.h) * self.scale).round().max(1.0) as u32;
        let key = (icon, side);
        if !self.cache.icons.contains_key(&key) {
            let Some(mut mask) = Pixmap::new(side, side) else {
                return;
            };
            {
                let logical = side as f32 / self.scale;
                let clip = (0, 0, side as i32, side as i32);
                let mut c = Canvas { pixmap: mask.as_mut(), scale: self.scale, offset: (0.0, 0.0), clip, cache: &mut *self.cache };
                icons::draw(&mut c, icon, Rect::new(0.0, 0.0, logical, logical), Color::WHITE);
            }
            let coverage = mask.data().chunks_exact(4).filter_map(|px| px.get(3).copied()).collect();
            self.cache.icons.insert(key, coverage);
        }
        let (cx, cy) = r.center();
        let x = (self.px(cx) - side as f32 / 2.0).round() as i32;
        let y = (self.py(cy) - side as f32 / 2.0).round() as i32;
        if let Some(coverage) = self.cache.icons.get(&key) {
            blend_mask(&mut self.pixmap, self.clip, x, y, side as usize, coverage, color.into().premultiply().to_color_u8());
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

    /// Draws one line of text with its top at `y`; returns its width.
    pub fn text(&mut self, x: f32, y: f32, size: f32, color: impl Into<Color>, text: &str) -> f32 {
        let px = size * self.scale;
        let baseline = (self.py(y) + self.cache.ascent(px)).round();
        let start = self.px(x);
        let mut pen = start;
        let color = color.into().premultiply().to_color_u8();
        for ch in text.chars() {
            let Some((metrics, coverage)) = self.cache.glyph(ch, px) else {
                return 0.0;
            };
            let gx = (pen + metrics.xmin as f32).round() as i32;
            let gy = (baseline - metrics.height as f32 - metrics.ymin as f32) as i32;
            blend_mask(&mut self.pixmap, self.clip, gx, gy, metrics.width, coverage, color);
            pen += metrics.advance_width;
        }
        (pen - start) / self.scale
    }

    /// Draws one line of text centred across `r`'s height, placed along its width as `align` says
    /// (not cut: see [`Canvas::text_fit`]). Returns its width.
    pub fn text_in(&mut self, r: Rect, size: f32, color: impl Into<Color>, text: &str, align: Align) -> f32 {
        let y = r.y + (r.h - size * LINE) / 2.0;
        let x = match align {
            Align::Start => r.x,
            Align::Center => r.x + (r.w - self.text_width(size, text)) / 2.0,
            Align::End => r.x + r.w - self.text_width(size, text),
        };
        self.text(x, y, size, color, text)
    }

    /// Draws one line of text from `r`'s left, centred across its height, cut to its width as `keep`
    /// says if it is wider. Returns the width drawn.
    pub fn text_fit(&mut self, r: Rect, size: f32, color: impl Into<Color>, text: &str, keep: Keep) -> f32 {
        let color = color.into();
        let y = r.y + (r.h - size * LINE) / 2.0;
        if self.text_width(size, text) <= r.w {
            return self.text(r.x, y, size, color, text);
        }
        let (start, end) = self.fit(size, text, r.w, keep);
        let part = text.get(start..end).unwrap_or("");
        let drawn = self.text(r.x, y, size, color, part);
        match keep {
            Keep::Start => drawn + self.text(r.x + drawn, y, size, color, ELLIPSIS),
            Keep::End => drawn,
        }
    }

    /// Draws `text` word by word from `at`, wrapping within its width, at most `max_lines` lines (the
    /// last one cut at a word). Newlines in the text start a new line. Returns the height used.
    pub fn wrap(&mut self, at: Span, size: f32, color: impl Into<Color>, text: &str, max_lines: usize) -> f32 {
        let color = color.into();
        let line_h = size * WRAP_LINE;
        let space = self.text_width(size, " ");
        let (mut cx, mut line) = (at.x, 0usize);
        for (p, paragraph) in text.split('\n').enumerate() {
            if p > 0 {
                line += 1;
                cx = at.x;
            }
            for word in paragraph.split_whitespace() {
                let ww = self.text_width(size, word);
                if cx > at.x && cx + ww > at.x + at.w {
                    line += 1;
                    cx = at.x;
                }
                if line >= max_lines {
                    return line_h * max_lines as f32;
                }
                self.text(cx, at.y + line as f32 * line_h, size, color, word);
                cx += ww + space;
            }
        }
        line_h * (line + 1) as f32
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

    /// Draws one line of text with its ink (what its glyphs cover, not their advance and the line's
    /// height) centred in `r`: a letter in a badge looks centred whatever its shape.
    pub fn text_ink_centred(&mut self, r: Rect, size: f32, color: impl Into<Color>, text: &str) {
        let px = size * self.scale;
        let ascent = self.cache.ascent(px);
        // The ink's box, in pixels from the text's left and top.
        let (mut pen, mut ink) = (0.0, None::<(f32, f32, f32, f32)>);
        for ch in text.chars() {
            let Some((m, _)) = self.cache.glyph(ch, px) else {
                return;
            };
            if m.width > 0 && m.height > 0 {
                let (x0, y0) = (pen + m.xmin as f32, ascent - m.height as f32 - m.ymin as f32);
                let (x1, y1) = (x0 + m.width as f32, y0 + m.height as f32);
                ink = Some(ink.map_or((x0, y0, x1, y1), |(a, b, c, d)| (a.min(x0), b.min(y0), c.max(x1), d.max(y1))));
            }
            pen += m.advance_width;
        }
        let Some((x0, y0, x1, y1)) = ink else {
            return;
        };
        let (cx, cy) = ((x0 + x1) / 2.0 / self.scale, (y0 + y1) / 2.0 / self.scale);
        self.text(r.x + r.w / 2.0 - cx, r.y + r.h / 2.0 - cy, size, color, text);
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
/// premultiplied; within `clip` (x0, y0, x1, y1, inside the pixmap).
fn blend_mask(pixmap: &mut PixmapMut, clip: (i32, i32, i32, i32), x: i32, y: i32, width: usize, coverage: &[u8], color: PremultipliedColorU8) {
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

    #[test]
    fn cells_fill_the_rect_with_gaps_between() {
        let r = Rect::new(10.0, 20.0, 310.0, 100.0);
        assert_eq!(r.cell(3, 2, 5.0, 0), Rect::new(10.0, 20.0, 100.0, 47.5));
        assert_eq!(r.cell(3, 2, 5.0, 5), Rect::new(220.0, 72.5, 100.0, 47.5));
        let mut rest = r;
        let top = rest.take_top(30.0);
        assert_eq!((top.h, rest.y, rest.h), (30.0, 50.0, 70.0));
        let right = rest.take_right(10.0);
        assert_eq!((right.x, rest.w), (310.0, 300.0));
    }

    #[test]
    fn a_font_found_on_the_finder_thread_draws_the_character_once_back() {
        let offered = Command::new("fc-match").args(["--format=%{file}", "sans-serif:charset=3042"]).output();
        if !offered.is_ok_and(|o| o.status.success() && !o.stdout.is_empty()) {
            return;
        }
        let (requests, finder) = std::sync::mpsc::channel();
        let (done, found) = std::sync::mpsc::channel();
        spawn_font_finder(finder, move |f| done.send(f).is_ok()).unwrap();
        let mut cache = DrawCache::with_finder(requests);
        assert_eq!(cache.face('あ'), Face::Waiting);
        assert!(cache.glyph('あ', 20.0).is_none(), "not drawn while looked for");
        // (Another character the built-in font lacks: looked for too, in turn.)
        assert_eq!(cache.face('い'), Face::Waiting);
        let first = found.recv().unwrap();
        assert!(first.font.is_some());
        assert!(matches!(cache.found(first), Face::Fallback(0)));
        assert!(cache.glyph('あ', 20.0).is_some_and(|(m, _)| m.width > 0));
        // The same font for the second: not read again.
        let second = found.recv().unwrap();
        assert!(second.font.is_none());
        assert!(matches!(cache.found(second), Face::Fallback(0)));
        assert_eq!(cache.fallbacks.len(), 1);
    }

    #[test]
    fn the_built_in_font_covers_the_ui_and_others_fill_in() {
        let mut cache = DrawCache::load();
        for ch in "Aa0…·°–€éøÅßΩЖ".chars() {
            assert_eq!(cache.face(ch), Face::Builtin, "{ch:?} is in the built-in font");
            assert!(cache.glyph(ch, 20.0).is_some_and(|(m, _)| m.width > 0), "{ch:?} draws");
        }
        // A kana: from a system font, when fontconfig knows one (not every machine has one).
        let offered = Command::new("fc-match").args(["--format=%{file}", "sans-serif:charset=3042"]).output();
        if offered.is_ok_and(|o| o.status.success() && !o.stdout.is_empty()) {
            assert!(matches!(cache.face('あ'), Face::Fallback(_)), "fontconfig offers a font with あ, so it is used");
            assert!(cache.glyph('あ', 20.0).is_some_and(|(m, _)| m.width > 0));
        }
    }

    #[test]
    fn text_is_cut_to_fit() {
        let mut cache = DrawCache::load();
        let mut pixmap = Pixmap::new(400, 50).unwrap();
        let mut c = Canvas::new(pixmap.as_mut(), 1.0, &mut cache);
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
        assert!(c.text_fit(Rect::new(0.0, 0.0, width, 30.0), 20.0, rgb(0xffffff), text, Keep::Start) <= width + 0.5);
    }
}
