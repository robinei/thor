//! The library's model: entries (games, apps, movies, groups of them such as a series' seasons and
//! episodes), the tabs they are in and how they sort, and the parts of Steam's files and of name patterns
//! that need no disk. No I/O and no dependencies: scanning (library.json's content dirs, Steam, desktop
//! files), play time and favourites are clamshell-system's (`library`).

pub mod pattern;
pub mod plays;
pub mod sort;
pub mod steam;

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::Arc;

pub use plays::Plays;

/// How an entry starts.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum Launch {
    /// Nothing to start (a group: it opens in shelf).
    #[default]
    None,
    /// Through Steam.
    Steam { appid: u32 },
    /// A program with its arguments (filled in already), in `workdir` (else the home dir).
    Program { argv: Vec<String>, workdir: Option<PathBuf> },
    /// One of ClamShell's own tools (the Tools tab), by name.
    Tool(String),
}

impl Launch {
    /// A program with these arguments, in the home dir.
    pub fn program(argv: Vec<String>) -> Launch {
        Launch::Program { argv, workdir: None }
    }
}

/// One entry as the rest of the program sees it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Game {
    /// Stable id: `<source id>:<path in the source's dir>`, `steam:<appid>` or `desktop:<file id>`. Focus,
    /// settings and play time are keyed by it. Shared, so the UI can put it in a frame's targets without
    /// allocating.
    pub id: Arc<str>,
    /// The id made fit for unit and workspace names (see [`key_of`]): what its launches are named by.
    pub key: String,
    pub title: String,
    /// What it sorts by (see [`sort`]).
    pub sort_key: Vec<(u8, char)>,
    /// Stand-in for box art while there is none or it is loading: 0xRRGGBB, from the id.
    pub color: u32,
    pub launch: Launch,
    /// Its file or folder in its content dir (none for Steam's games and desktop files).
    pub file: Option<PathBuf>,
    pub box_front: Option<PathBuf>,
    /// An icon (an app's), drawn where there is no box art.
    pub icon: Option<PathBuf>,
    /// Fetched (see [`Meta`]): a screenshot (a film's backdrop), what it is about, the year it came out,
    /// who made it.
    pub shot: Option<PathBuf>,
    pub summary: String,
    pub year: Option<u16>,
    pub developer: String,
    /// A video mpv plays: where mpv was when last quit (seconds; none: not started, or watched to the
    /// end), and how long it is (seconds, once read).
    pub position: Option<u32>,
    pub duration: Option<u32>,
    /// When mpv kept that place (seconds since the epoch): it was watched then, maybe outside ClamShell.
    pub position_at: u64,
    /// An app, not a game or a film: Recent leaves it out.
    pub tool: bool,
    /// A group's entries (a series' seasons, a season's episodes), as indices into [`Library::games`], in
    /// order; empty for anything else.
    pub items: Vec<usize>,
    /// The group it is in, if any.
    pub parent: Option<usize>,
    /// Play history and favourite (from the database).
    pub plays: Plays,
    pub favourite: bool,
    /// A file or folder of a browsed dir (see [`Browsed`]); none for the rest.
    pub browsed: Option<Browsed>,
    /// The runner that opens it (library.json's or ClamShell's), for content: a scanned entry (and a folder
    /// of them), a browsed file. None for an app or a Steam game (it is its own runner).
    pub runner: Option<RunnerOf>,
}

/// A file or folder of a browsed dir (library.json's `browse`, removable storage): not scanned, but listed
/// when its folder is opened (its entries' ids its own, `/`, their names); it has the file actions instead
/// of a game's (no favourite, no pin, not in Recent).
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Browsed {
    /// A folder (it opens), not a file.
    pub folder: bool,
    /// A folder whose entries are in (its `items`), from a listing.
    pub listed: bool,
    pub kind: BrowseKind,
    /// A file's size (bytes), and when it was last changed (seconds since the epoch; 0: not known).
    pub size: Option<u64>,
    pub modified: u64,
}

