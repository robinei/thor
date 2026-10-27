use crate::session::REOPEN_SECS;
use crate::session::running::{HANDOFF, LAUNCHER_WAIT};
use crate::testkit::*;
use crate::*;

fn launches(fx: &[Effect], id: &str) -> bool {
    fx.iter().any(|e| matches!(e, Effect::Launch { entry, .. } if &**entry == id))
}

fn steam_ff6() -> State {
    let mut library = clamshell_library::demo::library();
    if let Some(g) = library.entries.iter_mut().find(|g| &*g.id == "ff6") {
        g.launch = Launch::Steam { appid: 620 };
    }
    State::new(Init { library: Some(library), ..init() })
}

fn key(id: &str) -> String {
    clamshell_library::key_of(id)
}

#[test]
fn has_mark_agrees_with_mark_of() {
    let steam = Entry { launch: Launch::Steam { appid: 620 }, ..Entry::new("steam:620", "Portal 2") };
    let handed = Entry {
        runner: Some(clamshell_library::RunnerOf { handover: true, ..clamshell_library::RunnerOf::new("android", "Android", false) }),
        ..Entry::new("desktop:org.app", "App")
    };
    let heroic = Entry { launch: Launch::program(vec!["xdg-open".into(), "heroic://launch?appName=x&runner=gog".into()]), ..Entry::new("desktop:X", "X") };
    let ours = Entry::new("desktop:foot", "Terminal");
    let library = crate::Library { entries: vec![steam.clone(), handed.clone(), heroic.clone(), ours.clone()], ..crate::Library::default() };
    let marks = ["steam_app_620", "steam_app_62", "org.app", "foot", "heroic:gog:x", ""];
    for g in [&steam, &handed, &heroic, &ours] {
        for mark in marks {
            let mark_of = super::mark_of(&library, g);
            let by_app_id = WindowFacts { app_id: mark.into(), ..win(1, "") };
            let by_process = WindowFacts { game: Some(mark.into()), ..win(1, "other") };
            // A handed-over app is known by its app id alone; a launcher's game by either.
            let expected = mark_of.as_deref() == Some(mark);
            assert_eq!(super::has_mark(&library, g, &by_app_id), expected, "{} app id {mark}", g.id);
            assert_eq!(super::has_mark(&library, g, &by_process), expected && g.id != handed.id, "{} process {mark}", g.id);
        }
    }
}

#[test]
fn a_play_lasts_by_the_monotonic_clock_however_the_wall_clock_steps() {
    let mut s = state();
    let first = s.shelf().selected().cloned().unwrap();
    tap(&mut s, Id::Shelf(ShelfId::Launch));
    s.applied(Event::Tick { unix_secs: 100_000, mono_secs: 1000 });
    crate::testkit::start(&mut s, &first);
    // A minute later by the monotonic clock, with the wall clock set forward a day.
    s.applied(Event::Tick { unix_secs: 100_000 + 86_400 + 60, mono_secs: 1060 });
    let fx = crate::testkit::end(&mut s, &first);
    let start = 100_000 + 86_400;
    assert!(fx.contains(&Effect::RecordPlay { entry: first, start, duration: 60 }), "dated by the wall clock as it reads now: {fx:?}");
}

#[test]
fn an_entry_runs_its_plays_count_and_it_may_run_twice() {
    let mut s = state();
    let first = s.shelf().selected().cloned().unwrap();
    let fx = tap(&mut s, Id::Shelf(ShelfId::Launch));
    assert!(launches(&fx, &first));
    assert!(!launches(&tap(&mut s, Id::Shelf(ShelfId::Launch)), &first), "not twice while starting");
    s.applied(Event::tick(1000));
    crate::testkit::start(&mut s, &first);
    let started = s.session().units().first().cloned().unwrap();
    assert_eq!(tap(&mut s, Id::Shelf(ShelfId::Launch)), vec![], "running, no window yet: nothing to show");
    s.observed(vec![win_of(3, "game", &started)]);
    assert_eq!(tap(&mut s, Id::Shelf(ShelfId::Launch)), vec![Effect::PadRoute(crate::PadRoute::Game)]);
    assert_eq!(s.session().top_window().map(|w| w.id), Some(3), "running: shown");
    // Another launch, as from the dock's Open another, runs it twice until both end.
    assert!(launches(&launch_now(&mut s, &first), &first));
    crate::testkit::start(&mut s, &first);
    // The first launch's unit is gone and the second's is listed.
    let second: Vec<LaunchUnit> = s.session().units().iter().skip(1).cloned().collect();
    s.applied(Event::Units(second));
    assert!(s.session().running(&first).is_some(), "the other still runs");
    s.applied(Event::tick(1660));
    let fx = crate::testkit::end(&mut s, &first);
    assert!(s.session().running(&first).is_none());
    assert!(fx.contains(&Effect::RecordPlay { entry: first.clone(), start: 1000, duration: 660 }), "{fx:?}");
    let g = s.entry(&first).unwrap();
    assert_eq!((g.plays.count, g.plays.seconds), (1, 660), "one play: from its first start to its last end");
    assert!(s.shelf().library().collections.iter().any(|c| c.name == "Recent"));
}

#[test]
fn entries_run_side_by_side() {
    let mut s = state();
    s.applied(Event::tick(100));
    let zelda = launch(&mut s, "zelda-lttp");
    assert_eq!(zelda, LaunchUnit { key: key("zelda-lttp"), secs: 100, n: 0 });
    tap(&mut s, entry("ff6"));
    assert!(launches(&tap(&mut s, Id::Shelf(ShelfId::Launch)), "ff6"), "no choice: it starts");
    crate::testkit::start(&mut s, "ff6");
    assert_eq!(s.session().running_entries().len(), 2);
}

