//! Tests of the state that span features: launches starting and ending, the library's problems, the
//! start and the tick.

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

/// The session's inputs, systemd's list and the compositor's windows, come from separate threads in any
/// order. They end in the same state, because the steps that settle each read what is there, not which
/// came last.
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
        s.applied(Event::tick(1000));
        let fx = tap(s, game("zelda-lttp"));
        let fx: Vec<Effect> = fx.into_iter().chain(tap(s, game("zelda-lttp"))).collect();
        fx.iter().find_map(|e| match e {
            Effect::Launch { instance, .. } => Some(instance.clone()),
            _ => None,
        })
    };
    // If there is no launch, the shapes below show it.
    let probe = start_end(&mut state()).unwrap_or(Instance::Shelf);
    let window = Window { unit: probe.unit(), visible: true, ..win(5, "zelda3", &probe.workspace(), true) };
    let starts = [Event::Units(vec![probe.clone()]), Event::Windows(vec![window])];
    let ends = [Event::Units(vec![]), Event::Windows(vec![])];
    for start in orders(&starts) {
        for end in orders(&ends) {
            let mut s = state();
            start_end(&mut s);
            for e in start.clone() {
                s.applied(e);
            }
            let started = shape(&s);
            let mut recorded = 0;
            for e in end.clone() {
                recorded += s.applied(e).iter().filter(|e| matches!(e, Effect::RecordPlay { .. })).count();
            }
            s.applied(Event::tick(1001));
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
    s.applied(with(&["grim isn't installed: no screenshots", "x: no launch command"]));
    assert_eq!(notice(&s), Some(crate::bottom::notice::NoticeKind::Problems("grim isn't installed: no screenshots".into(), 2)));
    s.applied(Event::tick(10_000));
    s.applied(with(&["grim isn't installed: no screenshots", "x: no launch command"]));
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
    // A launch that showed a window and then ended gets no notice.
    let mut s = state();
    let zelda = launch(&mut s, "zelda-lttp");
    s.applied(Event::Windows(vec![Window { unit: zelda.unit(), visible: true, ..win(5, "zelda3", &zelda.workspace(), true) }]));
    s.applied(Event::Windows(vec![]));
    crate::testkit::end(&mut s, "zelda-lttp");
    assert!(!matches!(notice(&s), Some(crate::bottom::notice::NoticeKind::NotStarted(_))), "{:?}", notice(&s));
}

#[test]
fn a_launch_that_didnt_start_says_so() {
    let mut s = state();
    tap(&mut s, game("zelda-lttp"));
    tap(&mut s, game("zelda-lttp"));
    s.applied(Event::Launch(LaunchEvent::LaunchFailed("zelda-lttp".into())));
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
    assert!(!s.applied(ShelfEvent::ListStep(1).into()).iter().any(|e| matches!(e, Effect::FetchFirst(_))), "not fetching");
    let fx = s.applied(Event::Fetching(true));
    assert_eq!(fx, vec![Effect::FetchFirst(vec!["metroid-zm".into(), "mk-ds".into()])], "the selected first, then the rows shown");
    assert!(!s.applied(Event::tick(5)).iter().any(|e| matches!(e, Effect::FetchFirst(_))), "the same view: not again");
    let fx = s.applied(ShelfEvent::ListStep(1).into());
    assert!(fx.contains(&Effect::FetchFirst(vec!["mk-ds".into(), "ff6".into()])), "{fx:?}");
    s.applied(Event::Fetching(false));
    assert!(!s.applied(ShelfEvent::ListStep(1).into()).iter().any(|e| matches!(e, Effect::FetchFirst(_))), "done");
}

/// The windows' effects come after those of the steps that follow them: here the quit entry is ended
/// (its windows are gone), then the Steam game found by its window is launched, then a stray window on
/// the landing workspace goes up.
#[test]
fn the_windows_effects_follow_the_later_steps() {
    let mut library = clamshell_library::demo::library();
    if let Some(g) = library.entries.iter_mut().find(|g| &*g.id == "ff6") {
        g.launch = clamshell_library::Launch::Steam { appid: 620 };
    }
    let mut s = started(Init { library, ..init() });
    s.applied(Event::tick(1000));
    let zelda = launch(&mut s, "zelda-lttp").workspace();
    s.applied(Event::Windows(vec![shelf_window(false), win(3, "zelda3", &zelda, true)]));
    s.applied(Event::Entry("zelda-lttp".into(), EntryAct::Quit));
    let landing = Instance::Landing.name();
    let fx = s.applied(Event::Windows(vec![shelf_window(false), win(7, "steam_app_620", &landing, true), win(8, "foot", &landing, false)]));
    let kinds: Vec<&str> = fx
        .iter()
        .map(|e| match e {
            Effect::EndRun { .. } => "end zelda",
            Effect::Save(Save::Resume(_)) => "save resume",
            Effect::Launch { entry, .. } if &**entry == "ff6" => "launch ff6",
            Effect::Window(WindowCommand::Up { window: 8, .. }) => "foot up",
            _ => "other",
        })
        .collect();
    assert_eq!(kinds, ["end zelda", "save resume", "launch ff6", "foot up"], "{fx:?}");
}
