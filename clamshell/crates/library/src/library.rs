//! The library: every entry in one arena, the tabs (collections) in shelf that list them, and what is
//! derived from them: Recent, Favourites, a group's Continue and a browsed folder's listing.

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::Arc;

use crate::{BrowseKind, Entry, Meta};

/// A tab in shelf. It is a tab from library.json's sources, Games (which also holds the games from Steam
/// and the desktop files), Apps, Recent or Favourites.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Collection {
    pub name: String,
    /// The name of its icon among the UI's icons (`gamepad`, `film`, ...), if it has one.
    pub icon: Option<String>,
    /// Set for Recent and Favourites, which are made from the other tabs and rebuilt when plays or
    /// favourites change.
    pub derived: bool,
    /// Its top-level entries, as indices into [`Library::entries`], in title order. Groups appear as
    /// themselves, not their contents.
    pub entries: Vec<usize>,
}

impl Collection {
    /// A collection of these entries named `name`, without an icon.
    pub fn new(name: &str, entries: Vec<usize>) -> Collection {
        Collection { name: name.into(), icon: None, derived: false, entries }
    }

    /// The same, with the icon named `icon`.
    pub fn with_icon(name: &str, icon: &str, games: Vec<usize>) -> Collection {
        Collection { icon: Some(icon.into()), ..Collection::new(name, games) }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Library {
    pub collections: Vec<Collection>,
    pub entries: Vec<Entry>,
    /// Problems found while reading it, such as mistakes in library.json or a source's missing dir, to
    /// show to the user.
    pub problems: Vec<String>,
    /// The id prefixes (`<source id>:`) of sources whose dir is missing, such as an SD card that isn't
    /// inserted. Their entries are away, not gone.
    pub away: Vec<String>,
    /// Slots in `entries` free for the next listing to use. They were held by entries that left a
    /// listing, and by everything inside those.
    pub free: Vec<usize>,
    /// Runners, by key, that come with one of ClamShell's keymaps, by name (see clamshell-core's
    /// `keymap::preset`). The runner uses that keymap until another is chosen for it.
    pub runner_keymaps: Vec<(Arc<str>, String)>,
    /// The order of the tabs, by name; tabs not named come after, in their existing order. `recent` and
    /// `favourites` are set only if library.json declares those tabs.
    pub order: Vec<String>,
    pub recent: Option<OwnTab>,
    pub favourites: Option<OwnTab>,
}

/// One of ClamShell's own tabs as declared: its name and icon.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OwnTab {
    pub name: String,
    pub icon: Option<String>,
}

impl Library {
    /// The first video frame in folder `i`, searching its entries in order and into subfolders.
    pub fn first_frame(&self, i: usize) -> Option<PathBuf> {
        let g = self.entries.get(i)?;
        g.items.iter().find_map(|&item| match self.entries.get(item) {
            Some(e) if e.is_group() => self.first_frame(item),
            Some(e) => e.details.shot.clone().filter(|_| e.mpv_file().is_some()),
            None => None,
        })
    }

    /// Whether entry `id` is in it, or its source is away.
    pub fn has(&self, id: &str) -> bool {
        self.entries.iter().any(|g| &*g.id == id) || self.away.iter().any(|p| id.starts_with(p.as_str()))
    }

    /// Rebuilds Recent (the last entries played, without apps) and Favourites, if they are declared, and
    /// puts the tabs in order. Either tab is left out while it is empty.
    pub fn rebuild_derived(&mut self) {
        self.collections.retain(|c| !c.derived);
        let mut recent: Vec<usize> =
            (0..self.entries.len()).filter(|&i| self.entries.get(i).is_some_and(|g| g.plays.count > 0 && !g.app && g.browsed.is_none())).collect();
        recent.sort_by_key(|&i| std::cmp::Reverse(self.entries.get(i).map_or(0, |g| g.plays.last)));
        recent.truncate(RECENT);
        let mut favourites: Vec<usize> = (0..self.entries.len()).filter(|&i| self.entries.get(i).is_some_and(|g| g.favourite && g.browsed.is_none())).collect();
        favourites.sort_by(|&a, &b| {
            let key = |i: usize| self.entries.get(i).map(|g| (&g.sort_key, &g.title));
            key(a).cmp(&key(b))
        });
        let derived = |tab: &OwnTab, entries: Vec<usize>| Collection { name: tab.name.clone(), icon: tab.icon.clone(), derived: true, entries };
        for (tab, games) in [(&self.recent, recent), (&self.favourites, favourites)] {
            if let Some(tab) = tab.as_ref().filter(|_| !games.is_empty()) {
                let c = derived(tab, games);
                self.collections.push(c);
            }
        }
        let order = &self.order;
        self.collections.sort_by_key(|c| order.iter().position(|n| *n == c.name).unwrap_or(usize::MAX));
    }

