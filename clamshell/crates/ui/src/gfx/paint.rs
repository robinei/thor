//! Turns recordings into pixels. The painter draws shapes, images, icons and text into a pixmap, within a
//! clip; [`play`] draws a recording's commands with it, or those of them that meet a damaged area.
//!
//! Once the caches are warm, drawing a frame does no heap allocation, and a test checks this. Glyphs and
//! icons are rasterized once and blended straight into the pixmap as coverage masks. Rectangles, rounded
//! rectangles, circles and lines are computed per pixel from their coverage, with no path at all. Only the
//! strokes drawn into the icon cache go through tiny-skia.
//!
//! The pixels are drawn into a picture in ordinary memory, the screen's way up and red first; a display
//! buffer laid out otherwise (a panel mounted sideways, blue first) is brought up to date by copying from
//! it (see the compositor's software scanout).

use tiny_skia::{Color, LineCap, LineJoin, Paint, Path, Pixmap, PixmapMut, PremultipliedColorU8, Stroke, Transform};

use super::canvas::DrawCache;
use super::geom::Rect;
use super::images::Images;
use super::record::{Bounds, Op, Px, Recording};

/// The pixels a painter draws into, premultiplied, red first: `width` by `height`, rows one after another.
pub(crate) struct Pixels<'a> {
    data: &'a mut [u8],
    width: u32,
    height: u32,
}

impl<'a> From<&'a mut Pixmap> for Pixels<'a> {
    fn from(pixmap: &'a mut Pixmap) -> Self {
        let (width, height) = (pixmap.width(), pixmap.height());
        Pixels { data: pixmap.data_mut(), width, height }
    }
}

impl Pixels<'_> {
    fn width(&self) -> u32 {
        self.width
    }

    fn height(&self) -> u32 {
        self.height
    }

    /// Row `y` from `x` on, `len` pixels, as one slice; none if it runs outside the pixels (which callers'
    /// clipping rules out).
    fn row(&mut self, y: i32, x: i32, len: usize) -> Option<&mut [[u8; 4]]> {
        let from = (usize::try_from(y).ok()? * self.width as usize + usize::try_from(x).ok()?) * 4;
        self.data.get_mut(from..from + len * 4).map(|row| row.as_chunks_mut::<4>().0)
    }

    /// Calls `f` on each pixel of row `y` from `x` on (inside the pixels) with the item of `src` that goes
    /// there, one item a pixel. It walks the row as one slice, which the compiler vectorises.
    fn zip<T: Copy>(&mut self, y: i32, x: i32, src: impl IntoIterator<Item = T, IntoIter: ExactSizeIterator>, mut f: impl FnMut(&mut [u8; 4], T)) {
        let src = src.into_iter();
        if let Some(row) = self.row(y, x, src.len()) {
            for (px, item) in row.iter_mut().zip(src) {
                f(px, item);
            }
        }
    }

    /// Calls `f` on each pixel of row `y` from `x0` to `x1` (inside the pixels), with its x.
    fn run(&mut self, y: i32, x0: i32, x1: i32, mut f: impl FnMut(&mut [u8; 4], i32)) {
        self.zip(y, x0, x0..x1.max(x0), |px, x| f(px, x));
    }

    /// Row `y` from `x0` to `x1` set to `word`.
    fn fill_run(&mut self, y: i32, x0: i32, x1: i32, word: u32) {
        if let Some(row) = self.row(y, x0, (x1 - x0).max(0) as usize) {
            row.fill(word.to_le_bytes());
        }
    }

    /// Row `y` from `x` set to the pixels of `src`, as many as it has.
    fn copy_run(&mut self, y: i32, x: i32, src: &[u8]) {
        let pixels = src.as_chunks::<4>().0;
        if let Some(row) = self.row(y, x, pixels.len()) {
            row.copy_from_slice(pixels);
        }
    }

    /// The target as a tiny-skia pixmap.
    fn pixmap(&mut self) -> Option<PixmapMut<'_>> {
        PixmapMut::from_bytes(self.data, self.width, self.height)
    }
}

/// A premultiplied pixel packed into a little-endian word, so that alpha is the top byte.
fn word(rgba: [u8; 4]) -> u32 {
    u32::from_le_bytes(rgba)
}

