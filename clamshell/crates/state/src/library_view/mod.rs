//! The Library's navigation. Each collection keeps its own place: the groups opened in it, such as a series and
//! then a season, and a cursor over the list at each level, kept in range and on screen. Changes to the
//! library, such as plays and favourites, keep the current collection and the selection.
//!
//! The Library's rules are here too: its targets, the pad's steps through it, typing to find a title, the
//! details' actions and the file actions. A change to the place marks it to be saved (see
//! [`crate::State::resume`]), and a focus in the Library's list follows the selection. What needs another
//! area, such as playing an entry, is queued as an event.

pub mod actions;
pub mod edit;
pub mod files;
pub mod form;
pub mod pick;
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
pub struct LibraryView {
    library: Library,
    /// Whether the library has been read. ClamShell starts before it is, with none, and it comes as
    /// [`crate::Event::Library`].
    read: bool,
    /// Where the Library was at the last start, to go back to once the library is read.
    waiting: Option<crate::Resume>,
    /// The current collection, and each collection's place.
    current: usize,
    navs: Vec<Nav>,
    /// How many list rows are on screen, as reported by the UI.
    rows: usize,
    /// The listings of browsed folders, by folder, in the order they came; a folder's listing comes before
    /// those of the folders in it. They are applied again when the library is read again.
    listings: Vec<(Arc<str>, Vec<Entry>)>,
    /// The clipboard and confirmation for file actions, and the folder being browsed (see [`files`]).
    files: files::Files,
    /// Whether metadata is being fetched (see [`LibraryView::fetch_first`]).
    fetching: bool,
    /// Counts the listings that changed the entries in the current list, so that the view changes with
    /// them (see [`ShownList`]).
    listings_shown: u32,
    /// The edit mode, while the Library is in it (see [`edit`]).
    edit: Option<edit::Edit>,
    /// The pick going on (see [`pick`]).
    pick: Option<pick::Pick>,
    /// An app's pick answered, until the state takes the answer (see [`crate::portal`]).
    answered: Option<(u32, Option<std::path::PathBuf>)>,
}

/// A level of a collection's place kept across a library read: the entry selected in its list (the
/// group opened from it, for all but the last), and what is typed in it.
type KeptLevel = (Option<Arc<str>>, String);

/// What the Library's list shows: the collection, the open group, the first row shown, how many rows, the
/// selected position, and the listings that changed the list's entries.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct ShownList {
    collection: usize,
    group: Option<usize>,
    top: usize,
    rows: usize,
    selected: usize,
    listings: u32,
}

impl LibraryView {
    pub(crate) fn new(library: Library) -> LibraryView {
        let navs = vec![Nav::default(); library.collections.len()];
        let mut library_view = LibraryView { library, read: true, navs, rows: 1, ..LibraryView::default() };
        library_view.sync();
        library_view
    }

    /// The Library before the library is read, empty. `resume` is where it was, restored once the library comes
    /// ([`LibraryView::reload`]).
    pub(crate) fn unread(resume: Option<crate::Resume>) -> LibraryView {
        let mut library_view = LibraryView { waiting: resume, rows: 1, ..LibraryView::default() };
        library_view.sync();
        library_view
    }

    pub fn library(&self) -> &Library {
        &self.library
    }

    /// The edit mode, while the Library is in it (see [`edit`]).
    pub fn edit(&self) -> Option<&edit::Edit> {
        self.edit.as_ref()
    }

    /// Enters the edit mode, at the first tab.
    pub(crate) fn start_edit(&mut self) {
        self.edit.get_or_insert_with(edit::Edit::default);
    }