#[test]
fn quit_closes_the_windows_then_ends_the_slice_force_kill_at_once() {
    let mut s = state();
    let zelda = launch(&mut s, "zelda-lttp");
    let window = |id, title: &str| WindowFacts { title: title.into(), ..win_of(id, "zelda3", &zelda) };
    s.observed(vec![window(1, "game")]);
    let id: std::sync::Arc<str> = "zelda-lttp".into();
    assert_eq!(quit(&mut s, &id), vec![Effect::Compositor(Compositor::Close(vec![1]))]);
    let fx = s.observed(vec![window(2, "Save changes?")]);
    assert!(!fx.iter().any(|e| matches!(e, Effect::EndRun { .. })), "a window left: waits");
    let fx = s.observed(vec![]);
    assert!(fx.contains(&Effect::EndRun { key: key("zelda-lttp"), force: false }), "{fx:?}");
    assert_eq!(force_kill(&mut s, "zelda-lttp"), vec![Effect::EndRun { key: key("zelda-lttp"), force: true }]);
}

/// A quit that ends a program known by its app id runs its runner's stop once: with the end, when its
/// launch still runs after its windows closed, and not again when it stops running.
#[test]
fn a_quit_that_ends_an_entry_runs_its_runners_stop_once() {
    let mut library = clamshell_library::demo::library();
    let firefox = library.entries.iter_mut().find(|g| &*g.id == "desktop:firefox").unwrap();
    let stop = vec!["stop".to_owned(), "{id}".to_owned()].into();
    firefox.runner = Some(clamshell_library::RunnerOf { handover: true, stop, ..clamshell_library::RunnerOf::new("android", "Android", true) });
    let mut s = State::new(Init { library: Some(library), ..init() });
    s.applied(Event::tick(100));
    let firefox = launch_now(&mut s, "desktop:firefox");
    let unit = firefox.iter().find_map(|e| if let Effect::Launch { unit, .. } = e { Some(unit.clone()) } else { None }).unwrap();
    crate::testkit::start(&mut s, "desktop:firefox");
    s.observed(vec![win_of(6, "firefox", &unit)]);
    let stop = Effect::RunnerCommand(vec!["stop".into(), "firefox".into()]);
    quit(&mut s, "desktop:firefox");
    let fx = s.observed(vec![]);
    assert!(fx.contains(&Effect::EndRun { key: key("desktop:firefox"), force: false }) && fx.contains(&stop), "ended with its stop: {fx:?}");
    s.applied(Event::tick(200 + HANDOFF));
    let fx = crate::testkit::end(&mut s, "desktop:firefox");
    assert!(s.session().running("desktop:firefox").is_none());
    assert!(!fx.contains(&stop), "not again: {fx:?}");
}

/// A quit the program refuses, its "save changes?" answered with cancel, is given up once its windows
/// are still there [`super::QUIT_WAIT`] after they were asked to close: the entry runs on, is paused
/// hidden again, and isn't ended when its windows close later.
#[test]
fn a_quit_refused_is_given_up() {
    use crate::settings::Choice;
    let mut s = state();
    let zelda = launch(&mut s, "zelda-lttp");
    let key = key("zelda-lttp");
    s.settings_mut().set(Some(&"zelda-lttp".into()), crate::settings::Setting::Background, Some(crate::settings::InBackground::Pauses.value()));
    s.applied(Event::tick(100));
    s.observed(vec![win_of(1, "zelda3", &zelda)]);
    quit(&mut s, "zelda-lttp");
    s.applied(Event::Home);
    s.applied(Event::tick(100 + super::QUIT_WAIT - 1));
    assert!(s.session().is_quitting("zelda-lttp"), "still waiting");
    s.applied(Event::tick(100 + super::QUIT_WAIT));
    assert!(!s.session().is_quitting("zelda-lttp"), "given up");
    s.applied(Event::tick(101 + super::QUIT_WAIT));
    let fx = s.applied(Event::tick(101 + super::QUIT_WAIT + crate::session::background::GRACE));
    assert!(fx.contains(&Effect::Freeze { slice: LaunchUnit::slice_of(&key), frozen: true }), "paused hidden again: {fx:?}");
    let fx = s.observed(vec![]);
    assert!(!fx.iter().any(|e| matches!(e, Effect::EndRun { .. })), "{fx:?}");
    assert!(s.session().running("zelda-lttp").is_some());
}

/// A launch the launcher never answers is given up after [`super::LAUNCH_WAIT`], says it didn't start,
/// and can be launched again.
#[test]
fn a_launch_never_answered_is_given_up() {
    let mut s = state();
    s.applied(Event::tick(100));
    let fx: Vec<Effect> = tap(&mut s, entry("zelda-lttp")).into_iter().chain(tap(&mut s, entry("zelda-lttp"))).collect();
    assert!(launches(&fx, "zelda-lttp"));
    s.applied(Event::tick(100 + super::LAUNCH_WAIT - 1));
    assert!(s.session().is_launching("zelda-lttp"));
    s.applied(Event::tick(100 + super::LAUNCH_WAIT));
    assert!(!s.session().is_launching("zelda-lttp"));
    assert!(matches!(notice(&s), Some(crate::bottom::notice::NoticeKind::NotStarted(t)) if t == "A Link to the Past"), "{:?}", notice(&s));
    assert_eq!(s.session().front_launch(), None);
    assert!(launches(&tap(&mut s, entry("zelda-lttp")), "zelda-lttp"), "launched again");
}

/// A launch given up that starts after all runs as a launch of ours, though another launch of the
/// entry waits meanwhile; until then its windows would be taken for a program found by them, with its
/// unit listed.
#[test]
fn a_launch_given_up_that_starts_late_runs_beside_the_one_waiting() {
    let mut s = state();
    s.applied(Event::tick(100));
    let unit = |fx: &[Effect]| {
        fx.iter().find_map(|e| match e {
            Effect::Launch { unit, .. } => Some(unit.clone()),
            _ => None,
        })
    };
    let first = unit(&launch_now(&mut s, "desktop:foot")).unwrap();
    s.applied(Event::tick(100 + super::LAUNCH_WAIT));
    assert!(!s.session().is_launching("desktop:foot"), "given up");
    let second = unit(&launch_now(&mut s, "desktop:foot")).unwrap();
    s.applied(Event::Units(vec![first.clone()]));
    let running = s.session().running("desktop:foot").map(|r| r.origin.clone());
    assert_eq!(running, Some(Origin::Ours), "runs by the unit that started late");
    let fx = s.observed(vec![win_of(5, "foot", &first)]);
    assert!(!fx.iter().any(|e| matches!(e, Effect::Adopt { .. })), "its window is its own: {fx:?}");
    assert!(s.session().is_launching("desktop:foot"));
    s.applied(Event::Units(vec![first, second]));
    assert!(!s.session().is_launching("desktop:foot"));
    assert_eq!(s.session().running_entries().iter().filter(|r| &*r.id == "desktop:foot").count(), 1);
}

