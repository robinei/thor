//! The game library: Pegasus Frontend's collections and metadata (the format shelf used on the GPD,
//! docs/pegasus-format.md in the GPD repo) and installed Steam games. Pure reading: no UI, no launching.

pub mod metafile;
pub mod pegasus;
pub mod sort;
pub mod steam;

use std::path::PathBuf;
use std::sync::Arc;

/// How a game starts.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum Launch {
    #[default]
    None,
    /// A Pegasus launch command, still raw (tokenised and filled in at launch): its workdir, and the dir
    /// relative paths in it are against.
    Command { command: String, workdir: String, basedir: PathBuf },
    /// Through Steam.
    Steam { appid: u32 },
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
            developers: Vec::new(),
            publishers: Vec::new(),
            genres: Vec::new(),
            players: 1,
            release: None,
            summary: String::new(),
            description: String::new(),
            tool: false,
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
    /// Its games, as indices into [`Library::games`], in title order.
    pub games: Vec<usize>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Library {
    pub collections: Vec<Collection>,
    pub games: Vec<Game>,
}

/// Where to read from; `None` skips a source.
#[derive(Debug, Clone, Default)]
pub struct Sources {
    /// Pegasus' config dir (`~/.config/pegasus-frontend`), for `game_dirs.txt`.
    pub pegasus_config: Option<PathBuf>,
    /// Steam's data dir.
    pub steam: Option<PathBuf>,
}

impl Sources {
    /// The usual places for this user.
    pub fn default_for_user() -> Self {
        let config = std::env::var_os("XDG_CONFIG_HOME")
            .map(PathBuf::from)
            .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config")));
        Sources { pegasus_config: config.map(|c| c.join("pegasus-frontend")), steam: steam::default_root() }
    }
}

impl Library {
    /// Reads every source: Pegasus collections, then one "Steam" collection; collections sorted by
    /// name, games in each by title.
    pub fn load(sources: &Sources) -> Library {
        let dirs = sources.pegasus_config.as_deref().map(pegasus::read_game_dirs).unwrap_or_default();
        let (mut collections, mut games) = pegasus::scan(&dirs, 0);
        if let Some(root) = sources.steam.as_deref() {
            let steam = steam::games(root);
            if !steam.is_empty() {
                let first = games.len();
                collections.push(Collection {
                    name: "Steam".into(),
                    short_name: "steam".into(),
                    sort_key: sort::key("Steam"),
                    summary: "Installed Steam games".into(),
                    tools: false,
                    games: (first..first + steam.len()).collect(),
                });
                games.extend(steam);
            }
        }
        for c in &mut collections {
            c.games.sort_by(|&a, &b| {
                let key = |i: usize| games.get(i).map(|g| (&g.sort_key, &g.title));
                key(a).cmp(&key(b))
            });
        }
        collections.sort_by(|a, b| a.sort_key.cmp(&b.sort_key).then(a.name.cmp(&b.name)));
        Library { collections, games }
    }
}

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
    let collection = |name: &str, games: Vec<usize>| Collection {
        name: name.into(),
        short_name: name.to_lowercase(),
        sort_key: sort::key(name),
        summary: String::new(),
        tools: false,
        games,
    };
    Library { collections: vec![collection("Favourites", (0..n).collect()), collection("Recent", vec![3, 1])], games }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn colours_are_stable_and_muted() {
        assert_eq!(color_of("steam:1"), color_of("steam:1"));
        let c = color_of("anything");
        for shift in [0, 8, 16] {
            assert!((c >> shift) & 0xff >= 0x40);
        }
    }
}
