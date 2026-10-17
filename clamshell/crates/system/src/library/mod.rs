//! The library from disk: library.json's content dirs (see [`content`]), installed Steam games, the
//! desktop files' games and apps; play time and favourites from the database. The model is
//! clamshell-library's.

pub mod content;
pub mod steam;

use std::collections::HashMap;
use std::path::{Path, PathBuf};

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
    /// Where mpv keeps videos' places.
    pub watch_later: Vec<PathBuf>,
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
            watch_later: paths.watch_later.clone(),
        }
    }
}

/// The tab Steam's games and the desktop files' games go in, and the desktop files' apps.
const GAMES: &str = "Games";
const APPS: &str = "Apps";

/// Puts `entries` (a source's, from `dir`) in the folder at `path` in `tab` (`A/B`: B in A), made where it
/// isn't there yet (its art from `dir`); the folder at the top of the tab if it is new (to add to it).
fn into_folder(games: &mut Vec<Game>, folders: &mut HashMap<(String, String), usize>, tab: &str, path: &str, dir: &Path, entries: &[usize]) -> Option<usize> {
    let mut new_top = None;
    let mut parent: Option<usize> = None;
    let mut at = String::new();
    for part in path.split('/').map(str::trim).filter(|p| !p.is_empty()) {
        if !at.is_empty() {
            at.push('/');
        }
        at.push_str(part);
        let folder = match folders.get(&(tab.to_owned(), at.clone())) {
            Some(&f) => f,
            None => {
                let f = games.len();
                games.push(Game { parent, ..Game::new(&format!("folder:{tab}/{at}"), part) });
                folders.insert((tab.to_owned(), at.clone()), f);
                match parent.and_then(|p| games.get_mut(p)) {
                    Some(p) => p.items.push(f),
                    None => new_top = Some(f),
                }
                f
            }
        };
        parent = Some(folder);
    }
    let folder = parent?;
    for &e in entries {
        if let Some(g) = games.get_mut(e) {
            g.parent = Some(folder);
        }
    }
    if let Some(g) = games.get_mut(folder) {
        g.items.extend_from_slice(entries);
        if g.box_front.is_none() {
            g.box_front = content::folder_art(dir);
        }
    }
    new_top
}

/// The Tools tab's entries: id, title, the tool, what it does.
const TOOLS: [(&str, &str, &str, &str); 2] = [
    (
        "tool:rescan",
        "Rescan library",
        "rescan",
        "Reads library.json, its folders, Steam and the desktop files again, then fetches the covers and details still missing.",
    ),
    ("tool:refetch", "Fetch missing art again", "refetch", "Tries again for the entries whose covers and details weren't found, then rescans."),
];

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
    let (config, config_problem) = sources.config.as_deref().map(content::read_config).unwrap_or_default();
    let mut problems: Vec<String> = config_problem.into_iter().collect();
    let mut away: Vec<String> = Vec::new();
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
    // The sources' folders in their tabs, by tab and path.
    let mut folders: HashMap<(String, String), usize> = HashMap::new();
    for source in &config.sources {
        let dir = content::expand(&source.dir, sources.home.as_deref());
        if !dir.is_dir() {
            problems.push(format!("{}: no folder {}", source.id, dir.display()));
            away.push(format!("{}:", source.id));
        }
        let entries = content::scan(source, sources.home.as_deref(), &mut games);
        let how = source.metadata.as_deref().and_then(|m| {
            How::parse(m).or_else(|| {
                problems.push(format!("{}: metadata \"{m}\" isn't libretro:<system>, tmdb:movie or tmdb:tv", source.id));
                None
            })
        });
        if matches!(how, Some(How::Tmdb(_))) && config.keys.tmdb.is_none() {
            problems.push(format!("{}: tmdb needs keys.tmdb", source.id));
        }
        if let Some(how) = how {
            // (libretro has games, not folders of them.)
            let fits = |i: &usize| !matches!(how, How::Libretro(_)) || games.get(*i).is_some_and(|g| !g.is_group());
            hows.extend(entries.iter().copied().filter(fits).map(|i| (i, how.clone())));
        }
        let entries = match source.group.as_deref().map(|g| g.trim_matches('/')).filter(|g| !g.is_empty() && !entries.is_empty()) {
            Some(path) => {
                let dir = content::expand(&source.dir, sources.home.as_deref());
                into_folder(&mut games, &mut folders, &source.tab, path, &dir, &entries).into_iter().collect()
            }
            None => entries,
        };
        add(&mut collections, &source.tab, source.icon.as_deref(), entries);
    }
    // A folder's entries (from its sources, and its folders) by title.
    for &f in folders.values() {
        let mut items = games.get(f).map(|g| g.items.clone()).unwrap_or_default();
        items.sort_by(|&a, &b| {
            let key = |i: usize| games.get(i).map(|g| (&g.sort_key, &g.title));
            key(a).cmp(&key(b))
        });
        if let Some(g) = games.get_mut(f) {
            g.items = items;
        }
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
    // ClamShell's own tools, last.
    let first = games.len();
    for (id, title, tool, summary) in TOOLS {
        games.push(Game { launch: Launch::Tool(tool.into()), tool: true, summary: summary.into(), ..Game::new(id, title) });
    }
    collections.push(Collection::with_icon("Tools", "tools", (first..games.len()).collect()));
    for c in &mut collections {
        c.games.sort_by(|&a, &b| {
            let key = |i: usize| games.get(i).map(|g| (&g.sort_key, &g.title));
            key(a).cmp(&key(b))
        });
    }
    let mut metas = std::collections::HashMap::new();
    let mut durations = std::collections::HashMap::new();
    if let Some(db) = sources.db.as_deref().and_then(|f| crate::store::Store::open(f).map_err(|e| log::warn!("library: {}: {e}", f.display())).ok()) {
        metas = db.metas().unwrap_or_else(|e| {
            log::warn!("library: metadata: {e}");
            Default::default()
        });
        durations = db.durations().unwrap_or_else(|e| {
            log::warn!("library: lengths: {e}");
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
    let mut library = Library { collections, games, problems, away };
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
            wanted.push(Wanted { id: g.id.clone(), key: g.key.clone(), title: g.title.clone(), file: g.file.clone(), how, art: g.box_front.is_some() });
        } else if let Some(f) = fetched {
            library.apply_meta(i, &f.meta);
        }
    }
    // mpv's videos: where mpv left each, and its length (read once, then kept).
    for (i, g) in library.games.iter_mut().enumerate() {
        let Some(file) = g.mpv_file().map(std::path::Path::to_path_buf) else {
            continue;
        };
        g.position = crate::mpv::position(&sources.watch_later, &file);
        match durations.get(&*g.id) {
            Some(d) => g.duration = *d,
            None => wanted.push(Wanted { id: g.id.clone(), key: g.key.clone(), title: g.title.clone(), file: Some(file), how: How::Duration, art: true }),
        }
        let _ = i;
    }
    library.rebuild_derived();
    Loaded { library, wanted, keys: config.keys }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::indexing_slicing)]