/// `color` premultiplied, as a pixel's word.
fn color_word(color: Color) -> u32 {
    let c = color.premultiply().to_color_u8();
    word([c.red(), c.green(), c.blue(), c.alpha()])
}

/// Draws the commands of `rec` that meet `damage` into `target`, in order and within `damage`. The pixels
/// outside `damage` are left as they are.
pub fn play<'a>(target: impl Into<Pixels<'a>>, rec: &Recording, damage: Bounds, cache: &mut DrawCache, images: &Images) {
    let mut p = Painter::new(target.into(), rec.scale.get(), rec.offset, cache);
    for cmd in &rec.cmds {
        let clip = cmd.bounds.intersect(damage);
        if clip.is_empty() {
            continue;
        }
        p.clip_to(clip);
        draw(&mut p, rec, cmd.op, images);
    }
}

/// Draws all of `rec` into `pixmap`, each command within the clip in force when it was recorded but not
/// within its bounds. It is what the commands draw, to check against: it equals what [`play`] draws only
/// if every command's bounds hold all of its pixels.
#[cfg(any(test, feature = "dev"))]
pub fn play_unbounded<'a>(target: impl Into<Pixels<'a>>, rec: &Recording, cache: &mut DrawCache, images: &Images) {
    let mut p = Painter::new(target.into(), rec.scale.get(), rec.offset, cache);
    let all = Bounds::of_size(rec.size);
    let mut clips = rec.clips.iter().peekable();
    let mut clip = all;
    for (i, cmd) in rec.cmds.iter().enumerate() {
        while let Some(&&(from, c)) = clips.peek() {
            if from > i {
                break;
            }
            clip = c;
            clips.next();
        }
        p.clip_to(clip);
        draw(&mut p, rec, cmd.op, images);
    }
}

/// Draws one command of `rec` with `p`, within the clip `p` has.
fn draw(p: &mut Painter, rec: &Recording, op: Op, images: &Images) {
    let c = Color::from;
    match op {
        Op::Clear(color) => p.clear(c(color)),
        Op::Fill { r, color } => p.fill(Px::to_rect(r), c(color)),
        Op::HLine { x0, x1, y, px, color } => p.hline(x0.get(), x1.get(), y.get(), px, c(color)),
        Op::VLine { x, y0, y1, px, color } => p.vline(x.get(), y0.get(), y1.get(), px, c(color)),
        Op::Rounded { r, radius, color } => p.rounded(Px::to_rect(r), radius.get(), c(color)),
        Op::Outline { r, radius, width, color } => p.outline(Px::to_rect(r), radius.get(), width.get(), c(color)),
        Op::Circle { cx, cy, r, color } => p.circle(cx.get(), cy.get(), r.get(), c(color)),
        Op::Line { a: [ax, ay], b: [bx, by], width, color } => p.line((ax.get(), ay.get()), (bx.get(), by.get()), width.get(), c(color)),
        Op::Image { image, x, y, grey } => {
            if let Some(pixmap) = images.pixmap(image) {
                p.blit(pixmap, x.get(), y.get(), grey);
            }
        }
        Op::Icon { icon, r, color } => p.icon(icon, Px::to_rect(r), c(color)),
        Op::Text { x, y, size, color, .. } => {
            if let Ok(text) = std::str::from_utf8(rec.text_of(&op)) {
                p.text(x.get(), y.get(), size.get(), c(color), text);
            }
        }
    }
}

/// Draws into a target. It is the player's, and draws icons into their cache.
pub struct Painter<'a> {
    pub(super) target: Pixels<'a>,
    pub(super) scale: f32,
    /// How many whole buffer pixels everything is moved by, for the burn-in orbit.
    pub(super) offset: (f32, f32),
    /// The area drawing is limited to, as (x0, y0, x1, y1) in buffer pixels: the area being drawn, within
    /// a scrolled list's while one draws.
    pub(super) clip: (i32, i32, i32, i32),
    pub(super) cache: &'a mut DrawCache,
}

