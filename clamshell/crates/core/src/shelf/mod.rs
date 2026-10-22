//! Shelf's navigation: the collections, each with its own place: the groups opened in it (a series, then
//! a season) and a cursor over each level's list, kept in range and on screen; and changes to the
//! library (plays, favourites) that keep the collection and selection. The methods say whether the
//! selection changed; the state saves the place and moves the focus around them.

pub mod actions;
pub mod files;
mod rules;

use std::sync::Arc;

use crate::kit::list::{Rows, Scroll};
use crate::{Collection, Entry, Library};

/// A list's place: the selected entry's position, and the rows scrolled past (whole rows at rest;
/// fractions while a finger drags the list).
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct Cursor {
    pub index: usize,
    pub top: f32,
}

/// A list's place as kept: the selected entry's position and the list's scroll; what is typed to find an
/// entry by, and the entries whose titles start with it (the list shown while it isn't empty).
#[derive(Debug, Clone, Default)]
struct Place {
    index: usize,
    scroll: Scroll,
    filter: String,
    found: Vec<usize>,
}

/// A collection's place: the groups opened, outermost first, and the place in each level's list (the
/// collection's, then each open group's).
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
    /// List rows on screen (reported by the UI).
    rows: usize,
    /// Id of the selected entry (shelf's list on top, its settings below): the current list's cursor's.
    selected: Option<Arc<str>>,
    /// Browsed folders' listings by folder, in the order they came (a folder's before those of the
    /// folders in it): applied again to the library read again.
    listings: Vec<(Arc<str>, Vec<Entry>)>,
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

    /// The current list's place.
    fn place(&self) -> Option<&Place> {
        self.navs.get(self.current).and_then(|n| n.places.last())
    }

    fn place_mut(&mut self) -> Option<&mut Place> {
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
        self.library.entries.iter().find(|g| &*g.id == id)
    }

    /// The library entry (not browsed) of browsed file `id`'s file, if there is one.
    pub(crate) fn entry_of_file(&self, id: &str) -> Option<Arc<str>> {
        // (An archive, though browsed as a folder: a ROM's zip is a file of the library's.)
        let file = self.entry(id).filter(|g| g.browsed.as_ref().is_some_and(|b| !b.folder || b.archive))?.file.as_ref()?;
        self.library.entries.iter().find(|g| g.browsed.is_none() && !g.is_group() && g.file.as_ref() == Some(file)).map(|g| g.id.clone())
    }

    /// Collection `c`'s list as its place has it: the innermost open group's entries, else its own; those
    /// found by what is typed, while something is.
    fn list_of(&self, c: usize) -> &[usize] {
        match self.navs.get(c).and_then(|n| n.places.last()).filter(|p| !p.filter.is_empty()) {
            Some(p) => &p.found,
            None => self.whole_list_of(c),
        }
    }

    /// Collection `c`'s list as its place has it, all of it (what is typed aside).
    fn whole_list_of(&self, c: usize) -> &[usize] {
        match self.navs.get(c).and_then(|n| n.open.last()) {
            Some(&g) => self.library.entries.get(g).map_or(&[], |g| g.items.as_slice()),
            None => self.library.collections.get(c).map_or(&[], |c| c.entries.as_slice()),
        }
    }

    /// What is typed to find an entry in the current list by (empty: nothing).
    pub fn filter(&self) -> &str {
        self.place().map_or("", |p| p.filter.as_str())
    }

    /// What is typed in the current list now `filter`: its entries whose titles start with it (any case),
    /// the first selected. Whether the selection changed.
    pub(crate) fn set_filter(&mut self, filter: String) -> bool {
        let found = self.found(self.current, &filter);
        if let Some(p) = self.place_mut() {
            p.filter = filter;
            p.found = found;
            p.index = 0;
        }
        self.sync()
    }

    /// The entries of collection `c`'s whole list whose titles start with `filter` (any case).
    fn found(&self, c: usize, filter: &str) -> Vec<usize> {
        let filter = filter.to_lowercase();
        let games = &self.library.entries;
        self.whole_list_of(c).iter().copied().filter(|&i| games.get(i).is_some_and(|g| g.title.to_lowercase().starts_with(&filter))).collect()
    }

    /// Every list's entries found again by what is typed in it (the library changed under them).
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

    /// Keeps the cursor in range and on screen, and `selected` in step with it. Whether the selection
    /// changed.
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

    /// Moves the cursor by `step` entries (clamped at the ends). Whether the selection changed.
    pub(crate) fn step(&mut self, step: i32) -> bool {
        let len = self.list().len();
        if let Some(p) = self.place_mut() {
            let target = p.index as i64 + i64::from(step);
            p.index = target.clamp(0, len.saturating_sub(1) as i64) as usize;
        }
        self.sync()
    }

    /// Selects `game` in the current list, if it is there. Whether the selection changed.
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

    /// The entries in view, the selected first, then the rows shown, then the groups the list is in (a
    /// series, when its episodes show).
    pub(crate) fn shown(&self) -> Vec<Arc<str>> {
        let ShelfView { group, top, rows, selected, .. } = self.view();
        let list = self.list();
        let rows = (top..top + rows + 1).filter(|&p| p != selected);
        let mut indices: Vec<usize> = std::iter::once(selected).chain(rows).filter_map(|p| list.get(p).copied()).collect();
        if let Some(g) = group {
            indices.extend(self.library.groups_of(g));
            indices.push(g);
        }
        indices.iter().filter_map(|&i| self.library.entries.get(i)).map(|g| g.id.clone()).collect()
    }

    /// Opens the selected entry if it is a group (its list shown, from its first entry). Whether it did.
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

    /// Closes the open group (back to the list it is in, it selected). Whether there was one.
    pub(crate) fn up(&mut self) -> bool {
        let Some(n) = self.navs.get_mut(self.current).filter(|n| !n.open.is_empty()) else {
            return false;
        };
        n.open.pop();
        n.places.pop();
        self.sync();
        true
    }

    /// Shows `group` open: in the current collection if it has the group's outermost group, else the
    /// first that does. Whether it is there.
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

    /// Makes collection `i` the current one, if there is one there (whether there was).
    pub(crate) fn set_collection(&mut self, i: usize) -> bool {
        if i >= self.library.collections.len() {
            return false;
        }
        self.current = i;
        self.sync();
        true
    }

    /// Goes `step` collections on, wrapping (whether there are any).
    pub(crate) fn collection_step(&mut self, step: i32) -> bool {
        let n = self.library.collections.len() as i64;
        n > 0 && self.set_collection((self.current as i64 + i64::from(step)).rem_euclid(n) as usize)
    }

    /// How many rows are on screen. Whether the cursor moved (to keep the selection on screen).
    pub(crate) fn set_rows(&mut self, rows: usize) -> bool {
        if rows == self.rows {
            return false;
        }
        let before = self.cursor();
        self.rows = rows.max(1);
        self.sync();
        self.cursor() != before
    }

    /// Changes the library (favourites, plays) keeping the current collection, each one's place and the
    /// selected entry, all by name, since derived collections may come or go. Whether the selection
    /// changed.
    pub(crate) fn relayout(&mut self, change: impl FnOnce(&mut Library)) -> bool {
        let names: Vec<String> = self.library.collections.iter().map(|c| c.name.clone()).collect();
        let current_name = names.get(self.current).cloned();
        let selected = self.selected.clone();
        change(&mut self.library);
        let old = std::mem::take(&mut self.navs);
        // (Derived collections come and go: what is typed in each found again below.)
        self.navs =
            self.library.collections.iter().map(|c| names.iter().position(|n| *n == c.name).and_then(|i| old.get(i).cloned()).unwrap_or_default()).collect();
        self.current = current_name.and_then(|n| self.library.collections.iter().position(|c| c.name == n)).unwrap_or(0);
        self.refind();
        if let Some(id) = selected {
            self.select(&id);
        }
        self.sync()
    }

    /// The library read again: the browsed folders' listings in it again (those still there), each
    /// collection's place kept by name and ids (the scroll from the top). Whether the selection changed.
    pub(crate) fn reload(&mut self, library: Library) -> bool {
        let before = self.selected.clone();
        let places = self.places();
        let current = self.collection().map(|c| c.name.clone());
        let rows = self.rows;
        let listings = std::mem::take(&mut self.listings);
        *self = Shelf::new(library);
        self.rows = rows;
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

    /// Browsed folder `folder`'s entries now (see [`Library::set_listing`]): the selected entry kept by
    /// id, the groups open that went closed. Whether the selection changed.
    pub(crate) fn listed(&mut self, folder: Arc<str>, entries: Vec<Entry>) -> bool {
        let Some(f) = self.library.entries.iter().position(|g| g.id == folder) else {
            return false;
        };
        let before = self.selected.clone();
        self.listings.retain(|(id, _)| *id != folder);
        self.listings.push((folder, entries.clone()));
        self.library.set_listing(f, entries);
        // (The listings of folders gone with it.)
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

    /// Each collection's groups open, as far as each is still in the one before (or the collection).
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

    /// Where mpv left a video (none: at its end), at `now`.
    pub(crate) fn set_position(&mut self, id: &str, position: Option<u32>, now: u64) {
        if let Some(g) = self.library.entries.iter_mut().find(|g| &*g.id == id) {
            g.playback.position = position;
            g.playback.at = now;
        }
    }

    /// What was fetched about entry `id` (see [`Library::apply_meta`]). Whether it is there.
    pub(crate) fn apply_meta(&mut self, id: &str, meta: &crate::Meta) -> bool {
        let Some(i) = self.library.entries.iter().position(|g| &*g.id == id) else {
            return false;
        };
        self.library.apply_meta(i, meta);
        true
    }

    /// The selected entry in or out of the favourites, if there is one: whether the selection changed.
    pub(crate) fn toggle_favourite(&mut self) -> Option<bool> {
        let id = self.selected.clone()?;
        Some(self.relayout(|lib| {
            if let Some(g) = lib.entries.iter_mut().find(|g| g.id == id) {
                g.favourite = !g.favourite;
            }
            lib.rebuild_derived();
        }))
    }

    /// A play of `game` counted (it ended). Whether the selection changed.
    pub(crate) fn add_play(&mut self, game: &str, start: u64, duration: u64) -> bool {
        self.relayout(|lib| {
            if let Some(g) = lib.entries.iter_mut().find(|g| &*g.id == game) {
                g.plays.add(start, duration);
            }
            lib.rebuild_derived();
        })
    }

    /// Each collection's place: its name, then its open groups' ids and its selected entry's id.
    pub(crate) fn places(&self) -> Vec<(String, Vec<String>)> {
        let mut places = Vec::new();
        for (c, coll) in self.library.collections.iter().enumerate() {
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

    /// Opens the groups `ids` names but the last, then selects the last, in the collection named `name`
    /// (any may be gone: then it stops there).
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

/// The current list's entries as a list's rows (its place is kept by position, per collection and
/// group).
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
        assert!(!s.list_can_scroll(ListId::Shelf, 1.0) && s.list_can_scroll(ListId::Shelf, -1.0));
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
        // Into a folder and out: each list its own.
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
        let fx = s.launch("tv:Cosmos/Season 1".into());
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
        // (The demo's second episode is under way: not here.)
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
