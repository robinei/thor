//! An index of the library's entries, so that the lookups asked for on every event don't scan them all.
//! It is built where the library is made and after each listing, and dropped when the entries are changed
//! through [`crate::Library::entries_mut`], until [`crate::Library::reindex`] builds it again.

use std::collections::HashMap;
use std::sync::Arc;

use crate::Entry;

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct Index {
    /// Set when the entries changed since it was built; it is empty then.
    stale: bool,
    /// Each id's first slot. Freed slots, whose id is empty, are left out.
    ids: HashMap<Arc<str>, usize>,
}

impl Index {
    /// Builds it from `entries`, reusing what it holds.
    pub(crate) fn build(&mut self, entries: &[Entry]) {
        self.ids.clear();
        for (i, g) in entries.iter().enumerate().filter(|(_, g)| !g.id.is_empty()) {
            self.ids.entry(g.id.clone()).or_insert(i);
        }
        self.stale = false;
    }

    /// Empties it, as the entries are about to change.
    pub(crate) fn drop_all(&mut self) {
        self.ids.clear();
        self.stale = true;
    }

    pub(crate) fn stale(&self) -> bool {
        self.stale
    }

    /// The first slot with id `id`.
    pub(crate) fn id(&self, id: &str) -> Option<usize> {
        debug_assert!(!self.stale, "the library's index is used after its entries changed");
        self.ids.get(id).copied()
    }
}

#[cfg(test)]
#[allow(clippy::indexing_slicing, clippy::unwrap_used)]
mod tests {
    use crate::{Browsed, Entry, Library, demo};

    /// Asserts that each lookup gives what scanning the entries in slot order gives, for each entry's
    /// facts and for some that no entry has.
    fn agrees(lib: &Library) {
        assert!(lib.index_is_current());
        let entries = lib.entries();
        let named = |id: &str| entries.iter().position(|g| !g.id.is_empty() && &*g.id == id);
        for id in entries.iter().map(|g| &*g.id).chain(["", "nothing", "browse:Files/Home/gone"]) {
            assert_eq!(lib.position(id), named(id), "{id}");
            assert_eq!(lib.entry(id).map(|g| &g.id), named(id).map(|i| &entries[i].id), "{id}");
        }
    }

    fn browsed(id: &str, folder: bool) -> Entry {
        let file = format!("/home/thor/{}", id.rsplit('/').next().unwrap());
        Entry { file: Some(file.into()), browsed: Some(Browsed { folder, ..Browsed::default() }), ..Entry::new(id, id.rsplit('/').next().unwrap()) }
    }

    #[test]
    fn lookups_agree_with_a_scan() {
        agrees(&demo::library());
    }

    #[test]
    fn lookups_agree_after_listings_free_and_reuse_slots() {
        let mut lib = demo::library();
        let home = lib.position("browse:Files/Home").unwrap();
        let listing = |names: &[&str]| names.iter().map(|n| browsed(&format!("browse:Files/Home/{n}"), !n.contains('.'))).collect::<Vec<_>>();
        lib.set_listing(home, listing(&["Pictures", "Roms", "a.sfc", "b.txt"]));
        agrees(&lib);
        let roms = lib.position("browse:Files/Home/Roms").unwrap();
        lib.set_listing(roms, vec![browsed("browse:Files/Home/Roms/zelda.sfc", false)]);
        agrees(&lib);
        // Roms goes, and zelda.sfc in it; then c and d take freed slots.
        lib.set_listing(home, listing(&["Pictures", "a.sfc"]));
        agrees(&lib);
        assert_eq!(lib.position("browse:Files/Home/Roms/zelda.sfc"), None);
        let slots = lib.entries().len();
        lib.set_listing(home, listing(&["Pictures", "a.sfc", "c.txt", "d.txt"]));
        assert_eq!(lib.entries().len(), slots, "c and d in freed slots");
        agrees(&lib);
    }

    #[test]
    fn duplicates_give_the_first_slot() {
        let mut lib = demo::library();
        let first = lib.position("ff6").unwrap();
        let twin = Entry { title: "Twin".into(), ..lib.entries()[first].clone() };
        lib.entries_mut().push(twin);
        lib.reindex();
        assert_eq!(lib.position("ff6"), Some(first));
        agrees(&lib);
    }
}
