//! Shelf's navigation. Each collection keeps its own place: the groups opened in it, such as a series and
//! then a season, and a cursor over the list at each level, kept in range and on screen. Changes to the
//! library, such as plays and favourites, keep the current collection and the selection.
//!
//! Shelf's rules are here too: its targets, the pad's steps through it, typing to find a title, the
//! details' actions and the file actions. A change to the place marks it to be saved (see
//! [`crate::State::resume`]), and a focus in shelf's list follows the selection. What needs another
//! area, such as playing an entry, is queued as an event.

pub mod actions;
pub mod files;
mod rules;

use std::sync::Arc;

use crate::kit::list::{Rows, Scroll};
use crate::{Collection, Entry, Library};

/// A list's place: the selected entry's position, and the rows scrolled past. The rows are whole at rest
/// and fractional while a finger drags the list.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct Cursor {
    pub index: usize,
    pub top: f32,
}

/// A list's place as kept: the selected entry's position and the list's scroll, what has been typed to
/// find an entry, and the entries whose titles start with it. While the typed text isn't empty, the list
/// shows only those entries.
#[derive(Debug, Clone, Default)]
struct Place {
    index: usize,
    scroll: Scroll,
    filter: String,
    found: Vec<usize>,
}

/// A collection's place: the groups opened, outermost first, and the place in the list at each level,
/// which is the collection's list and then each open group's.
#[derive(Debug, Clone)]
struct Nav {
    open: Vec<usize>,
    places: Vec<Place>,
}

impl Default for Nav {
    fn default() -> Nav {
        Nav { open: Vec::new(), places: vec![Place::default()] }
    }
}

#[derive(Debug, Default)]
pub struct Shelf {
    library: Library,
    /// The current collection, and each collection's place.
    current: usize,
    navs: Vec<Nav>,
    /// How many list rows are on screen, as reported by the UI.
    rows: usize,
    /// The id of the selected entry, at the current list's cursor. It is shown in shelf's list on the top
    /// screen and in its settings below.
    selected: Option<Arc<str>>,
    /// The listings of browsed folders, by folder, in the order they came; a folder's listing comes before
    /// those of the folders in it. They are applied again when the library is read again.
    listings: Vec<(Arc<str>, Vec<Entry>)>,
    /// The place changed since it was last saved (see [`crate::State::resume`]).
    unsaved: bool,
    /// The clipboard and confirmation for file actions, and the folder being browsed (see [`files`]).
    files: files::Files,
    /// Whether metadata is being fetched, and what the list showed when the shown entries were last asked
    /// for first (see [`Shelf::follow_shown`]).
    fetching: bool,
    shown: Option<ShelfView>,
}

/// What shelf's list shows: the collection, the open group, the first row shown, how many rows, and the
/// selected position.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct ShelfView {
    collection: usize,
    group: Option<usize>,
    top: usize,
    rows: usize,
    selected: usize,
}

impl Shelf {
    pub(crate) fn new(library: Library) -> Shelf {
        let navs = vec![Nav::default(); library.collections.len()];
        let mut shelf = Shelf { library, navs, rows: 1, ..Shelf::default() };
        shelf.sync();
        shelf
    }

    pub fn library(&self) -> &Library {
        &self.library
    }

    /// The current collection's position.
    pub fn current(&self) -> usize {
        self.current
    }

    pub fn rows(&self) -> usize {
        self.rows
    }

    pub fn selected(&self) -> Option<&Arc<str>> {
        self.selected.as_ref()
    }

    pub fn collection(&self) -> Option<&Collection> {
        self.library.collections.get(self.current)
    }

    /// The group open in the current collection (the innermost), if any.
    pub fn group(&self) -> Option<&Entry> {
        let g = *self.navs.get(self.current)?.open.last()?;
        self.library.entries.get(g)
    }

    /// Whether there is a place for each collection, the current collection is among them unless there
    /// are none, and each place has a list place for the collection and for each open group.
    fn navs_consistent(&self) -> bool {
        self.navs.len() == self.library.collections.len()
            && (self.navs.is_empty() || self.current < self.navs.len())
            && self.navs.iter().all(|n| n.places.len() == n.open.len() + 1)
    }

