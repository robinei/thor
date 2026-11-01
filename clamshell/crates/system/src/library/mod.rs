//! Reads the library from disk: library.json's content dirs (see [`content`]), installed Steam games, and
//! the games and apps from desktop files, along with the stored play time and favourites (see
//! [`crate::store`]). The model is in clamshell-library.

mod build;
pub(crate) mod content;
mod desktop;
mod known;
pub mod meta;
pub(crate) mod runners;
mod steam;

use build::*;
use known::*;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use clamshell_library::{BrowseKind, Browsed, Collection, Details, Entry, Launch, Library, Meta, Tool, ids};

use meta::{How, Keys, RETRY_MISS, Wanted};
use runners::Opens as _;

use crate::paths::Paths;

/// Where to read from; `None` skips a source.
#[derive(Debug, Clone, Default)]
pub struct Sources {
    /// library.json.
    pub(crate) config: Option<PathBuf>,
    /// The home dir, for `~/` in library.json.
    pub(crate) home: Option<PathBuf>,
    /// Steam's data dir.
    steam: Option<PathBuf>,
    /// ClamShell's state dir, for play time and favourites.
    state: Option<PathBuf>,
    /// The art cache, for what was fetched about entries and videos' lengths.
    art: Option<PathBuf>,
    /// The XDG data dirs, whose `applications` dirs hold the installed apps' desktop files (see
    /// [`crate::library::desktop`]).
    data_dirs: Vec<PathBuf>,
    /// Where mpv keeps videos' places.
    watch_later: Vec<PathBuf>,
}

impl Sources {
    /// ClamShell's paths, and Steam's data dir under `home` unless `steam` is off.
    pub fn new(paths: &Paths, home: Option<&std::path::Path>, steam: bool) -> Sources {
        Sources {
            config: paths.library.clone(),
            home: home.map(std::path::Path::to_path_buf),
            steam: home.filter(|_| steam).and_then(steam::default_root),
            state: paths.state_dir.clone(),
            art: paths.art.clone(),
            data_dirs: crate::os::xdg::data_dirs(),
            watch_later: paths.watch_later.clone(),
        }
    }
}

/// The programs ClamShell runs itself, and what doesn't work without each. Programs that runners use are
/// checked with the runners.
const NEEDED: [(&str, &str); 7] = [
    ("systemd-run", "nothing can be launched"),
    ("ratarmount", "archives can't be browsed"),
    ("fusermount3", "archives can't be browsed"),
    ("bsdtar", "archives can't be extracted"),
    ("wpctl", "the volume can't be set"),
    ("pw-dump", "no volume in the bar or the sound tab"),
    ("curl", "no covers or details fetched"),
];

/// The Tools tab's entries: id, title, the tool and what it does.
const TOOLS: [(&str, &str, Tool, &str); 2] = [
    (
        "tool:rescan",
        "Rescan library",
        Tool::Rescan,
        "Reads library.json, its folders, Steam and the desktop files again, then fetches the covers and details still missing.",
    ),
    ("tool:refetch", "Fetch missing art again", Tool::Refetch, "Tries again for the entries whose covers and details weren't found, then rescans."),
];

/// A browsed dir's own folder.
fn browsed_root(id: &str, dir: PathBuf, kind: BrowseKind) -> Entry {
    let glyph = match kind {
        BrowseKind::Trash => "trash",
        BrowseKind::Removable(_) => "card",
        _ => "folder",
    };
    Entry { file: Some(dir), browsed: Some(Browsed { folder: true, kind, ..Browsed::default() }), glyph: Some(glyph.into()), ..Entry::new(id, "") }
}

/// The library, and what to fetch for it.
#[derive(Debug, Default)]
pub struct Loaded {
    pub library: Library,
    /// The entries to fetch metadata for: never fetched, fetched from a different source, missed long
    /// ago, or with images gone from the cache.
    pub wanted: Vec<Wanted>,
    pub keys: Keys,
}

