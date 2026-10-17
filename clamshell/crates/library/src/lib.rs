//! The game library's model: games, collections and how they sort, and the parts of Pegasus
//! Frontend's format (the one shelf used on the GPD, docs/pegasus-format.md in the GPD repo) and Steam's
//! files that need no disk. No I/O and no dependencies: scanning the game dirs and Steam's folders,
//! play time (stats.db) and favourites (favorites.txt) are clamshell-system's (`library`).

pub mod launch;
pub mod metafile;
pub mod pegasus;
pub mod plays;
pub mod sort;
pub mod steam;

use std::path::PathBuf;
use std::sync::Arc;

pub use launch::LaunchSpec;
pub use plays::Plays;

/// How a game starts.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum Launch {
    #[default]
    None,
    /// A Pegasus launch command.
    Command(LaunchSpec),
    /// Through Steam.
    Steam { appid: u32 },
    /// A program with its arguments, no file (an installed app's, from its desktop file).
    Program(Vec<String>),
}

/// One game as the rest of the program sees it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Game {
    /// Stable id: the game file's path, or `steam:<appid>`. Focus, settings and play time are keyed by
    /// it. Shared, so the UI can put it in a frame's targets without allocating.
    pub id: Arc<str>,
    pub title: String,
    /// What it sorts by (Pegasus order).
    pub sort_key: Vec<(u8, char)>,
    /// Stand-in for box art while there is none or it is loading: 0xRRGGBB, from the id.
    pub color: u32,
    pub file: Option<PathBuf>,
    pub launch: Launch,
    pub box_front: Option<PathBuf>,
    /// An icon (an app's), drawn where there is no box art.
    pub icon: Option<PathBuf>,
    pub developers: Vec<String>,
    pub publishers: Vec<String>,
    pub genres: Vec<String>,
    pub players: u8,
    /// Year, month, day.
    pub release: Option<(u16, u8, u8)>,
    pub summary: String,
    pub description: String,
    /// A tool (Utilities), not a game.
    pub tool: bool,
    /// Play history (stats.db) and favourite (favorites.txt).
    pub plays: Plays,
    pub favourite: bool,
}

impl Game {
    pub fn new(id: &str, title: &str) -> Self {
        Game {
            id: id.into(),
            title: title.into(),
            sort_key: sort::key(title),
            color: color_of(id),
            file: None,
            launch: Launch::None,
            box_front: None,
            icon: None,
            developers: Vec::new(),
            publishers: Vec::new(),
            genres: Vec::new(),
            players: 1,
            release: None,
            summary: String::new(),
            description: String::new(),
            tool: false,
            plays: Plays::default(),
            favourite: false,
        }
    }
}

/// A muted colour from a hash of `id`, stable across runs.
fn color_of(id: &str) -> u32 {
    let h = id.bytes().fold(0x811c_9dc5u32, |h, b| (h ^ u32::from(b)).wrapping_mul(0x0100_0193));
    let channel = |shift: u32| 0x40 + ((h >> shift) & 0x7f);
    (channel(16) << 16) | (channel(8) << 8) | channel(0)
}

/// A tab in shelf: a Pegasus collection, or the installed Steam games.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Collection {
    pub name: String,
    /// Lowercase short name (Pegasus `shortname`, used for logos).
    pub short_name: String,
    pub sort_key: Vec<(u8, char)>,
    pub summary: String,
    /// Utilities: tools, not games.
    pub tools: bool,
    /// Recent or Favourites: made from the others, rebuilt when plays or favourites change.
    pub derived: bool,
    /// Its games, as indices into [`Library::games`], in title order.
    pub games: Vec<usize>,
}

impl Collection {
    /// A collection of these games named `name` (its short name the name lowercased), without a summary.
    pub fn new(name: &str, games: Vec<usize>) -> Collection {
        Collection {
            name: name.into(),
            short_name: name.to_lowercase(),
            sort_key: sort::key(name),
            summary: String::new(),
            tools: false,
            derived: false,
            games,
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Library {
    pub collections: Vec<Collection>,
    pub games: Vec<Game>,
}

impl Library {
    /// Rebuilds Recent (the last games played, tools left out) and Favourites in front of the other
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
        let derived = |name: &str, games: Vec<usize>| Collection { derived: true, ..Collection::new(name, games) };
        let mut front = Vec::new();
        if !recent.is_empty() {
            front.push(derived("Recent", recent));
        }
        if !favourites.is_empty() {
            front.push(derived("Favourites", favourites));
        }
        self.collections.splice(0..0, front);
    }

    /// The ids of the favourite games (for favorites.txt).
    pub fn favourite_ids(&self) -> impl Iterator<Item = &str> {
        self.games.iter().filter(|g| g.favourite).map(|g| &*g.id)
    }
}

/// Games in the Recent tab.
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

/// Two demo collections of the demo games, for development and snapshots.
pub fn demo_library() -> Library {
    let games = demo_games();
    let n = games.len();
    Library { collections: vec![Collection::new("Games", (0..n).collect()), Collection::new("More", vec![3, 1])], games }
}

#[cfg(test)]
#[allow(clippy::indexing_slicing)]
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
    fn colours_are_stable_and_muted() {
        assert_eq!(color_of("steam:1"), color_of("steam:1"));
        let c = color_of("anything");
        for shift in [0, 8, 16] {
            assert!((c >> shift) & 0xff >= 0x40);
        }
    }
}