    /// The current list's place.
    fn place(&self) -> Option<&Place> {
        debug_assert!(self.navs_consistent(), "a place for each collection and open group: {:?}, current {}", self.navs, self.current);
        self.navs.get(self.current).and_then(|n| n.places.last())
    }

    fn place_mut(&mut self) -> Option<&mut Place> {
        debug_assert!(self.navs_consistent(), "a place for each collection and open group: {:?}, current {}", self.navs, self.current);
        self.navs.get_mut(self.current).and_then(|n| n.places.last_mut())
    }

    pub fn cursor(&self) -> Cursor {
        self.place().map_or_else(Cursor::default, |p| Cursor { index: p.index, top: p.scroll.offset() })
    }

    /// The current list's scroll.
    pub(crate) fn scroll(&self) -> &Scroll {
        self.place().map_or(&crate::kit::list::EMPTY, |p| &p.scroll)
    }

    pub(crate) fn scroll_mut(&mut self) -> Option<&mut Scroll> {
        self.place_mut().map(|p| &mut p.scroll)
    }

    /// An entry by its id.
    pub fn entry(&self, id: &str) -> Option<&Entry> {
        self.library.entry(id)
    }

    /// The library entry, not a browsed one, for the file of browsed entry `id`, if there is one.
    pub(crate) fn entry_of_file(&self, id: &str) -> Option<Arc<str>> {
        // An archive counts even though it is browsed as a folder, since a ROM's zip is a file in the
        // library.
        let file = self.entry(id).filter(|g| g.browsed.as_ref().is_some_and(|b| !b.folder || b.archive))?.file.as_ref()?;
        self.library.entries.iter().find(|g| g.browsed.is_none() && !g.is_group() && g.file.as_ref() == Some(file)).map(|g| g.id.clone())
    }

    /// Collection `c`'s list at its current place: the innermost open group's entries, or else the
    /// collection's own. While something is typed, it is only the entries found by it.
    fn list_of(&self, c: usize) -> &[usize] {
        match self.navs.get(c).and_then(|n| n.places.last()).filter(|p| !p.filter.is_empty()) {
            Some(p) => &p.found,
            None => self.whole_list_of(c),
        }
    }

    /// Collection `c`'s list at its current place, all of it, ignoring what is typed.
    fn whole_list_of(&self, c: usize) -> &[usize] {
        match self.navs.get(c).and_then(|n| n.open.last()) {
            Some(&g) => self.library.entries.get(g).map_or(&[], |g| g.items.as_slice()),
            None => self.library.collections.get(c).map_or(&[], |c| c.entries.as_slice()),
        }
    }

    /// What is typed to find an entry in the current list; empty when nothing is.
    pub fn filter(&self) -> &str {
        self.place().map_or("", |p| p.filter.as_str())
    }

    /// Sets what is typed in the current list to `filter`. The list then shows its entries whose titles
    /// start with it, ignoring case, with the first selected. Returns whether the selection changed.
    pub(crate) fn set_filter(&mut self, filter: String) -> bool {
        let found = self.found(self.current, &filter);
        if let Some(p) = self.place_mut() {
            p.filter = filter;
            p.found = found;
            p.index = 0;
        }
        self.sync()
    }

    /// The entries of collection `c`'s whole list whose titles start with `filter`, ignoring case.
    fn found(&self, c: usize, filter: &str) -> Vec<usize> {
        let filter = filter.to_lowercase();
        let games = &self.library.entries;
        self.whole_list_of(c).iter().copied().filter(|&i| games.get(i).is_some_and(|g| g.title.to_lowercase().starts_with(&filter))).collect()
    }

    /// Finds every list's entries again by what is typed in it, after the library changed under them.
    fn refind(&mut self) {
        for c in 0..self.navs.len() {
            let filter = self.navs.get(c).and_then(|n| n.places.last()).map(|p| p.filter.clone()).unwrap_or_default();
            if filter.is_empty() {
                continue;
            }
            let found = self.found(c, &filter);
            if let Some(p) = self.navs.get_mut(c).and_then(|n| n.places.last_mut()) {
                p.found = found;
            }
        }
    }