/// Writes library.json as `config` (see [`content::write_config`]). Errs if there is no library.json
/// to write (no config dir) or writing failed.
pub fn write(sources: &Sources, config: &clamshell_library::config::Config) -> std::io::Result<()> {
    let file = sources.config.as_deref().ok_or_else(|| std::io::Error::other("no config dir"))?;
    content::write_config(file, config)
}

/// Reads library.json's sources into its places (see [`content`]), with each entry's runner and where
/// its metadata comes from. Adds ClamShell's own tabs as declared and puts the tabs in order. Then adds
/// each entry's plays, whether it is a favourite, and what was fetched about it.
pub fn load(sources: &Sources) -> Loaded {
    let (config, config_problem) = sources.config.as_deref().map(content::read_config).unwrap_or_default();
    let config_problem_none = config_problem.is_none();
    let (places, place_problems) = content::placed(&config.places);
    let (runners, runner_problems) = runners::in_force(&config.runners);
    let config_dir = sources.config.as_deref().and_then(Path::parent);
    let mut b = Builder {
        config: &config,
        runners: &runners,
        sources,
        games: Vec::new(),
        collections: Vec::new(),
        places: &places,
        folders: Folders { made: HashMap::new(), places: &places, home: sources.home.as_deref(), config_dir },
        hows: Vec::new(),
        problems: config_problem.into_iter().chain(place_problems).chain(runner_problems).collect(),
        away: Vec::new(),
        mounts: if config.sources.iter().any(|s| matches!(s, content::Source::Removable(_) | content::Source::Dir(content::Scan { on: Some(_), .. }))) {
            crate::files::mounts::media_mounts()
        } else {
            Vec::new()
        },
    };
    b.check_tabs();
    for source in &config.sources {
        match source {
            content::Source::Dir(scan) => b.scan(scan),
            content::Source::Browse(root) => b.browse(root),
            content::Source::Builtin(place) => b.builtin(place),
            // These are handled one type at a time below, with each type's sources in turn.
            content::Source::Desktop(_) | content::Source::Steam(_) | content::Source::Removable(_) => {}
        }
    }
    b.removable();
    b.installed();
    b.unplaced_builtins();
    b.needed();
    b.bios();
    b.order();
    b.mark_recent();
    b.tools();
    let runner_keymaps = runners.iter().filter_map(|r| Some((clamshell_library::runner_key(&r.id), r.keymap.clone()?))).collect();
    let order: Vec<String> = places.iter().map(|p| p.path.trim().trim_matches('/')).filter(|p| !p.contains('/')).map(str::to_owned).collect();
    let (recent, favourites) = (b.own(content::OwnTab::Recent), b.own(content::OwnTab::Favourites));
    let Builder { games, collections, hows, problems, away, .. } = b;
    let parsed = config_problem_none.then(|| Arc::new(config.clone()));
    let mut library = Library::new(collections, games);
    (library.problems, library.away, library.runner_keymaps, library.order) = (problems, away, runner_keymaps, order);
    (library.recent, library.favourites, library.config) = (recent, favourites, parsed);
    let wanted = known(&mut library, hows, sources);
    library.reindex();
    library.rebuild_derived();
    Loaded { library, wanted, keys: config.keys }
}

/// The key of the runner that opens Steam game `appid` ([`clamshell_library::runner_key`]), as the library
/// gives it, for `clamshell --launch-env`: read from library.json's sources and runners and the game's
/// manifest (for a source matching by name), without reading the rest of the library. None if no Steam
/// source takes the game (or one hides it), or the source's runner isn't declared: it has then no
/// runner level, as it isn't in the library. An error if library.json doesn't read.
pub fn steam_runner(sources: &Sources, appid: u32) -> Result<Option<Arc<str>>, String> {
    let (config, problem) = sources.config.as_deref().map(content::read_config).unwrap_or_default();
    if let Some(problem) = problem {
        return Err(problem);
    }
    let rules: Vec<&content::Rule> = config
        .sources
        .iter()
        .filter_map(|s| match s {
            content::Source::Steam(r) => Some(r),
            _ => None,
        })
        .collect();
    if rules.is_empty() {
        return Ok(None);
    }
    let id = format!("{}{appid}", ids::STEAM);
    let title = sources.steam.as_deref().map(steam::games).unwrap_or_default().into_iter().find(|g| *g.id == *id).map(|g| g.title).unwrap_or_default();
    let Some(rule) = first_match(&rules, &appid.to_string(), &title, &[]).filter(|r| r.place.is_some() && !r.hide) else {
        return Ok(None);
    };
    let (runners, _) = runners::in_force(&config.runners);
    Ok(source_runner(rule, &runners, runners::Kind::Steam).map(|r| clamshell_library::runner_key(&r.id)))
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::indexing_slicing)]
mod tests {
    use super::*;