    /// The ids of the favourite entries, for saving.
    pub fn favourite_ids(&self) -> impl Iterator<Item = &str> {
        self.entries.iter().filter(|g| g.favourite).map(|g| &*g.id)
    }

    /// Applies what was fetched about entry `i`. The cover is used if the entry has no art of its own, and
    /// also for entries inside it without art, except videos, whose frame is their picture. The details
    /// and duration are applied as well.
    pub fn apply_meta(&mut self, i: usize, meta: &Meta) {
        let Some(g) = self.entries.get_mut(i) else {
            return;
        };
        if let Some(cover) = meta.cover.as_ref().filter(|_| g.box_front.is_none()) {
            g.box_front = Some(cover.clone());
            let mut inside = g.items.clone();
            while let Some(j) = inside.pop() {
                if let Some(item) = self.entries.get_mut(j).filter(|item| item.box_front.is_none() && item.mpv_file().is_none()) {
                    item.box_front = Some(cover.clone());
                    inside.extend(item.items.iter().copied());
                }
            }
        }
        if let Some(g) = self.entries.get_mut(i) {
            g.details.merge(&meta.details);
            if meta.duration.is_some() {
                g.playback.duration = meta.duration;
            }
        }
    }

    /// What a group's Continue plays, among the mpv videos inside it in order, including subfolders.
    ///
    /// This is the video played or watched last (see [`Entry::last_watched`]), or the one after it if it
    /// was watched to the end. If none was played, it is the first video. It is none when the last video
    /// was watched to the end, or the group holds no videos (a folder of games).
    pub fn to_continue(&self, group: usize) -> Option<usize> {
        // Walk in order without allocating, since shelf asks every frame to label its Continue.
        let mut walk = ContinueWalk::default();
        for &i in &self.entries.get(group)?.items {
            self.walk_videos(i, &mut walk, 0);
        }
        match walk.last {
            None => walk.first,
            Some((_, last)) if self.entries.get(last).is_some_and(Entry::watched) => walk.after_last,
            Some((_, last)) => Some(last),
        }
    }

    /// Feeds the videos under entry `i` to `walk`, in order. `depth` is how deep in groups this is.
    fn walk_videos(&self, i: usize, walk: &mut ContinueWalk, depth: u8) {
        let Some(g) = self.entries.get(i) else {
            return;
        };
        if g.is_group() {
            if depth < MAX_DEPTH {
                for &item in &g.items {
                    self.walk_videos(item, walk, depth + 1);
                }
            }
            return;
        }
        if g.mpv_file().is_none() {
            return;
        }
        walk.first.get_or_insert(i);
        if std::mem::take(&mut walk.after_pending) {
            walk.after_last = Some(i);
        }
        // Of videos watched at the same time, the last in order wins.
        let played = g.plays.count > 0 || g.playback.position.is_some();
        if played && walk.last.is_none_or(|(at, _)| g.last_watched() >= at) {
            walk.last = Some((g.last_watched(), i));
            walk.after_last = None;
            walk.after_pending = true;
        }
    }

