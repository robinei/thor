//! An index of the library's entries, so that the lookups asked for on every event don't scan them all.
//! It is built where the library is made and after each listing, and dropped when the entries are changed
//! through [`crate::Library::entries_mut`], until [`crate::Library::reindex`] builds it again.
//!
//! Lookups other than by id go through sorted (hash, slot) pairs, which a rebuild refills without
//! allocating unless the library grew. A lookup hashes what it is asked, walks the pairs with that hash in
//! slot order, and takes the first slot whose entry really matches, so a collision costs a comparison and
//! never gives a wrong entry, and of two matching entries the first in slot order is found, as a scan
//! would find it.

use std::collections::HashMap;
use std::hash::{DefaultHasher, Hash, Hasher};
use std::path::Path;
use std::sync::Arc;

use crate::{Entry, RunnerOf, ids};

/// A runner of the library's entries, and what they are.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Runner {
    /// The runner, as the first of its entries has it.
    pub of: RunnerOf,
    /// Whether every entry it opens is a game ([`Entry::is_game`]).
    pub all_games: bool,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct Index {
    /// Set when the entries changed since it was built; it is empty then.
    stale: bool,
    /// Each id's first slot. Freed slots, whose id is empty, are left out.
    ids: HashMap<Arc<str>, usize>,
    /// Each entry's key ([`Entry::key`]).
    keys: Vec<(u64, usize)>,
    /// The file of each entry neither browsed nor a group.
    files: Vec<(u64, usize)>,
    /// The desktop file name and `StartupWMClass` of each entry that has them, ASCII-lowercased.
    desktop: Vec<(u64, usize)>,
    /// The runners of the entries, sorted by key.
    runners: Vec<Runner>,
    /// In tests, every hash is the same, so that each lookup has to tell entries apart by comparing.
    #[cfg(test)]
    pub(crate) collide: bool,
}

impl Index {
    /// Builds it from `entries`, reusing what it holds.
    pub(crate) fn build(&mut self, entries: &[Entry]) {
        self.drop_all();
        for (i, g) in entries.iter().enumerate().filter(|(_, g)| !g.id.is_empty()) {
            self.ids.entry(g.id.clone()).or_insert(i);
            let key = self.hash(g.key.as_str());
            self.keys.push((key, i));
            if let Some(file) = g.file.as_deref().filter(|_| g.browsed.is_none() && !g.is_group()) {
                let file = self.hash(file);
                self.files.push((file, i));
            }
            for name in [ids::desktop_file(&g.id), g.wm_class.as_deref()].into_iter().flatten() {
                let name = self.hash_lowercase(name);
                self.desktop.push((name, i));
            }
            if let Some(r) = &g.runner {
                match self.runners.binary_search_by(|o| o.of.key.cmp(&r.key)) {
                    Ok(at) => {
                        if let Some(o) = self.runners.get_mut(at) {
                            o.all_games &= g.is_game();
                        }
                    }
                    Err(at) => self.runners.insert(at, Runner { of: r.clone(), all_games: g.is_game() }),
                }
            }
        }
        for pairs in [&mut self.keys, &mut self.files, &mut self.desktop] {
            pairs.sort_unstable();
            pairs.dedup();
        }
        self.stale = false;
    }