    /// The current list: the current collection's, or its open group's.
    pub fn list(&self) -> &[usize] {
        self.list_of(self.current)
    }

    /// The entry at `position` in the current list.
    pub fn entry_at(&self, position: usize) -> Option<&Entry> {
        self.library.entries.get(*self.list().get(position)?)
    }

    /// The selected entry's index in the library.
    pub fn selected_index(&self) -> Option<usize> {
        self.list().get(self.cursor().index).copied()
    }

    pub fn selected_entry(&self) -> Option<&Entry> {
        self.entry_at(self.cursor().index)
    }

    /// Where `game` is in collection `c`'s list.
    fn position_in(&self, c: usize, game: &str) -> Option<usize> {
        self.list_of(c).iter().position(|&i| self.library.entries.get(i).is_some_and(|g| &*g.id == game))
    }

    /// Shelf went to another collection or list. The place is saved, and a focus in shelf's list follows
    /// the selection.
    pub(crate) fn moved(&mut self, pointer: &mut crate::Pointer) {
        self.unsaved = true;
        pointer.follow_shelf(self.selected());
    }

    /// Whether the place changed since this was last asked, so it is saved.
    pub(crate) fn take_unsaved(&mut self) -> bool {
        std::mem::take(&mut self.unsaved)
    }

    /// Keeps the cursor in range and on screen, and `selected` in step with it. Returns whether the
    /// selection changed.
    pub(crate) fn sync(&mut self) -> bool {
        let before = self.selected.clone();
        let len = self.list().len();
        let rows = self.rows.max(1) as f32;
        if let Some(p) = self.place_mut() {
            p.index = p.index.min(len.saturating_sub(1));
            p.scroll.set_visible(rows, len);
            p.scroll.show(p.index, len);
        }
        self.selected = self.selected_entry().map(|g| g.id.clone());
        self.selected != before
    }

    /// Moves the cursor by `step` entries, stopping at the ends. Returns whether the selection changed.
    pub(crate) fn step(&mut self, step: i32) -> bool {
        let len = self.list().len();
        if let Some(p) = self.place_mut() {
            let target = p.index as i64 + i64::from(step);
            p.index = target.clamp(0, len.saturating_sub(1) as i64) as usize;
        }
        self.sync()
    }

    /// Selects `game` in the current list, if it is there. Returns whether the selection changed.
    pub(crate) fn select(&mut self, game: &str) -> bool {
        let position = self.position_in(self.current, game);
        if let (Some(i), Some(p)) = (position, self.place_mut()) {
            p.index = i;
        }
        self.sync()
    }

    /// What the list shows, to tell when it changed.
    pub(crate) fn view(&self) -> ShelfView {
        ShelfView {
            collection: self.current,
            group: self.navs.get(self.current).and_then(|n| n.open.last().copied()),
            top: self.cursor().top.floor().max(0.0) as usize,
            rows: self.rows,
            selected: self.cursor().index,
        }
    }

    /// Adds the entries in view to `ids`: the selected one first, then the rows shown, then the groups the
    /// list is in, such as a series when its episodes show.
    fn in_view(&self, ids: &mut Vec<Arc<str>>) {
        let ShelfView { group, top, rows, selected, .. } = self.view();
        let list = self.list();
        let rows = (top..top + rows + 1).filter(|&p| p != selected);
        let id = |i: usize| self.library.entries.get(i).map(|g| g.id.clone());
        ids.extend(std::iter::once(selected).chain(rows).filter_map(|p| list.get(p).copied()).filter_map(id));
        if let Some(g) = group {
            ids.extend(self.library.groups_of(g).into_iter().chain([g]).filter_map(id));
        }
    }

    /// Opens the selected entry if it is a group, showing its list from the first entry. Returns whether it
    /// did.
    pub(crate) fn open(&mut self) -> bool {
        let Some(&g) = self.list().get(self.cursor().index) else {
            return false;
        };
        if !self.library.entries.get(g).is_some_and(Entry::is_group) {
            return false;
        }
        if let Some(n) = self.navs.get_mut(self.current) {
            n.open.push(g);
            n.places.push(Place::default());
        }
        self.sync();
        true
    }