    #[test]
    fn folders_come_first_and_steam_can_have_its_own() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        let steam = root.join("steam");
        std::fs::create_dir_all(steam.join("steamapps")).unwrap();
        std::fs::write(steam.join("steamapps/libraryfolders.vdf"), format!("\"libraryfolders\" {{ \"0\" {{ \"path\" \"{}\" }} }}", steam.display())).unwrap();
        let manifest = |id: u32, name: &str| format!("\"AppState\" {{ \"appid\" \"{id}\" \"name\" \"{name}\" \"StateFlags\" \"4\" }}");
        std::fs::write(steam.join("steamapps/appmanifest_10.acf"), manifest(10, "Arma")).unwrap();
        std::fs::write(steam.join("steamapps/appmanifest_20.acf"), manifest(20, "Zork")).unwrap();
        std::fs::create_dir_all(root.join("snes")).unwrap();
        std::fs::write(root.join("snes/Mario.sfc"), "").unwrap();
        // The Steam sources go in `steam`.
        let config = |steam: &str| {
            format!(
                r#"{{ "places": [{{ "id": "games", "name": "Games" }}, {{ "id": "snes", "name": "SNES", "in": "games" }}, {{ "id": "steam", "name": "Steam", "in": "games" }},
                                {{ "id": "text", "name": "Text", "in": "steam" }}],
                    "runners": [{{ "name": "Test SNES", "match": ["*.sfc"], "launch": ["retroarch", "{{path}}"] }}, {{ "type": "steam", "id": "steam", "name": "Steam" }}],
                    "sources": [{{ "type": "dir", "place": "snes", "id": "snes", "dir": "{}" , "match": ["*.sfc"] }}, {steam}] }}"#,
                root.join("snes").display()
            )
        };
        let games_tab = |json: &str| {
            std::fs::write(root.join("library.json"), json).unwrap();
            let lib = load(&Sources { config: Some(root.join("library.json")), steam: Some(steam.clone()), ..Sources::default() }).library;
            let tab = lib.collections.iter().find(|c| c.name == "Games").unwrap();
            tab.entries.iter().map(|&i| lib.entries()[i].title.clone()).collect::<Vec<_>>()
        };
        assert_eq!(games_tab(&config(r#"{ "type": "steam", "place": "games" }"#)), ["SNES", "Arma", "Zork"], "the folder first, before Arma");
        assert_eq!(games_tab(&config(r#"{ "type": "steam", "place": "steam" }"#)), ["SNES", "Steam"], "Steam's games in their folder");
        // Match by name or app id, into folders or hidden, and the rest by a source that matches anything.
        // Each is taken by the first source matching it: Zork by z*, not by the last one.
        let rules = r#"{ "type": "steam", "name": "z*", "place": "text" }, { "type": "steam", "id": "10", "hide": true },
                       { "type": "steam", "place": "steam" }"#;
        std::fs::write(steam.join("steamapps/appmanifest_30.acf"), manifest(30, "Celeste")).unwrap();
        std::fs::write(root.join("library.json"), config(rules)).unwrap();
        let lib = load(&Sources { config: Some(root.join("library.json")), steam: Some(steam.clone()), ..Sources::default() }).library;
        let titles = |ids: &[usize]| ids.iter().map(|&i| lib.entries()[i].title.clone()).collect::<Vec<_>>();
        let folder = |title: &str| lib.entries().iter().find(|g| g.title == title).unwrap();
        assert_eq!(titles(&folder("Steam").items), ["Text", "Celeste"], "Arma hidden");
        assert_eq!(titles(&folder("Text").items), ["Zork"]);
        assert_eq!(folder("Zork").runner.as_ref().map(|r| &*r.key), Some("runner:steam"), "Steam's games: Steam's");
    }

    /// `clamshell --launch-env` finds a Steam game's runner as the library does: by the first source
    /// matching its id or name, that source's runner, and none for a game hidden or taken by no source.
    #[test]
    fn a_steam_games_runner_is_found_without_reading_the_library() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        let steam = root.join("steam");
        std::fs::create_dir_all(steam.join("steamapps")).unwrap();
        std::fs::write(steam.join("steamapps/libraryfolders.vdf"), format!("\"libraryfolders\" {{ \"0\" {{ \"path\" \"{}\" }} }}", steam.display())).unwrap();
        for (id, name) in [(10, "Arma"), (20, "Zork"), (30, "Celeste")] {
            std::fs::write(
                steam.join(format!("steamapps/appmanifest_{id}.acf")),
                format!("\"AppState\" {{ \"appid\" \"{id}\" \"name\" \"{name}\" \"StateFlags\" \"4\" }}"),
            )
            .unwrap();
        }
        let config = r#"{ "places": [{ "id": "games", "name": "Games" }],
          "runners": [{ "type": "steam", "id": "steam", "name": "Steam" }, { "type": "steam", "id": "steam-text", "name": "Text adventures" }],
          "sources": [{ "type": "steam", "name": "z*", "place": "games", "runner": "steam-text" }, { "type": "steam", "id": "10", "hide": true },
                      { "type": "steam", "id": "30", "place": "games" }] }"#;
        std::fs::write(root.join("library.json"), config).unwrap();
        let sources = Sources { config: Some(root.join("library.json")), steam: Some(steam.clone()), ..Sources::default() };
        let lib = load(&sources).library;
        for appid in [10, 20, 30, 40] {
            let in_library = lib.entry(&format!("steam:{appid}")).and_then(|g| g.runner.as_ref()).map(|r| r.key.clone());
            assert_eq!(steam_runner(&sources, appid).unwrap(), in_library, "{appid}");
        }
        assert_eq!(steam_runner(&sources, 20).unwrap().as_deref(), Some("runner:steam-text"), "by its name");
        std::fs::write(root.join("library.json"), "{").unwrap();
        assert!(steam_runner(&sources, 20).is_err(), "library.json unread");
    }

    #[test]
    fn sources_name_their_runner_and_metadata_and_wait_for_their_card() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        let apps = root.join("data/applications");
        std::fs::create_dir_all(&apps).unwrap();
        std::fs::write(apps.join("celeste.desktop"), "[Desktop Entry]\nType=Application\nName=Celeste\nExec=celeste\nCategories=Game;\n").unwrap();
        std::fs::write(apps.join("foot.desktop"), "[Desktop Entry]\nType=Application\nName=Foot\nExec=foot\nCategories=System;\n").unwrap();
        let steam = root.join("steam");
        std::fs::create_dir_all(steam.join("steamapps")).unwrap();
        std::fs::write(steam.join("steamapps/libraryfolders.vdf"), format!("\"libraryfolders\" {{ \"0\" {{ \"path\" \"{}\" }} }}", steam.display())).unwrap();
        std::fs::write(steam.join("steamapps/appmanifest_10.acf"), "\"AppState\" { \"appid\" \"10\" \"name\" \"Arma\" \"StateFlags\" \"4\" }").unwrap();
        let config = r#"{ "places": [{ "id": "games", "name": "Games" }, { "id": "apps", "name": "Apps" }],
          "runners": [{ "type": "desktop", "id": "apps", "name": "Apps" }, { "type": "desktop", "id": "desktop-games", "name": "Desktop games" },
            { "type": "steam", "name": "Steam" }],
          "sources": [
            { "type": "desktop", "categories": "Game", "place": "games", "runner": "desktop-games", "metadata": "steamgriddb" },
            { "type": "desktop", "place": "apps" },
            { "type": "steam", "place": "games", "metadata": "steam" },
            { "type": "dir", "id": "sd", "dir": "roms", "place": "games", "match": ["*"], "on": { "label": "NO SUCH CARD" } }
          ] }"#;
        std::fs::write(root.join("library.json"), config).unwrap();
        // The card's favourites are kept while it is out; those of entries gone aren't.
        std::fs::create_dir_all(root.join("state")).unwrap();
        std::fs::write(root.join("state/favourites.json"), r#"["sd:zelda", "gone:x", "sd:mario"]"#).unwrap();
        let loaded = load(&Sources {
            config: Some(root.join("library.json")),
            data_dirs: vec![root.join("data")],
            steam: Some(steam.clone()),
            state: Some(root.join("state")),
            ..Sources::default()
        });
        let lib = &loaded.library;
        let runner = |title: &str| lib.entries().iter().find(|g| g.title == title).and_then(|g| g.runner.as_ref()).map(|r| r.key.to_string());
        assert_eq!(runner("Celeste").as_deref(), Some("runner:desktop-games"), "the runner it names");
        assert_eq!(runner("Foot").as_deref(), Some("runner:apps"), "else the first of its type");
        assert!(lib.problems.iter().any(|p| p.ends_with("steamgriddb needs keys.steamgriddb")), "{:?}", lib.problems);
        assert!(loaded.wanted.iter().any(|w| w.how == How::Steam(10)), "Steam's store, as it says");
        assert!(!loaded.wanted.iter().any(|w| w.how == How::SteamGridDb), "not without the key");
        assert!(lib.problems.iter().any(|p| p == "sd: its card or stick isn't in") && lib.away.contains(&"sd:".to_owned()), "away, not gone");
        assert_eq!(lib.away_favourites, vec!["sd:mario".to_owned(), "sd:zelda".to_owned()]);
    }

    #[test]
    fn desktop_rules_move_or_hide_entries() {
        let rule = |json: &str| serde_json::from_str::<content::Rule>(json).unwrap();
        let cats = |c: &[&str]| c.iter().map(|s| (*s).to_owned()).collect::<Vec<_>>();
        assert!(
            content::rule_matches(&rule(r#"{ "name": "battle.NET", "place": "Apps" }"#), "net.lutris.battlenet-1", "Battle.net", &cats(&["Game"])),
            "any case"
        );
        assert!(content::rule_matches(&rule(r#"{ "id": "*lutris*" }"#), "net.lutris.Lutris", "Lutris", &[]));
        assert!(content::rule_matches(&rule(r#"{ "categories": "Emu*" }"#), "x", "RetroArch", &cats(&["Game", "Emulator"])));
        assert!(!content::rule_matches(&rule(r#"{ "id": "*lutris*", "name": "Steam" }"#), "net.lutris.Lutris", "Lutris", &[]), "all of them");
        assert!(content::rule_matches(&rule("{}"), "any", "Any", &[]), "nothing to match: every entry");
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
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
        let config = r#"{ "places": [{ "id": "apps", "name": "Apps", "recent": false }, { "id": "launchers", "name": "Launchers" },
            { "id": "games", "name": "Games" }, { "id": "platformers", "name": "Platformers", "in": "games" }],
          "runners": [{ "type": "desktop", "name": "Desktop" }], "sources": [
            { "type": "desktop", "id": "lutris", "place": "launchers" },
            { "type": "desktop", "id": "celeste", "place": "platformers" },
            { "type": "desktop", "name": "uninstall*", "hide": true },
            { "type": "desktop", "place": "apps" }
        ] }"#;
        std::fs::write(root.join("library.json"), config).unwrap();
        let lib = load(&Sources { config: Some(root.join("library.json")), data_dirs: vec![root.join("data")], ..Sources::default() }).library;
        let tab =
            |name: &str| lib.collections.iter().find(|c| c.name == name).map(|c| c.entries.iter().map(|&i| lib.entries()[i].title.clone()).collect::<Vec<_>>());
        assert_eq!(tab("Games"), Some(vec!["Platformers".to_owned()]), "in its folder");
        let folder = lib.entries().iter().find(|g| g.title == "Platformers").unwrap();
        assert_eq!(folder.items.iter().map(|&i| lib.entries()[i].title.as_str()).collect::<Vec<_>>(), ["Celeste"]);
        assert_eq!(tab("Launchers"), Some(vec!["Lutris".to_owned()]));
        assert_eq!(tab("Apps"), Some(vec!["Foot".to_owned()]), "Uninstall Foo hidden; the rest taken by the last");
        let entry = |title: &str| lib.entries().iter().find(|g| g.title == title).unwrap();
        assert!(entry("Foot").app && !entry("Celeste").app, "Apps says recent: false");
    }

    #[test]
    fn runners_fill_folders_in_a_tab_each_with_its_own_launch() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        for f in ["snes/Zelda.sfc", "snes/Mario.sfc", "snes/cover.png", "gba/Metroid.gba", "pc/Doom.sh"] {
            let p = root.join(f);
            std::fs::create_dir_all(p.parent().unwrap()).unwrap();
            std::fs::write(p, "").unwrap();
        }
        let source = |place: &str, id: &str, pattern: &str, emulator: &str| {
            (
                format!(r#"{{ "name": "Test {id}", "match": ["{pattern}"], "launch": ["{emulator}", "{{path}}"] }}"#),
                format!(r#"{{ "type": "dir", "place": "{place}", "id": "{id}", "dir": "{}/{id}", "match": ["{pattern}"] }}"#, root.display()),
            )
        };
        let (snes, gba, pc) = (source("snes", "snes", "*.sfc", "snes9x"), source("gba", "gba", "*.gba", "mgba"), source("games", "pc", "*.sh", "sh"));
        // The PC scan names its runner; the other files open by their names.
        let pc_scan = pc.1.replace(r#""match""#, r#""runner": "test-pc", "match""#);
        let config = format!(
            r#"{{ "places": [{{ "id": "other", "name": "Other", "icon": "star" }}, {{ "id": "games", "name": "Games", "icon": "film" }},
                {{ "id": "snes", "name": "SNES", "in": "games" }}, {{ "id": "handhelds", "name": "Handhelds", "in": "games" }},
                {{ "id": "gba", "name": "GBA", "in": "handhelds", "icon": "gba.png", "picture": "~/gba-large.png" }},
                {{ "id": "tools", "name": "Mine", "type": "tools", "icon": "tools" }}],
              "runners": [ {}, {}, {} ], "sources": [ {}, {}, {} ] }}"#,
            snes.0, gba.0, pc.0, snes.1, gba.1, pc_scan
        );
        std::fs::write(root.join("library.json"), config).unwrap();
        let lib = load(&Sources { config: Some(root.join("library.json")), ..Sources::default() }).library;
        // The test's emulators aren't installed, and that should be the only problem, besides the tools
        // ClamShell runs that this machine may lack.
        let tool_missing = |p: &str| NEEDED.iter().any(|(tool, _)| p.starts_with(&format!("{tool} isn't installed")));
        assert!(lib.problems.iter().all(|p| p.ends_with("isn't installed") || tool_missing(p)), "{:?}", lib.problems);
        assert_eq!(lib.collections.last().map(|c| c.name.as_str()), Some("Mine"), "ClamShell's tools, named and placed as declared");
        assert_eq!((lib.collections[0].name.as_str(), lib.collections[0].icon.as_deref()), ("Games", Some("film")), "listed: its icon; Other has nothing");
        let games = |ids: &[usize]| ids.iter().map(|&i| lib.entries()[i].title.clone()).collect::<Vec<_>>();
        let tab = lib.collections.iter().find(|c| c.name == "Games").unwrap();
        assert_eq!(games(&tab.entries), ["Handhelds", "SNES", "Doom"], "folders first");
        let snes = &lib.entries()[tab.entries[1]];
        assert_eq!(games(&snes.items), ["Mario", "Zelda"]);
        assert_eq!(snes.box_front, Some(root.join("snes/cover.png")), "its source's dir's art");
        let zelda = &lib.entries()[snes.items[1]];
        assert!(matches!(&zelda.launch, Launch::Program { argv, .. } if argv[0] == "snes9x"));
        assert_eq!(&*zelda.id, "snes:Zelda.sfc", "ids as without the folder");
        let handhelds = &lib.entries()[tab.entries[0]];
        let gba = &lib.entries()[handhelds.items[0]];
        assert_eq!((gba.title.as_str(), games(&gba.items)), ("GBA", vec!["Metroid".to_owned()]));
        assert_eq!(&*gba.id, "place:gba", "a declared place's id");
        assert_eq!(gba.icon.as_deref(), Some(root.join("gba.png").as_path()), "its icon, from library.json's dir");
        assert_eq!(handhelds.id.as_ref(), "place:handhelds");
        assert!(matches!(&lib.entries()[gba.items[0]].launch, Launch::Program { argv, .. } if argv[0] == "mgba"));
        assert_eq!(lib.groups_of(gba.items[0]), [tab.entries[0], handhelds.items[0]]);
    }

    /// A scan's files are its runner's, and `also`'s; a builtin source places a built-in in a tab that
    /// ClamShell's tools join, and one not placed is there all the same; a browsed dir is a folder in the
    /// place `in`, titled `name`; a runner's BIOS files not where its emulator looks are problems.
    #[test]
    fn also_builtins_browsed_folders_and_bios() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        for f in ["snes/Zelda.sfc", "snes/Mario.zip", "snes/notes.txt", "home/a.txt", "bios/scph5500.bin"] {
            let p = root.join(f);
            std::fs::create_dir_all(p.parent().unwrap()).unwrap();
            std::fs::write(p, "").unwrap();
        }
        let r = root.display();
        let config = format!(
            r#"{{ "places": [{{ "id": "games", "name": "Games" }}, {{ "id": "psx", "name": "PlayStation", "in": "games" }}, {{ "id": "files", "name": "Files" }},
                          {{ "id": "tools", "name": "Tools", "type": "tools" }}],
              "runners": [{{ "id": "snes", "name": "SNES", "match": ["*.sfc"], "launch": ["retroarch", "{{path}}"] }},
                          {{ "id": "psx", "name": "PlayStation", "match": ["*.chd"], "launch": ["retroarch", "{{path}}"],
                             "bios": {{ "dir": "{r}/bios", "files": ["scph5500.bin", "scph5501.bin"] }} }}],
              "sources": [{{ "type": "dir", "id": "snes", "dir": "{r}/snes", "place": "games", "runner": "snes", "also": ["*.zip"] }},
                          {{ "type": "dir", "id": "psx", "dir": "{r}/psx", "place": "psx", "runner": "psx" }},
                          {{ "type": "browse", "dir": "{r}/home", "in": "files", "name": "Home" }},
                          {{ "type": "builtin", "id": "device", "place": "tools" }}] }}"#
        );
        std::fs::write(root.join("library.json"), config).unwrap();
        let lib = load(&Sources { config: Some(root.join("library.json")), ..Sources::default() }).library;
        let tab = |name: &str| {
            let c = lib.collections.iter().find(|c| c.name == name).unwrap();
            c.entries.iter().map(|&i| lib.entries()[i].title.clone()).collect::<Vec<_>>()
        };
        assert_eq!(tab("Games"), ["PlayStation", "Mario", "Zelda"], "the runner's *.sfc and also *.zip, not the notes");
        let psx = lib.entry("place:psx").unwrap();
        assert!(psx.is_empty_folder(), "there, with nothing in it");
        let said = &psx.details.summary;
        assert!(said.starts_with("Nothing here yet: its files go in") && said.contains("Needs scph5501.bin in"), "{said}");
        assert_eq!(tab("Files"), ["Home"]);
        assert_eq!(tab("Tools"), ["Device", "Rescan library", "Fetch missing art again"], "one Tools tab");
        assert_eq!(lib.collections.iter().filter(|c| c.name == "Tools").count(), 1);
        assert!(lib.entry("builtin:library").is_some(), "not placed, but there");
        assert!(lib.problems.iter().any(|p| p.starts_with("PlayStation: its BIOS scph5501.bin isn't in")), "{:?}", lib.problems);
        assert!(!lib.problems.iter().any(|p| p.contains("scph5500")), "{:?}", lib.problems);
    }
}
