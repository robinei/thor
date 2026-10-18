//! The library from disk: library.json's content dirs (see [`content`]), installed Steam games, the
//! desktop files' games and apps; play time and favourites from the database. The model is
//! clamshell-library's.

pub mod content;
pub mod runners;
pub mod steam;

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use clamshell_library::{BrowseKind, Browsed, Collection, Game, Launch, Library, Meta};

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

/// The folders made in the tabs, by tab and path; library.json's places (their ids, icons, pictures),
/// image files in them from `config_dir` (`~/` the home dir).
struct Folders<'a> {
    made: HashMap<(String, String), usize>,
    places: &'a [content::Place],
    home: Option<&'a Path>,
    config_dir: Option<&'a Path>,
}

impl Folders<'_> {
    /// Puts `entries` (a scan's, from `dir`) in the folder at `path` in `tab` (`A/B`: B in A), made where it
    /// isn't there yet (its art from a place declared there, else from `dir`); the folder at the top of the
    /// tab if it is new (to add to it).
    fn put(&mut self, games: &mut Vec<Game>, tab: &str, path: &str, dir: &Path, entries: &[usize]) -> Option<usize> {
        let mut new_top = None;
        let mut parent: Option<usize> = None;
        let mut at = String::new();
        for part in path.split('/').map(str::trim).filter(|p| !p.is_empty()) {
            if !at.is_empty() {
                at.push('/');
            }
            at.push_str(part);
            let folder = match self.made.get(&(tab.to_owned(), at.clone())) {
                Some(&f) => f,
                None => {
                    let f = games.len();
                    games.push(Game { parent, ..self.folder(tab, &at, part) });
                    self.made.insert((tab.to_owned(), at.clone()), f);
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

    /// A new folder at `at` in `tab`, titled `title`: a place declared there gives its id (`place:<id>`),
    /// icon and picture; else its id is its path (`folder:<tab>/<at>`).
    fn folder(&self, tab: &str, at: &str, title: &str) -> Game {
        let path = format!("{tab}/{at}");
        let Some(place) = self.places.iter().find(|p| p.path.trim().trim_matches('/') == path) else {
            return Game::new(&format!("folder:{path}"), title);
        };
        let file = |f: &str| match (f.strip_prefix("~/"), self.home, self.config_dir) {
            (Some(rest), Some(home), _) => home.join(rest),
            (None, _, Some(dir)) if !f.starts_with('/') => dir.join(f),
            _ => PathBuf::from(f),
        };
        let mut g = Game::new(&format!("place:{}", place.id), title);
        g.box_front = place.picture.as_deref().map(file);
        match place.icon.as_deref() {
            // (An image file, or one of the UI's icons by name.)
            Some(icon) if icon.contains(['/', '.']) => g.icon = Some(file(icon)),
            Some(icon) => g.glyph = Some(icon.into()),
            None => {}
        }
        g
    }
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

/// The order of a tab's or a folder's entries: folders first, then by title; browsed dirs' own folders
/// after the other folders, as library.json and the mounts have them.
fn list_order(games: &[Game], a: usize, b: usize) -> std::cmp::Ordering {
    let key = |i: usize| {
        games.get(i).map(|g| match g.browsed.as_ref().map(|b| &b.kind) {
            Some(BrowseKind::Root | BrowseKind::Trash | BrowseKind::Removable(_)) => (false, true, None),
            _ => (!g.is_group(), false, Some((&g.sort_key, &g.title))),
        })
    };
    key(a).cmp(&key(b))
}

/// A browsed dir's own folder.
fn browsed_root(id: &str, dir: PathBuf, kind: BrowseKind) -> Game {
    let glyph = match kind {
        BrowseKind::Trash => "trash",
        BrowseKind::Removable(_) => "card",
        _ => "folder",
    };
    Game { file: Some(dir), browsed: Some(Browsed { folder: true, kind, ..Browsed::default() }), glyph: Some(glyph.into()), ..Game::new(id, "") }
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
    let add = |collections: &mut Vec<Collection>, tab: &str, entries: Vec<usize>| {
        let c = match collections.iter().position(|c| c.name == tab) {
            Some(i) => collections.get_mut(i),
            None => {
                collections.push(Collection::new(tab, Vec::new()));
                collections.last_mut()
            }
        };
        if let Some(c) = c {
            c.games.extend(entries);
        }
    };
    // Where each entry's metadata comes from, if anywhere.
    let mut hows: Vec<(usize, How)> = Vec::new();
    // The folders in the tabs.
    let config_dir = sources.config.as_deref().and_then(Path::parent);
    let mut folders = Folders { made: HashMap::new(), places: &config.places, home: sources.home.as_deref(), config_dir };
    // (A tab shows its icon only, one of the UI's: something in it is always selected.)
    for p in config.places.iter().filter(|p| !p.path.trim().trim_matches('/').contains('/')) {
        if p.picture.is_some() || p.icon.as_deref().is_some_and(|i| i.contains(['/', '.'])) {
            problems.push(format!("{}: a tab has an icon of the UI's (gamepad, film, ...), no picture", p.path));
        }
    }
    // A place as library.json gives it (`@id` a declared one's path), or what is wrong with it.
    let resolve = |place: &str| content::resolve(place, &config.places);
    let (runners, runner_problems) = runners::in_force(&config.runners);
    problems.extend(runner_problems);
    let runner_keymaps = runners.iter().filter_map(|r| Some((clamshell_library::runner_key(&r.id), r.keymap.clone()?))).collect();
    for source in runners.iter().flat_map(runners::Runner::sources) {
        let source = &source;
        let dir = match content::expand_dir(&source.dir, sources.home.as_deref()) {
            Ok(dir) => dir,
            Err(name) => {
                problems.push(format!("{}: ${name} isn't set", source.id));
                continue;
            }
        };
        if !dir.is_dir() {
            problems.push(format!("{}: no folder {}", source.id, dir.display()));
            away.push(format!("{}:", source.id));
        }
        let first = games.len();
        let entries = content::scan(source, sources.home.as_deref(), &mut games);
        let runner = runners
            .iter()
            .find(|r| r.id == source.runner)
            .map_or_else(|| clamshell_library::RunnerOf::new(&source.runner, &source.runner_name, true), runners::Runner::of);
        for g in games.get_mut(first..).into_iter().flatten() {
            g.runner = Some(runner.clone());
        }
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
        let place = match resolve(&source.place) {
            Ok(place) => place,
            Err(e) => {
                problems.push(format!("{}: {e}", source.id));
                continue;
            }
        };
        let (tab, path) = content::place(&place);
        let entries = match path {
            "" => entries,
            _ if entries.is_empty() => entries,
            path => folders.put(&mut games, tab, path, &dir, &entries).into_iter().collect(),
        };
        add(&mut collections, tab, entries);
    }
    // Browsed dirs (library.json's, then the removable filesystems mounted): each a folder, listed when
    // it is opened.
    let mut roots: Vec<(String, Game)> = Vec::new();
    for root in &config.browse {
        let place = match resolve(&root.place) {
            Ok(place) => place.trim().trim_matches('/').to_owned(),
            Err(e) => {
                problems.push(e);
                continue;
            }
        };
        let place = place.as_str();
        let dir = match content::expand_dir(&root.dir, sources.home.as_deref()) {
            Ok(dir) if dir.is_dir() || root.trash => dir,
            Ok(dir) => {
                problems.push(format!("{place}: no folder {}", dir.display()));
                continue;
            }
            Err(name) => {
                problems.push(format!("{place}: ${name} isn't set"));
                continue;
            }
        };
        // (The Trash's folder may not be made yet.)
        if root.trash {
            let _ = std::fs::create_dir_all(&dir);
        }
        let kind = if root.trash { BrowseKind::Trash } else { BrowseKind::Root };
        roots.push((place.to_owned(), browsed_root(&format!("browse:{place}"), dir, kind)));
    }
    for m in crate::files::media_mounts() {
        let rule = config.removable.iter().find(|r| r.matches(&m.label, &m.uuid, &m.fs));
        if rule.is_some_and(|r| r.hide) {
            continue;
        }
        let name = if m.label.is_empty() { &m.uuid } else { &m.label };
        let place = match resolve(rule.and_then(|r| r.place.as_deref()).unwrap_or("Files/{label}")) {
            Ok(place) => place.replace("{label}", name),
            Err(e) => {
                problems.push(e);
                continue;
            }
        };
        let id = format!("browse:media/{}", if m.uuid.is_empty() { &m.device } else { &m.uuid });
        roots.push((place, browsed_root(&id, m.dir, BrowseKind::Removable(m.device))));
    }
    // (Each its own folder: the place's last part its title, the rest where it is.)
    for (place, mut root) in roots {
        let (tab, path) = content::place(&place);
        if path.is_empty() {
            problems.push(format!("{place}: a browsed dir's place needs a tab and its folder's title (Files/Home)"));
            continue;
        }
        let (within, name) = path.rsplit_once('/').unwrap_or(("", path));
        name.clone_into(&mut root.title);
        root.sort_key = clamshell_library::sort::key(name);
        games.push(root);
        let i = vec![games.len() - 1];
        let top = if within.is_empty() { i } else { folders.put(&mut games, tab, within, Path::new(""), &i).into_iter().collect() };
        add(&mut collections, tab, top);
    }
    let extend = |games: &mut Vec<Game>, more: Vec<Game>| {
        let first = games.len();
        games.extend(more);
        (first..games.len()).collect::<Vec<_>>()
    };
    // Steam's installed games and the desktop files' apps and games: where library.json's rules put them,
    // else Games for a game and Apps for an app (the games first, then the apps, each tab made where it is
    // first named).
    let mut placed: Vec<(String, Game)> = Vec::new();
    let mut rule_problems: Vec<String> = Vec::new();
    let mut place = |rules: &[content::Rule], id: &str, name: &str, categories: &[String], game: Game| {
        let rule = rules.iter().find(|r| r.matches(id, name, categories));
        if rule.is_some_and(|r| r.hide) {
            return;
        }
        let default = if game.tool { APPS } else { GAMES };
        let place = match rule.and_then(|r| r.place.as_deref()).map(resolve) {
            Some(Ok(place)) => place,
            Some(Err(e)) => {
                rule_problems.push(e);
                return;
            }
            None => default.to_owned(),
        };
        // (In Apps: an app, which Recent leaves out; anywhere else, played like a game.)
        let game = Game { tool: content::place(&place).0 == APPS, ..game };
        placed.push((place, game));
    };
    // (Each by its runner of that type: without one, none.)
    let steam_runner = runners::of_kind(&runners, runners::Kind::Steam).map(runners::Runner::of);
    let desktop_runner = runners::of_kind(&runners, runners::Kind::Desktop).map(runners::Runner::of);
    let steam_games = sources.steam.as_deref().filter(|_| steam_runner.is_some()).map(steam::games).unwrap_or_default();
    for mut game in steam_games {
        game.runner.clone_from(&steam_runner);
        let appid = match game.launch {
            Launch::Steam { appid } => appid.to_string(),
            _ => String::new(),
        };
        place(&config.steam, &appid, &game.title.clone(), &[], game);
    }
    let desktop_files = if desktop_runner.is_some() { crate::desktop::entries(&sources.data_dirs) } else { Vec::new() };
    for mut d in desktop_files {
        d.game.runner.clone_from(&desktop_runner);
        place(&config.desktop, &d.id, &d.name, &d.categories, d.game);
    }
    rule_problems.sort();
    rule_problems.dedup();
    problems.extend(rule_problems);
    placed.sort_by_key(|(_, g)| g.tool);
    for (place, game) in placed {
        let how = match game.launch {
            Launch::Steam { appid } => Some(How::Steam(appid)),
            _ if config.keys.steamgriddb.is_some() && !game.tool => Some(How::SteamGridDb),
            _ => None,
        };
        let i = extend(&mut games, vec![game]);
        hows.extend(how.into_iter().flat_map(|how| i.iter().map(move |&i| (i, how.clone()))));
        // In a folder of the tab, or at its top.
        let (tab, path) = content::place(&place);
        let i = match path {
            "" => i,
            path => folders.put(&mut games, tab, path, Path::new(""), &i).into_iter().collect(),
        };
        add(&mut collections, tab, i);
    }
    // A folder's entries (from its sources, and its folders) in list order.
    for &f in folders.made.values() {
        let mut items = games.get(f).map(|g| g.items.clone()).unwrap_or_default();
        items.sort_by(|&a, &b| list_order(&games, a, b));
        if let Some(g) = games.get_mut(f) {
            g.items = items;
        }
    }
    collections.retain(|c| !c.games.is_empty());
    // The tabs in the order library.json's places declare them (the rest after, as first named), with
    // their icons (Games and Apps with theirs unless it says).
    let tab = |c: &Collection| config.places.iter().position(|p| p.path.trim().trim_matches('/') == c.name);
    collections.sort_by_key(|c| tab(c).unwrap_or(usize::MAX));
    for c in &mut collections {
        c.icon = tab(c).and_then(|i| config.places.get(i)).and_then(|p| p.icon.clone()).or_else(|| match c.name.as_str() {
            GAMES => Some("gamepad".to_owned()),
            APPS => Some("apps".to_owned()),
            _ => None,
        });
    }
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
    let mut library = Library { collections, games, problems, away, runner_keymaps, ..Library::default() };
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
                r#"{{ {steam_group} "runners": [{{ "name": "Test SNES", "match": ["*.sfc"], "launch": ["retroarch", "{{path}}"], "scan": [{{ "place": "Games/SNES", "id": "snes", "dir": "{}" }}] }}, {{ "type": "steam", "id": "steam", "name": "Steam" }}] }}"#,
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
        assert_eq!(games_tab(&config(r#""steam": [{ "place": "Games/Steam" }],"#)), ["SNES", "Steam"], "Steam's games in their folder");
        // By name or app id, into folders, or hidden; the rest by the rule matching anything.
        let rules = r#""steam": [{ "name": "z*", "place": "Games/Steam/Text" }, { "id": "10", "hide": true }, { "place": "Games/Steam" }],"#;
        std::fs::write(steam.join("steamapps/appmanifest_30.acf"), manifest(30, "Celeste")).unwrap();
        std::fs::write(root.join("library.json"), config(rules)).unwrap();
        let lib = load(&Sources { config: Some(root.join("library.json")), steam: Some(steam.clone()), ..Sources::default() }).library;
        let titles = |ids: &[usize]| ids.iter().map(|&i| lib.games[i].title.clone()).collect::<Vec<_>>();
        let folder = |title: &str| lib.games.iter().find(|g| g.title == title).unwrap();
        assert_eq!(titles(&folder("Steam").items), ["Text", "Celeste"], "Arma hidden");
        assert_eq!(titles(&folder("Text").items), ["Zork"]);
        assert_eq!(folder("Zork").runner.as_ref().map(|r| &*r.key), Some("runner:steam"), "Steam's games: Steam's");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn desktop_rules_move_or_hide_entries() {
        let rule = |json: &str| serde_json::from_str::<content::Rule>(json).unwrap();
        let cats = |c: &[&str]| c.iter().map(|s| (*s).to_owned()).collect::<Vec<_>>();
        assert!(rule(r#"{ "name": "battle.NET", "place": "Apps" }"#).matches("net.lutris.battlenet-1", "Battle.net", &cats(&["Game"])), "any case");
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
        let config = r#"{ "runners": [{ "type": "desktop", "name": "Desktop" }], "desktop": [
            { "id": "lutris", "place": "Launchers" },
            { "id": "celeste", "place": "Games/Platformers" },
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
    fn runners_fill_folders_in_a_tab_each_with_its_own_launch() {
        let root = std::env::temp_dir().join(format!("clamshell-folders-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        for f in ["snes/Zelda.sfc", "snes/Mario.sfc", "snes/cover.png", "gba/Metroid.gba", "pc/Doom.sh"] {
            let p = root.join(f);
            std::fs::create_dir_all(p.parent().unwrap()).unwrap();
            std::fs::write(p, "").unwrap();
        }
        let source = |group: &str, id: &str, pattern: &str, emulator: &str| {
            let place = match group {
                "" => "Games".to_owned(),
                g if g.starts_with('@') => g.to_owned(),
                g => format!("Games/{g}"),
            };
            format!(
                r#"{{ "name": "Test {id}", "match": ["{pattern}"], "launch": ["{emulator}", "{{path}}"], "scan": [{{ "place": "{place}", "id": "{id}", "dir": "{}/{id}" }}] }}"#,
                root.display()
            )
        };
        let config = format!(
            r#"{{ "places": [{{ "id": "other", "path": "Other", "icon": "star" }}, {{ "id": "games", "path": "Games", "icon": "film" }},
                {{ "id": "gba", "path": "Games/Handhelds/GBA", "icon": "gba.png", "picture": "~/gba-large.png" }}],
              "runners": [ {}, {}, {} ] }}"#,
            source("SNES", "snes", "*.sfc", "snes9x"),
            source("@gba", "gba", "*.gba", "mgba"),
            source("", "pc", "*.sh", "sh")
        );
        std::fs::write(root.join("library.json"), config).unwrap();
        let lib = load(&Sources { config: Some(root.join("library.json")), ..Sources::default() }).library;
        // (The test's emulators aren't installed: only that is wrong.)
        assert!(lib.problems.iter().all(|p| p.ends_with("isn't installed")), "{:?}", lib.problems);
        assert_eq!(lib.collections.last().map(|c| c.name.as_str()), Some("Tools"));
        assert_eq!((lib.collections[0].name.as_str(), lib.collections[0].icon.as_deref()), ("Games", Some("film")), "listed: its icon; Other has nothing");
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
        assert_eq!(&*gba.id, "place:gba", "a declared place's id");
        assert_eq!(gba.icon.as_deref(), Some(root.join("gba.png").as_path()), "its icon, from library.json's dir");
        assert_eq!(handhelds.id.as_ref(), "folder:Games/Handhelds", "one not declared: its path");
        assert!(matches!(&lib.games[gba.items[0]].launch, Launch::Program { argv, .. } if argv[0] == "mgba"));
        assert_eq!(lib.groups_of(gba.items[0]), [tab.games[0], handhelds.items[0]]);
        let _ = std::fs::remove_dir_all(&root);
    }
}
