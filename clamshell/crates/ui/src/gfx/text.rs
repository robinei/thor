//! Text on the canvas. It is drawn as a line, in a rect, cut to fit or wrapped, and can be measured. The
//! glyphs come from the cache: the canvas measures with their metrics and records each line, and the
//! painter blends them.

use tiny_skia::Color;

use super::canvas::Canvas;
use super::geom::{Align, Rect, Span};
use super::paint::{Painter, blend_mask};
use super::record::{Bounds, Op, Px};

/// The height of a line of text, as a multiple of its size: Noto Sans' ascent and descent, rounded up.
pub const LINE: f32 = 1.2;

/// Which part of a text [`Canvas::text_fit`] keeps when it is too wide.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Keep {
    /// The start, with "…" where it is cut.
    Start,
    /// The end, which is what was typed last.
    End,
}

const ELLIPSIS: &str = "…";

/// The distance between wrapped lines, as a multiple of the text's size. It is a little more than
/// [`LINE`], for easier reading.
pub const WRAP_LINE: f32 = 1.3;

impl Painter<'_> {
    /// Draws one line of text with its top at `y`. A character waiting for a font ends it.
    pub fn text(&mut self, x: f32, y: f32, size: f32, color: impl Into<Color>, text: &str) {
        let px = size * self.scale;
        let baseline = (self.py(y) + self.cache.ascent(px)).round();
        let mut pen = self.px(x);
        let color = color.into().premultiply().to_color_u8();
        for ch in text.chars() {
            let Some((metrics, coverage)) = self.cache.glyph(ch, px) else {
                return;
            };
            let gx = (pen + metrics.xmin as f32).round() as i32;
            let gy = (baseline - metrics.height as f32 - metrics.ymin as f32) as i32;
            blend_mask(&mut self.target, self.clip, gx, gy, metrics.width, coverage, color);
            pen += metrics.advance_width;
        }
    }
}

impl Canvas<'_> {
    /// Draws one line of text with its top at `y`; returns its width, or 0 if a character in it is
    /// waiting for a font. The glyphs drawn are where the painter puts them (see [`Painter::text`]).
    pub fn text(&mut self, x: f32, y: f32, size: f32, color: impl Into<Color>, text: &str) -> f32 {
        let px = size * self.scale;
        let baseline = (self.py(y) + self.cache.ascent(px)).round();
        let start = self.px(x);
        let mut pen = start;
        let mut ink = Bounds::EMPTY;
        let mut waiting = false;
        for ch in text.chars() {
            let Some((metrics, _)) = self.cache.glyph(ch, px) else {
                waiting = true;
                break;
            };
            if metrics.width > 0 && metrics.height > 0 {
                let gx = (pen + metrics.xmin as f32).round() as i32;
                let gy = (baseline - metrics.height as f32 - metrics.ymin as f32) as i32;
                ink = ink.union(Bounds::new(gx, gy, gx + metrics.width as i32, gy + metrics.height as i32));
            }
            pen += metrics.advance_width;
        }
        let op = Op::Text { x: Px::new(x), y: Px::new(y), size: Px::new(size), color: color.into().into(), start: 0, len: 0, fonts: self.cache.generation };
        self.record_text(op, text, ink);
        if waiting { 0.0 } else { (pen - start) / self.scale }
    }

    /// Draws one line of text centred vertically in `r` and placed horizontally as `align` says. The
    /// text is not cut; for that, see [`Canvas::text_fit`]. Returns its width.
    pub fn text_in(&mut self, r: Rect, size: f32, color: impl Into<Color>, text: &str, align: Align) -> f32 {
        let y = r.y + (r.h - size * LINE) / 2.0;
        let x = match align {
            Align::Start => r.x,
            Align::Center => r.x + (r.w - self.text_width(size, text)) / 2.0,
            Align::End => r.x + r.w - self.text_width(size, text),
        };
        self.text(x, y, size, color, text)
    }

    /// Draws one line of text from `r`'s left, centred vertically. If the text is wider than `r`, it is
    /// cut to fit as `keep` says. Returns the width drawn.
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

    /// Draws `text` word by word from `at`, wrapping within its width, in at most `max_lines` lines. The
    /// last line is cut at a word. Newlines in the text start a new line. Returns the height used.
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

    /// Draws one line of text with its ink centred in `r`. The ink is what the glyphs cover, rather than
    /// their advance and the line's height, so a letter in a badge looks centred whatever its shape.
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

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    use crate::gfx::DrawCache;
    use crate::gfx::color::rgb;
    use crate::gfx::fonts::{Face, spawn_font_finder};
    use crate::gfx::record::Recording;
    use std::process::Command;

    /// Whether a font with あ is installed. `fc-match` always names its closest font, even one without the
    /// character, so `fc-list` is asked instead: it lists only fonts that have it.
    fn kana_font_installed() -> bool {
        Command::new("fc-list").args([":charset=3042", "file"]).output().is_ok_and(|o| o.status.success() && !o.stdout.is_empty())
    }

    #[test]
    fn a_font_found_on_the_finder_thread_draws_the_character_once_back() {
        if !kana_font_installed() {
            return;
        }
        let (requests, finder) = std::sync::mpsc::channel();
        let (done, found) = std::sync::mpsc::channel();
        spawn_font_finder(finder, move |f| done.send(f).is_ok()).unwrap();
        let mut cache = DrawCache::with_finder(requests);
        assert_eq!(cache.face('あ'), Face::Waiting);
        assert!(cache.glyph('あ', 20.0).is_none(), "not drawn while looked for");
        // Another character the built-in font lacks is looked for too, in turn.
        assert_eq!(cache.face('い'), Face::Waiting);
        let first = found.recv().unwrap();
        assert!(first.font.is_some());
        assert!(matches!(cache.found(first), Face::Fallback(0)));
        assert!(cache.glyph('あ', 20.0).is_some_and(|(m, _)| m.width > 0));
        // The second character gets the same font, which is not read again.
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
        // A kana comes from a system font if fontconfig knows one. Not every machine has one.
        if kana_font_installed() {
            assert!(matches!(cache.face('あ'), Face::Fallback(_)), "fontconfig offers a font with あ, so it is used");
            assert!(cache.glyph('あ', 20.0).is_some_and(|(m, _)| m.width > 0));
        }
    }

    #[test]
    fn text_is_cut_to_fit() {
        let mut cache = DrawCache::load();
        let mut rec = Recording::default();
        let mut c = Canvas::new(&mut rec, (400, 50), 1.0, &mut cache);
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