    /// Sets the listing of browsed folder `folder` (see [`crate::Browsed`]) to `entries`, in order.
    ///
    /// An entry that was there before, by id, keeps its slot in `entries` and its own listing. Entries
    /// that are gone, and everything in them, free their slots for later listings but not for this one's
    /// new entries. That way a slot still held open, such as a removed folder the user is inside, never
    /// becomes another entry's straight away. Entries at the top of the Trash become
    /// [`BrowseKind::Trashed`].
    pub fn set_listing(&mut self, folder: usize, entries: Vec<Entry>) {
        let Some(f) = self.entries.get(folder) else {
            return;
        };
        let trash = f.browsed.as_ref().is_some_and(|b| b.kind == BrowseKind::Trash);
        let old: HashMap<Arc<str>, usize> = f.items.iter().filter_map(|&i| Some((self.entries.get(i)?.id.clone(), i))).collect();
        if let Some(b) = self.entries.get_mut(folder).and_then(|f| f.browsed.as_mut()) {
            b.listed = true;
        }
        let ids: HashSet<&Arc<str>> = entries.iter().map(|e| &e.id).collect();
        let mut gone: Vec<usize> = old.iter().filter(|(id, _)| !ids.contains(id)).map(|(_, &i)| i).collect();
        let mut freed = Vec::new();
        while let Some(i) = gone.pop() {
            if let Some(g) = self.entries.get_mut(i) {
                gone.append(&mut g.items);
                *g = Entry::new("", "");
            }
            freed.push(i);
        }
        let mut items = Vec::with_capacity(entries.len());
        for mut e in entries {
            e.parent = Some(folder);
            if trash && let Some(b) = e.browsed.as_mut() {
                b.kind = BrowseKind::Trashed;
            }
            let i = match old.get(&e.id).copied() {
                Some(i) => {
                    if let Some(g) = self.entries.get_mut(i) {
                        // Keep its own listing as it was.
                        e.items = std::mem::take(&mut g.items);
                        if let (Some(b), Some(was)) = (e.browsed.as_mut(), g.browsed.as_ref()) {
                            b.listed = was.listed;
                        }
                        *g = e;
                    }
                    i
                }
                None => match self.free.pop() {
                    Some(i) => {
                        if let Some(g) = self.entries.get_mut(i) {
                            *g = e;
                        }
                        i
                    }
                    None => {
                        self.entries.push(e);
                        self.entries.len() - 1
                    }
                },
            };
            items.push(i);
        }
        if let Some(f) = self.entries.get_mut(folder) {
            f.items = items;
        }
        self.free.append(&mut freed);
    }

    /// The files that enqueueing entry `i` adds, in order. For a file, that is the file itself. For a
    /// folder, it is its playlist if it has one (for a browsed folder, an `.m3u` entry in it counts).
    /// Otherwise it is the entries opened by runner `key`, in list order and including subfolders. A
    /// browsed folder that isn't listed yet is enqueued as itself, for the player to read.
    pub fn queue_files(&self, i: usize, key: &str) -> Vec<PathBuf> {
        let Some(g) = self.entries.get(i) else {
            return Vec::new();
        };
        if !g.is_group() {
            return g.file.clone().into_iter().collect();
        }
        let m3u =
            |g: &Entry| g.file.as_ref().filter(|f| f.extension().is_some_and(|e| e.eq_ignore_ascii_case("m3u") || e.eq_ignore_ascii_case("m3u8"))).cloned();
        let playlist = g.playlist.clone().or_else(|| g.items.iter().filter_map(|&e| self.entries.get(e)).find_map(m3u));
        if let Some(playlist) = playlist {
            return vec![playlist];
        }
        if g.browsed.as_ref().is_some_and(|b| !b.listed) {
            return g.file.clone().into_iter().collect();
        }
        let mut files = Vec::new();
        let mut stack: Vec<usize> = g.items.iter().rev().copied().collect();
        while let Some(e) = stack.pop() {
            match self.entries.get(e) {
                Some(e) if e.is_group() => stack.extend(e.items.iter().rev()),
                Some(e) if e.runner.as_ref().is_some_and(|r| &*r.key == key) => files.extend(e.file.clone()),
                _ => {}
            }
        }
        files
    }

    /// The groups entry `i` is in, outermost first.
    pub fn groups_of(&self, i: usize) -> Vec<usize> {
        let mut groups = Vec::new();
        let mut at = self.entries.get(i).and_then(|g| g.parent);
        while let Some(g) = at {
            groups.insert(0, g);
            at = self.entries.get(g).and_then(|g| g.parent);
        }
        groups
    }
}

/// The state of [`Library::to_continue`]'s walk: the first video, the one watched last with its time, and
/// the one after that.
#[derive(Default)]
struct ContinueWalk {
    first: Option<usize>,
    last: Option<(u64, usize)>,
    after_last: Option<usize>,
    after_pending: bool,
}

/// The deepest a walk goes into groups, so it always ends. A series with seasons of episodes is two deep.
const MAX_DEPTH: u8 = 8;

/// Entries in the Recent tab.
const RECENT: usize = 20;

#[cfg(test)]
#[allow(clippy::indexing_slicing, clippy::unwrap_used)]
mod tests {
    use super::*;
    use crate::{Browsed, Details, RunnerOf, demo};

