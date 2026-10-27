//! The canvas the views draw on. It records what they draw ([`super::record`]) rather than drawing it:
//! the player ([`super::paint`]) turns a recording into pixels. Its methods draw shapes and images, and
//! icons and text (see [`super::text`], [`super::icons`]); text is measured from the glyph cache while
//! recording. All coordinates are in logical pixels; the canvas scales them to the buffer.
//!
//! Once the caches are warm, recording a frame does no heap allocation, and a test checks this.

use std::collections::HashMap;
use std::sync::mpsc::Sender;

use tiny_skia::{Color, Path, PathBuilder};

use super::fonts::{Face, Found, Glyph, NOTO_SANS, find_font};
use super::geom::Rect;
use super::icons::Icon;
use super::images::Image;
use super::paint::RoundedRect;
use super::record::{Bounds, Cmd, Op, Px, Recording};

/// What drawing keeps between frames: the font, the fonts found for characters it lacks, the rasterized
/// glyphs by character and pixel size, and a path builder whose memory is reused for every shape.
pub struct DrawCache {
    pub(super) font: Option<fontdue::Font>,
    /// Fonts for characters the built-in one lacks, such as in a Japanese title. They are found through
    /// fontconfig when first needed. `fallback_for` records which of them has each such character, or
    /// `Face::Builtin` if none does.
    pub(super) fallbacks: Vec<(String, fontdue::Font)>,
    pub(super) fallback_for: HashMap<char, Face>,
    /// Where characters go to have a font found for them, since a font is a few MB to read and parse.
    /// `None` finds the font at once, which snapshots and tests use.
    pub(super) finder: Option<Sender<char>>,
    pub(super) glyphs: HashMap<(char, u32), Glyph>,
    /// Icon coverage masks, by icon and side in pixels.
    pub(super) icons: HashMap<(Icon, u32), Vec<u8>>,
    pub(super) path: Option<PathBuilder>,
    /// Counts the fonts found for characters. A text recorded before a font arrived for one of its
    /// characters is drawn differently after, so recordings keep it.
    pub(super) generation: u32,
}

impl DrawCache {
    pub fn load() -> DrawCache {
        let font = fontdue::Font::from_bytes(NOTO_SANS, fontdue::FontSettings::default()).map_err(|e| log::error!("the built-in font: {e}")).ok();
        DrawCache {
            font,
            fallbacks: Vec::new(),
            fallback_for: HashMap::new(),
            finder: None,
            glyphs: HashMap::new(),
            icons: HashMap::new(),
            path: None,
            generation: 0,
        }
    }

    /// Like [`DrawCache::load`], but fonts for characters the built-in one lacks are found by `finder`
    /// (see [`super::fonts::spawn_font_finder`]) and handed back through [`DrawCache::found`]. Such a
    /// character isn't drawn until then.
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

    /// The font to draw `ch` with. It is the built-in one, unless that lacks `ch` and another font has it.
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
                Some(finder) => match finder.send(ch) {
                    Ok(()) => Face::Waiting,
                    Err(e) => {
                        log::warn!("finding a font for {ch:?}: {e}");
                        Face::Builtin
                    }
                },
                None => {
                    let found = find_font(ch, &mut |path| self.fallbacks.iter().any(|(p, _)| p == path));
                    return self.found(found);
                }
            },
        };
        self.fallback_for.insert(ch, face);
        face
    }

    /// Takes what the finder found for a character. The font found is used from now on and the
    /// character is drawn with it. If no font has the character, the built-in font draws its mark for a
    /// missing character.
    pub fn found(&mut self, found: Found) -> Face {
        let Found { ch, path, font } = found;
        self.generation = self.generation.wrapping_add(1);
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
    pub(super) fn builder(&mut self) -> PathBuilder {
        self.path.take().unwrap_or_default()
    }

    /// Gives a finished path's memory back for the next shape.
    pub(super) fn recycle(&mut self, path: Path) {
        self.path = Some(path.clear());
    }
}

/// Records a frame into a [`Recording`]. Each command keeps the buffer pixels it may touch, within the
/// clip in force.
pub struct Canvas<'a> {
    rec: &'a mut Recording,
    pub(super) scale: f32,
    /// How many whole buffer pixels everything is moved by, for the burn-in orbit.
    pub(super) offset: (f32, f32),
    /// The area drawing is limited to, in buffer pixels. It is the whole buffer or a scrolled list's
    /// area.
    clip: Bounds,
    pub(super) cache: &'a mut DrawCache,
}

