use super::*;

fn shortcut(id: &str, title: &str, argv: &[&str]) -> Entry {
    Entry { launch: Launch::program(argv.iter().map(|a| (*a).to_owned()).collect()), ..Entry::new(id, title) }
}

fn library(entries: Vec<Entry>) -> Library {
    Library::new(Vec::new(), entries)
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
fn a_heroic_game_is_launched_by_heroic_without_its_window() {
    let heroic = shortcut("desktop:com.heroicgameslauncher.hgl", "Heroic Games Launcher", &["heroic"]);
    let game = shortcut("desktop:Pattern Game", "Pattern Game", &["xdg-open", "heroic://launch?appName=patterngame&runner=sideload"]);
    let lutris = shortcut("desktop:net.lutris.cave-1", "Cave", &["env", "LUTRIS_SKIP_INIT=1", "lutris", "lutris:rungameid/1"]);
    let all = library(vec![heroic.clone(), game.clone(), lutris.clone()]);
    let (launcher, command) = headless(&all, &game).unwrap();
    assert_eq!(launcher.id, heroic.id);
    assert_eq!(command, ["heroic", "--no-gui", "--no-sandbox", "heroic://launch?appName=patterngame&runner=sideload"]);
    assert!(headless(&all, &lutris).is_none(), "Lutris's shortcut already hides Lutris");
    assert_eq!(launcher_of(&lutris).map(|l| l.name), Some("Lutris"));
    assert!(headless(&library(vec![game.clone()]), &game).is_none(), "without Heroic's entry, its shortcut as it is");
}

/// Heroic and one of its games, both to start with ClamShell: the game goes first and starts Heroic,
/// whose own entry then finds that launch starting. Heroic is launched once.
#[test]
fn a_launcher_and_its_game_started_together_launch_the_launcher_once() {
    use crate::Effect;
    use crate::testkit::*;
    let heroic = shortcut("desktop:com.heroicgameslauncher.hgl", "Heroic Games Launcher", &["heroic"]);
    let game = shortcut("desktop:Pattern Game", "Pattern Game", &["xdg-open", "heroic://launch?appName=patterngame&runner=sideload"]);
    let init = crate::Init { autostart: vec![heroic.id.clone(), game.id.clone()], ..crate::testkit::init() };
    let mut s = crate::State::new(crate::Init { library: Some(library(vec![heroic, game])), ..init });
    let fx = s.applied(crate::Event::Units(vec![]));
    let launches: Vec<_> = fx.iter().filter_map(|e| if let Effect::Launch { launch, .. } = e { Some(launch.clone()) } else { None }).collect();
    assert_eq!(launches.len(), 1, "{fx:?}");
    assert!(
        matches!(launches.first(), Some(Launch::Program { argv, .. }) if argv.iter().any(|a| a.starts_with("heroic://launch"))),
        "the game's: {launches:?}"
    );
}

/// Two of Heroic's games to start with ClamShell: the first starts Heroic with it, and the second waits
/// for that launch, then goes once it has started (its unit listed), not dropped.
#[test]
fn a_launchers_second_game_waits_for_the_launchers_launch() {
    use crate::Effect;
    use crate::testkit::*;
    let heroic = shortcut("desktop:com.heroicgameslauncher.hgl", "Heroic Games Launcher", &["heroic"]);
    let first = shortcut("desktop:Pattern Game", "Pattern Game", &["xdg-open", "heroic://launch?appName=patterngame&runner=sideload"]);
    let second = shortcut("desktop:Other Game", "Other Game", &["xdg-open", "heroic://launch?appName=othergame&runner=sideload"]);
    let init = crate::Init { autostart: vec![first.id.clone(), second.id.clone()], ..crate::testkit::init() };
    let mut s = crate::State::new(crate::Init { library: Some(library(vec![heroic, first, second])), ..init });
    let launches = |fx: &[Effect]| {
        fx.iter().filter_map(|e| if let Effect::Launch { launch, unit, .. } = e { Some((launch.clone(), unit.clone())) } else { None }).collect::<Vec<_>>()
    };
    let fx = s.applied(crate::Event::Units(vec![]));
    let first_launch = launches(&fx);
    assert_eq!(first_launch.len(), 1, "one at a time: {fx:?}");
    let Some((_, unit)) = first_launch.into_iter().next() else { return };
    let fx = s.applied(crate::Event::Units(vec![unit]));
    let second_launch = launches(&fx);
    assert!(
        matches!(second_launch.as_slice(), [(Launch::Program { argv, .. }, _)] if argv.iter().any(|a| a.contains("othergame"))),
        "the second, once Heroic's launch started: {fx:?}"
    );
}
