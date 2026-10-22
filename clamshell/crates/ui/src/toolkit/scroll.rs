//! A scrolled list. Its rows all have one height and are drawn in an area, moved by the list's offset in
//! the state (counted in rows) and cut to the area. A thin dim bar shows at the right edge while the list
//! has more rows than fit.
//!
//! The area itself is a target under the rows, so a finger between rows drags the list too. Every target
//! drawn in it is cut to the area and marked as being in the list (see [`InList`]).

use clamshell_core::{Id, ListId};

use super::Frame;
use super::style::Tone;
use crate::gfx::Rect;

/// Marks a target as being in a scrolled list. It records which list and how tall its rows are, since a
/// drag of one row's height scrolls by one row.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct InList {
    pub list: ListId,
    pub row_h: f32,
}

/// The scroll bar's width and its least length, in logical pixels.
const BAR_W: f32 = 3.0;
const BAR_MIN: f32 = 24.0;
/// How far past the area drawing reaches, to leave room for a focus ring (see `style::Look`).
const RING_ROOM: f32 = 4.0;

/// A list being drawn. It is begun with [`Scrolled::begin`], its rows are drawn where [`Scrolled::row`]
/// says, and it is finished with [`Scrolled::end`].
pub(crate) struct Scrolled {
    list: ListId,
    area: Rect,
    row_h: f32,
    len: usize,
    /// How many rows are scrolled past, kept within the list's ends.
    offset: f32,
    visible: f32,
    /// The index in the frame's targets where this list's targets start.
    first_target: usize,
}

impl Scrolled {
    /// Begins `list`, with `len` rows of height `row_h`, in `area`. It reports how many rows show, adds
    /// the area's target and limits drawing to the area.
    pub fn begin(f: &mut Frame, list: ListId, area: Rect, row_h: f32, len: usize) -> Scrolled {
        let row_h = row_h.max(1.0);
        let visible = (area.h / row_h).max(0.0);
        if let Some(slot) = f.out.lists.get_mut(list.index()) {
            *slot = Some(visible);
        }
        // The state keeps it within the list's ends.
        let offset = f.s.scroll(list).offset();
        let first_target = f.targets.len();
        if area.h > 0.0 && area.w > 0.0 {
            f.targets.push(super::Target { rect: area, id: Id::List(list), track: None, list: None });
        }
        // Clip a little beyond the area, so the focus ring around a row that fits in it isn't cut.
        f.c.clip(Some(area.inset(-RING_ROOM)));
        Scrolled { list, area, row_h, len, offset, visible, first_target }
    }

    /// The positions of the rows that show, at least partly.
    pub fn shown(&self) -> std::ops::Range<usize> {
        let first = self.offset.floor() as usize;
        let last = ((self.offset + self.visible).ceil() as usize).min(self.len);
        first.min(last)..last
    }

    /// Row `i`'s rect. It may reach past the area's top or bottom; drawing and the row's target are cut
    /// to the area.
    pub fn row(&self, i: usize) -> Rect {
        Rect::new(self.area.x, self.area.y + (i as f32 - self.offset) * self.row_h, self.area.w, self.row_h)
    }

    /// Finishes the list. Drawing is no longer limited to the area. The list's targets are cut to the
    /// area and marked with their list, and those left with no area are dropped. While the list
    /// overflows, the scroll bar is drawn with its left edge at `bar_x`.
    pub fn end(self, f: &mut Frame, bar_x: f32) {
        f.c.clip(None);
        let area = self.area;
        let in_list = InList { list: self.list, row_h: self.row_h };
        let mut i = 0;
        f.targets.retain_mut(|t| {
            let keep = if i < self.first_target {
                true
            } else {
                t.rect = t.rect.intersect(area);
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
