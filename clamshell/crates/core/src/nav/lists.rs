//! The scrolled lists: shelf's games on the main screen, and on the second screen the Wi-Fi networks,
//! the Bluetooth devices, the apps playing and the backlights, among others. Each keeps its own scroll
//! position ([`Scroll`]), and shelf keeps one per collection.
//!
//! When the pad's focus moves to a row, the list scrolls just enough to show it. When a list changes
//! under the view, the row at the top of the view stays where it is, so a network vanishing above
//! doesn't move the rows under the finger. The exception is a view at the very top, where new rows show.
//!
//! A list's rows are all targets of one kind ([`Id::Row`]). What a row is, how many there are and what a
//! tap on one does are up to the list's feature ([`crate::kit::list::Rows`], [`State::on_row`]). This
//! module only maps each list to its feature.

use crate::State;
use crate::effect::Effect;
use crate::id::Id;
use crate::kit::list::{EMPTY, RowsOf, Scroll};

/// A scrolled list.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ListId {
    /// Shelf's games, from the current collection.
    Shelf,
    Wifi,
    Bluetooth,
    /// The apps playing, in the volume tab.
    Streams,
    Backlights,
    /// The keymap picker: Default, then the keymaps.
    Keymaps,
    /// A keymap's layers. To pick one to show, it lists all of them, then New layer. To pick one as a
    /// target, it lists all but Base, then New layer.
    Layers,
    /// The time tab's regions, or a region's zones.
    Time,
    /// The dock's rows of tiles, [`crate::session::dock::DOCK_COLUMNS`] to a row.
    Dock,
}

/// Which part of a row a target is: the row itself, or one of its controls.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum RowPart {
    Main,
    /// The row's level track, for the apps playing and the backlights.
    Level,
    /// The row's mute button, for the apps playing.
    Mute,
    /// The row's Edit button, for the keymaps.
    Edit,
}

impl ListId {
    pub const COUNT: usize = 9;
    pub const ALL: [ListId; ListId::COUNT] =
        [ListId::Shelf, ListId::Wifi, ListId::Bluetooth, ListId::Streams, ListId::Backlights, ListId::Keymaps, ListId::Layers, ListId::Time, ListId::Dock];

    pub fn index(self) -> usize {
        self as usize
    }

    /// Row `i`'s main target.
    pub fn row(self, i: u16) -> Id {
        Id::Row { list: self, i, part: RowPart::Main }
    }

    /// The list and row a target is in, if it belongs to a row. For a dock tile, this is its row of
    /// tiles.
    pub fn of(id: &Id) -> Option<(ListId, usize)> {
        match id {
            Id::Row { list, i, .. } => Some((*list, usize::from(*i))),
            Id::Dock(crate::DockId::Entry(i)) => Some((ListId::Dock, usize::from(*i) / crate::session::dock::DOCK_COLUMNS)),
            _ => None,
        }
    }

    /// The same part as the row target `id`, but in row `i`, for moving up or down. A dock tile moves to
    /// the same column, or to the row's last tile. `dock_len` is how many tiles there are.
    pub fn sibling(id: &Id, i: u16, dock_len: usize) -> Option<Id> {
        match id {
            Id::Row { list, part, .. } => Some(Id::Row { list: *list, i, part: *part }),
            Id::Dock(crate::DockId::Entry(at)) => {
                let tile =
                    (usize::from(i) * crate::session::dock::DOCK_COLUMNS + usize::from(*at) % crate::session::dock::DOCK_COLUMNS).min(dock_len.checked_sub(1)?);
                Some(Id::Dock(crate::DockId::Entry(u16::try_from(tile).ok()?)))
            }
            _ => None,
        }
    }

    /// Whether the list rests on whole rows. Shelf does: a drag or fling ends by settling on a row.
    pub fn whole_rows(self) -> bool {
        self == ListId::Shelf
    }

    /// Which screens show it.
    pub fn screens(self) -> crate::Dirty {
        if self == ListId::Shelf { crate::Dirty::BOTH } else { crate::Dirty::BOTTOM }
    }
}

/// The scroll position of every list.
#[derive(Debug, Clone, Default)]
pub struct Lists {
    scrolls: [Scroll; ListId::COUNT],
}

impl Lists {
    pub fn get(&self, list: ListId) -> &Scroll {
        // The array has an entry for every list.
        self.scrolls.get(list.index()).unwrap_or(&EMPTY)
    }

    fn get_mut(&mut self, list: ListId) -> Option<&mut Scroll> {
        self.scrolls.get_mut(list.index())
    }
}

/// What the UI says about a list.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum ListEvent {
    /// A finger, a fling or the wheel moved the list by this many rows. Positive is down the list.
    Scroll(ListId, f32),
    /// The list shows this many rows, from the layout.
    Visible(ListId, f32),
}

impl ListEvent {
    pub fn list(self) -> ListId {
        match self {
            ListEvent::Scroll(list, _) | ListEvent::Visible(list, _) => list,
        }
    }
}