/// The launcher's failure coming after the launch was given up doesn't say "didn't start" a second
/// time, nor start a browsed file's wait for its window.
#[test]
fn a_launch_failure_after_it_was_given_up_says_nothing() {
    let mut s = state();
    s.applied(Event::tick(100));
    tap(&mut s, entry("zelda-lttp"));
    tap(&mut s, entry("zelda-lttp"));
    s.applied(Event::tick(100 + super::LAUNCH_WAIT));
    assert!(matches!(notice(&s), Some(crate::bottom::notice::NoticeKind::NotStarted(_))), "{:?}", notice(&s));
    s.applied(Event::tick(200));
    assert_eq!(notice(&s), None);
    crate::testkit::launch_failed(&mut s, "zelda-lttp");
    assert_eq!(notice(&s), None, "said once");
    assert!(s.session().running("zelda-lttp").is_none());
    let mut s = files();
    s.applied(Event::tick(100));
    tap(&mut s, Id::Shelf(ShelfId::Launch));
    s.applied(Event::tick(100 + super::LAUNCH_WAIT));
    crate::testkit::launch_failed(&mut s, "browse:Home/notes.md");
    assert!(s.session().running("browse:Home/notes.md").is_none(), "no wait for a window");
}

#[test]
fn a_game_steam_started_is_followed_as_a_launch_beside_the_others() {
    let mut s = steam_ff6();
    s.applied(Event::tick(1000));
    let zelda = launch(&mut s, "zelda-lttp");
    let windows = vec![win_of(1, "zelda3", &zelda), win(7, "steam_app_620")];
    let fx = s.observed(windows.clone());
    // Found by its window, it is launched; the launcher finds it running and moves it into the
    // launch's unit. Its window is placed synchronously before the unit is listed.
    assert!(launches(&fx, "ff6"), "{fx:?}");
    let ff6 = Group::entry(&key("ff6"));
    assert_eq!(s.session().windows().iter().find(|w| w.id == 7).and_then(|w| w.group.clone()), Some(ff6.clone()));
    assert!(!launches(&s.observed(windows), "ff6"), "once");
    crate::testkit::start(&mut s, "ff6");
    assert_eq!(s.session().running_entries().len(), 2, "the other keeps running");
    let fx = s.observed(vec![win_of(1, "zelda3", &zelda), win(7, "steam_app_620")]);
    assert!(fx.is_empty(), "already placed: {fx:?}");
    // It is ended like any launch, through its slice.
    assert_eq!(force_kill(&mut s, "ff6"), vec![Effect::EndRun { key: key("ff6"), force: true }]);
    s.applied(Event::tick(1600));
    s.observed(vec![win_of(1, "zelda3", &zelda)]);
    let fx = crate::testkit::end(&mut s, "ff6");
    assert!(fx.contains(&Effect::RecordPlay { entry: "ff6".into(), start: 1000, duration: 600 }), "{fx:?}");
    assert!(s.session().running("ff6").is_none());
    // A Steam game the library doesn't have isn't one.
    let fx = s.observed(vec![win_of(1, "zelda3", &zelda), win(9, "steam_app_1")]);
    assert!(!fx.iter().any(|e| matches!(e, Effect::Launch { .. })), "{fx:?}");
    assert_eq!(s.session().running_entries().len(), 1, "zelda only");
    // A game started by a Steam of ours is in Steam's scope, but the app id says whose it is.
    let steam = LaunchUnit { key: key("desktop:steam"), secs: 1, n: 0 };
    let in_steam = win_of(8, "steam_app_620", &steam);
    let fx = s.observed(vec![win_of(1, "zelda3", &zelda), in_steam]);
    assert!(launches(&fx, "ff6"), "{fx:?}");
}

#[test]
fn a_steam_game_shelf_started_ends_by_the_launcher_not_its_windows() {
    let mut s = steam_ff6();
    launch(&mut s, "ff6");
    let ff6 = s.session().running("ff6").unwrap();
    assert_eq!((ff6.origin.clone(), ff6.mark.as_deref()), (Origin::Ours, Some("steam_app_620")));
    s.observed(vec![win(7, "steam_app_620")]);
    let ff6 = Group::entry(&key("ff6"));
    assert_eq!(s.session().window(7).and_then(|w| w.group.clone()), Some(ff6.clone()), "its windows go to its group");
    let fx = s.observed(vec![]);
    assert!(!fx.iter().any(|e| matches!(e, Effect::RecordPlay { .. })), "{fx:?}");
    assert!(s.session().running("ff6").is_some());
}

#[test]
fn a_steam_games_window_is_the_games_though_it_runs_in_steams_scope() {
    let mut s = steam_ff6();
    let steam = LaunchUnit { key: key("desktop:steam"), secs: 1, n: 0 };
    s.applied(Event::Units(vec![steam.clone()]));
    launch(&mut s, "ff6");
    let window = win_of(7, "steam_app_620", &steam);
    s.observed(vec![window.clone()]);
    let ff6 = Group::entry(&key("ff6"));
    assert_eq!(s.session().window(7).and_then(|w| w.group.clone()), Some(ff6.clone()), "the game's, not Steam's");
    assert_eq!(s.session().running_of(&window).map(|r| &*r.id), Some("ff6"));
}