    #[test]
    fn a_folder_queues_its_playlist_else_its_files_of_the_runner() {
        let music = RunnerOf::new("music", "Music", true);
        let file = |id: &str, runner: Option<&RunnerOf>| Entry {
            file: Some(format!("/m/{id}").into()),
            runner: runner.cloned(),
            parent: Some(0),
            ..Entry::new(id, id)
        };
        let mut games = vec![
            Entry { items: vec![1, 2, 3], ..Entry::new("album", "Album") },
            file("b.mp3", Some(&music)),
            file("cover.jpg", None),
            Entry { items: vec![4], parent: Some(0), ..Entry::new("cd2", "CD 2") },
            Entry { parent: Some(3), ..file("c.flac", Some(&music)) },
        ];
        let mut library = Library { entries: games.clone(), ..Library::default() };
        assert_eq!(library.queue_files(0, &music.key), [PathBuf::from("/m/b.mp3"), "/m/c.flac".into()], "down into CD 2, no cover");
        assert_eq!(library.queue_files(1, &music.key), [PathBuf::from("/m/b.mp3")]);
        games[0].playlist = Some("/m/album.m3u".into());
        library.entries = games;
        assert_eq!(library.queue_files(0, &music.key), [PathBuf::from("/m/album.m3u")], "its playlist instead");
    }

    #[test]
    fn recent_and_favourites_come_first() {
        let mut lib = demo::library();
        lib.collections.retain(|c| c.name == "Games");
        lib.entries[2].plays.add(100, 10);
        lib.entries[4].plays.add(500, 10);
        lib.entries[1].favourite = true;
        lib.rebuild_derived();
        let names: Vec<&str> = lib.collections.iter().map(|c| c.name.as_str()).collect();
        assert_eq!(names, vec!["Recent", "Favourites", "Games"]);
        assert_eq!(lib.collections[0].entries, vec![4, 2], "most recent first");
        assert_eq!(lib.collections[1].entries, vec![1]);
        lib.entries[1].favourite = false;
        lib.rebuild_derived();
        assert_eq!(lib.collections.len(), 2, "an empty Favourites goes");
    }

    #[test]
    fn a_series_is_a_group_of_seasons_of_episodes() {
        let lib = demo::library();
        let tv = lib.collections.iter().find(|c| c.name == "TV").unwrap();
        let series = &lib.entries[tv.entries[0]];
        assert!(series.is_group());
        let season = series.items[0];
        let episode = lib.entries[season].items[2];
        assert_eq!(lib.entries[episode].title, "10. The Edge of Forever");
        assert_eq!(lib.groups_of(episode), vec![tv.entries[0], season]);
        assert!(!lib.entries[episode].is_group());
    }

    #[test]
    fn a_series_cover_reaches_its_seasons_but_not_its_videos() {
        let mut lib = demo::library();
        let tv = lib.collections.iter().find(|c| c.name == "TV").unwrap().entries[0];
        let season = lib.entries[tv].items[0];
        let (first, second) = (lib.entries[season].items[0], lib.entries[season].items[1]);
        lib.entries[second].box_front = Some("own.jpg".into());
        let details = Details { summary: "Space.".into(), year: Some(1980), ..Details::default() };
        let meta = Meta { cover: Some("poster.jpg".into()), details, ..Meta::default() };
        lib.apply_meta(tv, &meta);
        assert_eq!(lib.entries[season].box_front.as_deref(), Some(std::path::Path::new("poster.jpg")), "a season");
        assert_eq!(lib.entries[first].box_front, None, "an episode: its frame is its picture");
        assert_eq!(lib.entries[second].box_front.as_deref(), Some(std::path::Path::new("own.jpg")));
        assert_eq!((lib.entries[tv].details.summary.as_str(), lib.entries[tv].details.year), ("Space.", Some(1980)));
    }