    /// Closes the open group, going back to the list it is in with the group selected. Returns whether
    /// there was one.
    pub(crate) fn up(&mut self) -> bool {
        let Some(n) = self.navs.get_mut(self.current).filter(|n| !n.open.is_empty()) else {
            return false;
        };
        n.open.pop();
        n.places.pop();
        self.sync();
        true
    }

    /// Shows `group` open, in the current collection if that has the group's outermost group, or else in
    /// the first collection that does. Returns whether the group is there.
    pub(crate) fn reveal(&mut self, group: &str) -> bool {
        let Some(g) = self.library.entries.iter().position(|x| &*x.id == group) else {
            return false;
        };
        let mut path = self.library.groups_of(g);
        path.push(g);
        let Some(&root) = path.first() else {
            return false;
        };
        let has = |c: &Collection| c.entries.contains(&root);
        let Some(c) = self.collection().filter(|c| has(c)).map(|_| self.current).or_else(|| self.library.collections.iter().position(has)) else {
            return false;
        };
        self.current = c;
        debug_assert!(self.navs.get(c).is_some(), "a place for collection {c}");
        if let Some(n) = self.navs.get_mut(c) {
            *n = Nav::default();
        }
        for id in path.iter().filter_map(|&i| self.library.entries.get(i)).map(|g| g.id.clone()).collect::<Vec<_>>() {
            self.select(&id);
            self.open();
        }
        self.sync();
        true
    }

    /// Makes collection `i` the current one, if it exists. Returns whether it does.
    pub(crate) fn set_collection(&mut self, i: usize) -> bool {
        if i >= self.library.collections.len() {
            return false;
        }
        self.current = i;
        self.sync();
        true
    }

    /// Moves `step` collections on, wrapping around. Returns whether there are any collections.
    pub(crate) fn collection_step(&mut self, step: i32) -> bool {
        let n = self.library.collections.len() as i64;
        n > 0 && self.set_collection((self.current as i64 + i64::from(step)).rem_euclid(n) as usize)
    }

    /// Sets how many rows are on screen. Returns whether the cursor moved to keep the selection on screen.
    pub(crate) fn set_rows(&mut self, rows: usize) -> bool {
        if rows == self.rows {
            return false;
        }
        let before = self.cursor();
        self.rows = rows.max(1);
        self.sync();
        self.cursor() != before
    }

    /// Changes the library, for favourites or plays, keeping the current collection, each collection's
    /// place and the selected entry. These are kept by name, since derived collections may come or go.
    /// Returns whether the selection changed.
    pub(crate) fn relayout(&mut self, change: impl FnOnce(&mut Library)) -> bool {
        let names: Vec<String> = self.library.collections.iter().map(|c| c.name.clone()).collect();
        let current_name = names.get(self.current).cloned();
        let selected = self.selected.clone();
        change(&mut self.library);
        let old = std::mem::take(&mut self.navs);
        // Derived collections come and go, so places are matched by name. What is typed in each list is
        // found again below.
        self.navs =
            self.library.collections.iter().map(|c| names.iter().position(|n| *n == c.name).and_then(|i| old.get(i).cloned()).unwrap_or_default()).collect();
        self.current = current_name.and_then(|n| self.library.collections.iter().position(|c| c.name == n)).unwrap_or(0);
        self.refind();
        if let Some(id) = selected {
            self.select(&id);
        }
        self.sync()
    }

    /// Takes the library read again. The listings of browsed folders that are still there are applied to
    /// it again, and each collection's place is kept by name and ids, with the scroll starting from the
    /// top. Returns whether the selection changed.
    pub(crate) fn reload(&mut self, library: Library) -> bool {
        let before = self.selected.clone();
        let places = self.places();
        let current = self.collection().map(|c| c.name.clone());
        let (rows, unsaved, fetching) = (self.rows, self.unsaved, self.fetching);
        let listings = std::mem::take(&mut self.listings);
        let files = std::mem::take(&mut self.files);
        // What shows is asked for first again, from the new library.
        *self = Shelf { files, fetching, ..Shelf::new(library) };
        (self.rows, self.unsaved) = (rows, unsaved);
        for (folder, entries) in listings {
            if let Some(f) = self.library.entries.iter().position(|g| g.id == folder) {
                self.library.set_listing(f, entries.clone());
                self.listings.push((folder, entries));
            }
        }
        for (name, ids) in &places {
            let ids: Vec<&str> = ids.iter().map(String::as_str).collect();
            self.restore_selection(name, &ids);
        }
        self.restore_current(current.as_deref().unwrap_or(""));
        self.selected != before
    }