/// While a Steam game's launch waits for its window, Steam's own windows (raised by the launch, or a
/// dialog of Steam's) open behind; the game's comes in front.
#[test]
fn steams_windows_open_behind_a_steam_games_launch() {
    let mut s = steam_ff6();
    let steam = LaunchUnit { key: key("desktop:steam"), secs: 1, n: 0 };
    s.applied(Event::Units(vec![steam.clone()]));
    let client = win_of(5, "steam", &steam);
    s.observed(vec![client.clone()]);
    s.applied(crate::WindowEvent::Activate(5).into());
    launch(&mut s, "ff6");
    assert!(!s.session().opening_focus(&WindowFacts { id: 6, ..client.clone() }), "Steam's window");
    assert!(!s.session().opening_focus(&WindowFacts { id: 6, parent: Some(5), ..client.clone() }), "a dialog of Steam's");
    let game = win_of(7, "steam_app_620", &steam);
    assert!(s.session().opening_focus(&game), "the game's, though in Steam's scope");
}

/// Another tool than the Device view is asked of the system; an id the library doesn't have plays
/// nothing.
#[test]
fn play_of_a_tool_asks_for_it_and_of_an_unknown_id_does_nothing() {
    let mut library = clamshell_library::demo::library();
    library.entries.push(Entry { launch: Launch::Tool(clamshell_library::Tool::Rescan), ..Entry::new("tool:rescan", "Rescan") });
    let mut s = state_with(library);
    let fx = s.applied(Event::Entry("tool:rescan".into(), EntryAct::Play));
    assert_eq!(fx, vec![Effect::Tool(clamshell_library::Tool::Rescan)]);
    assert!(!s.session().is_launching("tool:rescan") && s.session().running("tool:rescan").is_none());
    assert_eq!(s.applied(Event::Entry("nothing-such".into(), EntryAct::Play)), vec![]);
    assert!(s.session().launching().is_empty());
    assert_eq!(notice(&s), None);
}

/// A Steam game's launch that failed may still come by its window, as when Steam started it anyway:
/// it waits [`HANDOFF`] for one, which then makes it run, and without one it is gone with no play.
#[test]
fn a_failed_launch_known_by_app_id_waits_for_its_window() {
    let mut s = steam_ff6();
    s.ui_up();
    s.applied(Event::tick(1000));
    tap(&mut s, entry("ff6"));
    tap(&mut s, entry("ff6"));
    crate::testkit::launch_failed(&mut s, "ff6");
    let ff6 = s.session().running("ff6").unwrap();
    assert_eq!(ff6.origin, Origin::Windows { since: 1000, until: 1000 + HANDOFF });
    assert_eq!(notice(&s).map(|n| matches!(n, crate::bottom::notice::NoticeKind::NotStarted(_))), Some(false), "not said to have failed");
    s.applied(Event::tick(1010));
    s.observed(vec![win(7, "steam_app_620")]);
    assert_eq!(s.session().window(7).and_then(|w| w.group.clone()), Some(Group::entry(&key("ff6"))));
    s.applied(Event::tick(1100));
    assert!(s.session().running("ff6").is_some(), "runs by its window");
    s.observed(vec![]);
    assert!(s.session().running("ff6").is_some(), "kept a moment, for another window");
    let fx = s.applied(Event::tick(1100 + REOPEN_SECS + 1));
    assert!(fx.contains(&Effect::RecordPlay { entry: "ff6".into(), start: 1000, duration: 100 + REOPEN_SECS + 1 }), "{fx:?}");
    // Without a window it is gone once the wait is over, with no play.
    tap(&mut s, entry("ff6"));
    crate::testkit::launch_failed(&mut s, "ff6");
    s.applied(Event::tick(1103 + HANDOFF));
    assert!(s.session().running("ff6").is_some());
    let fx = s.applied(Event::tick(1104 + HANDOFF));
    assert!(s.session().running("ff6").is_none());
    assert!(!fx.iter().any(|e| matches!(e, Effect::RecordPlay { .. })), "{fx:?}");
}

/// A Steam game whose launch's unit ends while its window is open, long after it started, runs on by
/// its window. Its play, recorded when the window closes, is from its launch.
#[test]
fn a_game_known_by_app_id_runs_on_by_its_windows_after_its_unit() {
    let mut s = steam_ff6();
    s.ui_up();
    s.applied(Event::tick(1000));
    launch(&mut s, "ff6");
    s.observed(vec![win(7, "steam_app_620")]);
    s.applied(Event::tick(1600));
    let fx = crate::testkit::end(&mut s, "ff6");
    assert!(!fx.iter().any(|e| matches!(e, Effect::RecordPlay { .. })), "{fx:?}");
    let ff6 = s.session().running("ff6").unwrap();
    assert!(matches!(ff6.origin, Origin::Windows { .. }) && ff6.seen, "{ff6:?}");
    s.applied(Event::tick(1900));
    s.observed(vec![]);
    let fx = s.applied(Event::tick(1900 + REOPEN_SECS + 1));
    assert!(fx.contains(&Effect::RecordPlay { entry: "ff6".into(), start: 1000, duration: 900 + REOPEN_SECS + 1 }), "{fx:?}");
    assert!(s.session().running("ff6").is_none());
}

/// Quit or force kill of an entry that doesn't run does nothing, and leaves nothing quitting.
#[test]
fn quit_and_force_kill_of_an_entry_not_running_do_nothing() {
    let mut s = state();
    assert_eq!(s.applied(Event::Entry("zelda-lttp".into(), EntryAct::Quit)), vec![]);
    assert!(!s.session().is_quitting("zelda-lttp"));
    assert_eq!(s.applied(Event::Entry("zelda-lttp".into(), EntryAct::ForceKill)), vec![]);
}

/// The program in force is the focused window's owner; with shelf or another window in front it is
/// kept while it runs; once it ends, the entry started last that runs takes over, then none.
#[test]
fn the_program_in_force_falls_back_to_the_last_started() {
    let mut s = state();
    s.applied(Event::tick(1000));
    let zelda = launch(&mut s, "zelda-lttp");
    let ff6 = launch(&mut s, "ff6");
    let program = |s: &State| s.session().program().map(|p| p.to_string());
    let zelda_window = win_of(3, "zelda3", &zelda);
    let ff6_window = win_of(4, "ff6", &ff6);
    s.observed(vec![zelda_window.clone(), ff6_window.clone()]);
    s.applied(crate::WindowEvent::Activate(3).into());
    assert_eq!(program(&s).as_deref(), Some("zelda-lttp"), "in front");
    s.applied(Event::Home);
    assert_eq!(program(&s).as_deref(), Some("zelda-lttp"), "kept with shelf in front");
    s.observed(vec![foot(), zelda_window, ff6_window.clone()]);
    assert_eq!(program(&s).as_deref(), Some("zelda-lttp"), "kept with another window in front");
    s.observed(vec![foot(), ff6_window]);
    crate::testkit::end(&mut s, "zelda-lttp");
    assert_eq!(program(&s).as_deref(), Some("ff6"), "the last started that runs");
    s.observed(vec![foot()]);
    crate::testkit::end(&mut s, "ff6");
    assert_eq!(program(&s), None);
}