    /// Empties it, as the entries are about to change.
    pub(crate) fn drop_all(&mut self) {
        self.ids.clear();
        self.keys.clear();
        self.files.clear();
        self.desktop.clear();
        self.runners.clear();
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

    /// The first slot with key `key`.
    pub(crate) fn key(&self, entries: &[Entry], key: &str) -> Option<usize> {
        self.find(&self.keys, self.hash(key), entries, |g| g.key == key)
    }

    /// The first slot, of the entries neither browsed nor groups, with file `file`.
    pub(crate) fn file(&self, entries: &[Entry], file: &Path) -> Option<usize> {
        self.find(&self.files, self.hash(file), entries, |g| g.browsed.is_none() && !g.is_group() && g.file.as_deref() == Some(file))
    }

    /// The first slot of a desktop file named `name`, or with that `StartupWMClass`, ignoring ASCII case.
    pub(crate) fn desktop(&self, entries: &[Entry], name: &str) -> Option<usize> {
        self.find(&self.desktop, self.hash_lowercase(name), entries, |g| {
            ids::desktop_file(&g.id).is_some_and(|d| d.eq_ignore_ascii_case(name)) || g.wm_class.as_deref().is_some_and(|c| c.eq_ignore_ascii_case(name))
        })
    }

    /// The first slot of a desktop file named `name`, exactly.
    pub(crate) fn desktop_file(&self, entries: &[Entry], name: &str) -> Option<usize> {
        self.find(&self.desktop, self.hash_lowercase(name), entries, |g| ids::desktop_file(&g.id) == Some(name))
    }

    /// The runner with key `key`.
    pub(crate) fn runner(&self, key: &str) -> Option<&Runner> {
        self.runners().binary_search_by(|o| (*o.of.key).cmp(key)).ok().and_then(|at| self.runners.get(at))
    }

    pub(crate) fn runners(&self) -> &[Runner] {
        debug_assert!(!self.stale, "the library's index is used after its entries changed");
        &self.runners
    }

    /// The first slot among `pairs` with hash `hash` whose entry `matches`.
    fn find(&self, pairs: &[(u64, usize)], hash: u64, entries: &[Entry], matches: impl Fn(&Entry) -> bool) -> Option<usize> {
        debug_assert!(!self.stale, "the library's index is used after its entries changed");
        let start = pairs.partition_point(|&(h, _)| h < hash);
        let same = pairs.get(start..)?.iter().take_while(|&&(h, _)| h == hash);
        same.map(|&(_, i)| i).find(|&i| entries.get(i).is_some_and(&matches))
    }

    fn hash(&self, value: &(impl Hash + ?Sized)) -> u64 {
        let mut h = DefaultHasher::new();
        value.hash(&mut h);
        self.finish(h)
    }

    /// The hash of `name` ASCII-lowercased, as `eq_ignore_ascii_case` compares.
    fn hash_lowercase(&self, name: &str) -> u64 {
        let mut h = DefaultHasher::new();
        for b in name.bytes() {
            h.write_u8(b.to_ascii_lowercase());
        }
        self.finish(h)
    }

    fn finish(&self, h: DefaultHasher) -> u64 {
        #[cfg(test)]
        if self.collide {
            return 0;
        }
        h.finish()
    }
}

#[cfg(test)]
#[allow(clippy::indexing_slicing, clippy::unwrap_used)]
mod tests {
    use std::path::Path;

    use super::Runner;
    use crate::{Browsed, Entry, Library, RunnerOf, demo, ids};

    /// Asserts that each lookup gives what scanning the entries in slot order gives, for each entry's
    /// facts and for some that no entry has.
    fn agrees(lib: &Library) {
        let entries = lib.entries();
        let first = |is: &dyn Fn(&Entry) -> bool| entries.iter().position(|g| !g.id.is_empty() && is(g));
        let slot = |g: Option<&Entry>| g.and_then(|g| entries.iter().position(|e| std::ptr::eq(e, g)));
        let misses = ["", "nothing", "browse:Files/Home/gone", "STEAM"];
        for id in entries.iter().map(|g| &*g.id).chain(misses) {
            assert_eq!(lib.position(id), first(&|g| &*g.id == id), "{id}");
            assert_eq!(slot(lib.entry(id)), first(&|g| &*g.id == id), "{id}");
        }
        for key in entries.iter().map(|g| g.key.as_str()).chain(misses) {
            assert_eq!(slot(lib.entry_by_key(key)), first(&|g| g.key == key), "{key}");
        }
        for file in entries.iter().filter_map(|g| g.file.as_deref()).chain([Path::new("/nothing")]) {
            let of_file = first(&|g| g.browsed.is_none() && !g.is_group() && g.file.as_deref() == Some(file));
            assert_eq!(slot(lib.entry_of_file(file)), of_file, "{}", file.display());
        }
        let names = entries.iter().flat_map(|g| [ids::desktop_file(&g.id), g.wm_class.as_deref()]).flatten();
        for name in names.flat_map(|n| [n.to_owned(), n.to_ascii_uppercase()]).chain(misses.map(String::from)) {
            let named = |g: &Entry| {
                ids::desktop_file(&g.id).is_some_and(|d| d.eq_ignore_ascii_case(&name)) || g.wm_class.as_deref().is_some_and(|c| c.eq_ignore_ascii_case(&name))
            };
            assert_eq!(slot(lib.entry_by_desktop(&name)), first(&named), "{name}");
            assert_eq!(slot(lib.entry_of_desktop_file(&[&name])), first(&|g| ids::desktop_file(&g.id) == Some(&name)), "{name}");
        }
        for key in entries.iter().filter_map(|g| g.runner.as_ref()).map(|r| &*r.key).chain(misses) {
            let of = |g: &&Entry| g.runner.as_ref().is_some_and(|r| &*r.key == key);
            let runner = entries.iter().find(of).map(|g| Runner { of: g.runner.clone().unwrap(), all_games: entries.iter().filter(of).all(Entry::is_game) });
            assert_eq!(lib.runner(key), runner.as_ref(), "{key}");
        }
        assert!(lib.runners().is_sorted_by(|a, b| a.of.key < b.of.key));
    }

