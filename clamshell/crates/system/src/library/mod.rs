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

/// The order of a tab's or a folder's entries: folders first, then by title.
fn list_order(games: &[Game], a: usize, b: usize) -> std::cmp::Ordering {
    let key = |i: usize| games.get(i).map(|g| (!g.is_group(), &g.sort_key, &g.title));
    key(a).cmp(&key(b))
}

/// The first frame (a video's, in order, all the way down) in folder `i`.
fn first_frame(library: &Library, i: usize) -> Option<PathBuf> {
    let g = library.games.get(i)?;
    g.items.iter().find_map(|&item| match library.games.get(item) {
        Some(e) if e.is_group() => first_frame(library, item),
        Some(e) => e.shot.clone().filter(|_| e.mpv_file().is_some()),
        None => None,
    })
}

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
        // (Without the key nothing is asked: not a miss, so it is fetched once the key is there.)
        let how = match how {
            Some(How::Tmdb(_)) if config.keys.tmdb.is_none() => {
                problems.push(format!("{}: tmdb needs keys.tmdb", source.id));
                None
            }
            how => how,
        };
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
    let steam = match config.steam.group.as_deref().map(|g| g.trim_matches('/')).filter(|g| !g.is_empty()) {
        Some(path) => into_folder(&mut games, &mut folders, GAMES, path, Path::new(""), &steam).into_iter().collect(),
        None => steam,
    };
    add(&mut collections, GAMES, Some("gamepad"), steam);
    // The desktop files' apps and games: where library.json's rules put them, else Games for a game and
    // Apps for the rest (the games first, then the apps, each tab made where it is first named).
    let mut placed: Vec<(String, Option<String>, Option<String>, Game)> = Vec::new();
    for d in crate::desktop::entries(&sources.data_dirs) {
        let rule = config.desktop.iter().find(|r| r.matches(&d.id, &d.name, &d.categories));
        if rule.is_some_and(|r| r.hide) {
            continue;
        }
        let default = if d.game.tool { APPS } else { GAMES };
        let tab = rule.and_then(|r| r.tab.clone()).unwrap_or_else(|| default.to_owned());
        let icon = rule.and_then(|r| r.icon.clone()).or_else(|| (tab == default).then(|| if d.game.tool { "apps" } else { "gamepad" }.to_owned()));
        // (In Apps: an app, which Recent leaves out; anywhere else, played like a game.)
        let game = Game { tool: tab == APPS, ..d.game };
        let group = rule.and_then(|r| r.group.clone()).map(|g| g.trim_matches('/').to_owned()).filter(|g| !g.is_empty());
        placed.push((tab, icon, group, game));
    }
    placed.sort_by_key(|(_, _, _, g)| g.tool);
    for (tab, icon, group, game) in placed {
        let tool = game.tool;
        let i = extend(&mut games, vec![game]);
        if config.keys.steamgriddb.is_some() && !tool {
            hows.extend(i.iter().map(|&i| (i, How::SteamGridDb)));
        }
        // In a folder of the tab (as a source's `group`), or at its top.
        let i = match group {
            Some(path) => into_folder(&mut games, &mut folders, &tab, &path, Path::new(""), &i).into_iter().collect(),
            None => i,
        };
        add(&mut collections, &tab, icon.as_deref(), i);
    }
    // A folder's entries (from its sources, and its folders) in list order.
    for &f in folders.values() {
        let mut items = games.get(f).map(|g| g.items.clone()).unwrap_or_default();
        items.sort_by(|&a, &b| list_order(&games, a, b));
        if let Some(g) = games.get_mut(f) {
            g.items = items;
        }
    }
    collections.retain(|c| !c.games.is_empty());
    for c in &mut collections {
        c.games.sort_by(|&a, &b| list_order(&games, a, b));
    }
    // ClamShell's own tools, last, in their order.
    let first = games.len();
    for (id, title, tool, summary) in TOOLS {
        games.push(Game { launch: Launch::Tool(tool.into()), tool: true, summary: summary.into(), ..Game::new(id, title) });
    }
    collections.push(Collection::with_icon("Tools", "tools", (first..games.len()).collect()));
    let mut metas = std::collections::HashMap::new();
    let mut probes = std::collections::HashMap::new();
    if let Some(db) = sources.db.as_deref().and_then(|f| crate::store::Store::open(f).map_err(|e| log::warn!("library: {}: {e}", f.display())).ok()) {
        metas = db.metas().unwrap_or_else(|e| {
            log::warn!("library: metadata: {e}");
            Default::default()
        });
        probes = db.probes().unwrap_or_else(|e| {
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
    // mpv's videos: where mpv left each, its length and a frame of it (read once, then kept).
    for g in &mut library.games {
        let Some(file) = g.mpv_file().map(std::path::Path::to_path_buf) else {
            continue;
        };
        let kept = crate::mpv::position(&sources.watch_later, &file);
        (g.position, g.position_at) = (kept.map(|(s, _)| s), kept.map_or(0, |(_, at)| at));
        let probe = probes.get(&*g.id).filter(|p| p.frame.as_ref().is_none_or(|f| f.exists()));
        match probe {
            Some(p) => {
                g.duration = p.seconds;
                if g.shot.is_none() {
                    g.shot.clone_from(&p.frame);
                }
            }
            None => wanted.push(Wanted { id: g.id.clone(), key: g.key.clone(), title: g.title.clone(), file: Some(file), how: How::Duration, art: true }),
        }
    }
    // A folder of videos without art: the frame of its first video that has one.
    for i in 0..library.games.len() {
        let bare = library.games.get(i).is_some_and(|g| g.is_group() && g.box_front.is_none() && g.shot.is_none());
        if bare
            && let Some(frame) = first_frame(&library, i)
            && let Some(g) = library.games.get_mut(i)
        {
            g.shot = Some(frame);
        }
    }
    library.rebuild_derived();
    Loaded { library, wanted, keys: config.keys }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::indexing_slicing)]
mod tests {
    use super::*;

    #[test]
    fn folders_come_first_and_steam_can_have_its_own() {
        let root = std::env::temp_dir().join(format!("clamshell-steam-group-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let steam = root.join("steam");
        std::fs::create_dir_all(steam.join("steamapps")).unwrap();
        std::fs::write(steam.join("steamapps/libraryfolders.vdf"), format!("\"libraryfolders\" {{ \"0\" {{ \"path\" \"{}\" }} }}", steam.display())).unwrap();
        let manifest = |id: u32, name: &str| format!("\"AppState\" {{ \"appid\" \"{id}\" \"name\" \"{name}\" \"StateFlags\" \"4\" }}");
        std::fs::write(steam.join("steamapps/appmanifest_10.acf"), manifest(10, "Arma")).unwrap();
        std::fs::write(steam.join("steamapps/appmanifest_20.acf"), manifest(20, "Zork")).unwrap();
        std::fs::create_dir_all(root.join("snes")).unwrap();
        std::fs::write(root.join("snes/Mario.sfc"), "").unwrap();
        let config = |steam_group: &str| {
            format!(
                r#"{{ {steam_group} "sources": [{{ "tab": "Games", "group": "SNES", "id": "snes", "dir": "{}", "match": ["*.sfc"], "launch": ["retroarch", "{{path}}"] }}] }}"#,
                root.join("snes").display()
            )
        };
        let games_tab = |json: &str| {
            std::fs::write(root.join("library.json"), json).unwrap();
            let lib = load(&Sources { config: Some(root.join("library.json")), steam: Some(steam.clone()), ..Sources::default() }).library;
            let tab = lib.collections.iter().find(|c| c.name == "Games").unwrap();
            tab.games.iter().map(|&i| lib.games[i].title.clone()).collect::<Vec<_>>()
        };
        assert_eq!(games_tab(&config("")), ["SNES", "Arma", "Zork"], "the folder first, before Arma");
        assert_eq!(games_tab(&config(r#""steam": { "group": "Steam" },"#)), ["SNES", "Steam"], "Steam's games in their folder");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn desktop_rules_move_or_hide_entries() {
        let rule = |json: &str| serde_json::from_str::<content::DesktopRule>(json).unwrap();
        let cats = |c: &[&str]| c.iter().map(|s| (*s).to_owned()).collect::<Vec<_>>();
        assert!(rule(r#"{ "name": "battle.NET", "tab": "Apps" }"#).matches("net.lutris.battlenet-1", "Battle.net", &cats(&["Game"])), "any case");
        assert!(rule(r#"{ "id": "*lutris*" }"#).matches("net.lutris.Lutris", "Lutris", &[]));
        assert!(rule(r#"{ "categories": "Emu*" }"#).matches("x", "RetroArch", &cats(&["Game", "Emulator"])));
        assert!(!rule(r#"{ "id": "*lutris*", "name": "Steam" }"#).matches("net.lutris.Lutris", "Lutris", &[]), "all of them");
        assert!(rule("{}").matches("any", "Any", &[]), "nothing to match: every entry");
        let root = std::env::temp_dir().join(format!("clamshell-rules-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let apps = root.join("data/applications");
        std::fs::create_dir_all(&apps).unwrap();
        let write = |id: &str, name: &str, cats: &str| {
            std::fs::write(apps.join(format!("{id}.desktop")), format!("[Desktop Entry]\nType=Application\nName={name}\nExec={id}\nCategories={cats}\n"))
                .unwrap();
        };
        write("celeste", "Celeste", "Game;");
        write("lutris", "Lutris", "Game;");
        write("uninstall-foo", "Uninstall Foo", "Utility;");
        write("foot", "Foot", "System;");
        let config = r#"{ "desktop": [
            { "id": "lutris", "tab": "Launchers", "icon": "apps" },
            { "id": "celeste", "tab": "Games", "group": "Platformers" },
            { "name": "uninstall*", "hide": true }
        ] }"#;
        std::fs::write(root.join("library.json"), config).unwrap();
        let lib = load(&Sources { config: Some(root.join("library.json")), data_dirs: vec![root.join("data")], ..Sources::default() }).library;
        let tab = |name: &str| lib.collections.iter().find(|c| c.name == name).map(|c| c.games.iter().map(|&i| lib.games[i].title.clone()).collect::<Vec<_>>());
        assert_eq!(tab("Games"), Some(vec!["Platformers".to_owned()]), "in its folder");
        let folder = lib.games.iter().find(|g| g.title == "Platformers").unwrap();
        assert_eq!(folder.items.iter().map(|&i| lib.games[i].title.as_str()).collect::<Vec<_>>(), ["Celeste"]);
        assert_eq!(tab("Launchers"), Some(vec!["Lutris".to_owned()]));
        assert_eq!(tab("Apps"), Some(vec!["Foot".to_owned()]), "Uninstall Foo hidden");
        let _ = std::fs::remove_dir_all(&root);
    }

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
        assert_eq!(games(&tab.games), ["Handhelds", "SNES", "Doom"], "folders first");
        let snes = &lib.games[tab.games[1]];
        assert_eq!(games(&snes.items), ["Mario", "Zelda"]);
        assert_eq!(snes.box_front, Some(root.join("snes/cover.png")), "its source's dir's art");
        let zelda = &lib.games[snes.items[1]];
        assert!(matches!(&zelda.launch, Launch::Program { argv, .. } if argv[0] == "snes9x"));
        assert_eq!(&*zelda.id, "snes:Zelda.sfc", "ids as without the folder");
        let handhelds = &lib.games[tab.games[0]];
        let gba = &lib.games[handhelds.items[0]];
        assert_eq!((gba.title.as_str(), games(&gba.items)), ("GBA", vec!["Metroid".to_owned()]));
        assert!(matches!(&lib.games[gba.items[0]].launch, Launch::Program { argv, .. } if argv[0] == "mgba"));
        assert_eq!(lib.groups_of(gba.items[0]), [tab.games[0], handhelds.items[0]]);
        let _ = std::fs::remove_dir_all(&root);
    }
}