/// A Files tab with a file of its own and one that is a library entry's (zelda's).
fn files() -> State {
    let mut library = clamshell_library::demo::library();
    if let Some(zelda) = library.entries.first_mut() {
        zelda.file = Some("/roms/zelda.sfc".into());
    }
    let browsed = |id: &str, file: &str| crate::Entry {
        file: Some(file.into()),
        launch: Launch::program(vec!["xdg-open".into(), file.into()]),
        browsed: Some(crate::Browsed::default()),
        ..crate::Entry::new(id, file)
    };
    let n = library.entries.len();
    library.entries.push(browsed("browse:Home/notes.md", "/h/notes.md"));
    library.entries.push(browsed("browse:Home/zelda.sfc", "/roms/zelda.sfc"));
    library.collections.insert(0, crate::Collection::new("Files", vec![n, n + 1]));
    crate::testkit::state_with(library)
}

#[test]
fn a_file_that_is_a_library_entry_opens_as_it() {
    let mut s = files();
    s.applied(ShelfEvent::ListStep(1).into());
    let fx = tap(&mut s, Id::Shelf(ShelfId::Launch));
    assert!(launches(&fx, "zelda-lttp"), "{fx:?}");
}

#[test]
fn a_file_handed_to_another_program_runs_by_its_window() {
    let mut s = files();
    s.applied(Event::tick(100));
    let other = win_of(5, "steam", &launch_of("steam"));
    s.observed(vec![other.clone()]);
    let fx = tap(&mut s, Id::Shelf(ShelfId::Launch));
    let ws = fx.iter().find_map(|e| match e {
        Effect::Launch { unit, .. } => Some(group_of(unit)),
        _ => None,
    });
    let ws = ws.unwrap();
    // xdg-open ended before its unit was seen; the app's window comes up with no group.
    crate::testkit::launch_failed(&mut s, "browse:Home/notes.md");
    assert!(s.session().running("browse:Home/notes.md").is_some(), "waits for its window");
    // The demo's Terminal has foot's desktop file.
    let app = win(6, "foot");
    s.observed(vec![other.clone(), app.clone()]);
    assert_eq!(s.session().window(6).and_then(|w| w.group.clone()), Some(ws.clone()));
    assert_eq!(s.session().window(other.id).and_then(|w| w.group.clone()), Some(group_of(&launch_of("steam"))), "only the new one");
    assert_eq!(s.session().top_window().map(|w| w.id), Some(6));
    s.observed(vec![other.clone(), app]);
    s.applied(Event::tick(200));
    assert!(s.session().running("browse:Home/notes.md").is_some(), "runs while its window is open");
    assert_eq!(s.session().app_showing("browse:Home/notes.md", s.shelf().library()).map(|g| &*g.id), Some("desktop:foot"), "its picture: the app's");
    // With its file gone from the library meanwhile, it is still shown by what it was.
    s.applied(Event::Library(Box::new(clamshell_library::demo::library())));
    assert!(s.shelf().entry("browse:Home/notes.md").is_none());
    assert_eq!(s.entry("browse:Home/notes.md").map(|g| g.title.as_str()), Some("/h/notes.md"));
    s.applied(Event::Library(Box::new(files().shelf().library().clone())));
    s.observed(vec![other.clone()]);
    s.applied(Event::tick(200 + REOPEN_SECS + 1));
    assert!(s.session().running("browse:Home/notes.md").is_none(), "its window closed: ended");
    // Another launch that never shows a window is gone after a while.
    // (The library read again left shelf on another collection: the file is selected first.)
    while s.shelf().selected().map(|g| &**g) != Some("browse:Home/notes.md") {
        s.applied(ShelfEvent::CollectionStep(1).into());
    }
    assert!(launches(&tap(&mut s, Id::Shelf(ShelfId::Launch)), "browse:Home/notes.md"));
    crate::testkit::launch_failed(&mut s, "browse:Home/notes.md");
    s.applied(Event::tick(203 + HANDOFF));
    assert!(s.session().running("browse:Home/notes.md").is_some());
    s.applied(Event::tick(204 + HANDOFF));
    assert!(s.session().running("browse:Home/notes.md").is_none());
    assert!(s.dock().items().is_empty(), "off the dock");
}