    /// The demo library with entries that share facts: a key, a file, a desktop file name in other case
    /// and a `StartupWMClass`, and a library entry for a browsed file.
    fn shared() -> Library {
        let mut lib = demo::library();
        let entries = lib.entries_mut();
        entries.push(Entry { wm_class: Some("Navigator".into()), ..Entry::new("desktop:org.mozilla.firefox", "Firefox") });
        entries.push(Entry { wm_class: Some("firefox".into()), ..Entry::new("desktop:FIREFOX", "Firefox again") });
        entries.push(Entry { file: Some("/home/thor/notes.txt".into()), key: "notes".into(), ..Entry::new("notes:1", "Notes") });
        entries.push(Entry { file: Some("/home/thor/notes.txt".into()), key: "notes".into(), ..Entry::new("notes:2", "Notes again") });
        entries.push(Entry { file: Some("/home/thor/Pictures".into()), items: vec![0], ..Entry::new("folder:Pictures", "Pictures") });
        let snes = RunnerOf::new("snes", "SNES", true);
        entries.push(Entry { runner: Some(snes.clone()), ..Entry::new("snes:zelda", "Zelda") });
        entries.push(Entry { runner: Some(snes.clone()), ..Entry::new("snes:mario", "Mario") });
        entries.push(Entry { runner: Some(RunnerOf::new("mpv", "Videos", false)), app: true, ..Entry::new("mpv:film", "Film") });
        entries.push(Entry { runner: Some(snes), app: true, ..Entry::new("snes:tool", "Tool") });
        lib.reindex();
        lib
    }

    fn browsed(id: &str, folder: bool) -> Entry {
        let file = format!("/home/thor/{}", id.rsplit('/').next().unwrap());
        Entry { file: Some(file.into()), browsed: Some(Browsed { folder, ..Browsed::default() }), ..Entry::new(id, id.rsplit('/').next().unwrap()) }
    }

    #[test]
    fn lookups_agree_with_a_scan() {
        agrees(&demo::library());
        let lib = shared();
        assert!(lib.index_is_current());
        agrees(&lib);
        assert_eq!(lib.entry_by_key("notes").map(|g| &*g.id), Some("notes:1"));
        assert_eq!(lib.entry_by_desktop("FireFox").map(|g| &*g.id), Some("desktop:firefox"));
        assert_eq!(lib.entry_by_desktop("navigator").map(|g| &*g.id), Some("desktop:org.mozilla.firefox"));
        assert_eq!(lib.entry_of_desktop_file(&["FIREFOX", "firefox"]).map(|g| &*g.id), Some("desktop:firefox"));
        assert_eq!(lib.entry_of_file(Path::new("/home/thor/notes.txt")).map(|g| &*g.id), Some("notes:1"), "not the browsed one");
        assert_eq!(lib.entry_of_file(Path::new("/home/thor/Pictures")), None, "groups and browsed folders are left out");
    }

    /// With every hash the same, each lookup still finds its entry by comparing.
    #[test]
    fn lookups_agree_when_every_hash_collides() {
        agrees(&shared().colliding());
    }

    #[test]
    fn lookups_agree_after_listings_free_and_reuse_slots() {
        let mut lib = demo::library();
        let home = lib.position("browse:Files/Home").unwrap();
        let listing = |names: &[&str]| names.iter().map(|n| browsed(&format!("browse:Files/Home/{n}"), !n.contains('.'))).collect::<Vec<_>>();
        let mut listed = listing(&["Pictures", "Roms", "a.sfc", "b.txt"]);
        listed[2].runner = Some(RunnerOf::new("snes", "SNES", true));
        lib.set_listing(home, listed);
        assert!(lib.runner("runner:snes").is_some(), "a listing's runner");
        assert!(lib.index_is_current());
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
        assert!(lib.index_is_current());
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
        let key = lib.entries()[first].key.clone();
        assert_eq!(lib.entry_by_key(&key).map(|g| &*g.title), Some("Final Fantasy VI"));
        agrees(&lib);
    }
}
