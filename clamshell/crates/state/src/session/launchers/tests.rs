use super::*;

fn shortcut(id: &str, title: &str, argv: &[&str]) -> Entry {
    Entry { launch: Launch::program(argv.iter().map(|a| (*a).to_owned()).collect()), ..Entry::new(id, title) }
}

fn library(entries: Vec<Entry>) -> Library {
    Library { entries, ..Library::default() }
}

/// A process's command line as `/proc/<pid>/cmdline` has it.
fn cmdline(args: &[&str]) -> Vec<u8> {
    args.iter().flat_map(|a| a.bytes().chain([0])).collect()
}

#[test]
fn a_process_is_marked_by_its_launcher() {
    let none: [&[u8]; 0] = [];
    assert_eq!(of_process(b"HOME=/h\0SteamAppId=620\0", none), Some("steam_app_620".into()));
    assert_eq!(of_process(b"SteamAppId=0\0", none), None, "Steam's own client");
    let heroic = b"HEROIC_APP_NAME=patterngame\0HEROIC_APP_RUNNER=sideload\0HEROIC_APP_SOURCE=sideload\0";
    assert_eq!(of_process(heroic, none), Some("heroic:sideload:patterngame".into()));
    let game = cmdline(&["./pattern-game"]);
    let wrapper = cmdline(&["python3", "/usr/share/lutris/bin/lutris-wrapper", "Pattern Game", "0", "0", "./pattern-game"]);
    let lutris = cmdline(&["python3", "/usr/bin/lutris"]);
    assert_eq!(of_process(b"LUTRIS_GAME_UUID=x\0", [&game[..], &wrapper, &lutris]), Some("lutris:Pattern Game".into()), "by its wrapper, an ancestor");
    let unlike = cmdline(&["python3", "/usr/share/lutris/bin/lutris-wrapper", "Pattern Game", "./pattern-game"]);
    assert_eq!(of_process(b"", [&game[..], &unlike]), None, "a wrapper not as expected marks nothing");
    assert_eq!(of_process(b"HOME=/h\0", [&game[..], &lutris]), None);
}

#[test]
fn an_entry_is_marked_by_its_launchers_shortcut() {
    let heroic = shortcut("desktop:Pattern Game", "Pattern Game", &["xdg-open", "heroic://launch?appName=patterngame&runner=sideload"]);
    let lutris = shortcut("desktop:net.lutris.pattern-game-1", "Pattern Game", &["env", "LUTRIS_SKIP_INIT=1", "lutris", "lutris:rungameid/1"]);
    let steam = Entry { launch: Launch::Steam { appid: 620 }, ..Entry::new("steam:620", "Portal 2") };
    let other = shortcut("desktop:foot", "Terminal", &["foot"]);
    let all = library(vec![heroic.clone(), lutris.clone(), steam.clone(), other.clone()]);
    for (g, mark) in [(&heroic, Some("heroic:sideload:patterngame")), (&lutris, Some("lutris:Pattern Game")), (&steam, Some("steam_app_620")), (&other, None)] {
        assert_eq!(of_entry(&all, g).as_deref(), mark, "{}", g.id);
        if let Some(mark) = mark {
            assert!(is_marked(&all, g, mark), "{}", g.id);
        }
    }
    assert!(!is_marked(&all, &heroic, "heroic:gog:patterngame") && !is_marked(&all, &steam, "steam_app_6200"));
    // Two Lutris shortcuts of one name: neither is known by it.
    let twin = shortcut("desktop:net.lutris.pattern-game-2", "Pattern Game", &["lutris", "lutris:rungameid/2"]);
    let twins = library(vec![lutris.clone(), twin]);
    assert_eq!(of_entry(&twins, &lutris), None);
    assert!(!is_marked(&twins, &lutris, "lutris:Pattern Game"));
}

#[test]
fn a_heroic_game_is_launched_by_heroic_without_its_window() {
    let heroic = shortcut("desktop:com.heroicgameslauncher.hgl", "Heroic Games Launcher", &["heroic"]);
    let game = shortcut("desktop:Pattern Game", "Pattern Game", &["xdg-open", "heroic://launch?appName=patterngame&runner=sideload"]);
    let lutris = shortcut("desktop:net.lutris.cave-1", "Cave", &["env", "LUTRIS_SKIP_INIT=1", "lutris", "lutris:rungameid/1"]);
    let all = library(vec![heroic.clone(), game.clone(), lutris.clone()]);
    let (launcher, command) = headless(&all, &game).unwrap();
    assert_eq!(launcher.id, heroic.id);
    assert_eq!(command, ["heroic", "--no-gui", "--no-sandbox", "heroic://launch?appName=patterngame&runner=sideload"]);
    assert!(headless(&all, &lutris).is_none(), "Lutris's shortcut already hides Lutris");
    assert!(launcher_of(&lutris).is_some());
    assert!(headless(&library(vec![game.clone()]), &game).is_none(), "without Heroic's entry, its shortcut as it is");
}
