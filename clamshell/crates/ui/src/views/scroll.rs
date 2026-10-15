//! A scrolled list: rows of one height in an area, moved by the list's offset in the state (in rows),
//! drawn cut to the area, with a thin dim bar at its right edge while the list has more rows than fit.
//! The area itself is a target under the rows (a finger between rows drags the list too), and every
//! target drawn in it says it is in the list (see [`InList`]), cut to the area.

use clamshell_core::{Id, ListId};

use super::Frame;
use super::style::Tone;
use crate::draw::Rect;

/// A target in a scrolled list: which, and how tall its rows are (a drag of a row's height scrolls by
/// one row).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct InList {
    pub list: ListId,
    pub row_h: f32,
}

/// The scroll bar's width and its least length, in logical pixels.
const BAR_W: f32 = 3.0;
const BAR_MIN: f32 = 24.0;
/// How far past the area drawing reaches: a focus ring's room (see `style::Look`).
const RING_ROOM: f32 = 4.0;

/// A list being drawn: begun with [`Scrolled::begin`], its rows drawn where [`Scrolled::row`] says,
/// finished with [`Scrolled::end`].
pub(super) struct Scrolled {
    list: ListId,
    area: Rect,
    row_h: f32,
    len: usize,
    /// Rows scrolled past (within the ends).
    offset: f32,
    visible: f32,
    /// Where this list's targets start in the frame's.
    first_target: usize,
}

impl Scrolled {
    /// Begins `list` of `len` rows of `row_h` in `area`: reports how many rows show, puts the area's
    /// target down and keeps drawing within it.
    pub fn begin(f: &mut Frame, list: ListId, area: Rect, row_h: f32, len: usize) -> Scrolled {
        let row_h = row_h.max(1.0);
        let visible = (area.h / row_h).max(0.0);
        if let Some(slot) = f.out.lists.get_mut(list.index()) {
            *slot = Some(visible);
        }
        // (The state keeps it within the list's ends.)
        let offset = f.s.list_offset(list);
        let first_target = f.targets.len();
        if area.h > 0.0 && area.w > 0.0 {
            f.targets.push(super::Target { rect: area, id: Id::List(list), track: None, list: None });
        }
        // (A little beyond the area, so a focus ring around a row whole in it isn't cut.)
        f.c.clip(Some(area.inset(-RING_ROOM)));
        Scrolled { list, area, row_h, len, offset, visible, first_target }
    }

    /// The rows that show, at least partly: their positions.
    pub fn shown(&self) -> std::ops::Range<usize> {
        let first = self.offset.floor() as usize;
        let last = ((self.offset + self.visible).ceil() as usize).min(self.len);
        first.min(last)..last
    }

    /// Row `i`'s rect (it may reach past the area's top or bottom: drawing and its target are cut to the
    /// area).
    pub fn row(&self, i: usize) -> Rect {
        Rect::new(self.area.x, self.area.y + (i as f32 - self.offset) * self.row_h, self.area.w, self.row_h)
    }

    /// The list is drawn: drawing goes on over the whole screen; the list's targets are cut to its area
    /// (those left without any are dropped) and know their list; the scroll bar while it overflows. `bar_x`:
    /// where the bar goes (its left edge).
    pub fn end(self, f: &mut Frame, bar_x: f32) {
        f.c.clip(None);
        let area = self.area;
        let in_list = InList { list: self.list, row_h: self.row_h };
        let mut i = 0;
        f.targets.retain_mut(|t| {
            let keep = if i < self.first_target {
                true
            } else {
                t.rect = intersect(t.rect, area);
                t.list = Some(in_list);
                t.rect.w > 0.0 && t.rect.h > 0.0
            };
            i += 1;
            keep
        });
        if self.len as f32 > self.visible && self.len > 0 {
            let h = (area.h * self.visible / self.len as f32).max(BAR_MIN).min(area.h);
            let max = self.len as f32 - self.visible;
            let y = area.y + (area.h - h) * (self.offset / max).clamp(0.0, 1.0);
            f.c.rounded(Rect::new(bar_x, y, BAR_W, h), BAR_W / 2.0, Tone::Dim);
        }
    }
}

/// Where two rects overlap (empty, zero-sized, where they don't).
fn intersect(a: Rect, b: Rect) -> Rect {
    let (x0, y0) = (a.x.max(b.x), a.y.max(b.y));
    let (x1, y1) = (a.right().min(b.right()), a.bottom().min(b.bottom()));
    Rect::new(x0, y0, (x1 - x0).max(0.0), (y1 - y0).max(0.0))
}