#[test]
fn an_app_handed_over_runs_by_its_window_found_by_its_app_id() {
    let mut library = clamshell_library::demo::library();
    let firefox = library.entries.iter_mut().find(|g| &*g.id == "desktop:firefox").unwrap();
    let stop = vec!["stop".to_owned(), "{id}".to_owned()].into();
    firefox.runner = Some(clamshell_library::RunnerOf { handover: true, stop, ..clamshell_library::RunnerOf::new("android", "Android", true) });
    let mut s = State::new(Init { library: Some(library), ..init() });
    s.applied(Event::tick(100));
    s.observed(vec![]);
    let ws = Group::entry(&key("desktop:firefox"));
    assert!(launches(&launch_now(&mut s, "desktop:firefox"), "desktop:firefox"));
    crate::testkit::start(&mut s, "desktop:firefox");
    // Its launch handed the app over to the Waydroid session and ended.
    crate::testkit::end(&mut s, "desktop:firefox");
    assert!(s.session().running("desktop:firefox").is_some(), "waits for its window");
    let app = win(6, "firefox");
    s.observed(vec![app.clone()]);
    assert_eq!(s.session().window(6).and_then(|w| w.group.clone()), Some(ws.clone()));
    s.observed(vec![app.clone()]);
    assert!(!launches(&launch_now(&mut s, "desktop:firefox"), "desktop:firefox"), "it runs once: shown");
    s.applied(Event::tick(400));
    assert!(s.session().running("desktop:firefox").is_some(), "runs while its window is open");
    let fx = force_kill(&mut s, "desktop:firefox");
    assert!(fx.contains(&Effect::RunnerCommand(vec!["stop".into(), "firefox".into()])), "its runner's stop: {fx:?}");
    s.observed(vec![]);
    let fx = s.applied(Event::tick(400 + REOPEN_SECS + 1));
    assert!(s.session().running("desktop:firefox").is_none(), "its window closed: ended");
    assert!(fx.contains(&Effect::RecordPlay { entry: "desktop:firefox".into(), start: 100, duration: 300 + REOPEN_SECS + 1 }), "played until then: {fx:?}");
    // Opened by another app, it runs, and its window goes to its group and is shown.
    s.observed(vec![app.clone()]);
    assert!(s.session().running("desktop:firefox").is_some());
    assert_eq!(s.session().window(6).and_then(|w| w.group.clone()), Some(ws.clone()));
    assert_eq!(s.session().top_window().map(|w| w.id), Some(6), "shown in the same event");
    // Quit closes its window. It ends with the window, and its runner's stop runs then, once.
    let stop = Effect::RunnerCommand(vec!["stop".into(), "firefox".into()]);
    let fx = quit(&mut s, "desktop:firefox");
    assert!(fx.contains(&Effect::Compositor(Compositor::Close(vec![6]))) && !fx.contains(&stop), "{fx:?}");
    let fx = s.observed(vec![]);
    assert!(fx.contains(&stop), "{fx:?}");
    assert!(!s.observed(vec![]).contains(&stop), "once");
    s.observed(vec![app]);
    // A window of no entry and no launch of ours gets a group of its own on the main screen.
    s.observed(vec![win(9, "gone")]);
    assert_eq!(s.session().windows().iter().find(|w| w.id == 9).and_then(|w| w.group.clone()), Some(Group::Window(9)));
    // An entry of another runner ends with its launch.
    let mut s = state();
    launch_now(&mut s, "desktop:firefox");
    crate::testkit::start(&mut s, "desktop:firefox");
    crate::testkit::end(&mut s, "desktop:firefox");
    assert!(s.session().running("desktop:firefox").is_none());
}

/// Heroic, with its game's shortcut, and Lutris, with its game's (see [`crate::session::launchers`]).
fn launchers() -> State {
    let mut library = clamshell_library::demo::library();
    let shortcut = |id: &str, title: &str, argv: &[&str]| Entry {
        launch: Launch::program(argv.iter().map(|a| (*a).to_owned()).collect()),
        app: argv.len() == 1,
        ..Entry::new(id, title)
    };
    library.entries.extend([
        shortcut("desktop:com.heroicgameslauncher.hgl", "Heroic Games Launcher", &["heroic"]),
        shortcut("desktop:Pattern Game", "Pattern Game", &["xdg-open", "heroic://launch?appName=patterngame&runner=sideload"]),
        shortcut("desktop:net.lutris.Lutris", "Lutris", &["lutris"]),
        shortcut("desktop:net.lutris.cave-1", "Cave", &["env", "LUTRIS_SKIP_INIT=1", "lutris", "lutris:rungameid/1"]),
    ]);
    State::new(Init { library: Some(library), ..init() })
}

/// A Heroic game launched from shelf is launched as a launch of Heroic's entry, which starts Heroic
/// without its window and hands it the game: Heroic and the game are entries of their own. The game runs by
/// its window, which Heroic's mark makes the game's though it is in Heroic's unit; it comes in front, its
/// play is the game's, and when it closes shelf comes back while Heroic runs on.
#[test]
fn a_heroic_game_is_launched_by_heroic_and_runs_as_its_own_entry() {
    let mut s = launchers();
    s.ui_up();
    s.applied(Event::tick(1000));
    let fx = launch_now(&mut s, "desktop:Pattern Game");
    let launch = fx.iter().find_map(|e| match e {
        Effect::Launch { entry, unit, launch, .. } => Some((entry.clone(), unit.clone(), launch.clone())),
        _ => None,
    });
    let (entry, heroic, launch) = launch.unwrap();
    assert_eq!(&*entry, "desktop:com.heroicgameslauncher.hgl", "a launch of Heroic's");
    let argv = ["heroic", "--no-gui", "--no-sandbox", "heroic://launch?appName=patterngame&runner=sideload"];
    assert_eq!(launch, Launch::program(argv.iter().map(|a| (*a).to_owned()).collect()));
    let game = s.session().running("desktop:Pattern Game").unwrap();
    assert_eq!(game.origin, Origin::Windows { since: 1000, until: 1000 + LAUNCHER_WAIT });
    s.applied(Event::Units(vec![heroic.clone()]));
    assert!(s.session().running("desktop:com.heroicgameslauncher.hgl").is_some(), "Heroic runs as itself");
    s.applied(Event::tick(1060));
    let window = WindowFacts { game: Some("heroic:sideload:patterngame".into()), ..win_of(7, "sdl-pattern", &heroic) };
    s.observed(vec![window]);
    assert_eq!(s.session().window(7).and_then(|w| w.group.clone()), Some(Group::entry(&key("desktop:Pattern Game"))), "the game's, not Heroic's");
    assert_eq!(s.session().top_window().map(|w| w.id), Some(7), "in front, after a minute");
    s.applied(Event::tick(1300));
    s.observed(vec![]);
    assert!(s.session().shelf_in_front(), "back where it was launched from");
    let fx = s.applied(Event::tick(1300 + REOPEN_SECS + 1));
    assert!(fx.contains(&Effect::RecordPlay { entry: "desktop:Pattern Game".into(), start: 1000, duration: 300 + REOPEN_SECS + 1 }), "{fx:?}");
    assert!(s.session().running("desktop:com.heroicgameslauncher.hgl").is_some(), "Heroic runs on");
    // Launched again while Heroic runs, it is again a launch of Heroic's, which hands it over and ends.
    let fx = launch_now(&mut s, "desktop:Pattern Game");
    assert!(launches(&fx, "desktop:com.heroicgameslauncher.hgl"), "{fx:?}");
}