impl<'a> Canvas<'a> {
    /// Starts recording a frame of `size` buffer pixels into `rec`, which is emptied first.
    pub fn new(rec: &'a mut Recording, size: (u32, u32), scale: f32, cache: &'a mut DrawCache) -> Self {
        rec.start(size, scale, (0, 0));
        Canvas { rec, scale, offset: (0.0, 0.0), clip: Bounds::of_size(size), cache }
    }

    /// Moves everything drawn by whole buffer pixels.
    pub fn with_offset(mut self, (dx, dy): (i32, i32)) -> Self {
        self.rec.offset = (dx, dy);
        self.offset = (dx as f32, dy as f32);
        self
    }

    /// Keeps drawing within `r` from now on. `None` allows the whole buffer again.
    pub fn clip(&mut self, r: Option<Rect>) {
        let all = Bounds::of_size(self.rec.size);
        self.clip = match r {
            Some(r) => Bounds::new(
                (self.px(r.x).floor() as i32).clamp(0, all.x1),
                (self.py(r.y).floor() as i32).clamp(0, all.y1),
                (self.px(r.x + r.w).ceil() as i32).clamp(0, all.x1),
                (self.py(r.y + r.h).ceil() as i32).clamp(0, all.y1),
            ),
            None => all,
        };
        self.rec.clips.push((self.rec.cmds.len(), self.clip));
    }

    /// Buffer pixels per logical pixel.
    pub fn scale(&self) -> f32 {
        self.scale
    }

    /// The buffer's size in pixels.
    pub fn buffer_size(&self) -> (u32, u32) {
        self.rec.size
    }

    /// `r`, given in buffer pixels, in logical pixels, pinned there: unmoved by the burn-in offset, for what
    /// must line up with what the compositor shows on the screen, such as the overview's cells.
    pub fn pinned(&self, r: Rect) -> Rect {
        Rect::new((r.x - self.offset.0) / self.scale, (r.y - self.offset.1) / self.scale, r.w / self.scale, r.h / self.scale)
    }

    /// Converts logical x and y to buffer pixels.
    pub(super) fn px(&self, x: f32) -> f32 {
        x * self.scale + self.offset.0
    }

    pub(super) fn py(&self, y: f32) -> f32 {
        y * self.scale + self.offset.1
    }

    /// Records `op`, which touches `bounds` at most. A command clipped away entirely is left out.
    fn record(&mut self, op: Op, bounds: Bounds) {
        let bounds = bounds.intersect(self.clip);
        if !bounds.is_empty() {
            self.rec.cmds.push(Cmd { op, bounds });
        }
    }

    /// Records `op`, whose shape covers `extent`, as (x0, y0, x1, y1) in buffer pixels.
    fn shape(&mut self, op: Op, extent: (f32, f32, f32, f32)) {
        self.record(op, Bounds::around(extent));
    }

    /// Records a line of text with its glyphs' pixels.
    pub(super) fn record_text(&mut self, op: Op, text: &str, ink: Bounds) {
        let Op::Text { x, y, size, color, fonts, .. } = op else {
            return;
        };
        let (start, len) = (self.rec.text.len(), text.len());
        let (Ok(start), Ok(len)) = (u32::try_from(start), u32::try_from(len)) else {
            return;
        };
        let bounds = Bounds::new(ink.x0 - 1, ink.y0 - 1, ink.x1 + 1, ink.y1 + 1).intersect(self.clip);
        if !ink.is_empty() && !bounds.is_empty() {
            self.rec.text.extend_from_slice(text.as_bytes());
            self.rec.cmds.push(Cmd { op: Op::Text { x, y, size, color, start, len, fonts }, bounds });
        }
    }

    /// Records an icon drawn in a `side` pixel square at (x, y).
    pub(super) fn record_icon(&mut self, icon: Icon, r: Rect, color: Color, (x, y): (i32, i32), side: i32) {
        self.record(Op::Icon { icon, r: Px::rect(r), color: color.into() }, Bounds::new(x - 1, y - 1, x + side + 1, y + side + 1));
    }

    /// The whole buffer in `color`, under everything drawn after.
    pub fn clear(&mut self, color: impl Into<Color>) {
        let all = Bounds::of_size(self.rec.size);
        self.record(Op::Clear(color.into().into()), all);
    }

