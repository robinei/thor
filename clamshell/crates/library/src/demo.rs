//! A demo library for tests and snapshots, behind the `demo` feature. It has placeholder games, apps and
//! a series, none of which launch anything real.

use crate::{Collection, Details, Entry, Launch, Library, OwnTab, Playback, Tool};

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
    // ClamShell's own tools, as a library.json with a Tools place would have them.
    let tools = games.len();
    let summary = "Calibrates the controller's sticks and gyro and the top screen's refresh rate; sets the password and remote login.";
    games.push(Entry {
        launch: Launch::Tool(Tool::Device),
        app: true,
        details: Details { summary: summary.into(), ..Details::default() },
        ..Entry::new("tool:device", "Device")
    });
    let collections = vec![
        Collection::with_icon("Games", "gamepad", (0..n).collect()),
        Collection::new("More", ["ff6", "metroid-zm"].iter().filter_map(|id| games.iter().position(|g| &*g.id == *id)).collect()),
        Collection::with_icon("TV", "tv", vec![series]),
        Collection::with_icon("Apps", "apps", (n..n + apps.len()).collect()),
        Collection::with_icon("Tools", "tools", vec![tools]),
    ];
    let own = |name: &str, icon: &str| Some(OwnTab { name: name.into(), icon: Some(icon.into()) });
    Library {
        collections,
        entries: games,
        order: vec!["Recent".into(), "Favourites".into()],
        recent: own("Recent", "clock"),
        favourites: own("Favourites", "star"),
        ..Library::default()
    }
}