    /// Takes browsed folder `folder`'s entries as they are now (see [`Library::set_listing`]). The
    /// selected entry is kept by id, and open groups that are gone are closed. Returns whether the
    /// selection changed.
    pub(crate) fn listed(&mut self, folder: Arc<str>, entries: Vec<Entry>) -> bool {
        let Some(f) = self.library.entries.iter().position(|g| g.id == folder) else {
            return false;
        };
        let before = self.selected.clone();
        self.listings.retain(|(id, _)| *id != folder);
        self.listings.push((folder, entries.clone()));
        self.library.set_listing(f, entries);
        // Drops the listings of folders that went with it.
        let library = &self.library;
        self.listings.retain(|(id, _)| library.entries.iter().any(|g| g.id == *id));
        self.close_gone();
        self.refind();
        if let Some(id) = &before {
            self.select(&id.clone());
        }
        self.sync();
        self.selected != before
    }

    /// Keeps each collection's open groups only as far as each is still in the list of the group before
    /// it, or of the collection for the first one.
    fn close_gone(&mut self) {
        for (c, nav) in self.navs.iter_mut().enumerate() {
            let mut list = self.library.collections.get(c).map_or(&[][..], |c| c.entries.as_slice());
            let mut keep = 0;
            for &g in &nav.open {
                if !list.contains(&g) || !self.library.entries.get(g).is_some_and(Entry::is_group) {
                    break;
                }
                keep += 1;
                list = self.library.entries.get(g).map_or(&[], |g| g.items.as_slice());
            }
            nav.open.truncate(keep);
            nav.places.truncate(keep + 1);
        }
    }

    /// Records where mpv left a video, at `now`. None means at its end.
    pub(crate) fn set_position(&mut self, id: &str, position: Option<u32>, now: u64) {
        if let Some(g) = self.library.entries.iter_mut().find(|g| &*g.id == id) {
            g.playback.position = position;
            g.playback.at = now;
        }
    }

    /// Applies what was fetched about entry `id` (see [`Library::apply_meta`]). Returns whether the entry
    /// is there.
    pub(crate) fn apply_meta(&mut self, id: &str, meta: &crate::Meta) -> bool {
        let Some(i) = self.library.entries.iter().position(|g| &*g.id == id) else {
            return false;
        };
        self.library.apply_meta(i, meta);
        true
    }

    /// Puts the selected entry in or out of the favourites, if there is one. Returns whether the selection
    /// changed.
    fn favourite_selected(&mut self) -> Option<bool> {
        let id = self.selected.clone()?;
        Some(self.relayout(|lib| {
            if let Some(g) = lib.entries.iter_mut().find(|g| g.id == id) {
                g.favourite = !g.favourite;
            }
            lib.rebuild_derived();
        }))
    }

    /// Counts a play of `game`, which has ended. Returns whether the selection changed.
    pub(crate) fn add_play(&mut self, game: &str, start: u64, duration: u64) -> bool {
        self.relayout(|lib| {
            if let Some(g) = lib.entries.iter_mut().find(|g| &*g.id == game) {
                g.plays.add(start, duration);
            }
            lib.rebuild_derived();
        })
    }

    /// Counts and records a play of `game` from `start` (monotonic seconds, as in [`crate::Event::Tick`])
    /// until now, dated by the wall clock. For a video, it also reads where mpv left it, which mpv wrote as
    /// it quit.
    pub(crate) fn record_play(&mut self, game: Arc<str>, start: u64, clock: &crate::Clock, out: &mut crate::out::Out) {
        let duration = clock.mono().saturating_sub(start);
        let start = clock.unix_at(start);
        if self.add_play(&game, start, duration) {
            self.unsaved = true;
        }
        let file = self.entry(&game).and_then(|g| g.mpv_file()).map(std::path::Path::to_path_buf);
        out.effects.push(crate::Effect::RecordPlay { entry: game.clone(), start, duration });
        if let Some(file) = file {
            out.effects.push(crate::Effect::ReadPosition { entry: game, file });
        }
    }

