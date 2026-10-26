//! Tests of the state that span features: launches starting and ending, the library's problems, the
//! start and the tick.

use crate::settings::Setting;
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
        let fx = tap(s, entry("zelda-lttp"));
        let fx: Vec<Effect> = fx.into_iter().chain(tap(s, entry("zelda-lttp"))).collect();
        fx.iter().find_map(|e| match e {
            Effect::Launch { instance, .. } => Some(instance.clone()),
            _ => None,
        })
    };
    // If there is no launch, the shapes below show it.
    let probe = start_end(&mut state()).unwrap_or(Instance { key: String::new(), secs: 0, n: 0 });
    let window = win_of(5, "zelda3", &probe);
    let starts = [Event::Units(vec![probe.clone()]), WindowEvent::Opened(window).into()];
    let ends = [Event::Units(vec![]), WindowEvent::Closed(5).into()];
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
    s.observed(vec![win_of(5, "zelda3", &zelda)]);
    s.observed(vec![]);
    crate::testkit::end(&mut s, "zelda-lttp");
    assert!(!matches!(notice(&s), Some(crate::bottom::notice::NoticeKind::NotStarted(_))), "{:?}", notice(&s));
}

#[test]
fn a_launch_that_didnt_start_says_so() {
    let mut s = state();
    tap(&mut s, entry("zelda-lttp"));
    tap(&mut s, entry("zelda-lttp"));
    s.applied(Event::Launch(LaunchEvent::LaunchFailed("zelda-lttp".into())));
    let notice = s.bottom().notice().map(|n| n.what.clone());
    assert!(matches!(&notice, Some(crate::bottom::notice::NoticeKind::NotStarted(t)) if t == "A Link to the Past"), "{notice:?}");
    assert!(s.session().running("zelda-lttp").is_none());
}