mod tests {
    use super::*;

    #[test]
    fn sources_fill_folders_in_a_tab_each_with_its_own_launch() {
        let root = std::env::temp_dir().join(format!("clamshell-folders-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        for f in ["snes/Zelda.sfc", "snes/Mario.sfc", "snes/cover.png", "gba/Metroid.gba", "pc/Doom.sh"] {
            let p = root.join(f);
            std::fs::create_dir_all(p.parent().unwrap()).unwrap();
            std::fs::write(p, "").unwrap();
        }
        let source = |group: &str, id: &str, pattern: &str, emulator: &str| {
            let group = if group.is_empty() { String::new() } else { format!(r#""group": "{group}","#) };
            format!(
                r#"{{ "tab": "Games", {group} "id": "{id}", "dir": "{}/{id}", "match": ["{pattern}"], "launch": ["{emulator}", "{{path}}"] }}"#,
                root.display()
            )
        };
        let config = format!(
            r#"{{ "sources": [ {}, {}, {} ] }}"#,
            source("SNES", "snes", "*.sfc", "snes9x"),
            source("Handhelds/GBA", "gba", "*.gba", "mgba"),
            source("", "pc", "*.sh", "sh")
        );
        std::fs::write(root.join("library.json"), config).unwrap();
        let lib = load(&Sources { config: Some(root.join("library.json")), ..Sources::default() }).library;
        assert!(lib.problems.is_empty(), "{:?}", lib.problems);
        assert_eq!(lib.collections.last().map(|c| c.name.as_str()), Some("Tools"));
        let games = |ids: &[usize]| ids.iter().map(|&i| lib.games[i].title.clone()).collect::<Vec<_>>();
        let tab = lib.collections.iter().find(|c| c.name == "Games").unwrap();
        assert_eq!(games(&tab.games), ["Doom", "Handhelds", "SNES"]);
        let snes = &lib.games[tab.games[2]];
        assert_eq!(games(&snes.items), ["Mario", "Zelda"]);
        assert_eq!(snes.box_front, Some(root.join("snes/cover.png")), "its source's dir's art");
        let zelda = &lib.games[snes.items[1]];
        assert!(matches!(&zelda.launch, Launch::Program { argv, .. } if argv[0] == "snes9x"));
        assert_eq!(&*zelda.id, "snes:Zelda.sfc", "ids as without the folder");
        let handhelds = &lib.games[tab.games[1]];
        let gba = &lib.games[handhelds.items[0]];
        assert_eq!((gba.title.as_str(), games(&gba.items)), ("GBA", vec!["Metroid".to_owned()]));
        assert!(matches!(&lib.games[gba.items[0]].launch, Launch::Program { argv, .. } if argv[0] == "mgba"));
        assert_eq!(lib.groups_of(gba.items[0]), [tab.games[1], handhelds.items[0]]);
        let _ = std::fs::remove_dir_all(&root);
    }
}