    /// Each collection's place: its name, then its open groups' ids and its selected entry's id.
    pub(crate) fn places(&self) -> Vec<(String, Vec<String>)> {
        let mut places = Vec::new();
        for (c, coll) in self.library.collections.iter().enumerate() {
            debug_assert!(self.navs.get(c).is_some(), "a place for collection {c}");
            let Some(nav) = self.navs.get(c) else {
                continue;
            };
            let index = nav.places.last().map_or(0, |p| p.index);
            let Some(g) = self.list_of(c).get(index).and_then(|&g| self.library.entries.get(g)) else {
                continue;
            };
            let mut ids: Vec<String> = nav.open.iter().filter_map(|&o| self.library.entries.get(o)).map(|g| g.id.to_string()).collect();
            ids.push(g.id.to_string());
            places.push((coll.name.clone(), ids));
        }
        places
    }

    /// In the collection named `name`, opens the groups that `ids` names except the last, then selects the
    /// last. If one of them is gone, it stops there.
    pub(crate) fn restore_selection(&mut self, name: &str, ids: &[&str]) {
        let Some(c) = self.library.collections.iter().position(|c| c.name == name) else {
            return;
        };
        let current = std::mem::replace(&mut self.current, c);
        for (i, id) in ids.iter().enumerate() {
            let Some(p) = self.position_in(c, id) else {
                break;
            };
            if let Some(place) = self.place_mut() {
                place.index = p;
            }
            if i + 1 < ids.len() && !self.open() {
                break;
            }
        }
        self.current = current;
    }

    /// Makes the collection named `name` the current one, if there is one.
    pub(crate) fn restore_current(&mut self, name: &str) {
        if let Some(i) = self.library.collections.iter().position(|c| c.name == name) {
            self.current = i;
        }
        self.sync();
    }
}

/// The current list's entries as a list's rows. Their place is kept by position, per collection and
/// group.
impl Rows for Shelf {
    fn len(&self) -> usize {
        self.list().len()
    }
}

#[cfg(test)]
#[allow(clippy::indexing_slicing)]
mod tests {
    use crate::bottom::keyboard::Key;
    use crate::testkit::*;
    use crate::*;

    #[test]
    fn the_list_scrolls_to_keep_the_selection_on_screen() {
        let mut s = state();
        s.apply(ShelfEvent::Rows(3).into());
        s.apply(ShelfEvent::ListStep(4).into());
        assert_eq!(s.shelf().cursor(), Cursor { index: 4, top: 2.0 });
        assert_eq!(s.shelf().selected().map(|g| &**g), Some("dark-souls"));
        assert_eq!(s.pointer().focus(), Some(&game("dark-souls")));
        s.apply(ShelfEvent::ListStep(-100).into());
        assert_eq!(s.shelf().cursor(), Cursor { index: 0, top: 0.0 });
        s.apply(ShelfEvent::ListStep(100).into());
        assert_eq!(s.shelf().cursor().index, 6);
    }

    #[test]
    fn a_drag_scrolls_without_moving_the_selection_and_the_pad_brings_it_back() {
        let mut s = state();
        s.apply(ShelfEvent::Rows(3).into());
        let (dirty, _) = s.apply(ListEvent::Scroll(ListId::Shelf, 2.5).into());
        assert_eq!((dirty, s.shelf().cursor()), (Dirty::BOTH, Cursor { index: 0, top: 2.5 }));
        s.apply(ListEvent::Scroll(ListId::Shelf, 100.0).into());
        assert_eq!(s.shelf().cursor().top, 4.0, "the last game on the last row");
        let can_scroll = |rows| s.scroll(ListId::Shelf).can_scroll(rows, s.list_len(ListId::Shelf));
        assert!(!can_scroll(1.0) && can_scroll(-1.0));
        s.apply(ShelfEvent::ListStep(1).into());
        assert_eq!(s.shelf().cursor(), Cursor { index: 1, top: 1.0 }, "the selection on screen again");
    }

