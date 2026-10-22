//! The state's tests across features: launches starting and ending, the library's problems, the start, the tick.

use crate::testkit::*;
use crate::*;

/// Every order of `events`.
fn orders(events: &[Event]) -> Vec<Vec<Event>> {
    if events.len() <= 1 {
        return vec![events.to_vec()];
    }
    let mut all = Vec::new();
    for i in 0..events.len() {
        let mut rest = events.to_vec();
        let first = rest.remove(i);
        for mut order in orders(&rest) {
            order.insert(0, first.clone());
            all.push(order);
        }
    }
    all
}

/// The session's inputs coming in any order (systemd's list, the compositor's windows: separate
/// threads) end in the same state: the reconcile after each reads what is there,
/// not which came last.
#[test]
fn a_launchs_start_and_end_come_to_the_same_in_any_order() {
    let shape = |s: &State| {
        let running: Vec<(String, bool)> =
            s.session().running_entries().iter().map(|r| (r.id.to_string(), r.origin == crate::session::running::Origin::Ours)).collect();
        let dock: Vec<String> = s.dock().items().iter().map(|i| i.id.to_string()).collect();
        (running, dock)
    };
    let mut shapes = Vec::new();
    let mut plays = Vec::new();
    let start_end = |s: &mut State| {
        s.apply(Event::tick(1000));
        let fx = tap(s, game("zelda-lttp"));
        let fx: Vec<Effect> = fx.into_iter().chain(tap(s, game("zelda-lttp"))).collect();
        fx.iter().find_map(|e| match e {
            Effect::Launch { instance, .. } => Some(instance.clone()),
            _ => None,
        })
    };
    // (No launch: the shapes below say so.)
    let probe = start_end(&mut state()).unwrap_or(Instance::Shelf);
    let window = Window { unit: probe.unit(), visible: true, ..win(5, "zelda3", &probe.workspace(), true) };
    let starts = [Event::Units(vec![probe.clone()]), Event::Windows(vec![window])];
    let ends = [Event::Units(vec![]), Event::Windows(vec![])];
    for start in orders(&starts) {
        for end in orders(&ends) {
            let mut s = state();
            start_end(&mut s);
            for e in start.clone() {
                s.apply(e);
            }
            let started = shape(&s);
            let mut recorded = 0;
            for e in end.clone() {
                recorded += s.apply(e).1.iter().filter(|e| matches!(e, Effect::RecordPlay { .. })).count();
            }
            s.apply(Event::tick(1001));
            shapes.push((started, shape(&s)));
            plays.push(recorded);
        }
    }
    assert!(shapes.windows(2).all(|w| w.first() == w.last()), "{shapes:#?}");
    let first = shapes.first().cloned().unwrap_or_default();
    assert_eq!(first.0.0, vec![("zelda-lttp".to_owned(), true)], "running as a launch of ours");
    assert!(first.1.0.is_empty(), "ended");
    assert!(plays.iter().all(|&n| n == 1), "one play recorded each time: {plays:?}");
}

#[test]
fn new_library_problems_are_said_once() {
    let mut s = state();
    let with = |problems: &[&str]| {
        let mut library = clamshell_library::demo::library();
        library.problems = problems.iter().map(|p| (*p).to_owned()).collect();
        Event::Library(Box::new(library))
    };
    s.apply(with(&["grim isn't installed: no screenshots", "x: no launch command"]));
    assert_eq!(notice(&s), Some(crate::bottom::notice::NoticeKind::Problems("grim isn't installed: no screenshots".into(), 2)));
    s.apply(Event::tick(10_000));
    s.apply(with(&["grim isn't installed: no screenshots", "x: no launch command"]));
    assert_eq!(notice(&s), None, "the same again: not said");
}

#[test]
fn a_launch_says_it_is_starting() {
    let mut s = state();
    launch(&mut s, "zelda-lttp");
    assert!(matches!(s.bottom().notice().map(|n| &n.what), Some(crate::bottom::notice::NoticeKind::Starting(t)) if t == "A Link to the Past"));
}