impl State {
    pub fn lists(&self) -> &Lists {
        &self.lists
    }

    /// A list's rows, from its feature.
    pub(crate) fn rows(&self, list: ListId) -> RowsOf<'_> {
        match list {
            ListId::Shelf => RowsOf::Of(&self.shelf),
            ListId::Wifi => RowsOf::Of(self.radios.wifi()),
            ListId::Bluetooth => RowsOf::Of(self.radios.bluetooth()),
            ListId::Streams => RowsOf::Of(self.levels.audio()),
            ListId::Backlights => RowsOf::Of(self.levels.backlight_rows()),
            ListId::Keymaps => RowsOf::Count(self.keymap_rows_len()),
            ListId::Layers => RowsOf::Count(self.layer_rows()),
            ListId::Time => RowsOf::Count(self.time_rows()),
            ListId::Dock => RowsOf::Count(self.dock.items().len().div_ceil(crate::session::dock::DOCK_COLUMNS)),
        }
    }

    /// A tap or long press on a row, handled by the list's feature.
    pub(crate) fn on_row(&mut self, list: ListId, i: usize, part: RowPart, long: bool) -> Vec<Effect> {
        match list {
            ListId::Wifi => {
                let mutations = self.radios.tap_network(i, long);
                self.settle(mutations)
            }
            ListId::Bluetooth => self.radios.tap_device(i, long),
            ListId::Streams => self.levels.on_stream(i, part),
            ListId::Keymaps => self.on_keymap_row(i, part, long),
            ListId::Layers => self.on_layer_row(i, part, long),
            ListId::Time => self.on_time_row(i),
            // Its tiles are targets of their own.
            ListId::Dock => vec![],
            // A shelf game's row is its game's target, and a backlight's row is only its level.
            ListId::Shelf | ListId::Backlights => vec![],
        }
    }

    /// Where `list` is scrolled. For shelf, this is the current collection's position.
    pub fn scroll(&self, list: ListId) -> &Scroll {
        match list {
            ListId::Shelf => self.shelf.scroll(),
            _ => self.lists.get(list),
        }
    }

    pub(crate) fn scroll_mut(&mut self, list: ListId) -> Option<&mut Scroll> {
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

    /// A list scrolled or was laid out. Returns whether it moved.
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

    /// The pad's focus moved to `id`. If it is a list row, the list scrolls to show it.
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
        let key = self.rows(list).key(self.scroll(list).top());
        if let Some(s) = self.scroll_mut(list) {
            s.anchor(key);
        }
    }

    /// Runs after the lists' contents may have changed. Each scrolled list keeps its top item where it
    /// is, while that item is still there, and stays within its ends. Returns whether any moved.
    pub(crate) fn follow_lists(&mut self) -> bool {
        let mut moved = false;
        for list in ListId::ALL {
            let len = self.list_len(list);
            let found = self.scroll(list).anchored().and_then(|key| self.rows(list).find(key));
            if let Some(s) = self.scroll_mut(list) {
                moved |= s.follow(found, len);
            }
        }
        moved
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    use crate::radios::wifi::{Network, Wifi};
    use crate::testkit::*;
    use crate::*;

    fn network(ssid: &str) -> Network {
        Network { ssid: ssid.into(), strength: 50, ..Network::default() }
    }

    fn networks(s: &mut State, names: &[&str]) {
        read(s, Reading::Wifi(Wifi { present: true, on: true, networks: names.iter().map(|n| network(n)).collect(), ..Wifi::default() }));
    }

    #[test]
    fn rows_under_the_view_stay_when_the_list_changes() {
        let mut s = state();
        networks(&mut s, &["a", "b", "c", "d", "e", "f", "g", "h"]);
        s.apply(ListEvent::Visible(ListId::Wifi, 3.0).into());
        s.apply(ListEvent::Scroll(ListId::Wifi, 2.5).into());
        // "a" vanishes above the view, and "c" stays at the top, half scrolled past.
        networks(&mut s, &["b", "c", "d", "e", "f", "g", "h"]);
        assert_eq!(s.lists().get(ListId::Wifi).offset(), 1.5);
        // With the top one gone too, the offset stays, within the ends.
        networks(&mut s, &["b", "d", "e"]);
        assert_eq!(s.lists().get(ListId::Wifi).offset(), 0.0);
        // At the very top, a new network above is shown.
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

    #[test]
    fn a_dock_tile_a_row_down_keeps_its_column() {
        let tile = |i| Id::Dock(DockId::Entry(i));
        assert_eq!(ListId::of(&tile(6)), Some((ListId::Dock, 1)));
        assert_eq!(ListId::sibling(&tile(6), 2, 20), Some(tile(10)));
        assert_eq!(ListId::sibling(&tile(7), 2, 10), Some(tile(9)), "the last row short: its last tile");
        assert_eq!(ListId::sibling(&tile(0), 1, 0), None);
    }
}