    #[test]
    fn collections_keep_their_own_place() {
        let mut s = state();
        s.apply(ShelfEvent::ListStep(2).into());
        s.apply(ShelfEvent::CollectionStep(1).into());
        assert_eq!(s.shelf().current(), 1);
        assert_eq!(s.shelf().cursor().index, 0);
        s.apply(ShelfEvent::CollectionStep(4).into());
        assert_eq!(s.shelf().current(), 0, "wraps around (past TV, Apps and Tools)");
        assert_eq!(s.shelf().cursor().index, 2);
    }

    #[test]
    fn a_favourite_shows_in_its_collection_and_is_saved() {
        let mut s = state();
        s.apply(ShelfEvent::ListStep(2).into());
        let name = s.shelf().collection().map(|c| c.name.clone());
        let (_, fx) = s.apply(ShelfEvent::ToggleFavourite.into());
        assert_eq!(fx, vec![Effect::Save(Save::Favourites(vec!["mk-ds".into()]))]);
        assert!(s.shelf().library().collections.iter().any(|c| c.name == "Favourites" && c.entries == vec![2]));
        assert_eq!(s.shelf().collection().map(|c| c.name.clone()), name, "the collection stays");
        assert_eq!(s.shelf().selected().map(|g| &**g), Some("mk-ds"), "so does the selection");
    }

    #[test]
    fn typing_with_shelf_in_front_finds_entries_by_their_titles_start() {
        use crate::bottom::keyboard::find;
        let mut s = state();
        tap(&mut s, Id::Tab(crate::Tab::Keyboard));
        let key = |s: &mut State, k| find(k).map(|pos| tap(s, Id::Key(pos))).unwrap_or_default();
        let titles = |s: &State| s.shelf().list().iter().filter_map(|&i| s.shelf().library().entries.get(i)).map(|g| g.title.clone()).collect::<Vec<_>>();
        assert!(key(&mut s, Key::Char('m')).is_empty(), "not sent to an app: shelf is in front");
        assert_eq!(titles(&s), ["Metroid: Zero Mission", "Mario Kart DS"]);
        key(&mut s, Key::Char('a'));
        assert_eq!(titles(&s), ["Mario Kart DS"], "any case: Ma");
        assert_eq!(s.shelf().selected().map(|g| &**g), Some("mk-ds"));
        // Going into a folder and back out, each list keeps what was typed in it.
        key(&mut s, Key::Named(crate::bottom::keyboard::Named::Backspace));
        assert_eq!(titles(&s).len(), 2);
        s.apply(ShelfEvent::ClearFilter.into());
        assert_eq!(s.shelf().filter(), "");
        assert_eq!(titles(&s).len(), 7);
        tap(&mut s, Id::Shelf(ShelfId::Collection(2)));
        key(&mut s, Key::Char('c'));
        assert_eq!(titles(&s), ["Cosmos"]);
        let fx = key(&mut s, Key::Named(crate::bottom::keyboard::Named::Enter));
        assert!(fx.is_empty() && s.shelf().group().is_some(), "Enter: opened");
        assert_eq!(s.shelf().filter(), "", "the folder's own list");
        tap(&mut s, Id::Shelf(ShelfId::Up));
        assert_eq!(s.shelf().filter(), "c", "the series' list as it was left");
    }

    #[test]
    fn a_group_opens_and_back_goes_out_to_it() {
        let mut s = state();
        tap(&mut s, Id::Shelf(ShelfId::Collection(2)));
        assert_eq!(tap(&mut s, game("tv:Cosmos")), vec![], "selected already: opened, nothing launched");
        assert_eq!(s.shelf().group().map(|g| g.title.as_str()), Some("Cosmos"));
        let titles = |s: &State| s.shelf().list().iter().map(|&i| s.shelf().library().entries[i].title.clone()).collect::<Vec<_>>();
        assert_eq!(titles(&s), vec!["Season 1", "Season 2"]);
        s.apply(ShelfEvent::ListStep(1).into());
        tap(&mut s, Id::Shelf(ShelfId::Launch));
        assert_eq!(titles(&s), vec!["1. Standing Up in the Milky Way"]);
        let fx = tap(&mut s, Id::Shelf(ShelfId::Launch));
        assert!(fx.iter().any(|e| matches!(e, Effect::Launch { entry: game, .. } if game.ends_with("Milky Way.mkv"))), "an episode plays: {fx:?}");
        tap(&mut s, Id::Shelf(ShelfId::Up));
        assert_eq!(s.shelf().selected().map(|g| &**g), Some("tv:Cosmos/Season 2"), "back where it was");
        tap(&mut s, Id::Shelf(ShelfId::Up));
        assert_eq!(s.shelf().group(), None);
        assert_eq!(s.shelf().selected().map(|g| &**g), Some("tv:Cosmos"));
        assert!(!tap(&mut s, Id::Shelf(ShelfId::Up)).iter().any(|_| true) && s.shelf().group().is_none(), "nothing to go out of");
    }