/// What a browsed entry is besides a file or folder.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum BrowseKind {
    #[default]
    Plain,
    /// A dir library.json's `browse` names.
    Root,
    /// A removable filesystem's top (its device, to eject).
    Removable(String),
    /// The Trash (its `files` dir).
    Trash,
    /// At the top of the Trash: it can be restored.
    Trashed,
}

impl Game {
    pub fn new(id: &str, title: &str) -> Self {
        Game {
            id: id.into(),
            key: key_of(id),
            title: title.into(),
            sort_key: sort::key(title),
            color: color_of(id),
            launch: Launch::None,
            file: None,
            box_front: None,
            icon: None,
            shot: None,
            summary: String::new(),
            year: None,
            developer: String::new(),
            position: None,
            duration: None,
            position_at: 0,
            tool: false,
            items: Vec::new(),
            parent: None,
            plays: Plays::default(),
            favourite: false,
            browsed: None,
            runner: None,
        }
    }

    /// Whether it is a group (it opens rather than starts): one with entries, or a browsed folder.
    pub fn is_group(&self) -> bool {
        !self.items.is_empty() || self.browsed.as_ref().is_some_and(|b| b.folder)
    }

    /// A browsed folder: its dir, listed when it is opened.
    pub fn browsed_dir(&self) -> Option<&std::path::Path> {
        self.file.as_deref().filter(|_| self.browsed.as_ref().is_some_and(|b| b.folder))
    }

    /// Its file, if mpv plays it (its launch command's program is mpv): where mpv keeps its place.
    pub fn mpv_file(&self) -> Option<&std::path::Path> {
        let Launch::Program { argv, .. } = &self.launch else {
            return None;
        };
        let program = argv.first().map(|p| p.rsplit('/').next().unwrap_or(p));
        self.file.as_deref().filter(|_| program == Some("mpv"))
    }

    /// When it was last played or watched (seconds since the epoch): through ClamShell, or where mpv kept
    /// its place.
    pub fn last_watched(&self) -> u64 {
        self.plays.last.max(self.position_at)
    }

    /// Watched: played and left at the end (mpv keeps no place), or quit near it (past [`DONE`]).
    pub fn watched(&self) -> bool {
        match (self.position, self.duration) {
            (None, _) => self.plays.count > 0,
            (Some(at), Some(length)) => length > 0 && f64::from(at) >= f64::from(length) * DONE,
            (Some(_), None) => false,
        }
    }
}

/// The key of runner `id`: its settings, environment and own keymap are kept by it (`runner:snes`).
pub fn runner_key(id: &str) -> Arc<str> {
    format!("runner:{id}").into()
}

/// An entry's runner: its key ([`runner_key`] of its id, which outlives a new name) and its name (shown).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RunnerOf {
    pub key: Arc<str>,
    pub name: Arc<str>,
}

impl RunnerOf {
    pub fn new(id: &str, name: &str) -> RunnerOf {
        RunnerOf { key: runner_key(id), name: name.into() }
    }
}

/// How far into a video counts as watched (the credits left).
pub const DONE: f64 = 0.9;

/// What was fetched about an entry (clamshell-system's `meta`): its cover, a screenshot, what it is
/// about, the year it came out, who made it; each if found.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Meta {
    pub cover: Option<PathBuf>,
    pub shot: Option<PathBuf>,
    pub summary: String,
    pub year: Option<u16>,
    pub developer: String,
    /// A video's length (seconds).
    pub duration: Option<u32>,
}

/// An entry's key, from its id: what its launches' units and its workspace are named by. Letters, digits
/// and `_` only (no `-`, which separates the parts of those names): the id's last part (its file's name
/// without the extension, or after `:`), lowercase, at most 16 characters, then `_` and 6 hex digits of a
/// hash of the whole id, so ids alike stay apart.
pub fn key_of(id: &str) -> String {
    let last = id.rsplit(['/', ':']).next().unwrap_or(id);
    let stem = last.rsplit_once('.').map_or(last, |(stem, _)| stem);
    let mut slug: String = stem.chars().map(|c| if c.is_ascii_alphanumeric() { c.to_ascii_lowercase() } else { '_' }).take(16).collect();
    let trimmed = slug.trim_matches('_').len();
    if trimmed == 0 {
        slug = "entry".into();
    }
    let h = id.bytes().fold(0x811c_9dc5u32, |h, b| (h ^ u32::from(b)).wrapping_mul(0x0100_0193));
    format!("{}_{:06x}", slug.trim_matches('_'), h & 0x00ff_ffff)
}