impl<'a> Painter<'a> {
    pub(crate) fn new(target: Pixels<'a>, scale: f32, (dx, dy): (i32, i32), cache: &'a mut DrawCache) -> Self {
        let clip = (0, 0, target.width() as i32, target.height() as i32);
        Painter { target, scale, offset: (dx as f32, dy as f32), clip, cache }
    }

    /// Keeps drawing within `clip`, which must be inside the target.
    fn clip_to(&mut self, clip: Bounds) {
        self.clip = clip.intersect(Bounds::of_size((self.target.width(), self.target.height()))).tuple();
    }

    /// `color` over the pixels in the clip, replacing them.
    fn clear(&mut self, color: Color) {
        let word = color_word(color);
        let (x0, y0, x1, y1) = self.clip;
        for y in y0.max(0)..y1.max(0) {
            self.target.fill_run(y, x0.max(0), x1, word);
        }
    }

    /// Converts logical x and y to buffer pixels.
    pub(super) fn px(&self, x: f32) -> f32 {
        x * self.scale + self.offset.0
    }

    pub(super) fn py(&self, y: f32) -> f32 {
        y * self.scale + self.offset.1
    }

    /// A vertical line `px` buffer pixels wide, centred on `x` and snapped to whole pixels so that it is
    /// sharp.
    pub fn vline(&mut self, x: f32, y0: f32, y1: f32, px: u32, color: impl Into<Color>) {
        let left = (self.px(x) - px as f32 / 2.0).round();
        let (top, bottom) = (self.py(y0).round(), self.py(y1).round());
        self.rect((left, top, left + px as f32, bottom), color.into());
    }

    /// A horizontal line `px` buffer pixels high, centred on `y` and snapped to whole pixels.
    pub fn hline(&mut self, x0: f32, x1: f32, y: f32, px: u32, color: impl Into<Color>) {
        let top = (self.py(y) - px as f32 / 2.0).round();
        let (left, right) = (self.px(x0).round(), self.px(x1).round());
        self.rect((left, top, right, top + px as f32), color.into());
    }

    pub fn fill(&mut self, r: Rect, color: impl Into<Color>) {
        let (x0, y0) = (self.px(r.x), self.py(r.y));
        self.rect((x0, y0, x0 + r.w * self.scale, y0 + r.h * self.scale), color.into());
    }

