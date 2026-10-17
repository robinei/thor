//! The library from disk: library.json's content dirs (see [`content`]), installed Steam games, the
//! desktop files' games and apps; play time and favourites from the database. The model is
//! clamshell-library's.

pub mod content;
pub mod steam;

use std::path::PathBuf;

use clamshell_library::{Collection, Game, Launch, Library, Meta};

use crate::meta::{How, Keys, RETRY_MISS, Wanted};

use crate::paths::Paths;

/// Where to read from; `None` skips a source.
#[derive(Debug, Clone, Default)]
pub struct Sources {
    /// library.json.
    pub config: Option<PathBuf>,
    /// The home dir, for `~/` in library.json.
    pub home: Option<PathBuf>,
    /// Steam's data dir.
    pub steam: Option<PathBuf>,
    /// The database, for play time and favourites.
    pub db: Option<PathBuf>,
    /// The XDG data dirs, the installed apps' desktop files in their `applications` (see
    /// [`crate::desktop`]).
    pub data_dirs: Vec<PathBuf>,
}

impl Sources {
    /// ClamShell's paths, and Steam's data dir under `home` unless `steam` is off.
    pub fn new(paths: &Paths, home: Option<&std::path::Path>, steam: bool) -> Sources {
        Sources {
            config: paths.library.clone(),
            home: home.map(std::path::Path::to_path_buf),
            steam: home.filter(|_| steam).and_then(steam::default_root),
            db: paths.db(),
            data_dirs: crate::desktop::data_dirs(),
        }
    }
}

/// The tab Steam's games and the desktop files' games go in, and the desktop files' apps.
const GAMES: &str = "Games";
const APPS: &str = "Apps";

/// The library, and what to fetch for it.
#[derive(Debug, Default)]
pub struct Loaded {
    pub library: Library,
    /// The entries to fetch metadata for (none fetched, the source changed, a miss long ago, or its
    /// images gone from the cache).
    pub wanted: Vec<Wanted>,
    pub keys: Keys,
}

/// Reads every source into tabs: library.json's, in the order the sources first name them, then Games
/// and Apps unless named already (each left out while empty); entries in each by title. Then each
/// entry's plays, whether it is a favourite, and what was fetched about it.
pub fn load(sources: &Sources) -> Loaded {
    let config = sources.config.as_deref().map(content::read_config).unwrap_or_default();
    let mut games: Vec<Game> = Vec::new();
    let mut collections: Vec<Collection> = Vec::new();
    let add = |collections: &mut Vec<Collection>, tab: &str, icon: Option<&str>, entries: Vec<usize>| {
        let c = match collections.iter().position(|c| c.name == tab) {
            Some(i) => collections.get_mut(i),
            None => {
                collections.push(Collection::new(tab, Vec::new()));
                collections.last_mut()
            }
        };
        if let Some(c) = c {
            if c.icon.is_none() {
                c.icon = icon.map(str::to_owned);
            }
            c.games.extend(entries);
        }
    };
    // Where each entry's metadata comes from, if anywhere.
    let mut hows: Vec<(usize, How)> = Vec::new();
    for source in &config.sources {
        let entries = content::scan(source, sources.home.as_deref(), &mut games);
        let how = source.metadata.as_deref().and_then(|m| {
            How::parse(m).or_else(|| {
                log::warn!("library: {}: metadata {m:?} isn't one of libretro:<system>, tmdb:movie, tmdb:tv", source.id);
                None
            })
        });
        if let Some(how) = how {
            // (libretro has games, not folders of them.)
            let fits = |i: &usize| !matches!(how, How::Libretro(_)) || games.get(*i).is_some_and(|g| !g.is_group());
            hows.extend(entries.iter().copied().filter(fits).map(|i| (i, how.clone())));
        }
        add(&mut collections, &source.tab, source.icon.as_deref(), entries);
    }
    let extend = |games: &mut Vec<Game>, more: Vec<Game>| {
        let first = games.len();
        games.extend(more);
        (first..games.len()).collect::<Vec<_>>()
    };
    let steam = sources.steam.as_deref().map(steam::games).unwrap_or_default();
    let steam = extend(&mut games, steam);
    for &i in &steam {
        if let Some(Launch::Steam { appid }) = games.get(i).map(|g| &g.launch) {
            hows.push((i, How::Steam(*appid)));
        }
    }
    add(&mut collections, GAMES, Some("gamepad"), steam);
    let (apps, desktop_games): (Vec<Game>, Vec<Game>) = crate::desktop::entries(&sources.data_dirs).into_iter().partition(|g| g.tool);
    let desktop_games = extend(&mut games, desktop_games);
    if config.keys.steamgriddb.is_some() {
        hows.extend(desktop_games.iter().map(|&i| (i, How::SteamGridDb)));
    }
    add(&mut collections, GAMES, Some("gamepad"), desktop_games);
    let apps = extend(&mut games, apps);
    add(&mut collections, APPS, Some("apps"), apps);
    collections.retain(|c| !c.games.is_empty());
    for c in &mut collections {
        c.games.sort_by(|&a, &b| {
            let key = |i: usize| games.get(i).map(|g| (&g.sort_key, &g.title));
            key(a).cmp(&key(b))
        });
    }
    let mut metas = std::collections::HashMap::new();
    if let Some(db) = sources.db.as_deref().and_then(|f| crate::store::Store::open(f).map_err(|e| log::warn!("library: {}: {e}", f.display())).ok()) {
        metas = db.metas().unwrap_or_else(|e| {
            log::warn!("library: metadata: {e}");
            Default::default()
        });
        let plays = db.plays().unwrap_or_else(|e| {
            log::warn!("library: play time: {e}");
            Default::default()
        });
        let favourites = db.favourites().unwrap_or_else(|e| {
            log::warn!("library: favourites: {e}");
            Default::default()
        });
        for g in &mut games {
            if let Some(p) = plays.get(&*g.id) {
                g.plays = *p;
            }
            g.favourite = favourites.contains(&*g.id);
        }
    }
    let mut library = Library { collections, games };
    // What was fetched before, where its images are still there; what to fetch.
    let now = crate::meta::now();
    let mut wanted = Vec::new();
    for (i, how) in hows {
        let Some(g) = library.games.get(i) else {
            continue;
        };
        let fetched = metas.get(&*g.id);
        let gone = |p: &Option<std::path::PathBuf>| p.as_ref().is_some_and(|p| !p.exists());
        let want = match fetched {
            None => true,
            Some(f) => {
                f.source != how.source() || gone(&f.meta.cover) || gone(&f.meta.shot) || (f.meta == Meta::default() && now.saturating_sub(f.at) > RETRY_MISS)
            }
        };
        if want {
            wanted.push(Wanted { id: g.id.clone(), key: g.key.clone(), title: g.title.clone(), how, art: g.box_front.is_some() });
        } else if let Some(f) = fetched {
            library.apply_meta(i, &f.meta);
        }
    }
    library.rebuild_derived();
    Loaded { library, wanted, keys: config.keys }
}
