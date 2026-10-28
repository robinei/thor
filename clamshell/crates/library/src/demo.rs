//! A demo library for tests and snapshots, behind the `demo` feature. It has placeholder games, apps and
//! a series, none of which launch anything real.

use crate::{Collection, Details, Entry, Launch, Library, OwnTab, Playback};

/// Placeholder games.
pub fn games() -> Vec<Entry> {
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
        // Details as fetched metadata would give them.
        "zelda-lttp" => Entry {
            color,
            details: Details {
                developer: "Nintendo".into(),
                year: Some(1991),
                summary: "Link sets out across Hyrule and the Dark World to free the descendants of the Seven Sages from the wizard Agahnim.".into(),
                shot: None,
            },
            ..Entry::new(id, title)
        },
        _ => Entry { color, ..Entry::new(id, title) },
    })
    .collect()
}

/// The demo games in two collections, a few installed apps, a series (two seasons of episodes) and the
/// Tools.
pub fn library() -> Library {
    let mut games = games();
    let n = games.len();
    // A few installed apps, as desktop files give them.
    let apps = [("desktop:steam", "Steam", "steam"), ("desktop:foot", "Terminal", "foot"), ("desktop:firefox", "Firefox", "firefox")];
    games.extend(apps.iter().map(|(id, title, program)| Entry { launch: Launch::program(vec![(*program).into()]), app: true, ..Entry::new(id, title) }));
    let series = games.len();
    games.push(Entry { color: 0x305a78, ..Entry::new("tv:Cosmos", "Cosmos") });
    let seasons: [(&str, &[&str]); 2] = [
        ("Season 1", &["1. The Shores of the Cosmic Ocean", "2. One Voice in the Cosmic Fugue", "10. The Edge of Forever"]),
        ("Season 2", &["1. Standing Up in the Milky Way"]),
    ];
    for (season, episodes) in seasons {
        let s = games.len();
        let id = format!("tv:Cosmos/{season}");
        games.push(Entry { parent: Some(series), color: 0x305a78, ..Entry::new(&id, season) });
        for episode in episodes {
            let e = games.len();
            let file = format!("{id}/{episode}.mkv");
            let path = format!("/videos/{}", file.trim_start_matches("tv:"));
            let argv = vec!["mpv".into(), path.clone()];
            // The second episode is left halfway through.
            let position = episode.starts_with("2.").then_some(1500);
            let launch = Launch::program(argv);
            games.push(Entry {
                parent: Some(s),
                color: 0x305a78,
                launch,
                file: Some(path.into()),
                playback: Playback { position, duration: Some(2640), at: 0 },
                ..Entry::new(&file, episode)
            });
            if let Some(season) = games.get_mut(s) {
                season.items.push(e);
            }
        }
        if let Some(g) = games.get_mut(series) {
            g.items.push(s);
        }
    }
    // ClamShell's own screens, as a library.json with a Tools place would have them.
    let tools = games.len();
    for (builtin, title, summary) in crate::BUILTINS {
        games.push(Entry {
            launch: Launch::Builtin(builtin),
            app: true,
            details: Details { summary: summary.into(), ..Details::default() },
            ..Entry::new(builtin.id(), title)
        });
    }
    // A Files tab: Home, listed, with two folders and two files.
    let files = games.len();
    let browsed = |name: &str, folder: bool, kind: crate::BrowseKind| Entry {
        file: Some(format!("/home/thor{}", if name.is_empty() { String::new() } else { format!("/{name}") }).into()),
        browsed: Some(crate::Browsed { folder, listed: name.is_empty(), kind, size: (!folder).then_some(48_000), ..crate::Browsed::default() }),
        ..Entry::new(
            &format!("browse:Files/Home{}", if name.is_empty() { String::new() } else { format!("/{name}") }),
            if name.is_empty() { "Home" } else { name },
        )
    };
    games.push(Entry { items: (files + 1..files + 5).collect(), ..browsed("", true, crate::BrowseKind::Root) });
    for (name, folder) in [("Pictures", true), ("Roms", true), ("notes.txt", false), ("snes.png", false)] {
        games.push(Entry { parent: Some(files), ..browsed(name, folder, crate::BrowseKind::Plain) });
    }
    let collections = vec![
        Collection::with_icon("Games", "gamepad", (0..n).collect()),
        Collection::new("More", ["ff6", "metroid-zm"].iter().filter_map(|id| games.iter().position(|g| &*g.id == *id)).collect()),
        Collection::with_icon("TV", "tv", vec![series]),
        Collection::with_icon("Apps", "apps", (n..n + apps.len()).collect()),
        Collection::with_icon("Tools", "tools", vec![tools, tools + 1]),
        Collection::with_icon("Files", "folder", vec![files]),
    ];
    let own = |name: &str, icon: &str| Some(OwnTab { name: name.into(), icon: Some(icon.into()) });
    Library {
        collections,
        entries: games,
        order: vec!["Recent".into(), "Favourites".into()],
        recent: own("Recent", "clock"),
        favourites: own("Favourites", "star"),
        config: Some(std::sync::Arc::new(config())),
        ..Library::default()
    }
}

/// A library.json the demo could have been read from: its tabs, a folder per system, and their sources.
pub fn config() -> crate::config::Config {
    use crate::config::{BuiltinPlace, Config, OwnTab, Place, Rule, Runner, Scan, Source};
    let place = |id: &str, name: &str, within: Option<&str>, icon: Option<&str>| Place {
        id: id.into(),
        name: name.into(),
        within: within.map(Into::into),
        icon: icon.map(Into::into),
        ..Place::default()
    };
    let own = |kind, id: &str, name: &str, icon: &str| Place { kind: Some(kind), ..place(id, name, None, Some(icon)) };
    Config {
        places: vec![
            own(OwnTab::Recent, "recent", "Recent", "clock"),
            own(OwnTab::Favourites, "favourites", "Favourites", "star"),
            place("games", "Games", None, Some("gamepad")),
            place("more", "More", None, None),
            place("tv", "TV", None, Some("tv")),
            place("apps", "Apps", None, Some("apps")),
            own(OwnTab::Tools, "tools", "Tools", "tools"),
            place("snes", "SNES", Some("games"), None),
            place("gba", "Game Boy Advance", Some("games"), None),
            place("gc", "Game Cube", Some("games"), None),
        ],
        sources: vec![
            Source::Dir(Scan { id: "snes".into(), dir: "~/Roms/snes".into(), place: "snes".into(), runner: Some("snes".into()), ..Scan::default() }),
            Source::Dir(Scan { id: "gba".into(), dir: "~/Roms/gba".into(), place: "gba".into(), runner: Some("gba".into()), ..Scan::default() }),
            Source::Desktop(Rule { place: Some("apps".into()), ..Rule::default() }),
            Source::Builtin(BuiltinPlace { id: "device".into(), place: "tools".into() }),
        ],
        runners: ["snes", "gba"]
            .iter()
            .zip([["*.sfc", "*.smc"].as_slice(), ["*.gba"].as_slice()])
            .map(|(id, patterns)| Runner {
                id: (*id).into(),
                name: id.to_uppercase(),
                patterns: patterns.iter().map(|p| (*p).into()).collect(),
                launch: vec!["retroarch".into(), "-f".into(), "-L".into(), format!("/usr/lib/libretro/{id}_libretro.so"), "{path}".into()],
                ..Runner::default()
            })
            .collect(),
        ..Config::default()
    }
}
