//! The place of a scrolled list ([`Scroll`]). It holds how far the list is scrolled, how many rows show,
//! and the item at the top. Scrolling is counted in rows so that touch and the pad agree whatever the
//! layout. The item at the top stays there when the list changes; it is identified by an [`ItemKey`],
//! which the list's feature gives for its rows through [`Rows`].
//!
//! Which lists there are and what their rows do is in `nav::lists`.

/// What identifies a list item: a network's SSID, a device's address, a game's workspace, a collection's
/// name, a backlight's name, or a PipeWire object's id.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum ItemKey {
    Name(String),
    Number(u32),
}

/// A list's rows: either the feature's own [`Rows`], or just a count. A count is for rows that can't be
/// found again by a key when they change, such as a keymap's layers on the screen that shows them.
pub(crate) enum RowsOf<'a> {
    Of(&'a dyn Rows),
    Count(usize),
}

impl RowsOf<'_> {
    pub(crate) fn len(&self) -> usize {
        match self {
            RowsOf::Of(r) => r.len(),
            RowsOf::Count(n) => *n,
        }
    }

    pub(crate) fn key(&self, i: usize) -> Option<ItemKey> {
        match self {
            RowsOf::Of(r) => r.key(i),
            RowsOf::Count(_) => None,
        }
    }

    pub(crate) fn find(&self, key: &ItemKey) -> Option<usize> {
        match self {
            RowsOf::Of(r) => r.find(key),
            RowsOf::Count(_) => None,
        }
    }
}

/// A list's rows as its feature has them: how many there are, and a key for each so it can be found
/// again after the list changes. A list whose rows have no key doesn't keep its place by item.
pub(crate) trait Rows {
    fn len(&self) -> usize;

    fn key(&self, _i: usize) -> Option<ItemKey> {
        None
    }

    fn find(&self, _key: &ItemKey) -> Option<usize> {
        None
    }
}

/// One list's place. The offset is in rows and is fractional while a finger drags the list. The number
/// of rows that fit on screen is as last drawn; until it is known, nothing scrolls. The anchor is the
/// item at the top and the fraction of it scrolled past, to keep it there when the list changes.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Scroll {
    offset: f32,
    visible: Option<f32>,
    anchor: Option<(ItemKey, f32)>,
}

impl Scroll {
    pub fn offset(&self) -> f32 {
        self.offset
    }

    /// How many rows show, as last drawn.
    pub fn visible(&self) -> Option<f32> {
        self.visible
    }

    /// The furthest the list scrolls with `len` rows, which puts its last row at the bottom.
    pub fn max_offset(&self, len: usize) -> f32 {
        self.visible.map_or(0.0, |v| (len as f32 - v).max(0.0))
    }

    /// Keeps the offset within the list's ends. Returns whether it changed.
    fn clamp(&mut self, len: usize) -> bool {
        let clamped = self.offset.clamp(0.0, self.max_offset(len));
        let changed = clamped != self.offset;
        self.offset = clamped;
        changed
    }

    /// Scrolls by `rows` within the list's ends; positive is further down. Returns whether it moved.
    pub fn scroll_by(&mut self, rows: f32, len: usize) -> bool {
        let before = self.offset;
        self.offset += rows;
        self.clamp(len);
        self.offset != before
    }

    /// Scrolls just enough to show row `i` whole. Returns whether it moved.
    pub fn show(&mut self, i: usize, len: usize) -> bool {
        let Some(visible) = self.visible else {
            return false;
        };
        let before = self.offset;
        let i = i as f32;
        if i < self.offset {
            self.offset = i;
        } else if i + 1.0 > self.offset + visible {
            self.offset = i + 1.0 - visible;
        }
        self.clamp(len);
        self.offset != before
    }

    /// Sets how many rows the UI shows. Returns whether that changed.
    pub fn set_visible(&mut self, rows: f32, len: usize) -> bool {
        if self.visible == Some(rows) {
            return false;
        }
        self.visible = Some(rows);
        self.clamp(len);
        true
    }

    /// Whether the list has more rows than show.
    pub fn overflows(&self, len: usize) -> bool {
        self.max_offset(len) > 0.0
    }

    /// The row at the top of the view.
    pub(crate) fn top(&self) -> usize {
        self.offset.floor() as usize
    }

    /// Remembers `key`, the item at [`Scroll::top`], to keep there when the list changes.
    pub(crate) fn anchor(&mut self, key: Option<ItemKey>) {
        let fraction = self.offset - self.offset.floor();
        self.anchor = key.map(|k| (k, fraction));
    }

    /// The item to keep at the top. There is none when the view is at the very top, so that new rows
    /// added there show.
    pub(crate) fn anchored(&self) -> Option<&ItemKey> {
        self.anchor.as_ref().filter(|_| self.offset > 0.0).map(|(k, _)| k)
    }

    /// After the list changes, puts the [`Scroll::anchored`] item, now at row `found`, back where it was,
    /// and keeps the view within the list's ends (`len` rows). Returns whether it moved.
    pub(crate) fn follow(&mut self, found: Option<usize>, len: usize) -> bool {
        let before = self.offset;
        if let (Some(i), Some((_, fraction))) = (found, &self.anchor) {
            self.offset = i as f32 + fraction;
        }
        self.clamp(len);
        self.offset != before
    }

    /// Whether scrolling by `rows` would move the list. A fling stops at the ends.
    pub fn can_scroll(&self, rows: f32, len: usize) -> bool {
        (rows > 0.0 && self.offset < self.max_offset(len)) || (rows < 0.0 && self.offset > 0.0)
    }
}

pub(crate) static EMPTY: Scroll = Scroll { offset: 0.0, visible: None, anchor: None };

#[cfg(test)]
mod tests {
    use super::*;

    fn scroll(visible: f32) -> Scroll {
        Scroll { visible: Some(visible), ..Scroll::default() }
    }

    #[test]
    fn the_offset_stays_within_the_ends() {
        let mut s = scroll(4.5);
        assert!(!s.scroll_by(-1.0, 10), "at the top already");
        assert!(s.scroll_by(3.25, 10));
        assert_eq!(s.offset(), 3.25);
        s.scroll_by(100.0, 10);
        assert_eq!(s.offset(), 5.5, "the last row at the bottom");
        assert!(s.can_scroll(-1.0, 10) && !s.can_scroll(1.0, 10));
        // Fewer rows than fit: nothing scrolls.
        let mut short = scroll(4.5);
        assert!(!short.scroll_by(2.0, 3) && !short.overflows(3));
        // Not laid out yet: nothing scrolls either.
        assert!(!Scroll::default().scroll_by(1.0, 10));
        // Fewer rows fit now: the offset comes back within the ends.
        s.set_visible(8.0, 10);
        assert_eq!(s.offset(), 2.0);
    }

    #[test]
    fn showing_a_row_scrolls_just_enough() {
        let mut s = scroll(4.0);
        assert!(!s.show(3, 10), "already shown");
        assert!(s.show(4, 10));
        assert_eq!(s.offset(), 1.0, "its bottom at the view's bottom");
        assert!(s.show(9, 10));
        assert_eq!(s.offset(), 6.0);
        assert!(s.show(2, 10));
        assert_eq!(s.offset(), 2.0, "its top at the view's top");
        s.scroll_by(0.5, 10);
        assert!(s.show(2, 10), "a row partly scrolled past comes back whole");
        assert_eq!(s.offset(), 2.0);
    }
}
