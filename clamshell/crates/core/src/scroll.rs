//! Scrolled lists: shelf's games on the main screen; on the second screen Wi-Fi's networks, Bluetooth's
//! devices, the apps playing, the backlights. Each keeps how far it is scrolled, in rows (so touch and the
//! pad agree whatever the layout), and how many rows the UI last showed of it ([`Scroll`]; shelf keeps one
//! per collection). The pad's focus on a row scrolls just enough to show it; a list that changes under
//! the view keeps the row at its top where it is (a network vanishing above doesn't move the rows under
//! the finger), unless the view is at the very top, where new rows show.
//!
//! A list's rows are targets of one kind ([`Id::Row`]); what a row is, how many there are and what a tap
//! on one does is its feature's ([`Rows`], [`State::on_row`]): this module only says which feature a
//! list is.

use crate::State;
use crate::effect::Effect;
use crate::id::Id;
use crate::items::ItemKey;

/// A scrolled list.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ListId {
    /// Shelf's games (the current collection's).
    Shelf,
    Wifi,
    Bluetooth,
    /// The apps playing (the volume tab).
    Streams,
    Backlights,
    /// The keymap picker's: Default, then the keymaps.
    Keymaps,
    /// A keymap's layers: to show one (all, then New layer), or as a target (all but Base, then New
    /// layer).
    Layers,
}

/// Which part of a row a target is: the row itself, or one of its controls.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum RowPart {
    Main,
    /// Its level's track (the apps playing, the backlights).
    Level,
    /// Its mute button (the apps playing).
    Mute,
    /// Its Edit button (the keymaps).
    Edit,
}

impl ListId {
    pub const COUNT: usize = 7;
    pub const ALL: [ListId; ListId::COUNT] =
        [ListId::Shelf, ListId::Wifi, ListId::Bluetooth, ListId::Streams, ListId::Backlights, ListId::Keymaps, ListId::Layers];

    pub fn index(self) -> usize {
        self as usize
    }

    /// Row `i`'s main target.
    pub fn row(self, i: u16) -> Id {
        Id::Row { list: self, i, part: RowPart::Main }
    }

    /// The list and row a target is in, if it is a row's.
    pub fn of(id: &Id) -> Option<(ListId, usize)> {
        match id {
            Id::Row { list, i, .. } => Some((*list, usize::from(*i))),
            _ => None,
        }
    }

    /// `id` (a row's target) for row `i` instead: the same part a row up or down.
    pub fn sibling(id: &Id, i: u16) -> Option<Id> {
        match id {
            Id::Row { list, part, .. } => Some(Id::Row { list: *list, i, part: *part }),
            _ => None,
        }
    }

    /// Whether it rests on whole rows (shelf: a drag or fling ends settling on one).
    pub fn whole_rows(self) -> bool {
        self == ListId::Shelf
    }

    /// Which screens show it.
    pub fn screens(self) -> crate::Dirty {
        if self == ListId::Shelf { crate::Dirty::BOTH } else { crate::Dirty::BOTTOM }
    }
}

/// A list's rows as its feature has them: how many, and what each is (to find it again after the list
/// changed; none: its place isn't kept by item).
/// A list's rows as the lists see them: its feature's, or just how many (rows not found again by a key
/// when they change, as a keymap's layers on the screen showing them).
pub(crate) enum RowsOf<'a> {
    Of(&'a dyn Rows),
    Count(usize),
}

impl RowsOf<'_> {
    fn len(&self) -> usize {
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

pub(crate) trait Rows {
    fn len(&self) -> usize;

    fn key(&self, _i: usize) -> Option<ItemKey> {
        None
    }

    fn find(&self, _key: &ItemKey) -> Option<usize> {
        None
    }
}

/// One list's place: the offset in rows (fractions while a finger drags it), how many rows fit on
/// screen (as last drawn; none known yet: nothing scrolls), and the item at the top with the fraction
/// of it scrolled past, to keep it there when the list changes.
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

    /// The furthest the list scrolls with `len` rows: its last row at the bottom.
    pub fn max_offset(&self, len: usize) -> f32 {
        self.visible.map_or(0.0, |v| (len as f32 - v).max(0.0))
    }

    /// The offset kept within the list's ends; whether it changed.
    fn clamp(&mut self, len: usize) -> bool {
        let clamped = self.offset.clamp(0.0, self.max_offset(len));
        let changed = clamped != self.offset;
        self.offset = clamped;
        changed
    }

    /// Scrolled by `rows` (positive: further down the list), within its ends; whether it moved.
    pub fn scroll_by(&mut self, rows: f32, len: usize) -> bool {
        let before = self.offset;
        self.offset += rows;
        self.clamp(len);
        self.offset != before
    }

    /// Scrolled just enough to show row `i` whole; whether it moved.
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

    /// How many rows the UI shows; whether that changed.
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

    /// Whether scrolling by `rows` would move it (a fling stops at the ends).
    pub fn can_scroll(&self, rows: f32, len: usize) -> bool {
        (rows > 0.0 && self.offset < self.max_offset(len)) || (rows < 0.0 && self.offset > 0.0)
    }
}

