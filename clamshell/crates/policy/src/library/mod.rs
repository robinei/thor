//! The game library from disk: Pegasus' game dirs, installed Steam games, play time and favourites. The
//! model and the files' syntax are clamshell-library's.

pub mod favorites;
pub mod pegasus;
pub mod stats;
pub mod steam;

use std::path::PathBuf;

use clamshell_library::{Collection, Library};

use crate::paths::Paths;

/// Where to read from; `None` skips a source.
#[derive(Debug, Clone, Default)]
pub struct Sources {
    /// Pegasus' config dir, for `game_dirs.txt`.
    pub pegasus_config: Option<PathBuf>,
    /// Steam's data dir.
    pub steam: Option<PathBuf>,
    /// Where play time (stats.db) and favourites (favorites.txt) are kept.
    pub state_dir: Option<PathBuf>,
}

impl Sources {
    /// ClamShell's paths, and Steam's data dir under `home` unless `steam` is off.
    pub fn new(paths: &Paths, home: Option<&std::path::Path>, steam: bool) -> Sources {
        Sources {
            pegasus_config: paths.pegasus_config.clone(),
            steam: home.filter(|_| steam).and_then(steam::default_root),
            state_dir: paths.state_dir.clone(),
        }
    }
}

/// Reads every source: Pegasus collections, then one "Steam" collection; collections sorted by name,
/// games in each by title; then each game's plays and whether it is a favourite.
pub fn load(sources: &Sources) -> Library {
    let dirs = sources.pegasus_config.as_deref().map(pegasus::read_game_dirs).unwrap_or_default();
    let (mut collections, mut games) = pegasus::scan(&dirs);
    if let Some(root) = sources.steam.as_deref() {
        let steam = steam::games(root);
        if !steam.is_empty() {
            // After Pegasus' games.
            let first = games.len();
            collections.push(Collection { summary: "Installed Steam games".into(), ..Collection::new("Steam", (first..first + steam.len()).collect()) });
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
    if let Some(dir) = sources.state_dir.as_deref() {
        let plays = stats::read(&dir.join("stats.db"));
        let favourites = favorites::read(dir);
        for g in &mut games {
            if let Some(p) = plays.get(&*g.id) {
                g.plays = *p;
            }
            g.favourite = favourites.contains(&*g.id);
        }
    }
    let mut library = Library { collections, games };
    library.rebuild_derived();
    library
}
