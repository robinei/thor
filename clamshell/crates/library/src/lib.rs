//! The library's model: entries (games, apps, movies, groups of them such as a series' seasons and
//! episodes), the tabs they are in and how they sort, and the parts of Steam's files and of name patterns
//! that need no disk. No I/O and no dependencies: scanning (library.json's content dirs, Steam, desktop
//! files), play time and favourites are clamshell-system's (`library`).

pub mod pattern;
pub mod plays;
pub mod sort;
pub mod steam;

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
    pub box_front: Option<PathBuf>,
    /// An icon (an app's), drawn where there is no box art.
    pub icon: Option<PathBuf>,
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
            box_front: None,
            icon: None,
            tool: false,
            items: Vec::new(),
            parent: None,
            plays: Plays::default(),
            favourite: false,
        }
    }

    /// Whether it is a group (it opens rather than starts).
    pub fn is_group(&self) -> bool {
        !self.items.is_empty()
    }
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
}

impl Library {
    /// Rebuilds Recent (the last entries played, apps left out) and Favourites in front of the other
    /// collections; either is left out while empty.
    pub fn rebuild_derived(&mut self) {
        self.collections.retain(|c| !c.derived);
        let mut recent: Vec<usize> = (0..self.games.len()).filter(|&i| self.games.get(i).is_some_and(|g| g.plays.count > 0 && !g.tool)).collect();
        recent.sort_by_key(|&i| std::cmp::Reverse(self.games.get(i).map_or(0, |g| g.plays.last)));
        recent.truncate(RECENT);
        let mut favourites: Vec<usize> = (0..self.games.len()).filter(|&i| self.games.get(i).is_some_and(|g| g.favourite)).collect();
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
    .map(|(id, title, color)| Game { color, ..Game::new(id, title) })
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
            let argv = vec!["mpv".into(), format!("/videos/{}", file.trim_start_matches("tv:"))];
            games.push(Game { parent: Some(s), color: 0x305a78, launch: Launch::program(argv), ..Game::new(&file, episode) });
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
    Library { collections, games }
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
    fn colours_are_stable_and_muted() {
        assert_eq!(color_of("steam:1"), color_of("steam:1"));
        let c = color_of("anything");
        for shift in [0, 8, 16] {
            assert!((c >> shift) & 0xff >= 0x40);
        }
    }
}
