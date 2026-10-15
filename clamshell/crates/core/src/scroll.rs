//! Scrolled lists on the second screen: Wi-Fi's networks, Bluetooth's devices, the apps playing, the
//! backlights, the keymaps. Each keeps how far it is scrolled, in rows (so touch and the pad agree
//! whatever the layout), and how many rows the UI last showed of it. The pad's focus on a row scrolls
//! just enough to show it; a list that changes under the view keeps the row at its top where it is (a
//! network vanishing above doesn't move the rows under the finger), unless the view is at the very top,
//! where new rows show. Shelf keeps its own place per collection (see [`crate::Shelf`]): only touch
//! scrolls it through here.

use crate::State;
use crate::id::{AudioId, BtId, Id, WifiId};
use crate::items::ItemKey;
use crate::levels::Level;

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
}

impl ListId {
    pub const COUNT: usize = 5;
    pub const ALL: [ListId; ListId::COUNT] = [ListId::Shelf, ListId::Wifi, ListId::Bluetooth, ListId::Streams, ListId::Backlights];

    pub fn index(self) -> usize {
        self as usize
    }

    /// Row `i`'s main target (shelf's rows are named by their game).
    pub fn row(self, i: u16) -> Option<Id> {
        Some(match self {
            ListId::Shelf => return None,
            ListId::Wifi => Id::Wifi(WifiId::Network(i)),
            ListId::Bluetooth => Id::Bluetooth(BtId::Device(i)),
            ListId::Streams => Id::Level(Level::Stream(i)),
            ListId::Backlights => Id::Level(Level::Backlight(i)),
        })
    }

    /// `id` (a row's target, see [`ListId::of`]) for row `i` instead: the same target a row up or down.
    pub fn sibling(id: &Id, i: u16) -> Option<Id> {
        Some(match id {
            Id::Wifi(WifiId::Network(_)) => Id::Wifi(WifiId::Network(i)),
            Id::Bluetooth(BtId::Device(_)) => Id::Bluetooth(BtId::Device(i)),
            Id::Level(Level::Stream(_)) => Id::Level(Level::Stream(i)),
            Id::Audio(AudioId::StreamMute(_)) => Id::Audio(AudioId::StreamMute(i)),
            Id::Level(Level::Backlight(_)) => Id::Level(Level::Backlight(i)),
            _ => return None,
        })
    }

    /// Whether it rests on whole rows (shelf: a drag or fling ends settling on one).
    pub fn whole_rows(self) -> bool {
        self == ListId::Shelf
    }

    /// The list and row a target is in, if it is in one.
    pub fn of(id: &Id) -> Option<(ListId, usize)> {
        let (list, i) = match id {
            Id::Wifi(WifiId::Network(i)) => (ListId::Wifi, i),
            Id::Bluetooth(BtId::Device(i)) => (ListId::Bluetooth, i),
            Id::Level(Level::Stream(i)) | Id::Audio(AudioId::StreamMute(i)) => (ListId::Streams, i),
            Id::Level(Level::Backlight(i)) => (ListId::Backlights, i),
            _ => return None,
        };
        Some((list, usize::from(*i)))
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

static EMPTY: Scroll = Scroll { offset: 0.0, visible: None, anchor: None };

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

    /// How many rows `list` has now.
    pub fn list_len(&self, list: ListId) -> usize {
        match list {
            ListId::Shelf => self.shelf.collection().map_or(0, |c| c.games.len()),
            ListId::Wifi => self.radios.wifi.networks.len(),
            ListId::Bluetooth => self.radios.bluetooth.devices.len(),
            ListId::Streams => self.levels.audio().streams.len(),
            ListId::Backlights => self.levels.backlights().len(),
        }
    }

    /// How far `list` is scrolled, in rows.
    pub fn list_offset(&self, list: ListId) -> f32 {
        match list {
            ListId::Shelf => self.shelf.cursor().top,
            _ => self.lists.get(list).offset(),
        }
    }

    /// Whether scrolling `list` by `rows` would move it.
    pub fn list_can_scroll(&self, list: ListId, rows: f32) -> bool {
        match list {
            ListId::Shelf => self.shelf.can_scroll(rows),
            _ => self.lists.get(list).can_scroll(rows, self.list_len(list)),
        }
    }

    /// A list scrolled or laid out; whether it moved.
    pub(crate) fn on_list(&mut self, event: ListEvent) -> bool {
        let (list, moved) = match event {
            // (Shelf's rows come as ShelfEvent::Rows.)
            ListEvent::Scroll(ListId::Shelf, rows) => return self.shelf.scroll(rows),
            ListEvent::Visible(ListId::Shelf, _) => return false,
            ListEvent::Scroll(list, rows) => {
                let len = self.list_len(list);
                (list, self.lists.get_mut(list).is_some_and(|s| s.scroll_by(rows, len)))
            }
            ListEvent::Visible(list, rows) => {
                let len = self.list_len(list);
                (list, self.lists.get_mut(list).is_some_and(|s| s.set_visible(rows, len)))
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
        if self.lists.get_mut(list).is_some_and(|s| s.show(i, len)) {
            self.anchor(list);
        }
    }

    /// Remembers which item is at the top of `list`'s view.
    fn anchor(&mut self, list: ListId) {
        let offset = self.lists.get(list).offset;
        let top = offset.floor();
        let key = u16::try_from(top as usize).ok().and_then(|i| list.row(i)).and_then(|id| self.item_key(&id));
        if let Some(s) = self.lists.get_mut(list) {
            s.anchor = key.map(|k| (k, offset - top));
        }
    }

    /// After the lists' contents may have changed: each scrolled list keeps its top item where it is
    /// (while that item is still there), and stays within its ends. Whether any moved.
    pub(crate) fn follow_lists(&mut self) -> bool {
        let mut moved = false;
        for list in ListId::ALL {
            let len = self.list_len(list);
            let scroll = self.lists.get(list);
            let found = match &scroll.anchor {
                // At the very top it stays there: new rows show.
                Some((key, fraction)) if scroll.offset > 0.0 => list
                    .row(0)
                    .and_then(|first| self.find_item(&first, key))
                    .and_then(|id| ListId::of(&id))
                    .map(|(_, i)| i as f32 + fraction),
                _ => None,
            };
            if let Some(s) = self.lists.get_mut(list) {
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
        let (dirty, _) = s.apply(PointerEvent::Focus(Id::Wifi(WifiId::Network(4))).into());
        assert_eq!((dirty, s.lists().get(ListId::Wifi).offset()), (Dirty::BOTH, 3.0));
        s.apply(PointerEvent::Focus(Id::Wifi(WifiId::Network(0))).into());
        assert_eq!(s.lists().get(ListId::Wifi).offset(), 0.0);
    }
}