    #[test]
    fn continue_goes_on_from_the_episode_played_last() {
        let mut lib = demo::library();
        let tv = lib.collections.iter().find(|c| c.name == "TV").unwrap().entries[0];
        let s1 = lib.entries[tv].items[0];
        let s2 = lib.entries[tv].items[1];
        let (e1, e2, e10) = (lib.entries[s1].items[0], lib.entries[s1].items[1], lib.entries[s1].items[2]);
        let s2e1 = lib.entries[s2].items[0];
        lib.entries[e2].playback.position = None;
        assert_eq!(lib.to_continue(tv), Some(e1), "none played: the first");
        // The first is watched to the end, so mpv kept no place: the next one.
        lib.entries[e1].plays.add(100, 2700);
        assert_eq!(lib.to_continue(tv), Some(e2));
        // The second is left halfway: the second again.
        lib.entries[e2].plays.add(5000, 600);
        lib.entries[e2].playback.position = Some(1200);
        lib.entries[e2].playback.duration = Some(2700);
        assert_eq!(lib.to_continue(tv), Some(e2));
        // Quit during its credits counts as watched, so the next one, across seasons too.
        lib.entries[e2].playback.position = Some(2600);
        assert_eq!(lib.to_continue(tv), Some(e10));
        lib.entries[e10].plays.add(9000, 2700);
        assert_eq!(lib.to_continue(tv), Some(s2e1));
        assert_eq!(lib.to_continue(s1), None, "the season watched");
        // A place mpv kept outside ClamShell wins over earlier plays.
        lib.entries[e1].playback.position = Some(100);
        lib.entries[e1].playback.at = 20_000;
        assert_eq!(lib.to_continue(tv), Some(e1), "watched last, in mpv");
        let games = lib.collections.iter().find(|c| c.name == "Games").unwrap().entries.clone();
        lib.entries.push(Entry { items: games, ..Entry::new("folder:Games/All", "All") });
        assert_eq!(lib.to_continue(lib.entries.len() - 1), None, "games: no Continue");
    }

    fn browsed(id: &str, folder: bool) -> Entry {
        Entry { browsed: Some(Browsed { folder, ..Browsed::default() }), ..Entry::new(id, id.rsplit('/').next().unwrap()) }
    }

    #[test]
    fn listings_keep_places_and_free_what_went() {
        let mut lib = Library { entries: vec![browsed("browse:Home", true)], ..Library::default() };
        assert!(lib.entries[0].is_group(), "a folder opens before it is listed");
        lib.set_listing(0, vec![browsed("browse:Home/a", true), browsed("browse:Home/b.txt", false)]);
        assert!(lib.entries[0].browsed.as_ref().unwrap().listed);
        assert_eq!(lib.entries[0].items, vec![1, 2]);
        assert_eq!(lib.entries[1].parent, Some(0));
        lib.set_listing(1, vec![browsed("browse:Home/a/x", false)]);
        // Listed again: a keeps its slot and its listing. b is gone and its slot is freed, but not for c.
        lib.set_listing(0, vec![browsed("browse:Home/a", true), browsed("browse:Home/c", false)]);
        assert_eq!(lib.entries[0].items, vec![1, 4]);
        assert_eq!(&*lib.entries[4].id, "browse:Home/c");
        assert_eq!(lib.entries[1].items, vec![3], "a's listing kept");
        assert!(lib.entries[1].browsed.as_ref().unwrap().listed);
        assert_eq!(lib.free, vec![2]);
        // a is gone, and x in it. d takes b's slot, not a's.
        lib.set_listing(0, vec![browsed("browse:Home/c", false), browsed("browse:Home/d", true)]);
        assert_eq!(lib.entries[0].items, vec![4, 2]);
        let mut free = lib.free.clone();
        free.sort_unstable();
        assert_eq!(free, vec![1, 3]);
        assert!(!lib.has("browse:Home/a/x"));
        assert!(!lib.entries[1].is_group(), "a's place, held open, is no folder now");
    }

    #[test]
    fn the_trashs_top_entries_can_be_restored() {
        let mut trash = browsed("browse:Trash", true);
        trash.browsed.as_mut().unwrap().kind = BrowseKind::Trash;
        let mut lib = Library { entries: vec![trash], ..Library::default() };
        lib.set_listing(0, vec![browsed("browse:Trash/old", true)]);
        assert_eq!(lib.entries[1].browsed.as_ref().unwrap().kind, BrowseKind::Trashed);
        lib.set_listing(1, vec![browsed("browse:Trash/old/in", false)]);
        assert_eq!(lib.entries[2].browsed.as_ref().unwrap().kind, BrowseKind::Plain, "only the top");
    }
}