/// A game that its launcher, already running, took from its own shortcut runs by its window in the
/// launcher's unit, known by the launcher's mark, and waits for it as long as a launcher may take.
#[test]
fn a_game_handed_over_to_its_running_launcher_is_known_by_the_launchers_mark() {
    let mut s = launchers();
    s.ui_up();
    s.applied(Event::tick(1000));
    launch_now(&mut s, "desktop:net.lutris.Lutris");
    crate::testkit::start(&mut s, "desktop:net.lutris.Lutris");
    let lutris = s.session().units().first().cloned().unwrap();
    s.observed(vec![win_of(5, "net.lutris.Lutris", &lutris)]);
    s.applied(Event::Home);
    s.applied(Event::tick(1010));
    launch_now(&mut s, "desktop:net.lutris.cave-1");
    crate::testkit::start(&mut s, "desktop:net.lutris.cave-1");
    crate::testkit::end(&mut s, "desktop:net.lutris.cave-1");
    s.applied(Event::tick(1010 + LAUNCHER_WAIT));
    assert!(s.session().running("desktop:net.lutris.cave-1").is_some(), "waits for its window");
    let game = WindowFacts { game: Some("lutris:Cave".into()), ..win_of(7, "cave", &lutris) };
    s.observed(vec![win_of(5, "net.lutris.Lutris", &lutris), game]);
    assert_eq!(s.session().window(7).and_then(|w| w.group.clone()), Some(Group::entry(&key("desktop:net.lutris.cave-1"))), "the game's, not Lutris's");
    assert_eq!(s.session().top_window().map(|w| w.id), Some(7), "in front");
    s.observed(vec![win_of(5, "net.lutris.Lutris", &lutris)]);
    assert!(s.session().shelf_in_front(), "back to shelf, where it was launched from");
}

/// A game whose launcher ran it in the launch's own unit, as Lutris does when it isn't running, ends with
/// that unit once its window has come and gone, however soon, and its play is recorded.
#[test]
fn a_launchers_game_ends_with_its_unit_once_its_window_has_closed() {
    let mut s = launchers();
    s.ui_up();
    s.applied(Event::tick(1000));
    launch_now(&mut s, "desktop:net.lutris.cave-1");
    crate::testkit::start(&mut s, "desktop:net.lutris.cave-1");
    let cave = s.session().units().first().cloned().unwrap();
    s.observed(vec![WindowFacts { game: Some("lutris:Cave".into()), ..win_of(7, "cave", &cave) }]);
    s.applied(Event::tick(1008));
    s.observed(vec![]);
    let fx = crate::testkit::end(&mut s, "desktop:net.lutris.cave-1");
    assert!(fx.contains(&Effect::RecordPlay { entry: "desktop:net.lutris.cave-1".into(), start: 1000, duration: 8 }), "{fx:?}");
    assert!(s.session().running("desktop:net.lutris.cave-1").is_none());
}

/// A game started in its launcher's own window runs as its entry, shown, though ClamShell didn't launch
/// it.
#[test]
fn a_game_started_in_its_launcher_runs_as_its_entry() {
    let mut s = launchers();
    s.ui_up();
    launch_now(&mut s, "desktop:net.lutris.Lutris");
    crate::testkit::start(&mut s, "desktop:net.lutris.Lutris");
    let lutris = s.session().units().first().cloned().unwrap();
    s.observed(vec![win_of(5, "net.lutris.Lutris", &lutris)]);
    let game = WindowFacts { game: Some("lutris:Cave".into()), ..win_of(7, "cave", &lutris) };
    s.observed(vec![win_of(5, "net.lutris.Lutris", &lutris), game]);
    assert!(s.session().running("desktop:net.lutris.cave-1").is_some());
    assert_eq!(s.session().window(7).and_then(|w| w.group.clone()), Some(Group::entry(&key("desktop:net.lutris.cave-1"))));
    assert_eq!(s.session().top_window().map(|w| w.id), Some(7), "shown");
}

#[test]
fn entries_started_before_clamshell_are_found_by_their_units() {
    let mut s = state();
    s.applied(Event::tick(1000));
    let foot = LaunchUnit { key: key("desktop:foot"), secs: 400, n: 0 };
    s.applied(Event::Units(vec![foot.clone()]));
    assert!(s.session().running("desktop:foot").is_some(), "adopted");
    s.observed(vec![win_of(4, "foot", &foot)]);
    assert_eq!(tap(&mut s, Id::Dock(DockId::Entry(0))), vec![Effect::PadRoute(crate::PadRoute::Game)]);
    assert_eq!(s.session().top_window().map(|w| w.id), Some(4), "on the dock, shown");
    s.applied(Event::tick(1300));
    let fx = s.applied(Event::Units(vec![]));
    assert!(s.session().running("desktop:foot").is_none(), "its units gone: ended");
    assert!(fx.contains(&Effect::RecordPlay { entry: "desktop:foot".into(), start: 400, duration: 900 }), "played since its launch: {fx:?}");
}

#[test]
fn a_launch_runs_while_a_unit_of_it_is_listed() {
    let mut s = state();
    s.applied(Event::tick(100));
    let fx = tap(&mut s, entry("zelda-lttp"));
    let fx: Vec<Effect> = fx.into_iter().chain(tap(&mut s, entry("zelda-lttp"))).collect();
    let zelda = fx.iter().find_map(|e| match e {
        Effect::Launch { unit, .. } => Some(unit.clone()),
        _ => None,
    });
    let zelda = zelda.unwrap();
    assert!(s.session().running("zelda-lttp").is_none(), "asked for, its unit not there yet: starting");
    // While its unit is listed it runs. Once it is no longer listed, it has ended and its play is
    // recorded once.
    s.applied(Event::Units(vec![zelda.clone()]));
    assert!(s.session().running("zelda-lttp").is_some());
    s.applied(Event::tick(400));
    let fx = s.applied(Event::Units(vec![]));
    assert!(s.session().running("zelda-lttp").is_none());
    assert!(fx.contains(&Effect::RecordPlay { entry: "zelda-lttp".into(), start: 100, duration: 300 }), "once: {fx:?}");
    assert!(!s.applied(Event::Units(vec![])).iter().any(|e| matches!(e, Effect::RecordPlay { .. })), "once");
}

