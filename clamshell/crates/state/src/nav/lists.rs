//! The scrolled lists: shelf's games on the main screen, and on the second screen the Wi-Fi networks,
//! the Bluetooth devices, the apps playing and the backlights, among others. Each keeps its own scroll
//! position ([`Scroll`]), and shelf keeps one per collection.
//!
//! When the pad's focus moves to a row, the list scrolls just enough to show it. When a list changes
//! under the view, the row at the top of the view stays where it is, so a network vanishing above
//! doesn't move the rows under the finger. The exception is a view at the very top, where new rows show.
//!
//! A list's rows are all targets of one kind ([`Id::Row`]). What a row is, how many there are and what a
//! tap on one does are up to the list's feature ([`crate::kit::list::Rows`], [`crate::State::on_row`]). This
//! module only maps each list to its feature.

use crate::id::{AudioId, DockId, Id, ShelfId};
use crate::keymap::screens::Keymapping;
use crate::kit::list::{EMPTY, ItemKey, RowsOf, Scroll};
use crate::{Dock, Level, Levels, Panel, Pointer, Prefs, Radios, Session, Shelf, Sys};

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
    /// The curve picker: Off, then the curves.
    Curves,
    /// A keymap's layers. To pick one to show, it lists all of them, then New layer. To pick one as a
    /// target, it lists all but Base, then New layer.
    Layers,
    /// The time tab's regions, or a region's zones.
    Time,
    /// The dock's rows of tiles, [`crate::session::dock::DOCK_COLUMNS`] to a row.
    Dock,
    /// A settings tile's menu: its values, then Reset when there is something to reset.
    Values,
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
    pub const COUNT: usize = 11;
    pub const ALL: [ListId; ListId::COUNT] = [
        ListId::Shelf,
        ListId::Wifi,
        ListId::Bluetooth,
        ListId::Streams,
        ListId::Backlights,
        ListId::Keymaps,
        ListId::Curves,
        ListId::Layers,
        ListId::Time,
        ListId::Dock,
        ListId::Values,
    ];

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
}

/// The scroll position of every list but shelf, which keeps one per collection.
#[derive(Debug, Clone, Default)]
pub(crate) struct Scrolls {
    scrolls: [Scroll; ListId::COUNT],
}

impl Scrolls {
    pub(crate) fn get(&self, list: ListId) -> &Scroll {
        // The array has an entry for every list.
        self.scrolls.get(list.index()).unwrap_or(&EMPTY)
    }

    pub(crate) fn get_mut(&mut self, list: ListId) -> Option<&mut Scroll> {
        self.scrolls.get_mut(list.index())
    }
}

/// The lists as they are now: each one's rows, from its feature, and where it is scrolled. Built by
/// [`crate::State::lists`] from the areas.
#[derive(Clone, Copy)]
pub struct Lists<'a> {
    pub(crate) scrolls: &'a Scrolls,
    pub(crate) shelf: &'a Shelf,
    pub(crate) session: &'a Session,
    pub(crate) prefs: &'a Prefs,
    pub(crate) keymapping: &'a Keymapping,
    pub(crate) curving: &'a crate::curve::screens::Curving,
    pub(crate) radios: &'a Radios,
    pub(crate) levels: &'a Levels,
    pub(crate) sys: &'a Sys,
    pub(crate) dock: &'a Dock,
    pub(crate) pointer: &'a Pointer,
    /// The open tile menu's rows (see [`crate::State::menu_row`]).
    pub(crate) menu_rows: usize,
}