    /// A tap on one of the edit mode's targets (see [`edit::Edit::tap`]); Done leaves it. Choosing a
    /// folder for the form starts a pick.
    pub(crate) fn edit_tap(&mut self, id: edit::EditId, out: &mut crate::out::Out) {
        // A form's field picked in the Library: a folder or a program.
        if let edit::EditId::Pick(i) = id {
            let field = self.edit.as_ref().and_then(|e| e.form()).and_then(|f| f.item.fields().get(usize::from(i)).copied());
            if let Some((field, takes)) = field.and_then(|f| Some((f, f.picks()?))) {
                if let Some(e) = &mut self.edit {
                    e.select_field(usize::from(i));
                }
                if !self.start_pick(takes, pick::PickFor::Field(field)) {
                    out.log.push(format!("library edit: {} not picked: the library has no Files tab", field.label()));
                }
            }
            return;
        }
        // A folder's picture: an image chosen in the Library.
        if id == edit::EditId::Picture {
            let read = self.library.config.as_ref();
            let Some(place) = self.edit.as_ref().and_then(|e| e.selected(e.config(read)?)).filter(|p| p.within.is_some()) else { return };
            let images = ["*.png", "*.jpg", "*.jpeg", "*.webp", "*.svg"].map(String::from).to_vec();
            if !self.start_pick(pick::Takes::Files(images), pick::PickFor::PlacePicture(place.id.clone())) {
                out.log.push("library edit: the picture not picked: the library has no Files tab".to_owned());
            }
            return;
        }
        let read = self.library.config.as_ref();
        if self.edit.as_mut().is_some_and(|e| e.tap(id, read, out)) {
            self.edit = None;
        }
    }

    /// Selects the edit mode's row `i`, as the focus comes to it.
    pub(crate) fn edit_select(&mut self, i: usize) {
        if let Some(e) = &mut self.edit {
            e.select(i);
        }
    }

    /// A field of the edit mode's form typed (see [`form`]).
    pub(crate) fn form_typed(&mut self, field: form::Field, text: &str) {
        if let Some(e) = &mut self.edit {
            e.field_typed(field, text);
        }
    }

    /// A place's name typed (see [`edit::Edit::named`]).
    pub(crate) fn place_named(&mut self, what: edit::PlaceName, name: &str, out: &mut crate::out::Out) {
        let read = self.library.config.as_ref();
        if let Some(e) = &mut self.edit {
            e.named(what, name, read, out);
        }
    }

    /// Whether the library has been read (see [`LibraryView::unread`]).
    pub fn read(&self) -> bool {
        self.read
    }

    /// Where the Library was at the last start, while the library isn't read yet.
    pub(crate) fn waiting(&self) -> Option<&crate::Resume> {
        self.waiting.as_ref()
    }

    /// The current collection's position.
    pub fn current(&self) -> usize {
        self.current
    }

    pub fn rows(&self) -> usize {
        self.rows
    }

    /// The id of the selected entry: the current list's, at its cursor. It is shown in the Library's list on the
    /// top screen and in its settings below.
    pub fn selected(&self) -> Option<&Arc<str>> {
        self.selected_entry().map(|g| &g.id)
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
        let mut found = Vec::new();
        self.found(self.current, &filter, &mut found);
        if let Some(p) = self.place_mut() {
            p.filter = filter;
            p.found = found;
            p.index = 0;
        }
        self.sync()
    }

    /// Appends the entries of collection `c`'s whole list whose titles start with `filter`, ignoring case,
    /// to `found`.
    fn found(&self, c: usize, filter: &str, found: &mut Vec<usize>) {
        let entries = &self.library.entries;
        found.extend(self.whole_list_of(c).iter().copied().filter(|&i| entries.get(i).is_some_and(|g| starts_with_ignoring_case(&g.title, filter))));
    }