/// Every list's place.
#[derive(Debug, Clone, Default)]
pub struct Lists {
    scrolls: [Scroll; ListId::COUNT],
}

impl Lists {
    pub fn get(&self, list: ListId) -> &Scroll {
        // (The array has a place for every list.)
        self.scrolls.get(list.index()).unwrap_or(&EMPTY)
    }

    fn get_mut(&mut self, list: ListId) -> Option<&mut Scroll> {
        self.scrolls.get_mut(list.index())
    }
}

pub(crate) static EMPTY: Scroll = Scroll { offset: 0.0, visible: None, anchor: None };

impl ListEvent {
    pub fn list(self) -> ListId {
        match self {
            ListEvent::Scroll(list, _) | ListEvent::Visible(list, _) => list,
        }
    }
}

/// What the UI says about a list.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum ListEvent {
    /// A finger, a fling or the wheel moved it by this many rows (positive: down the list).
    Scroll(ListId, f32),
    /// The list shows this many rows (from the layout).
    Visible(ListId, f32),
}

impl State {
    pub fn lists(&self) -> &Lists {
        &self.lists
    }

    /// Each list's rows: its feature's.
    pub(crate) fn rows(&self, list: ListId) -> RowsOf<'_> {
        match list {
            ListId::Shelf => RowsOf::Of(&self.shelf),
            ListId::Wifi => RowsOf::Of(&self.radios.wifi),
            ListId::Bluetooth => RowsOf::Of(&self.radios.bluetooth),
            ListId::Streams => RowsOf::Of(self.levels.audio()),
            ListId::Backlights => RowsOf::Of(self.levels.backlight_rows()),
            ListId::Keymaps => RowsOf::Of(&self.keymaps),
            ListId::Layers => RowsOf::Count(self.layer_rows()),
        }
    }

    /// A tap or long press on a row: its feature's.
    pub(crate) fn on_row(&mut self, list: ListId, i: usize, part: RowPart, long: bool) -> Vec<Effect> {
        match list {
            ListId::Wifi => self.tap_network(i, long),
            ListId::Bluetooth => self.tap_device(i, long),
            ListId::Streams => self.on_stream(i, part),
            ListId::Keymaps => self.on_keymap_row(i, part, long),
            ListId::Layers => self.on_layer_row(i, part, long),
            // (A shelf game's row is its game's target; a backlight's row is only its level.)
            ListId::Shelf | ListId::Backlights => vec![],
        }
    }

    /// Where `list` is scrolled (shelf: the current collection's place).
    pub fn scroll(&self, list: ListId) -> &Scroll {
        match list {
            ListId::Shelf => self.shelf.scroll(),
            _ => self.lists.get(list),
        }
    }

    fn scroll_mut(&mut self, list: ListId) -> Option<&mut Scroll> {
        match list {
            ListId::Shelf => self.shelf.scroll_mut(),
            _ => self.lists.get_mut(list),
        }
    }

    /// How many rows `list` has now.
    pub fn list_len(&self, list: ListId) -> usize {
        self.rows(list).len()
    }

    /// How far `list` is scrolled, in rows.
    pub fn list_offset(&self, list: ListId) -> f32 {
        self.scroll(list).offset()
    }

    /// Whether scrolling `list` by `rows` would move it.
    pub fn list_can_scroll(&self, list: ListId, rows: f32) -> bool {
        self.scroll(list).can_scroll(rows, self.list_len(list))
    }

    /// A list scrolled or laid out; whether it moved.
    pub(crate) fn on_list(&mut self, event: ListEvent) -> bool {
        let (list, moved) = match event {
            ListEvent::Scroll(list, rows) => {
                let len = self.list_len(list);
                (list, self.scroll_mut(list).is_some_and(|s| s.scroll_by(rows, len)))
            }
            ListEvent::Visible(list, rows) => {
                let len = self.list_len(list);
                (list, self.scroll_mut(list).is_some_and(|s| s.set_visible(rows, len)))
            }
        };
        if moved {
            self.anchor(list);
        }
        moved
    }

    /// The pad's focus came to `id`: if it is a list's row, the list shows it.
    pub(crate) fn show_focused(&mut self, id: &Id) {
        let Some((list, i)) = ListId::of(id) else {
            return;
        };
        let len = self.list_len(list);
        if self.scroll_mut(list).is_some_and(|s| s.show(i, len)) {
            self.anchor(list);
        }
    }

    /// Remembers which item is at the top of `list`'s view.
    fn anchor(&mut self, list: ListId) {
        let offset = self.scroll(list).offset;
        let top = offset.floor();
        let key = self.rows(list).key(top as usize);
        if let Some(s) = self.scroll_mut(list) {
            s.anchor = key.map(|k| (k, offset - top));
        }
    }

    /// After the lists' contents may have changed: each scrolled list keeps its top item where it is
    /// (while that item is still there), and stays within its ends. Whether any moved.
    pub(crate) fn follow_lists(&mut self) -> bool {
        let mut moved = false;
        for list in ListId::ALL {
            let len = self.list_len(list);
            let scroll = self.scroll(list);
            let found = match &scroll.anchor {
                // At the very top it stays there: new rows show.
                Some((key, fraction)) if scroll.offset > 0.0 => self.rows(list).find(key).map(|i| i as f32 + fraction),
                _ => None,
            };
            if let Some(s) = self.scroll_mut(list) {
                let before = s.offset;
                if let Some(offset) = found {
                    s.offset = offset;
                }
                s.clamp(len);
                moved |= s.offset != before;
            }
        }
        moved
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    use crate::testkit::*;
    use crate::wifi::{Network, Wifi};
    use crate::*;

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

    fn network(ssid: &str) -> Network {
        Network { ssid: ssid.into(), strength: 50, ..Network::default() }
    }

    fn networks(s: &mut State, names: &[&str]) {
        read(s, Reading::Wifi(Wifi { device: true, enabled: true, networks: names.iter().map(|n| network(n)).collect(), ..Wifi::default() }));
    }

    #[test]
    fn rows_under_the_view_stay_when_the_list_changes() {
        let mut s = state();
        networks(&mut s, &["a", "b", "c", "d", "e", "f", "g", "h"]);
        s.apply(ListEvent::Visible(ListId::Wifi, 3.0).into());
        s.apply(ListEvent::Scroll(ListId::Wifi, 2.5).into());
        // "a" vanishes above the view: "c" stays at its top, half scrolled past.
        networks(&mut s, &["b", "c", "d", "e", "f", "g", "h"]);
        assert_eq!(s.lists().get(ListId::Wifi).offset(), 1.5);
        // The top one gone too: the offset stays, within the ends.
        networks(&mut s, &["b", "d", "e"]);
        assert_eq!(s.lists().get(ListId::Wifi).offset(), 0.0);
        // At the very top, a new network above shows.
        networks(&mut s, &["b", "d", "e", "f", "g"]);
        networks(&mut s, &["new", "b", "d", "e", "f", "g"]);
        assert_eq!(s.lists().get(ListId::Wifi).offset(), 0.0);
    }

    #[test]
    fn the_pads_focus_on_a_row_shows_it() {
        let mut s = state();
        networks(&mut s, &["a", "b", "c", "d", "e", "f"]);
        s.apply(ListEvent::Visible(ListId::Wifi, 2.0).into());
        let (dirty, _) = s.apply(PointerEvent::Focus(ListId::Wifi.row(4)).into());
        assert_eq!((dirty, s.lists().get(ListId::Wifi).offset()), (Dirty::BOTH, 3.0));
        s.apply(PointerEvent::Focus(ListId::Wifi.row(0)).into());
        assert_eq!(s.lists().get(ListId::Wifi).offset(), 0.0);
    }
}