/// Show, as from shelf, of an entry whose windows are all on the bottom screen shows them there, as a
/// tap on its dock tile does, and doesn't focus its empty group on the main screen.
#[test]
fn show_of_an_entry_whose_windows_are_all_below_shows_them_there() {
    let mut s = state();
    let zelda = launch(&mut s, "zelda-lttp");
    s.observed(vec![win_of(3, "zelda3", &zelda)]);
    s.applied(Event::HardwareKey(crate::HardwareKey::WindowDown));
    s.applied(Event::HardwareKey(crate::HardwareKey::BottomView));
    assert!(!s.bottom().shows_windows(s.session()));
    show(&mut s, "zelda-lttp");
    assert!(s.bottom().shows_windows(s.session()));
    assert_eq!(s.session().bottom_window().map(|w| w.id), Some(3));
    assert!(s.session().shelf_in_front());
    s.applied(Event::HardwareKey(crate::HardwareKey::BottomView));
    assert!(!s.bottom().shows_windows(s.session()));
    tap(&mut s, Id::Dock(DockId::Entry(0)));
    assert!(s.bottom().shows_windows(s.session()), "the dock tile's tap: the same");
    assert!(s.session().shelf_in_front());
    // The bottom screen turned off by hand comes on again for them.
    s.applied(Event::HardwareKey(crate::HardwareKey::BottomPower));
    assert!(s.bottom().off());
    s.applied(Event::HardwareKey(crate::HardwareKey::BottomView));
    tap(&mut s, Id::Dock(DockId::Entry(0)));
    assert!(!s.bottom().off() && s.bottom().shows_windows(s.session()), "on again");
    // With a window of it on the main screen, that is shown.
    s.observed(vec![win_of(3, "zelda3", &zelda), win_of(4, "second", &zelda)]);
    show(&mut s, "zelda-lttp");
    assert_eq!(s.session().top_window().map(|w| w.id), Some(4));
}

#[test]
fn the_settings_follow_the_front() {
    let mut s = state();
    let zelda = launch(&mut s, "zelda-lttp");
    let in_front = |s: &mut State, w: WindowFacts| {
        let id = w.id;
        s.observed(vec![w]);
        s.applied(crate::WindowEvent::Activate(id).into())
    };
    tap(&mut s, entry("ff6"));
    let id = |s: &State| s.panel().context().map(|g| g.id.to_string());
    in_front(&mut s, win_of(2, "zelda3", &zelda));
    assert_eq!(id(&s).as_deref(), Some("zelda-lttp"), "the entry in front");
    s.applied(Event::Home);
    assert_eq!(id(&s).as_deref(), Some("ff6"), "shelf in front: its selection");
    in_front(&mut s, win(3, "foot"));
    assert_eq!(s.panel().context(), None, "a window of none of ours: the defaults");
}

#[test]
fn a_capped_game_is_launched_presenting_in_fifo() {
    let env_of = |fx: &[Effect]| fx.iter().find_map(|e| if let Effect::Launch { env, .. } = e { Some(env.clone()) } else { None });
    let mut s = state();
    assert_eq!(env_of(&launch_now(&mut s, "zelda-lttp")), Some(Vec::new()), "no cap");
    let mut s = state();
    s.settings_mut().set(Some(&"zelda-lttp".into()), crate::settings::Setting::FpsCap, Some(40));
    let env = env_of(&launch_now(&mut s, "zelda-lttp")).unwrap();
    assert!(env.contains(&("MESA_VK_WSI_PRESENT_MODE".into(), "fifo".into())), "{env:?}");
}

/// A program running by its windows that closes its only window and opens another, as Waydroid's apps do
/// as they start, runs on: it ends only once it has had no window for a moment.
#[test]
fn a_program_replacing_its_only_window_runs_on() {
    let mut s = steam_ff6();
    s.ui_up();
    s.applied(Event::tick(1000));
    launch(&mut s, "ff6");
    s.observed(vec![win(7, "steam_app_620")]);
    s.applied(Event::tick(1600));
    crate::testkit::end(&mut s, "ff6");
    let fx = s.observed(vec![]);
    assert!(!fx.iter().any(|e| matches!(e, Effect::RecordPlay { .. } | Effect::Apply { entry: None, .. })), "{fx:?}");
    let fx = s.observed(vec![win(8, "steam_app_620")]);
    assert!(!fx.iter().any(|e| matches!(e, Effect::RecordPlay { .. } | Effect::Apply { .. })), "the same run: {fx:?}");
    let ff6 = s.session().running("ff6").unwrap();
    assert!(matches!(ff6.origin, Origin::Windows { since: 1000, .. }), "{ff6:?}");
}

/// The launcher's late failure of a launch already given up leaves a newer launch of the same entry
/// waiting: the failure names its launch.
#[test]
fn a_late_failure_leaves_a_newer_launch_of_the_entry_waiting() {
    let mut s = state();
    s.applied(Event::tick(1000));
    let launched = |fx: &[Effect]| fx.iter().find_map(|e| if let Effect::Launch { unit, .. } = e { Some(unit.clone()) } else { None });
    let mut fx = tap(&mut s, entry("zelda-lttp"));
    fx.extend(tap(&mut s, entry("zelda-lttp")));
    let first = launched(&fx).unwrap();
    s.applied(Event::tick(1000 + super::LAUNCH_WAIT));
    assert!(s.session().launching().is_empty(), "given up");
    let second = launched(&launch_now(&mut s, "zelda-lttp")).unwrap();
    assert_ne!(first, second);
    s.applied(Event::Launch(crate::LaunchEvent::LaunchFailed { entry: "zelda-lttp".into(), unit: first }));
    assert_eq!(s.session().launching().iter().map(|l| &l.unit).collect::<Vec<_>>(), [&second], "the newer one waits on");
}