    #[test]
    fn a_group_started_from_elsewhere_opens_in_shelf() {
        let mut s = state();
        let fx = launch_now(&mut s, "tv:Cosmos/Season 1");
        assert_eq!(fx, vec![Effect::Window(WindowCommand::ShowShelf)]);
        assert_eq!(s.shelf().collection().map(|c| c.name.as_str()), Some("TV"));
        assert_eq!(s.shelf().group().map(|g| &*g.id), Some("tv:Cosmos/Season 1"));
        assert_eq!(s.shelf().selected().map(|g| &**g), Some("tv:Cosmos/Season 1/1. The Shores of the Cosmic Ocean.mkv"));
        assert!(s.session().launching.is_empty(), "nothing started");
    }

    #[test]
    fn the_details_actions_toggle_the_favourite_and_the_pin() {
        let mut s = state();
        let fx = tap(&mut s, Id::Shelf(ShelfId::Favourite));
        assert_eq!(fx, vec![Effect::Save(Save::Favourites(vec!["zelda-lttp".into()]))]);
        let fx = tap(&mut s, Id::Shelf(ShelfId::Pin));
        assert!(matches!(fx.as_slice(), [Effect::Save(Save::Dock(ids))] if ids.len() == 1), "{fx:?}");
        assert!(s.dock().is_pinned("zelda-lttp"));
    }

    #[test]
    fn continue_plays_on_where_the_series_was_left() {
        let mut s = state();
        tap(&mut s, Id::Shelf(ShelfId::Collection(2)));
        let first = "tv:Cosmos/Season 1/1. The Shores of the Cosmic Ocean.mkv";
        let launched = |fx: &[Effect]| {
            fx.iter().find_map(|e| match e {
                Effect::Launch { entry: game, .. } => Some(game.to_string()),
                _ => None,
            })
        };
        // The demo has the second episode under way. This test marks it as watched to its end instead.
        s.apply(LaunchEvent::Position { entry: "tv:Cosmos/Season 1/2. One Voice in the Cosmic Fugue.mkv".into(), position: None }.into());
        let fx = tap(&mut s, Id::Shelf(ShelfId::Continue));
        assert_eq!(launched(&fx).as_deref(), Some(first), "none watched: the first");
        crate::testkit::start(&mut s, first);
        s.apply(LaunchEvent::Position { entry: first.into(), position: Some(600) }.into());
        end(&mut s, first);
        let fx = s.apply(ShelfEvent::Continue.into()).1;
        assert_eq!(launched(&fx).as_deref(), Some(first), "left early: it again");
        crate::testkit::start(&mut s, first);
        s.apply(LaunchEvent::Position { entry: first.into(), position: None }.into());
        end(&mut s, first);
        let fx = s.apply(ShelfEvent::Continue.into()).1;
        assert_eq!(launched(&fx).as_deref(), Some("tv:Cosmos/Season 1/2. One Voice in the Cosmic Fugue.mkv"), "watched to its end: the next");
    }

    #[test]
    fn selecting_then_activating_again_launches() {
        let mut s = state();
        assert!(tap(&mut s, game("ff6")).is_empty());
        let fx = tap(&mut s, game("ff6"));
        assert!(
            fx.iter().any(
                |e| matches!(e, Effect::Launch { entry: game, settings, env, launch: Launch::None, .. } if &**game == "ff6" && *settings == Values::default() && env.is_empty())
            ),
            "{fx:?}"
        );
    }
}