#[test]
fn a_launch_that_ends_without_a_window_didnt_start() {
    let mut s = state();
    launch(&mut s, "zelda-lttp");
    crate::testkit::end(&mut s, "zelda-lttp");
    assert!(matches!(notice(&s), Some(crate::bottom::notice::NoticeKind::NotStarted(_))), "{:?}", notice(&s));
    // One that showed a window and ended: nothing said.
    let mut s = state();
    let zelda = launch(&mut s, "zelda-lttp");
    s.apply(Event::Windows(vec![Window { unit: zelda.unit(), visible: true, ..win(5, "zelda3", &zelda.workspace(), true) }]));
    s.apply(Event::Windows(vec![]));
    crate::testkit::end(&mut s, "zelda-lttp");
    assert!(!matches!(notice(&s), Some(crate::bottom::notice::NoticeKind::NotStarted(_))), "{:?}", notice(&s));
}

#[test]
fn a_launch_that_didnt_start_says_so() {
    let mut s = state();
    tap(&mut s, game("zelda-lttp"));
    tap(&mut s, game("zelda-lttp"));
    s.apply(Event::Launch(LaunchEvent::LaunchFailed("zelda-lttp".into())));
    let notice = s.bottom().notice().map(|n| n.what.clone());
    assert!(matches!(&notice, Some(crate::bottom::notice::NoticeKind::NotStarted(t)) if t == "A Link to the Past"), "{notice:?}");
    assert!(s.session().running("zelda-lttp").is_none());
}

#[test]
fn starts_with_first_game_selected_and_focused() {
    let s = state();
    assert_eq!(s.shelf().selected().map(|g| &**g), Some("zelda-lttp"));
    assert_eq!(s.pointer().focus(), Some(&game("zelda-lttp")));
}

#[test]
fn starts_where_it_was_told() {
    let s = State::new(Init { start_tab: Some(Tab::Dock), pad_mouse: true, ..init() });
    assert_eq!((s.bottom().tab(), s.pad().mouse(), s.pad().route()), (Tab::Dock, PadMouse::ByUser, PadRoute::Mouse));
}

#[test]
fn while_metadata_is_fetched_shelfs_view_goes_first() {
    let mut s = state();
    assert!(!s.apply(ShelfEvent::ListStep(1).into()).1.iter().any(|e| matches!(e, Effect::FetchFirst(_))), "not fetching");
    let (_, fx) = s.apply(Event::Fetching(true));
    assert_eq!(fx, vec![Effect::FetchFirst(vec!["metroid-zm".into(), "mk-ds".into()])], "the selected first, then the rows shown");
    assert!(!s.apply(Event::tick(5)).1.iter().any(|e| matches!(e, Effect::FetchFirst(_))), "the same view: not again");
    let (_, fx) = s.apply(ShelfEvent::ListStep(1).into());
    assert!(fx.contains(&Effect::FetchFirst(vec!["mk-ds".into(), "ff6".into()])), "{fx:?}");
    s.apply(Event::Fetching(false));
    assert!(!s.apply(ShelfEvent::ListStep(1).into()).1.iter().any(|e| matches!(e, Effect::FetchFirst(_))), "done");
}

#[test]
fn home_goes_to_shelf_and_back_to_what_was_in_front() {
    let mut s = state();
    let zelda = Instance::workspace_of(&clamshell_library::key_of("zelda-lttp"));
    let home = |s: &mut State| s.apply(Event::Home).1;
    assert_eq!(home(&mut s), vec![], "nothing before shelf: it stays");
    crate::testkit::start(&mut s, "zelda-lttp");
    assert_eq!(home(&mut s), vec![], "the program in force, no window of it yet: shelf stays");
    s.apply(Event::Windows(vec![shelf_window(true), win(3, "retroarch", &zelda, false)]));
    assert_eq!(home(&mut s), vec![Effect::Window(WindowCommand::Focus(zelda.clone()))], "no history: the program in force");
    // A terminal in front, then shelf: home goes back to the terminal.
    let terminal = "clamshell-run-foot_1";
    s.apply(Event::Windows(vec![shelf_window(false), win(2, "foot", terminal, true)]));
    assert_eq!(home(&mut s), vec![Effect::Window(WindowCommand::ShowShelf)]);
    s.apply(Event::Windows(vec![shelf_window(true), win(2, "foot", terminal, false)]));
    assert_eq!(home(&mut s), vec![Effect::Window(WindowCommand::Focus(terminal.into()))]);
    // The terminal closed: back to the game, which was in front before.
    s.apply(Event::Windows(vec![shelf_window(false), win(3, "retroarch", &zelda, true)]));
    s.apply(Event::Windows(vec![shelf_window(true), win(3, "retroarch", &zelda, false)]));
    assert_eq!(home(&mut s), vec![Effect::Window(WindowCommand::Focus(zelda))]);
}