impl<'a> Lists<'a> {
    fn panel(&self) -> Panel<'a> {
        Panel::new(self.session, self.shelf, self.prefs, self.keymapping, self.curving)
    }

    /// Where `list` is scrolled. For shelf, this is the current collection's position.
    pub fn scroll(&self, list: ListId) -> &'a Scroll {
        match list {
            ListId::Shelf => self.shelf.scroll(),
            _ => self.scrolls.get(list),
        }
    }

    /// How many rows `list` has now.
    pub fn len(&self, list: ListId) -> usize {
        self.rows(list).len()
    }

    /// A list's rows, from its feature.
    pub(crate) fn rows(&self, list: ListId) -> RowsOf<'a> {
        match list {
            ListId::Shelf => RowsOf::Of(self.shelf),
            ListId::Wifi => RowsOf::Of(self.radios.wifi()),
            ListId::Bluetooth => RowsOf::Of(self.radios.bluetooth()),
            ListId::Streams => RowsOf::Of(self.levels.audio()),
            ListId::Backlights => RowsOf::Of(self.levels.backlight_rows()),
            ListId::Keymaps => RowsOf::Count(self.panel().keymap_rows_len()),
            ListId::Curves => RowsOf::Count(self.panel().curve_rows_len()),
            ListId::Layers => RowsOf::Count(self.keymapping.layer_rows()),
            ListId::Time => RowsOf::Count(self.sys.time_rows()),
            ListId::Dock => RowsOf::Count(self.dock.items().len().div_ceil(crate::session::dock::DOCK_COLUMNS)),
            ListId::Values => RowsOf::Count(self.menu_rows),
        }
    }

    /// The key of the item `id` points at now, if `id` is a list item. The UI records it with the target
    /// it draws for `id`.
    pub fn item_key(&self, id: &Id) -> Option<ItemKey> {
        let at = |i: &u16| usize::from(*i);
        match id {
            Id::Row { list, i, .. } => self.rows(*list).key(at(i)),
            Id::Dock(DockId::Entry(i)) => self.dock.items().get(at(i)).map(|e| ItemKey::name(&e.id)),
            Id::Shelf(ShelfId::Collection(i)) => self.shelf.library().collections.get(at(i)).map(|c| ItemKey::name(&c.name)),
            Id::Audio(AudioId::Output(i)) => self.levels.audio().outputs.get(at(i)).map(|o| ItemKey::Number(o.id)),
            _ => None,
        }
    }

    /// Where the item with `key` is now, as an id of the same kind as `id`.
    fn find_item(&self, id: &Id, key: &ItemKey) -> Option<Id> {
        let index = |p: Option<usize>| p.and_then(|p| u16::try_from(p).ok());
        match (id, key) {
            (Id::Row { list, part, .. }, key) => index(self.rows(*list).find(key)).map(|i| Id::Row { list: *list, i, part: *part }),
            (Id::Dock(DockId::Entry(_)), ItemKey::Name(_)) => {
                index(self.dock.items().iter().position(|e| ItemKey::name(&e.id) == *key)).map(|i| Id::Dock(DockId::Entry(i)))
            }
            (Id::Shelf(ShelfId::Collection(_)), ItemKey::Name(_)) => {
                index(self.shelf.library().collections.iter().position(|x| ItemKey::name(&x.name) == *key)).map(|i| Id::Shelf(ShelfId::Collection(i)))
            }
            (Id::Audio(AudioId::Output(_)), ItemKey::Number(n)) => {
                index(self.levels.audio().outputs.iter().position(|x| x.id == *n)).map(|i| Id::Audio(AudioId::Output(i)))
            }
            _ => None,
        }
    }

    /// `id` resolved to the item the press landed on: the same item where it is now, or `None` once it
    /// is gone. Ids that aren't list items, or weren't pressed, are returned as they are.
    pub(crate) fn resolve(&self, id: &Id) -> Option<Id> {
        match self.pointer.pressed_item(id) {
            Some(item) => self.find_item(id, item),
            None => Some(id.clone()),
        }
    }

    /// The target pressed, resolved to where the item the press came down on is now (see
    /// [`Lists::resolve`]), so that the item lit is the one a tap acts on.
    pub fn pressed(&self) -> Option<Id> {
        self.pointer.pressed().and_then(|id| self.resolve(id))
    }

    /// A level row resolved to the one the finger landed on (see [`Lists::resolve`]).
    pub(crate) fn resolve_level(&self, level: Level) -> Option<Level> {
        self.resolve(&level.id()).as_ref().and_then(Level::of)
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
        s.applied(ListEvent::Visible(ListId::Wifi, 3.0).into());
        s.applied(ListEvent::Scroll(ListId::Wifi, 2.5).into());
        // "a" vanishes above the view, and "c" stays at the top, half scrolled past.
        networks(&mut s, &["b", "c", "d", "e", "f", "g", "h"]);
        assert_eq!(s.lists().scroll(ListId::Wifi).offset(), 1.5);
        // With the top one gone too, the offset stays, within the ends.
        networks(&mut s, &["b", "d", "e"]);
        assert_eq!(s.lists().scroll(ListId::Wifi).offset(), 0.0);
        // At the very top, a new network above is shown.
        networks(&mut s, &["b", "d", "e", "f", "g"]);
        networks(&mut s, &["new", "b", "d", "e", "f", "g"]);
        assert_eq!(s.lists().scroll(ListId::Wifi).offset(), 0.0);
    }

    /// The dock changes without a reading, and the list follows at once.
    #[test]
    fn a_list_changed_by_any_event_stays_within_its_ends() {
        let mut s = state();
        let ids: Vec<std::sync::Arc<str>> = s.shelf().library().entries.iter().take(3 * crate::session::dock::DOCK_COLUMNS).map(|g| g.id.clone()).collect();
        for id in &ids {
            s.applied(Event::Entry(id.clone(), EntryAct::TogglePin));
        }
        s.applied(ListEvent::Visible(ListId::Dock, 1.0).into());
        s.applied(ListEvent::Scroll(ListId::Dock, 10.0).into());
        assert_eq!(s.lists().scroll(ListId::Dock).offset(), 2.0);
        for id in ids.iter().skip(1) {
            s.applied(Event::Entry(id.clone(), EntryAct::TogglePin));
        }
        assert_eq!(s.lists().len(ListId::Dock), 1);
        assert_eq!(s.lists().scroll(ListId::Dock).offset(), 0.0);
    }

    /// A row pressed stays lit as the list changes under the finger, where the tap would act.
    #[test]
    fn the_pressed_item_is_lit_where_it_is_now() {
        let mut s = state();
        networks(&mut s, &["a", "b", "c"]);
        let row = ListId::Wifi.row(2);
        let item = s.lists().item_key(&row);
        s.applied(PointerEvent::Down(row, item).into());
        networks(&mut s, &["b", "c"]);
        assert_eq!(s.lists().pressed(), Some(ListId::Wifi.row(1)));
        networks(&mut s, &["b"]);
        assert_eq!(s.lists().pressed(), None, "gone");
    }

    #[test]
    fn the_pads_focus_on_a_row_shows_it() {
        let mut s = state();
        networks(&mut s, &["a", "b", "c", "d", "e", "f"]);
        s.applied(ListEvent::Visible(ListId::Wifi, 2.0).into());
        s.applied(PointerEvent::Focus(ListId::Wifi.row(4)).into());
        assert_eq!(s.lists().scroll(ListId::Wifi).offset(), 3.0);
        s.applied(PointerEvent::Focus(ListId::Wifi.row(0)).into());
        assert_eq!(s.lists().scroll(ListId::Wifi).offset(), 0.0);
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