#[test]
fn starts_with_first_game_selected_and_focused() {
    let s = state();
    assert_eq!(s.shelf().selected().map(|g| &**g), Some("zelda-lttp"));
    assert_eq!(s.pointer().focus(), Some(&entry("zelda-lttp")));
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

/// Sync sends the settings in force, then the keymap, then the pad's route. Nothing outside depends on
/// the settings coming before the keymap; the keymap must come before the route (see
/// [`crate::sync::Sent::sync`]).
#[test]
fn a_game_coming_in_front_gets_its_settings_then_its_keymap_then_the_pad() {
    let mut s = started(Init { keymaps: crate::keymap::Keymaps::initial(), ..init() });
    s.settings_mut().set(Some(&"zelda-lttp".into()), Setting::Keymap, Some(1));
    s.applied(Event::tick(1000));
    tap(&mut s, entry("zelda-lttp"));
    tap(&mut s, entry("zelda-lttp"));
    let mut fx = start(&mut s, "zelda-lttp");
    assert_eq!(s.pad().route(), PadRoute::Ui, "the launch has no window yet");
    fx.extend(show_window(&mut s, "zelda-lttp", 7));
    let sent: Vec<&str> = fx
        .iter()
        .filter_map(|e| match e {
            Effect::Apply { .. } => Some("apply"),
            Effect::Keymap(k) if k.id == 1 => Some("keymap"),
            Effect::PadRoute(PadRoute::Game) => Some("route"),
            _ => None,
        })
        .collect();
    assert_eq!(sent, ["apply", "keymap", "route"], "{fx:?}");
}

/// The events a rule queues are handled in the order queued, each with what it queues in turn before the
/// next: Launch queues Play, whose launch comes before the screenshot queued after Launch.
#[test]
fn queued_events_are_handled_in_order_with_what_they_queue_first() {
    let mut s = state();
    let fx = s.handle_as_queued(vec![PointerEvent::Activate(Id::Shelf(ShelfId::Launch)).into(), Event::Device(crate::device::buttons::DeviceKey::Screenshot)]);
    let kinds: Vec<&str> = fx
        .iter()
        .map(|e| match e {
            Effect::Launch { .. } => "launch",
            Effect::Screenshot => "screenshot",
            _ => "other",
        })
        .filter(|k| *k != "other")
        .collect();
    assert_eq!(kinds, ["launch", "screenshot"], "{fx:?}");
}

/// When a launch's first window comes as the window in front that closed comes back, both are shown in
/// that order, so the one that came back ends in front.
#[test]
fn a_launchs_first_window_is_shown_before_a_window_that_came_back() {
    let mut s = state();
    s.applied(Event::tick(1000));
    let zelda = launch(&mut s, "zelda-lttp");
    let zelda_window = win_of(3, "zelda3", &zelda);
    s.observed(vec![zelda_window.clone()]);
    // Its window closes with nothing else in front: shelf is shown, and the group remembered.
    s.observed(vec![]);
    assert!(s.session().shelf_in_front());
    let ff6 = launch(&mut s, "ff6");
    let ff6_window = win_of(4, "ff6", &ff6);
    s.observed(vec![zelda_window, ff6_window]);
    assert_eq!(s.session().top_window().map(|w| w.id), Some(3));
}

/// The steps in the background run before the quitting entries are followed: here a paused game shown
/// again as the quitting one's window in front closes is thawed, then the quitting one, its windows gone,
/// is ended.
#[test]
fn the_background_is_followed_before_the_quitting() {
    let mut s = state();
    s.applied(Event::tick(100));
    let zelda = launch(&mut s, "zelda-lttp");
    let zelda_window = win_of(3, "zelda3", &zelda);
    s.observed(vec![zelda_window.clone()]);
    let ff6 = launch(&mut s, "ff6");
    s.observed(vec![zelda_window.clone(), win_of(4, "ff6", &ff6)]);
    s.applied(Event::tick(100 + crate::session::background::GRACE));
    assert!(s.session().paused("zelda-lttp"));
    s.applied(Event::Entry("ff6".into(), EntryAct::Quit));
    let fx = s.observed(vec![zelda_window]);
    let kinds: Vec<&str> = fx
        .iter()
        .filter_map(|e| match e {
            Effect::Freeze { frozen: false, .. } => Some("thaw zelda"),
            Effect::EndRun { .. } => Some("end ff6"),
            _ => None,
        })
        .collect();
    assert_eq!(kinds, ["thaw zelda", "end ff6"], "{fx:?}");
}

/// A paused game shown again because the window in front closed is thawed in the same event: the
/// background is followed once the front is settled.
#[test]
fn a_paused_game_shown_when_the_front_closes_is_thawed_at_once() {
    let mut s = state();
    s.applied(Event::tick(100));
    let zelda = launch(&mut s, "zelda-lttp");
    let zelda_window = win_of(3, "zelda3", &zelda);
    s.observed(vec![zelda_window.clone()]);
    let ff6 = launch(&mut s, "ff6");
    let ff6_window = win_of(4, "ff6", &ff6);
    s.observed(vec![zelda_window.clone(), ff6_window]);
    s.applied(Event::tick(100 + crate::session::background::GRACE));
    assert!(s.session().paused("zelda-lttp"));
    let fx = s.observed(vec![zelda_window]);
    assert_eq!(s.session().top_window().map(|w| w.id), Some(3));
    assert!(fx.iter().any(|e| matches!(e, Effect::Freeze { frozen: false, .. })), "{fx:?}");
}

/// The UI starting sends what was waiting for it, in this order: the brightness as it was left, then
/// after the event the keymap in force and the pad's route (keymap before route, as
/// [`crate::sync::Sent::sync`] says). The pad thread starts with neither, so both are sent though the
/// state took them as sent when it was built.
#[test]
fn the_ui_starting_sends_the_brightness_the_keymap_and_the_route() {
    use crate::bottom::screen::{WindowRule, WindowRules};
    let rule = WindowRule { owner: "zelda-lttp".into(), app_id: "zelda3".into(), title: None };
    let brightness = Brightness { level: Some(40), shares: Vec::new() };
    let keymaps = crate::keymap::Keymaps::initial();
    let mut s = State::new(Init { keymaps: keymaps.clone(), window_rules: WindowRules::from_list(vec![rule]), brightness, ..init() });
    s.settings_mut().global.set(Setting::Keymap, 1);
    s.applied(Event::tick(1000));
    let keymap = s.keymapping().in_force(1);
    let fx = s.applied(Event::Start);
    let sent: Vec<&Effect> = fx.iter().filter(|e| !matches!(e, Effect::Watch(..))).collect();
    assert_eq!(sent, [&Effect::Brightness { level: 40, shares: Vec::new() }, &Effect::Keymap(keymap), &Effect::PadRoute(PadRoute::Ui),], "{fx:?}");
    let fx = s.applied(Event::Start);
    assert!(fx.contains(&Effect::PadRoute(PadRoute::Ui)), "sent again at another start: {fx:?}");
}

/// Back from sleep, which may have reset what the settings set, the settings in force are applied again
/// though they didn't change; idle counts from the wake, so the screens come back on at their brightness.
#[test]
fn back_from_sleep_applies_the_settings_again_and_wakes_the_screens() {
    let mut s = State::new(Init { device_session: true, ..init() });
    s.ui_up();
    s.applied(Reading::Status(Status { brightness: Some(60), ..Status::default() }).into());
    s.applied(Event::tick(1000));
    s.applied(Event::Idle(crate::device::idle::IdleEvent::Watching(true)));
    assert!(s.applied(Event::tick(1900)).contains(&Effect::Sleep));
    let fx = s.applied(Event::Resumed);
    let global = s.prefs().settings().global;
    assert_eq!(s.idle().stage(), crate::device::idle::Stage::Awake);
    assert!(fx.contains(&Effect::Apply { entry: None, settings: global }), "{fx:?}");
    assert!(fx.contains(&Effect::Outputs { top: true, bottom: true }), "{fx:?}");
    assert!(fx.iter().any(|e| matches!(e, Effect::Brightness { level: 60, .. })), "undimmed: {fx:?}");
    assert!(!s.applied(Event::Resumed).iter().any(|e| matches!(e, Effect::Outputs { .. })), "awake already: the screens aren't sent again");
}

/// The refresh rate calibration has the bottom screen off while it runs, and the main screen at the rates
/// its tool asks for. When it ends, both are as ClamShell wants them: the main screen's rate as the
/// settings say, and the bottom screen on, or off when it was turned off by hand.
#[test]
fn a_refresh_calibration_sets_the_screens_and_then_gives_them_back() {
    let refresh = |fx: &[Effect]| fx.iter().filter_map(|e| if let Effect::Refresh(hz) = e { Some(*hz) } else { None }).collect::<Vec<_>>();
    let mut s = state();
    s.applied(Event::tick(1000));
    let fx = s.applied(PointerEvent::Activate(Id::Device(DeviceId::Refresh)).into());
    assert!(fx.contains(&Effect::Outputs { top: true, bottom: false }), "{fx:?}");
    assert_eq!(refresh(&s.applied(Event::CalibrationRate(60))), [60]);
    let fx = s.applied(Event::RefreshCalibrated(Ok("done".into())));
    assert!(fx.contains(&Effect::Outputs { top: true, bottom: true }), "{fx:?}");
    assert_eq!(refresh(&fx), [0], "the rate as it was");
    let fx = s.applied(Event::Device(crate::device::buttons::DeviceKey::BottomPower));
    assert!(fx.contains(&Effect::Outputs { top: true, bottom: false }), "{fx:?}");
    s.applied(PointerEvent::Activate(Id::Device(DeviceId::Refresh)).into());
    let fx = s.applied(Event::RefreshCalibrated(Ok("done".into())));
    assert!(!fx.iter().any(|e| matches!(e, Effect::Outputs { .. })), "off by hand, it stays off: {fx:?}");
    assert!(s.applied(Event::CalibrationRate(120)).is_empty(), "none runs: the rate isn't changed");
}

/// The main screen's refresh rate goes to a game's as it launches, so that the switch, which blanks the
/// screen for a moment, is over before its window shows. Then it follows the settings in force, and it is
/// sent only when it changes.
#[test]
fn the_refresh_rate_follows_a_launch_at_once_then_the_settings_in_force() {
    let mut settings = crate::settings::Settings::default();
    settings.set(Some(&"zelda-lttp".into()), Setting::Refresh, Some(60));
    let mut s = State::new(Init { settings, ..init() });
    s.ui_up();
    let refresh = |fx: &[Effect]| fx.iter().filter_map(|e| if let Effect::Refresh(hz) = e { Some(*hz) } else { None }).collect::<Vec<_>>();
    let mut fx = tap(&mut s, entry("zelda-lttp"));
    fx.extend(tap(&mut s, entry("zelda-lttp")));
    assert!(fx.iter().any(|e| matches!(e, Effect::Launch { .. })), "{fx:?}");
    assert_eq!(refresh(&fx), [60], "at the launch: {fx:?}");
    let fx = crate::testkit::start(&mut s, "zelda-lttp");
    assert!(fx.iter().any(|e| matches!(e, Effect::Apply { .. })) && refresh(&fx).is_empty(), "in force, at the same rate: {fx:?}");
    assert_eq!(refresh(&crate::testkit::end(&mut s, "zelda-lttp")), [0], "ended: the global rate, as is");
}

/// The screens' power is sent when it changes: the bottom one alone while they are on, and both for idle,
/// with the bottom one as it is set.
#[test]
fn the_screens_power_is_sent_when_it_changes() {
    use crate::device::idle::IdleEvent;
    let mut s = state();
    s.applied(Event::tick(1000));
    s.applied(Event::Idle(IdleEvent::Watching(true)));
    let outputs = |fx: &[Effect]| fx.iter().filter(|e| matches!(e, Effect::Outputs { .. })).cloned().collect::<Vec<_>>();
    let key = |s: &mut State| s.applied(Event::Device(crate::device::buttons::DeviceKey::BottomPower));
    assert_eq!(outputs(&key(&mut s)), [Effect::Outputs { top: true, bottom: false }], "the bottom one by hand");
    assert_eq!(outputs(&s.applied(Event::tick(1300))), [Effect::Outputs { top: false, bottom: false }], "off for idle");
    assert_eq!(outputs(&s.applied(Event::Idle(IdleEvent::DeviceInput))), [Effect::Outputs { top: true, bottom: false }], "back, the bottom one still off");
    assert!(outputs(&s.applied(Event::tick(1301))).is_empty(), "once");
}

/// While metadata is fetched, a library read again asks for shelf's entries shown first again, since the
/// metadata thread starts over with the new library.
#[test]
fn a_library_read_while_fetching_asks_for_shelfs_view_first_again() {
    let mut s = state();
    let first = Effect::FetchFirst(vec!["zelda-lttp".into(), "metroid-zm".into()]);
    assert_eq!(s.applied(Event::Fetching(true)), std::slice::from_ref(&first));
    let fx = s.applied(Event::Library(Box::new(clamshell_library::demo::library())));
    assert!(fx.contains(&first), "the same view, asked for again: {fx:?}");
    s.applied(Event::Fetching(false));
    let fx = s.applied(Event::Library(Box::new(clamshell_library::demo::library())));
    assert!(!fx.iter().any(|e| matches!(e, Effect::FetchFirst(_))), "not fetching: {fx:?}");
}

/// A library read again updates what depends on its entries: the runners paused together, which a runner
/// new in the library then is.
#[test]
fn a_library_read_updates_the_runners_paused_together() {
    let mut s = state();
    s.applied(Event::tick(100));
    let runner = clamshell_library::RunnerOf {
        handover: true,
        pause: vec!["pause".to_owned()].into(),
        resume: vec!["resume".to_owned()].into(),
        ..clamshell_library::RunnerOf::new("android", "Android", true)
    };
    s.settings_mut().set(Some(&runner.key), Setting::Background, Some(crate::settings::Choice::value(crate::settings::InBackground::Pauses)));
    let mut library = clamshell_library::demo::library();
    for g in library.entries.iter_mut() {
        match &*g.id {
            "ff6" => g.launch = clamshell_library::Launch::Steam { appid: 620 },
            "desktop:firefox" => g.runner = Some(runner.clone()),
            _ => {}
        }
    }
    s.applied(Event::Library(Box::new(library)));
    let pause = Effect::RunnerCommand(vec!["pause".into()]);
    assert!(!s.applied(Event::tick(100 + crate::session::background::GRACE - 1)).contains(&pause));
    assert!(s.applied(Event::tick(100 + crate::session::background::GRACE)).contains(&pause), "the new runner is paused together");
}

/// A video's play, once recorded, asks where mpv left it; the answer is kept as the entry's place.
#[test]
fn a_video_ended_asks_where_it_was_left() {
    let episode = "tv:Cosmos/Season 1/1. The Shores of the Cosmic Ocean.mkv";
    let mut s = state();
    s.applied(Event::tick(100));
    launch_now(&mut s, episode);
    start(&mut s, episode);
    s.applied(Event::tick(400));
    let fx = end(&mut s, episode);
    let file = std::path::PathBuf::from("/videos/Cosmos/Season 1/1. The Shores of the Cosmic Ocean.mkv");
    let at = fx.iter().position(|e| matches!(e, Effect::RecordPlay { entry, duration: 300, .. } if &**entry == episode));
    let read = fx.iter().position(|e| *e == Effect::ReadPosition { entry: episode.into(), file: file.clone() });
    assert!(at.is_some() && read == at.map(|i| i + 1), "the play, then the position: {fx:?}");
    s.applied(LaunchEvent::Position { entry: episode.into(), position: Some(700) }.into());
    assert_eq!(s.shelf().entry(episode).map(|g| g.playback.position), Some(Some(700)));
}

#[test]
fn dumping_window_events_is_one_backend_effect_without_changing_windows() {
    use crate::testkit::*;
    let mut s = state();
    let before = s.session().windows().to_vec();
    assert_eq!(s.applied(crate::Event::DumpWindowEvents("events.json".into())), vec![crate::Effect::DumpWindowEvents("events.json".into())]);
    assert_eq!(s.session().windows(), before);
}

#[test]
fn immutable_device_profile_is_carried_from_init_through_state_events() {
    for profile in [crate::Profile::thor(), crate::Profile::vm(), crate::Profile::generic()] {
        let mut state = crate::State::new(crate::Init { profile, ..Default::default() });
        assert_eq!(*state.profile(), profile);
        let mut effects = Vec::new();
        state.apply(crate::Event::Home, &mut effects);
        assert_eq!(*state.profile(), profile);
    }
}