    /// Finds every list's entries again by what is typed in it, after the library changed under them.
    fn refind(&mut self) {
        for c in 0..self.navs.len() {
            let filter = self.navs.get(c).and_then(|n| n.places.last()).map(|p| p.filter.clone()).unwrap_or_default();
            if filter.is_empty() {
                continue;
            }
            let mut found = Vec::new();
            self.found(c, &filter, &mut found);
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
    pub(crate) fn selected_index(&self) -> Option<usize> {
        self.list().get(self.cursor().index).copied()
    }

    pub(crate) fn selected_entry(&self) -> Option<&Entry> {
        self.entry_at(self.cursor().index)
    }

    /// Where `entry` is in collection `c`'s list.
    fn position_in(&self, c: usize, entry: &str) -> Option<usize> {
        self.list_of(c).iter().position(|&i| self.library.entries.get(i).is_some_and(|g| &*g.id == entry))
    }

    /// Where the Library is, for telling when it moved (see [`crate::State::follow_library_view`]): the collection, the
    /// group open in it, and the entry selected.
    pub(crate) fn place_key(&self) -> (usize, Option<usize>, Option<&Arc<str>>) {
        (self.current, self.navs.get(self.current).and_then(|n| n.open.last().copied()), self.selected())
    }

    /// Adds to `hasher` what of the Library a resume saves (see [`crate::Resume::at`]): the collection, and each
    /// collection's open groups and selected entry. It allocates nothing, so that a change is told each
    /// event (see [`crate::State::apply`]).
    pub(crate) fn hash_place(&self, hasher: &mut impl std::hash::Hasher) {
        use std::hash::Hash;
        if self.waiting.is_some() {
            // Before the library is read, the places are where the last start left them.
            return true.hash(hasher);
        }
        self.collection().map(|c| &c.name).hash(hasher);
        for (c, nav) in self.navs.iter().enumerate().take(self.library.collections.len()) {
            let list = self.list_of(c);
            let index = nav.places.last().map_or(0, |p| p.index).min(list.len().saturating_sub(1));
            for g in nav.open.iter().chain(list.get(index)).filter_map(|&g| self.library.entries.get(g)) {
                g.id.hash(hasher);
            }
            c.hash(hasher);
        }
    }
    /// Keeps the cursor in range and on screen. Returns whether the selection changed.
    pub(crate) fn sync(&mut self) -> bool {
        let before = self.selected().cloned();
        let len = self.list().len();
        let rows = self.rows.max(1) as f32;
        if let Some(p) = self.place_mut() {
            p.index = p.index.min(len.saturating_sub(1));
            p.scroll.set_visible(rows, len);
            p.scroll.show(p.index, len);
        }
        self.selected() != before.as_ref()
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

    /// Selects `entry` in the current list, if it is there. Returns whether the selection changed.
    pub(crate) fn select(&mut self, entry: &str) -> bool {
        let position = self.position_in(self.current, entry);
        if let (Some(i), Some(p)) = (position, self.place_mut()) {
            p.index = i;
        }
        self.sync()
    }

    /// Whether `id` is one of the Library's targets as they are now: an entry has to be in the current list.
    /// Other ids are taken as they come.
    pub(crate) fn has_target(&self, id: &crate::Id) -> bool {
        match id {
            crate::Id::LibraryView(crate::LibraryViewId::Entry(entry)) => self.position_in(self.current, entry).is_some(),
            _ => true,
        }
    }

    /// What the list shows, to tell when it changed.
    pub(crate) fn view(&self) -> ShownList {
        ShownList {
            collection: self.current,
            group: self.navs.get(self.current).and_then(|n| n.open.last().copied()),
            top: self.cursor().top.floor().max(0.0) as usize,
            rows: self.rows,
            selected: self.cursor().index,
            listings: self.listings_shown,
        }
    }

    /// Adds the entries in view to `ids`: the selected one first, then the rows shown, then the groups the
    /// list is in, such as a series when its episodes show.
    pub(crate) fn in_view(&self, ids: &mut Vec<Arc<str>>) {
        let ShownList { group, top, rows, selected, .. } = self.view();
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
        // While picking, the Files tab stays.
        if i >= self.library.collections.len() || self.pick.is_some() && Some(i) != self.files_tab() {
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
    /// place and the selected entry. These are kept by name, since derived collections may come or go. A
    /// group that left its collection, such as one no longer a favourite, is closed. Returns whether the
    /// selection changed.
    pub(crate) fn relayout(&mut self, change: impl FnOnce(&mut Library)) -> bool {
        let names: Vec<String> = self.library.collections.iter().map(|c| c.name.clone()).collect();
        let current_name = names.get(self.current).cloned();
        let selected = self.selected().cloned();
        change(&mut self.library);
        let old = std::mem::take(&mut self.navs);
        // Derived collections come and go, so places are matched by name. What is typed in each list is
        // found again below.
        self.navs =
            self.library.collections.iter().map(|c| names.iter().position(|n| *n == c.name).and_then(|i| old.get(i).cloned()).unwrap_or_default()).collect();
        self.current = current_name.and_then(|n| self.library.collections.iter().position(|c| c.name == n)).unwrap_or(0);
        self.library_changed(selected.as_ref());
        self.selected() != selected.as_ref()
    }

    /// What the library's entries changing takes, wherever it changed (a relayout, a listing, a read):
    /// groups that are gone closed, each list's typed filter found again, the entry that was `selected`
    /// selected again, the cursor kept in range, and the files selected that are gone dropped.
    fn library_changed(&mut self, selected: Option<&Arc<str>>) {
        self.close_gone();
        self.refind();
        if let Some(id) = selected {
            self.select(id);
        }
        self.sync();
        self.files.keep_selected(&self.library);
    }

    /// Takes the library read again. The listings of browsed folders that are still there are applied to
    /// it again, and each collection's place is kept by name and ids, with what is typed in each list and
    /// the scroll starting from the top. Returns whether the selection changed.
    pub(crate) fn reload(&mut self, library: Library) -> bool {
        if let Some(edit) = &mut self.edit {
            edit.library_read();
        }
        let before = self.selected().cloned();
        let mut places = Vec::new();
        self.kept_places(&mut places);
        let current = self.collection().map(|c| c.name.clone());
        let listings = std::mem::take(&mut self.listings);
        let waiting = self.waiting.take();
        // The places start over in the new library, and are restored below; the rest of the Library stays.
        self.navs = vec![Nav::default(); library.collections.len()];
        (self.library, self.read, self.current) = (library, true, 0);
        self.listings_shown = self.listings_shown.wrapping_add(1);
        for (folder, entries) in listings {
            if let Some(f) = self.library.entries.iter().position(|g| g.id == folder) {
                self.library.set_listing(f, entries.clone());
                self.listings.push((folder, entries));
            }
        }
        for (name, levels) in &places {
            self.restore_place(name, levels);
        }
        self.restore_current(current.as_deref().unwrap_or(""));
        // The first library read: the Library goes back to where it was at the last start.
        if let Some(resume) = waiting {
            resume.restore_library_view(self);
        }
        self.library_changed(None);
        self.selected() != before.as_ref()
    }

    /// Appends each collection's place to `places`, to be kept across a library read: its name, and for
    /// its list and each open group's, the entry selected in it and what is typed in it.
    fn kept_places(&self, places: &mut Vec<(String, Vec<KeptLevel>)>) {
        for (c, coll) in self.library.collections.iter().enumerate() {
            let Some(nav) = self.navs.get(c) else {
                continue;
            };
            let id = |i: Option<&usize>| i.and_then(|&i| self.library.entries.get(i)).map(|g| g.id.clone());
            let levels = nav
                .places
                .iter()
                .enumerate()
                .map(|(k, p)| {
                    let at = nav.open.get(k).or_else(|| self.list_of(c).get(p.index));
                    (id(at), p.filter.clone())
                })
                .collect();
            places.push((coll.name.clone(), levels));
        }
    }

    /// In the collection named `name`, types what each level had typed, selects its entry and opens it,
    /// down to the last level, whose entry is selected. If an entry is gone, it stops there.
    fn restore_place(&mut self, name: &str, levels: &[KeptLevel]) {
        let Some(c) = self.library.collections.iter().position(|c| c.name == name) else {
            return;
        };
        let current = std::mem::replace(&mut self.current, c);
        for (k, (id, filter)) in levels.iter().enumerate() {
            if !filter.is_empty() {
                self.set_filter(filter.clone());
            }
            let Some(p) = id.as_ref().and_then(|id| self.position_in(c, id)) else {
                break;
            };
            if let Some(place) = self.place_mut() {
                place.index = p;
            }
            if k + 1 < levels.len() && !self.open() {
                break;
            }
        }
        self.current = current;
    }

    /// Takes browsed folder `folder`'s entries as they are now (see [`Library::set_listing`]). The
    /// selected entry is kept by id, and open groups that are gone are closed. Returns whether the
    /// selection changed.
    pub(crate) fn listed(&mut self, folder: Arc<str>, entries: Vec<Entry>) -> bool {
        let Some(f) = self.library.entries.iter().position(|g| g.id == folder) else {
            return false;
        };
        let shown = |s: &LibraryView| s.list().iter().filter_map(|&i| s.library.entries.get(i)).map(|g| g.id.clone()).collect::<Vec<_>>();
        let shown_before = shown(self);
        let before = self.selected().cloned();
        self.listings.retain(|(id, _)| *id != folder);
        self.listings.push((folder, entries.clone()));
        self.library.set_listing(f, entries);
        // Drops the listings of folders that went with it.
        let library = &self.library;
        self.listings.retain(|(id, _)| library.entries.iter().any(|g| g.id == *id));
        self.library_changed(before.as_ref());
        if shown(self) != shown_before {
            self.listings_shown = self.listings_shown.wrapping_add(1);
        }
        self.selected() != before.as_ref()
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

    /// Applies what was fetched about entry `id`, if it is there (see [`Library::apply_meta`]).
    pub(crate) fn apply_meta(&mut self, id: &str, meta: &crate::Meta) {
        if let Some(i) = self.library.entries.iter().position(|g| &*g.id == id) {
            self.library.apply_meta(i, meta);
        }
    }

    /// Puts the selected entry in or out of the favourites, if there is one. Returns whether the selection
    /// changed.
    fn favourite_selected(&mut self) -> Option<bool> {
        let id = self.selected().cloned()?;
        Some(self.relayout(|lib| {
            if let Some(g) = lib.entries.iter_mut().find(|g| g.id == id) {
                g.favourite = !g.favourite;
            }
            lib.rebuild_derived();
        }))
    }

    /// Counts a play of `entry`, which has ended. Returns whether the selection changed.
    pub(crate) fn add_play(&mut self, entry: &str, start: u64, duration: u64) -> bool {
        self.relayout(|lib| {
            if let Some(g) = lib.entries.iter_mut().find(|g| &*g.id == entry) {
                g.plays.add(start, duration);
            }
            lib.rebuild_derived();
        })
    }

    /// Counts and records a play of `entry` from `start` (monotonic seconds, as in [`crate::Event::Tick`])
    /// until now, dated by the wall clock. For a video, it also reads where mpv left it, which mpv wrote as
    /// it quit. A focus in the list follows the selection.
    pub(crate) fn record_play(&mut self, entry: Arc<str>, start: u64, clock: &crate::Clock, out: &mut crate::out::Out) {
        let duration = clock.mono().saturating_sub(start);
        let start = clock.unix_at(start);
        self.add_play(&entry, start, duration);
        let file = self.entry(&entry).and_then(|g| g.mpv_file()).map(std::path::Path::to_path_buf);
        out.effects.push(crate::Effect::RecordPlay { entry: entry.clone(), start, duration });
        if let Some(file) = file {
            out.effects.push(crate::Effect::ReadPosition { entry, file });
        }
    }

    /// Appends each collection's place to `places`: its name, then its open groups' ids and its selected
    /// entry's id. A place past the end of its list, as after the list shrank while another collection
    /// was shown, is taken as the list's last entry. A collection whose list is empty has no place.
    pub(crate) fn places(&self, places: &mut Vec<(String, Vec<String>)>) {
        for (c, coll) in self.library.collections.iter().enumerate() {
            debug_assert!(self.navs.get(c).is_some(), "a place for collection {c}");
            let Some(nav) = self.navs.get(c) else {
                continue;
            };
            let list = self.list_of(c);
            let index = nav.places.last().map_or(0, |p| p.index).min(list.len().saturating_sub(1));
            let Some(g) = list.get(index).and_then(|&g| self.library.entries.get(g)) else {
                continue;
            };
            let mut ids: Vec<String> = nav.open.iter().filter_map(|&o| self.library.entries.get(o)).map(|g| g.id.to_string()).collect();
            ids.push(g.id.to_string());
            places.push((coll.name.clone(), ids));
        }
    }

    /// In the collection named `name`, opens the groups that `ids` names except the last, then selects the
    /// last. If one of them is gone, it stops there.
    pub(crate) fn restore_selection(&mut self, name: &str, ids: &[&str]) {
        let levels: Vec<KeptLevel> = ids.iter().map(|&id| (Some(id.into()), String::new())).collect();
        self.restore_place(name, &levels);
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
impl Rows for LibraryView {
    fn len(&self) -> usize {
        self.list().len()
    }
}

/// Whether `text` starts with `prefix`, ignoring case, compared char by char without a lowercased copy.
fn starts_with_ignoring_case(text: &str, prefix: &str) -> bool {
    let mut text = text.chars().flat_map(char::to_lowercase);
    prefix.chars().flat_map(char::to_lowercase).all(|p| text.next() == Some(p))
}

#[cfg(test)]
#[allow(clippy::indexing_slicing, clippy::unwrap_used)]
mod tests;