    /// The rectangle (x0, y0, x1, y1) in buffer pixels, each pixel covered by as much of it as it holds.
    fn rect(&mut self, (x0, y0, x1, y1): (f32, f32, f32, f32), color: Color) {
        let overlap = |a: f32, from: f32, to: f32| (to.min(a + 0.5) - from.max(a - 0.5)).clamp(0.0, 1.0);
        self.shade((x0, y0, x1, y1), color, Runs::One, |x, y| overlap(x, x0, x1) * overlap(y, y0, y1));
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

    /// Blends `color` over the pixels in `bounds`, given as (x0, y0, x1, y1) in buffer pixels, with the
    /// coverage that `coverage` gives for each pixel centre. With [`Runs::One`], each row is asked for
    /// coverage from either end only up to its first fully covered pixel, and the pixels between are
    /// filled.
    fn shade(&mut self, (x0, y0, x1, y1): (f32, f32, f32, f32), color: Color, runs: Runs, coverage: impl Fn(f32, f32) -> f32) {
        let (cx0, cy0, cx1, cy1) = self.clip;
        let (x0, y0) = ((x0.floor() as i32).max(cx0), (y0.floor() as i32).max(cy0));
        let (x1, y1) = ((x1.ceil() as i32).min(cx1), (y1.ceil() as i32).min(cy1));
        let src = color_word(color);
        let k_at = |x: i32, y: i32| (coverage(x as f32 + 0.5, y as f32 + 0.5) * 255.0) as u32;
        let target = &mut self.target;
        for y in y0..y1 {
            let (mut from, mut to) = (x0, x1);
            if runs == Runs::One {
                while from < to
                    && let k = k_at(from, y)
                    && k < 255
                {
                    blend_at(target, from, y, src, k);
                    from += 1;
                }
                while to > from
                    && let k = k_at(to - 1, y)
                    && k < 255
                {
                    blend_at(target, to - 1, y, src, k);
                    to -= 1;
                }
                if src >> 24 == 255 {
                    target.fill_run(y, from, to, src);
                    continue;
                }
            }
            target.run(y, from, to, |px, x| {
                let k = k_at(x, y);
                if k > 0 {
                    blend(px, src, k);
                }
            });
        }
    }

    /// A straight line from (x0, y0) to (x1, y1), `width` wide with round ends. It can be at any angle,
    /// is antialiased, and is computed per pixel without a path.
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

    /// Blends the premultiplied `image` with its top left at (x, y). With `grey`, the image is drawn as
    /// its luminance scaled by `grey`/256.
    fn blit(&mut self, image: &Pixmap, x: f32, y: f32, grey: Option<u32>) {
        let (x0, y0) = (self.px(x).round() as i32, self.py(y).round() as i32);
        let clip = self.clip;
        let iw = image.width() as usize;
        let Some((skip, x, len)) = span(clip, x0, iw) else {
            return;
        };
        for (row, src_row) in image.data().chunks_exact(iw * 4).enumerate() {
            let y = y0 + row as i32;
            let Some(src) = src_row.get(skip * 4..(skip + len) * 4).filter(|_| (clip.1..clip.3).contains(&y)) else {
                continue;
            };
            let pixels = src.as_chunks::<4>().0;
            match grey {
                // Rows without transparency, as in box art and screenshots, are copied whole.
                None if pixels.iter().fold(255, |all, px| all & px[3]) == 255 => self.target.copy_run(y, x, src),
                None => self.target.zip(y, x, pixels.iter(), |d, s| blend(d, word(*s), 255)),
                Some(level) => self.target.zip(y, x, pixels.iter(), |d, &[r, g, b, a]| {
                    let luma = ((u32::from(r) * 54 + u32::from(g) * 183 + u32::from(b) * 19) / 256 * level / 256) as u8;
                    blend(d, u32::from_le_bytes([luma, luma, luma, a]), 255);
                }),
            }
        }
    }

    /// Connected lines through `points`, with round caps and joins. It uses tiny-skia, which allocates,
    /// so it is only for drawing icons into their cache.
    pub fn polyline(&mut self, points: &[(f32, f32)], width: f32, color: Color) {
        self.stroke_points(points, width, color, false);
    }

    /// [`Painter::polyline`] with the last point joined back to the first.
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

    /// Strokes `path` with tiny-skia: only the icon cache draws strokes.
    fn stroke_path(&mut self, path: Path, stroke: &Stroke, color: Color) {
        let mut paint = Paint::default();
        paint.set_color(color);
        paint.anti_alias = true;
        match self.target.pixmap() {
            Some(mut pixmap) => pixmap.stroke_path(&path, &paint, stroke, Transform::identity(), None),
            None => log::warn!("paint: a stroke into pixels tiny-skia can't take (none wide or high) is left out"),
        }
        self.cache.recycle(path);
    }
}

/// Blends a glyph's coverage, `width` bytes per row, in `color` onto `target` at (x, y). It blends
/// source-over with premultiplied alpha, within `clip` (x0, y0, x1, y1, inside the target).
pub(super) fn blend_mask(target: &mut Pixels, clip: (i32, i32, i32, i32), x: i32, y: i32, width: usize, coverage: &[u8], color: PremultipliedColorU8) {
    if width == 0 {
        return;
    }
    let Some((skip, x, len)) = span(clip, x, width) else {
        return;
    };
    let src = word([color.red(), color.green(), color.blue(), color.alpha()]);
    for (gy, row) in coverage.chunks_exact(width).enumerate() {
        let ty = y + gy as i32;
        let Some(cov) = row.get(skip..skip + len).filter(|_| (clip.1..clip.3).contains(&ty)) else {
            continue;
        };
        // Without a branch: coverage 0 leaves a pixel as it is, full coverage of an opaque colour sets it.
        target.zip(ty, x, cov.iter(), |px, &k| blend(px, src, u32::from(k)));
    }
}

/// The part of a row `width` pixels long, starting at `x`, that is inside `clip`'s columns. Returns how
/// many pixels it skips, where it starts and how long it is.
fn span((cx0, _, cx1, _): (i32, i32, i32, i32), x: i32, width: usize) -> Option<(usize, i32, usize)> {
    let (from, to) = (x.max(cx0), (x + width as i32).min(cx1));
    (to > from).then(|| ((from - x) as usize, from, (to - from) as usize))
}

/// A rounded rectangle in buffer pixels, used for its signed distance, which is negative inside.
pub(super) struct RoundedRect {
    cx: f32,
    cy: f32,
    /// Half the size minus the radius. This is the inner rectangle the corners are centred on.
    hx: f32,
    hy: f32,
    radius: f32,
}

impl RoundedRect {
    pub(super) fn new(r: Rect, radius: f32, scale: f32, (dx, dy): (f32, f32)) -> Self {
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
    pub(super) fn bounds(&self, margin: f32) -> (f32, f32, f32, f32) {
        let (ex, ey) = (self.hx + self.radius + margin, self.hy + self.radius + margin);
        (self.cx - ex, self.cy - ey, self.cx + ex, self.cy + ey)
    }
}

/// How a shape covers each row of pixels. A filled convex shape, such as a rounded rectangle, a circle or
/// a line, covers it in one run. An outline may cover it any way.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Runs {
    One,
    Any,
}

/// [`blend`] at pixel (x, y) of `target`, if it is covered.
fn blend_at(target: &mut Pixels, x: i32, y: i32, src: u32, k: u32) {
    if k > 0
        && let Some([px]) = target.row(y, x, 1)
    {
        blend(px, src, k);
    }
}

/// Source-over of the premultiplied word `src` (see [`word`]) at coverage `k` (0-255) onto one
/// pixel; at full coverage, plain source-over.
fn blend(px: &mut [u8; 4], src: u32, k: u32) {
    // Two channels to a word, each in a 16-bit lane: bytes 0 and 2, then 1 and 3. A lane's product of
    // two bytes, and its division by 255 rounded, stay within the lane, so this is the per-channel blend.
    const LANES: u32 = 0x00ff_00ff;
    let div = |x: u32| {
        let x = x + 0x0080_0080;
        ((x + ((x >> 8) & LANES)) >> 8) & LANES
    };
    let s = src;
    let d = u32::from_le_bytes(*px);
    let inverse = 255 - u32::from(div255(((s >> 24) * k) as u16));
    let even = div((s & LANES) * k) + div((d & LANES) * inverse);
    let odd = div((s >> 8 & LANES) * k) + div((d >> 8 & LANES) * inverse);
    *px = (even | odd << 8).to_le_bytes();
}

/// `x / 255` rounded, for `x` up to 255 * 255, done with shifts so that it vectorises.
fn div255(x: u16) -> u16 {
    let x = x + 128;
    (x + (x >> 8)) >> 8
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    /// The blend, two channels to a word, is source-over per channel: for every alpha and coverage, each of
    /// the four bytes (both lanes) as the per-channel formula gives it, within its three roundings (the
    /// source's term, the destination's, and the coverage of alpha it is scaled by), half a level each.
    #[test]
    fn blend_is_source_over_per_channel() {
        for a in 0..=255u32 {
            for k in (0..=255u32).step_by(3).chain([255]) {
                for d in [0u32, 1, 77, 128, 254, 255] {
                    // Premultiplied: each channel at most alpha, a different one in each byte.
                    let src = [a, a / 2, a / 3, a];
                    let mut px = [d as u8, (255 - d) as u8, (d / 2) as u8, d as u8];
                    let before = px;
                    blend(&mut px, u32::from_le_bytes(src.map(|c| c as u8)), k);
                    let inverse = 255.0 - (a * k) as f32 / 255.0;
                    for (c, ((got, s), d)) in px.iter().zip(src).zip(before).enumerate() {
                        let want = (s * k) as f32 / 255.0 + f32::from(d) * inverse / 255.0;
                        assert!((f32::from(*got) - want).abs() <= 1.5, "a {a} k {k} d {d}: byte {c} is {got}, want {want}");
                    }
                }
            }
        }
    }

    #[test]
    fn div255_divides() {
        for x in 0..=255 * 255 {
            assert_eq!(div255(x), (x + 127) / 255, "{x}");
        }
    }
}