    /// A vertical line `px` buffer pixels wide, centred on `x` and snapped to whole pixels so that it is
    /// sharp.
    pub fn vline(&mut self, x: f32, y0: f32, y1: f32, px: u32, color: impl Into<Color>) {
        let left = (self.px(x) - px as f32 / 2.0).round();
        let (top, bottom) = (self.py(y0).round(), self.py(y1).round());
        let op = Op::VLine { x: Px::new(x), y0: Px::new(y0), y1: Px::new(y1), px, color: color.into().into() };
        self.shape(op, (left, top, left + px as f32, bottom));
    }

    /// A horizontal line `px` buffer pixels high, centred on `y` and snapped to whole pixels.
    pub fn hline(&mut self, x0: f32, x1: f32, y: f32, px: u32, color: impl Into<Color>) {
        let top = (self.py(y) - px as f32 / 2.0).round();
        let (left, right) = (self.px(x0).round(), self.px(x1).round());
        let op = Op::HLine { x0: Px::new(x0), x1: Px::new(x1), y: Px::new(y), px, color: color.into().into() };
        self.shape(op, (left, top, right, top + px as f32));
    }

    pub fn fill(&mut self, r: Rect, color: impl Into<Color>) {
        let (x0, y0) = (self.px(r.x), self.py(r.y));
        self.shape(Op::Fill { r: Px::rect(r), color: color.into().into() }, (x0, y0, x0 + r.w * self.scale, y0 + r.h * self.scale));
    }

    /// A rounded rectangle, filled.
    pub fn rounded(&mut self, r: Rect, radius: f32, color: impl Into<Color>) {
        let bounds = RoundedRect::new(r, radius, self.scale, self.offset).bounds(1.0);
        self.shape(Op::Rounded { r: Px::rect(r), radius: Px::new(radius), color: color.into().into() }, bounds);
    }

    /// A rounded rectangle's outline, `width` wide, centred on its edge.
    pub fn outline(&mut self, r: Rect, radius: f32, width: f32, color: impl Into<Color>) {
        let bounds = RoundedRect::new(r, radius, self.scale, self.offset).bounds(width * self.scale / 2.0 + 1.0);
        self.shape(Op::Outline { r: Px::rect(r), radius: Px::new(radius), width: Px::new(width), color: color.into().into() }, bounds);
    }

    pub fn circle(&mut self, cx: f32, cy: f32, r: f32, color: impl Into<Color>) {
        let (x, y, rr) = (self.px(cx), self.py(cy), r * self.scale);
        let op = Op::Circle { cx: Px::new(cx), cy: Px::new(cy), r: Px::new(r), color: color.into().into() };
        self.shape(op, (x - rr - 1.0, y - rr - 1.0, x + rr + 1.0, y + rr + 1.0));
    }

    /// A straight line from (x0, y0) to (x1, y1), `width` wide with round ends. It can be at any angle,
    /// is antialiased, and is computed per pixel without a path.
    pub fn line(&mut self, (x0, y0): (f32, f32), (x1, y1): (f32, f32), width: f32, color: impl Into<Color>) {
        let (ax, ay, bx, by) = (self.px(x0), self.py(y0), self.px(x1), self.py(y1));
        let half = width * self.scale / 2.0;
        let op = Op::Line { a: [Px::new(x0), Px::new(y0)], b: [Px::new(x1), Px::new(y1)], width: Px::new(width), color: color.into().into() };
        self.shape(op, (ax.min(bx) - half - 1.0, ay.min(by) - half - 1.0, ax.max(bx) + half + 1.0, ay.max(by) + half + 1.0));
    }

    /// Draws `image`, already at its pixel size, with its top left at (x, y), blending its alpha.
    pub fn image(&mut self, image: Image, x: f32, y: f32) {
        self.blit(image, x, y, None);
    }

    /// Draws `image` in grey at `level` (0..1) of its brightness, as for an idle app's icon.
    pub fn image_grey(&mut self, image: Image, x: f32, y: f32, level: f32) {
        self.blit(image, x, y, Some((level.clamp(0.0, 1.0) * 256.0) as u32));
    }

    fn blit(&mut self, image: Image, x: f32, y: f32, grey: Option<u32>) {
        let (x0, y0) = (self.px(x).round() as i32, self.py(y).round() as i32);
        let (w, h) = (i32::try_from(image.width()).unwrap_or(0), i32::try_from(image.height()).unwrap_or(0));
        self.record(Op::Image { image, x: Px::new(x), y: Px::new(y), grey }, Bounds::new(x0, y0, x0 + w, y0 + h));
    }
}