/// A muted colour from a hash of `id`, stable across runs.
fn color_of(id: &str) -> u32 {
    let h = id.bytes().fold(0x811c_9dc5u32, |h, b| (h ^ u32::from(b)).wrapping_mul(0x0100_0193));
    let channel = |shift: u32| 0x40 + ((h >> shift) & 0x7f);
    (channel(16) << 16) | (channel(8) << 8) | channel(0)
}

/// A tab in shelf: library.json's sources' tab, Games (also Steam's and the desktop files' games), Apps,
/// or Recent and Favourites.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Collection {
    pub name: String,
    /// Its icon's name (the UI's: `gamepad`, `film`, ...), if it has one.
    pub icon: Option<String>,
    /// Recent or Favourites: made from the others, rebuilt when plays or favourites change.
    pub derived: bool,
    /// Its entries (groups at their top), as indices into [`Library::games`], in title order.
    pub games: Vec<usize>,
}

impl Collection {
    /// A collection of these entries named `name`, without an icon.
    pub fn new(name: &str, games: Vec<usize>) -> Collection {
        Collection { name: name.into(), icon: None, derived: false, games }
    }

    /// The same, with the icon named `icon`.
    pub fn with_icon(name: &str, icon: &str, games: Vec<usize>) -> Collection {
        Collection { icon: Some(icon.into()), ..Collection::new(name, games) }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Library {
    pub collections: Vec<Collection>,
    pub games: Vec<Game>,
    /// What was wrong reading it (library.json's mistakes, a source's missing dir), to show.
    pub problems: Vec<String>,
    /// The id prefixes (`<source id>:`) of sources whose dir is missing (an SD card not in): their
    /// entries are away, not gone.
    pub away: Vec<String>,
    /// Places in `games` free for the next listing's entries (a listing's entries gone, and theirs).
    pub free: Vec<usize>,
    /// Runners (by key) that come with a keymap of ClamShell's (its name: see clamshell-core's
    /// `keymap::preset`), their own until one is chosen for them.
    pub runner_keymaps: Vec<(Arc<str>, String)>,
}

impl Library {
    /// Whether entry `id` is in it, or its source is away.
    pub fn has(&self, id: &str) -> bool {
        self.games.iter().any(|g| &*g.id == id) || self.away.iter().any(|p| id.starts_with(p.as_str()))
    }
}

impl Library {
    /// Rebuilds Recent (the last entries played, apps left out) and Favourites in front of the other
    /// collections; either is left out while empty.
    pub fn rebuild_derived(&mut self) {
        self.collections.retain(|c| !c.derived);
        let mut recent: Vec<usize> =
            (0..self.games.len()).filter(|&i| self.games.get(i).is_some_and(|g| g.plays.count > 0 && !g.tool && g.browsed.is_none())).collect();
        recent.sort_by_key(|&i| std::cmp::Reverse(self.games.get(i).map_or(0, |g| g.plays.last)));
        recent.truncate(RECENT);
        let mut favourites: Vec<usize> = (0..self.games.len()).filter(|&i| self.games.get(i).is_some_and(|g| g.favourite && g.browsed.is_none())).collect();
        favourites.sort_by(|&a, &b| {
            let key = |i: usize| self.games.get(i).map(|g| (&g.sort_key, &g.title));
            key(a).cmp(&key(b))
        });
        let derived = |name: &str, icon: &str, games: Vec<usize>| Collection { derived: true, ..Collection::with_icon(name, icon, games) };
        let mut front = Vec::new();
        if !recent.is_empty() {
            front.push(derived("Recent", "clock", recent));
        }
        if !favourites.is_empty() {
            front.push(derived("Favourites", "star", favourites));
        }
        self.collections.splice(0..0, front);
    }

    /// The ids of the favourite entries (as saved).
    pub fn favourite_ids(&self) -> impl Iterator<Item = &str> {
        self.games.iter().filter(|g| g.favourite).map(|g| &*g.id)
    }

    /// What was fetched about entry `i`: its cover where it has no art of its own (and so the entries in
    /// it without art, but videos: an episode's frame is its picture), and the rest.
    pub fn apply_meta(&mut self, i: usize, meta: &Meta) {
        let Some(g) = self.games.get_mut(i) else {
            return;
        };
        if let Some(cover) = meta.cover.as_ref().filter(|_| g.box_front.is_none()) {
            g.box_front = Some(cover.clone());
            let mut inside = g.items.clone();
            while let Some(j) = inside.pop() {
                if let Some(item) = self.games.get_mut(j).filter(|item| item.box_front.is_none() && item.mpv_file().is_none()) {
                    item.box_front = Some(cover.clone());
                    inside.extend(item.items.iter().copied());
                }
            }
        }
        if let Some(g) = self.games.get_mut(i) {
            if meta.shot.is_some() {
                g.shot.clone_from(&meta.shot);
            }
            if !meta.summary.is_empty() {
                g.summary.clone_from(&meta.summary);
            }
            if meta.year.is_some() {
                g.year = meta.year;
            }
            if !meta.developer.is_empty() {
                g.developer.clone_from(&meta.developer);
            }
            if meta.duration.is_some() {
                g.duration = meta.duration;
            }
        }
    }

    /// What a group's Continue plays: of the videos in it (mpv's, all the way down, in order), the one
    /// played or watched last (see [`Game::last_watched`]) unless it was watched to its end, then the one after it; the first if none was played; none when
    /// the last was watched to the end, or it holds no videos (a folder of games).
    pub fn to_continue(&self, group: usize) -> Option<usize> {
        let mut leaves = Vec::new();
        let mut stack: Vec<usize> = self.games.get(group)?.items.iter().rev().copied().collect();
        while let Some(i) = stack.pop() {
            match self.games.get(i) {
                Some(g) if g.is_group() => stack.extend(g.items.iter().rev()),
                Some(g) if g.mpv_file().is_some() => leaves.push(i),
                Some(_) => {}
                None => {}
            }
        }
        let played = |i: &usize| self.games.get(*i).is_some_and(|g| g.plays.count > 0 || g.position.is_some());
        let Some(at) =
            leaves.iter().enumerate().filter(|(_, i)| played(i)).max_by_key(|(_, i)| self.games.get(**i).map_or(0, Game::last_watched)).map(|(n, _)| n)
        else {
            return leaves.first().copied();
        };
        let last = *leaves.get(at)?;
        if self.games.get(last).is_some_and(Game::watched) { leaves.get(at + 1).copied() } else { Some(last) }
    }

    /// Folder `folder`'s listing (browsed: see [`Browsed`]): `entries`, in order, its entries. One there
    /// before (by id) keeps its place in `games` and its own listing; those gone, and everything in them,
    /// leave theirs free. At the top of the Trash, an entry is [`BrowseKind::Trashed`].
    pub fn set_listing(&mut self, folder: usize, entries: Vec<Game>) {
        let Some(f) = self.games.get(folder) else {
            return;
        };
        let trash = f.browsed.as_ref().is_some_and(|b| b.kind == BrowseKind::Trash);
        let old: HashMap<Arc<str>, usize> = f.items.iter().filter_map(|&i| Some((self.games.get(i)?.id.clone(), i))).collect();
        if let Some(b) = self.games.get_mut(folder).and_then(|f| f.browsed.as_mut()) {
            b.listed = true;
        }
        // (Those gone first, so their places go to the new ones.)
        let ids: HashSet<&Arc<str>> = entries.iter().map(|e| &e.id).collect();
        let mut gone: Vec<usize> = old.iter().filter(|(id, _)| !ids.contains(id)).map(|(_, &i)| i).collect();
        while let Some(i) = gone.pop() {
            if let Some(g) = self.games.get_mut(i) {
                gone.append(&mut g.items);
                *g = Game::new("", "");
            }
            self.free.push(i);
        }
        let mut items = Vec::with_capacity(entries.len());
        for mut e in entries {
            e.parent = Some(folder);
            if trash && let Some(b) = e.browsed.as_mut() {
                b.kind = BrowseKind::Trashed;
            }
            let i = match old.get(&e.id).copied() {
                Some(i) => {
                    if let Some(g) = self.games.get_mut(i) {
                        // (Its own listing, as it was.)
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
                        if let Some(g) = self.games.get_mut(i) {
                            *g = e;
                        }
                        i
                    }
                    None => {
                        self.games.push(e);
                        self.games.len() - 1
                    }
                },
            };
            items.push(i);
        }
        if let Some(f) = self.games.get_mut(folder) {
            f.items = items;
        }
    }

    /// The groups entry `i` is in, outermost first.
    pub fn groups_of(&self, i: usize) -> Vec<usize> {
        let mut groups = Vec::new();
        let mut at = self.games.get(i).and_then(|g| g.parent);
        while let Some(g) = at {
            groups.insert(0, g);
            at = self.games.get(g).and_then(|g| g.parent);
        }
        groups
    }
}

/// Entries in the Recent tab.
const RECENT: usize = 20;

/// Placeholder games for development and snapshots.
pub fn demo_games() -> Vec<Game> {
    [
        ("zelda-lttp", "A Link to the Past", 0x3f8f4f),
        ("metroid-zm", "Metroid: Zero Mission", 0xb0503a),
        ("mk-ds", "Mario Kart DS", 0xc8a038),
        ("ff6", "Final Fantasy VI", 0x5060b0),
        ("dark-souls", "Dark Souls", 0x606060),
        ("hades", "Hades", 0xa03060),
        ("celeste", "Celeste", 0x8050b0),
    ]
    .into_iter()
    .map(|(id, title, color)| match id {
        // As fetched metadata gives it.
        "zelda-lttp" => Game {
            color,
            developer: "Nintendo".into(),
            year: Some(1991),
            summary: "Link sets out across Hyrule and the Dark World to free the descendants of the Seven Sages from the wizard Agahnim.".into(),
            ..Game::new(id, title)
        },
        _ => Game { color, ..Game::new(id, title) },
    })
    .collect()
}

/// The demo games in two collections, a few installed apps, and a series (two seasons of episodes),
/// for development and snapshots.
pub fn demo_library() -> Library {
    let mut games = demo_games();
    let n = games.len();
    // A few installed apps, as desktop files give them.
    let apps = [("desktop:steam", "Steam", "steam"), ("desktop:foot", "Terminal", "foot"), ("desktop:firefox", "Firefox", "firefox")];
    games.extend(apps.iter().map(|(id, title, program)| Game { launch: Launch::program(vec![(*program).into()]), tool: true, ..Game::new(id, title) }));
    let series = games.len();
    games.push(Game { color: 0x305a78, ..Game::new("tv:Cosmos", "Cosmos") });
    let seasons: [(&str, &[&str]); 2] = [
        ("Season 1", &["1. The Shores of the Cosmic Ocean", "2. One Voice in the Cosmic Fugue", "10. The Edge of Forever"]),
        ("Season 2", &["1. Standing Up in the Milky Way"]),
    ];
    for (season, episodes) in seasons {
        let s = games.len();
        let id = format!("tv:Cosmos/{season}");
        games.push(Game { parent: Some(series), color: 0x305a78, ..Game::new(&id, season) });
        for episode in episodes {
            let e = games.len();
            let file = format!("{id}/{episode}.mkv");
            let path = format!("/videos/{}", file.trim_start_matches("tv:"));
            let argv = vec!["mpv".into(), path.clone()];
            // (The second episode left halfway.)
            let position = episode.starts_with("2.").then_some(1500);
            let launch = Launch::program(argv);
            games.push(Game { parent: Some(s), color: 0x305a78, launch, file: Some(path.into()), duration: Some(2640), position, ..Game::new(&file, episode) });
            if let Some(season) = games.get_mut(s) {
                season.items.push(e);
            }
        }
        if let Some(g) = games.get_mut(series) {
            g.items.push(s);
        }
    }
    let collections = vec![
        Collection::with_icon("Games", "gamepad", (0..n).collect()),
        Collection::new("More", vec![3, 1]),
        Collection::with_icon("TV", "tv", vec![series]),
        Collection::with_icon("Apps", "apps", (n..n + apps.len()).collect()),
    ];
    Library { collections, games, ..Library::default() }
}

#[cfg(test)]
#[allow(clippy::indexing_slicing, clippy::unwrap_used)]
mod tests {
    use super::*;

    #[test]
    fn recent_and_favourites_come_first() {
        let mut lib = demo_library();
        lib.collections.retain(|c| c.name == "Games");
        lib.games[2].plays.add(100, 10);
        lib.games[4].plays.add(500, 10);
        lib.games[1].favourite = true;
        lib.rebuild_derived();
        let names: Vec<&str> = lib.collections.iter().map(|c| c.name.as_str()).collect();
        assert_eq!(names, vec!["Recent", "Favourites", "Games"]);
        assert_eq!(lib.collections[0].games, vec![4, 2], "most recent first");
        assert_eq!(lib.collections[1].games, vec![1]);
        lib.games[1].favourite = false;
        lib.rebuild_derived();
        assert_eq!(lib.collections.len(), 2, "an empty Favourites goes");
    }

    #[test]
    fn a_series_is_a_group_of_seasons_of_episodes() {
        let lib = demo_library();
        let tv = lib.collections.iter().find(|c| c.name == "TV").unwrap();
        let series = &lib.games[tv.games[0]];
        assert!(series.is_group());
        let season = series.items[0];
        let episode = lib.games[season].items[2];
        assert_eq!(lib.games[episode].title, "10. The Edge of Forever");
        assert_eq!(lib.groups_of(episode), vec![tv.games[0], season]);
        assert!(!lib.games[episode].is_group());
    }

    #[test]
    fn a_series_cover_reaches_its_seasons_but_not_its_videos() {
        let mut lib = demo_library();
        let tv = lib.collections.iter().find(|c| c.name == "TV").unwrap().games[0];
        let season = lib.games[tv].items[0];
        let (first, second) = (lib.games[season].items[0], lib.games[season].items[1]);
        lib.games[second].box_front = Some("own.jpg".into());
        let meta = Meta { cover: Some("poster.jpg".into()), summary: "Space.".into(), year: Some(1980), ..Meta::default() };
        lib.apply_meta(tv, &meta);
        assert_eq!(lib.games[season].box_front.as_deref(), Some(std::path::Path::new("poster.jpg")), "a season");
        assert_eq!(lib.games[first].box_front, None, "an episode: its frame is its picture");
        assert_eq!(lib.games[second].box_front.as_deref(), Some(std::path::Path::new("own.jpg")));
        assert_eq!((lib.games[tv].summary.as_str(), lib.games[tv].year), ("Space.", Some(1980)));
    }

    #[test]
    fn continue_goes_on_from_the_episode_played_last() {
        let mut lib = demo_library();
        let tv = lib.collections.iter().find(|c| c.name == "TV").unwrap().games[0];
        let s1 = lib.games[tv].items[0];
        let s2 = lib.games[tv].items[1];
        let (e1, e2, e10) = (lib.games[s1].items[0], lib.games[s1].items[1], lib.games[s1].items[2]);
        let s2e1 = lib.games[s2].items[0];
        lib.games[e2].position = None;
        assert_eq!(lib.to_continue(tv), Some(e1), "none played: the first");
        // The first watched to the end (mpv kept no place): the next.
        lib.games[e1].plays.add(100, 2700);
        assert_eq!(lib.to_continue(tv), Some(e2));
        // The second left halfway: it again.
        lib.games[e2].plays.add(5000, 600);
        lib.games[e2].position = Some(1200);
        lib.games[e2].duration = Some(2700);
        assert_eq!(lib.to_continue(tv), Some(e2));
        // Quit in its credits: watched, so the one after (across seasons too).
        lib.games[e2].position = Some(2600);
        assert_eq!(lib.to_continue(tv), Some(e10));
        lib.games[e10].plays.add(9000, 2700);
        assert_eq!(lib.to_continue(tv), Some(s2e1));
        assert_eq!(lib.to_continue(s1), None, "the season watched");
        // Places mpv kept outside ClamShell: the one kept last wins over plays before it.
        lib.games[e1].position = Some(100);
        lib.games[e1].position_at = 20_000;
        assert_eq!(lib.to_continue(tv), Some(e1), "watched last, in mpv");
        let games = lib.collections.iter().find(|c| c.name == "Games").unwrap().games.clone();
        lib.games.push(Game { items: games, ..Game::new("folder:Games/All", "All") });
        assert_eq!(lib.to_continue(lib.games.len() - 1), None, "games: no Continue");
    }

    fn browsed(id: &str, folder: bool) -> Game {
        Game { browsed: Some(Browsed { folder, ..Browsed::default() }), ..Game::new(id, id.rsplit('/').next().unwrap()) }
    }

    #[test]
    fn listings_keep_places_and_free_what_went() {
        let mut lib = Library { games: vec![browsed("browse:Home", true)], ..Library::default() };
        assert!(lib.games[0].is_group(), "a folder opens before it is listed");
        lib.set_listing(0, vec![browsed("browse:Home/a", true), browsed("browse:Home/b.txt", false)]);
        assert!(lib.games[0].browsed.as_ref().unwrap().listed);
        assert_eq!(lib.games[0].items, vec![1, 2]);
        assert_eq!(lib.games[1].parent, Some(0));
        lib.set_listing(1, vec![browsed("browse:Home/a/x", false)]);
        // Listed again: a stays where it was, with its own listing; b went, its place free for c.
        lib.set_listing(0, vec![browsed("browse:Home/a", true), browsed("browse:Home/c", false)]);
        assert_eq!(lib.games[0].items, vec![1, 2]);
        assert_eq!(&*lib.games[2].id, "browse:Home/c", "b's place");
        assert_eq!(lib.games[1].items, vec![3], "a's listing kept");
        assert!(lib.games[1].browsed.as_ref().unwrap().listed);
        // a went, and x in it.
        lib.set_listing(0, vec![browsed("browse:Home/c", false)]);
        assert_eq!(lib.games[0].items, vec![2]);
        let mut free = lib.free.clone();
        free.sort_unstable();
        assert_eq!(free, vec![1, 3]);
        assert!(!lib.has("browse:Home/a/x"));
    }

    #[test]
    fn the_trashs_top_entries_can_be_restored() {
        let mut trash = browsed("browse:Trash", true);
        trash.browsed.as_mut().unwrap().kind = BrowseKind::Trash;
        let mut lib = Library { games: vec![trash], ..Library::default() };
        lib.set_listing(0, vec![browsed("browse:Trash/old", true)]);
        assert_eq!(lib.games[1].browsed.as_ref().unwrap().kind, BrowseKind::Trashed);
        lib.set_listing(1, vec![browsed("browse:Trash/old/in", false)]);
        assert_eq!(lib.games[2].browsed.as_ref().unwrap().kind, BrowseKind::Plain, "only the top");
    }

    #[test]
    fn colours_are_stable_and_muted() {
        assert_eq!(color_of("steam:1"), color_of("steam:1"));
        let c = color_of("anything");
        for shift in [0, 8, 16] {
            assert!((c >> shift) & 0xff >= 0x40);
        }
    }
}