#[test]
fn a_closed_window_goes_back_to_what_was_in_front_before_it_shelf_last() {
    let mut s = state();
    let (zelda, terminal) = ("clamshell-run-zelda-1".to_owned(), "clamshell-run-foot-2".to_owned());
    // Shelf, then the terminal, then the game in front.
    s.apply(Event::Windows(vec![shelf_window(true)]));
    s.apply(Event::Windows(vec![shelf_window(false), win(2, "foot", &terminal, true)]));
    s.apply(Event::Windows(vec![shelf_window(false), win(2, "foot", &terminal, false), win(3, "retroarch", &zelda, true)]));
    // The game quits: the terminal, in front before it.
    let (_, fx) = s.apply(Event::Windows(vec![shelf_window(false), win(2, "foot", &terminal, false)]));
    assert_eq!(window_effects(fx), vec![Effect::Window(WindowCommand::Focus(terminal.clone()))]);
    s.apply(Event::Windows(vec![shelf_window(false), win(2, "foot", &terminal, true)]));
    // The terminal quits: shelf.
    let (_, fx) = s.apply(Event::Windows(vec![shelf_window(false)]));
    assert_eq!(window_effects(fx), vec![Effect::Window(WindowCommand::ShowShelf)]);
    // A program's own second window closing, or a new workspace before its window: nothing.
    s.apply(Event::Windows(vec![shelf_window(false), win(3, "retroarch", &zelda, true), win(4, "retroarch", &zelda, false)]));
    assert_eq!(window_effects(s.apply(Event::Windows(vec![shelf_window(false), win(4, "retroarch", &zelda, true)])).1), vec![]);
    s.apply(Event::Windows(vec![shelf_window(true), win(4, "retroarch", &zelda, false)]));
    assert_eq!(
        window_effects(s.apply(Event::Windows(vec![shelf_window(false), win(4, "retroarch", &zelda, false)])).1),
        vec![],
        "shelf's window is still there"
    );
}

#[test]
fn a_window_in_front_replaced_at_once_is_shown_again() {
    let mut s = state();
    let app = "clamshell-run-fdroid-1".to_owned();
    s.apply(Event::tick(100));
    s.apply(Event::Windows(vec![shelf_window(false), win(2, "waydroid.org.fdroid.fdroid", &app, true)]));
    // Its window closes (shelf), another comes on its workspace (moved there, not in front): shown.
    assert_eq!(window_effects(s.apply(Event::Windows(vec![shelf_window(false)])).1), vec![Effect::Window(WindowCommand::ShowShelf)]);
    let (_, fx) = s.apply(Event::Windows(vec![shelf_window(true), win(3, "waydroid.org.fdroid.fdroid", &app, false)]));
    assert_eq!(window_effects(fx), vec![Effect::Window(WindowCommand::Focus(app.clone()))]);
    // Later: not.
    s.apply(Event::Windows(vec![shelf_window(false), win(3, "waydroid.org.fdroid.fdroid", &app, true)]));
    s.apply(Event::Windows(vec![shelf_window(false)]));
    s.apply(Event::tick(103));
    let (_, fx) = s.apply(Event::Windows(vec![shelf_window(true), win(4, "waydroid.org.fdroid.fdroid", &app, false)]));
    assert_eq!(window_effects(fx), vec![], "too late: it stays behind");
}

#[test]
fn home_doesnt_go_back_to_a_workspace_whose_window_moved_down() {
    let mut s = state();
    let terminal = "clamshell-run-foot_1";
    s.apply(Event::Windows(vec![shelf_window(false), win(2, "foot", terminal, true)]));
    // Its window moved to the bottom screen, shelf in front: nothing to go back to on the main screen.
    s.apply(Event::Windows(vec![shelf_window(true), Window { down: true, ..win(2, "foot", terminal, false) }]));
    assert_eq!(s.apply(Event::Home).1, vec![]);
}

#[test]
fn home_goes_back_to_a_workspace_that_isnt_ours() {
    let mut s = state();
    s.apply(Event::Windows(vec![shelf_window(false), foot()]));
    s.apply(Event::Windows(vec![shelf_window(true), Window { focused: false, ..foot() }]));
    assert_eq!(s.apply(Event::Home).1, vec![Effect::Window(WindowCommand::Focus("1".into()))]);
}

#[test]
fn tick_redraws_once_a_minute() {
    let mut s = state();
    assert_eq!(s.apply(Event::tick(60)).0, Dirty::BOTH);
    assert_eq!(s.apply(Event::tick(119)).0, Dirty::NONE);
}
