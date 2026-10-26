use super::*;
use crate::DeviceKey;
use crate::testkit::*;
use crate::{Effect, Event};

/// The session's windows' facts.
fn facts(s: &crate::State) -> Vec<WindowFacts> {
    s.session().windows().iter().map(|w| w.facts.clone()).collect()
}

#[test]
fn opening_focus_respects_launch_intent_and_shown_program() {
    let mut s = state();
    let unit = "clamshell-run-one-1-1.scope";
    let first = WindowFacts { unit: Some(unit.into()), ..win(7, "one") };
    assert!(!s.session().opening_focus(&first));
    s.observed(vec![first.clone()]);
    s.applied(WindowEvent::Activate(7).into());
    let next = WindowFacts { id: 8, ..first.clone() };
    assert!(s.session().opening_focus(&next));
    let other_launch = WindowFacts { unit: Some("clamshell-run-one-1-2.scope".into()), ..next.clone() };
    assert!(!s.session().opening_focus(&other_launch));
    assert!(s.session().opening_focus(&WindowFacts { parent: Some(7), ..next.clone() }), "a dialog of the program in front");
    s.applied(Event::Home);
    assert!(!s.session().opening_focus(&next));
    assert!(!s.session().opening_focus(&WindowFacts { parent: Some(7), ..next }), "a dialog of a program behind stays behind");
    assert!(s.session().opening_focus(&win(9, "foot")));
}

#[test]
fn a_window_opens_in_front_as_its_opening_focus_says() {
    let mut s = state();
    let one = launch_of("one");
    // The first window of a launch with no launch waiting for it opens behind.
    assert!(s.applied(WindowEvent::Opened(win_of(7, "one", &one)).into()).is_empty());
    assert!(s.session().shelf_in_front());
    s.applied(WindowEvent::Activate(7).into());
    // A later window of the program in front comes in front, with the keyboard.
    s.applied(WindowEvent::Opened(win_of(8, "one", &one)).into());
    assert_eq!(s.session().top_window().map(|w| w.id), Some(8));
    assert_eq!(s.session().keyboard_focus(), Some(8));
    // Another program's window comes in front at once.
    s.applied(WindowEvent::Opened(win(9, "foot")).into());
    assert_eq!(s.session().top_window().map(|w| w.id), Some(9));
}

#[test]
fn lifecycle_messages_keep_one_window_and_ignore_late_messages() {
    let mut s = state();
    let window = win_of(7, "test", &launch_of("one"));
    assert!(s.applied(WindowEvent::Opened(window.clone()).into()).is_empty());
    assert_eq!(facts(&s), std::slice::from_ref(&window));
    let duplicate = WindowFacts { title: "duplicate".into(), ..window.clone() };
    assert!(s.applied(WindowEvent::Opened(duplicate).into()).is_empty());
    assert_eq!(facts(&s), std::slice::from_ref(&window));
    assert!(s.applied(WindowEvent::Title { window: 7, title: "new title".into() }.into()).is_empty());
    let renamed = WindowFacts { title: "new title".into(), ..window };
    assert_eq!(facts(&s), &[renamed]);
    assert!(s.applied(WindowEvent::Closed(7).into()).is_empty());
    assert!(s.session().windows().is_empty());
    for event in [WindowEvent::Closed(7), WindowEvent::Title { window: 7, title: "late".into() }, WindowEvent::Activate(7)] {
        assert!(s.applied(event.into()).is_empty());
        assert!(s.session().windows().is_empty());
    }
    let reused = win_of(7, "another", &launch_of("two"));
    assert!(s.applied(WindowEvent::Opened(reused.clone()).into()).is_empty());
    assert_eq!(facts(&s), &[reused]);
}

#[test]
fn a_backend_batch_keeps_replacement_history_until_all_changes_are_applied() {
    let mut s = state();
    let other = launch_of("other");
    s.observed(vec![win_of(7, "test", &other)]);
    s.applied(WindowEvent::Activate(7).into());
    let replacement = win_of(8, "test", &other);
    let event = Event::WindowBatch(vec![WindowEvent::Closed(7), WindowEvent::Opened(replacement.clone())]);
    // Closing first and settling would prune this group from history. One dispatch keeps the
    // replacement present throughout settling.
    assert_eq!(s.applied(event), vec![Effect::PadRoute(crate::PadRoute::Ui)]);
    assert_eq!(facts(&s), &[replacement]);
    assert!(s.applied(Event::Home).is_empty());
    assert_eq!(s.applied(WindowEvent::Activate(8).into()), vec![Effect::PadRoute(crate::PadRoute::Game)]);
    assert_eq!(s.session().top_window().map(|w| w.id), Some(8));
}

#[test]
fn backend_changes_keep_the_placement() {
    let mut s = state();
    let other = launch_of("other");
    let one = win_of(7, "one", &other);
    let two = win_of(8, "two", &other);
    s.observed(vec![one.clone(), two.clone()]);
    s.applied(WindowEvent::Activate(8).into());
    let changed = WindowFacts { title: "new title".into(), pids: vec![100], ..two };
    let windows = vec![one, changed];
    assert!(s.observed(windows.clone()).is_empty());
    assert_eq!(facts(&s), windows);
    assert!(s.observed(windows.clone()).is_empty());
    assert_eq!(facts(&s), windows);
    assert_eq!(s.session().top_window().map(|w| w.id), Some(8));
    assert!(s.applied(WindowEvent::Changed(win(99, "late")).into()).is_empty());
    assert_eq!(facts(&s), windows);
}

#[test]
fn lifecycle_close_falls_back_once_and_retains_the_other_windows() {
    let mut s = state();
    let other = win_of(6, "other", &launch_of("two"));
    s.observed(vec![other.clone(), win_of(7, "test", &launch_of("one"))]);
    s.applied(WindowEvent::Activate(7).into());
    assert_eq!(s.applied(WindowEvent::Closed(7).into()), vec![Effect::PadRoute(crate::PadRoute::Ui)]);
    assert_eq!(facts(&s), std::slice::from_ref(&other));
    assert!(s.session().shelf_in_front());
    assert!(s.applied(WindowEvent::Closed(7).into()).is_empty());
    assert_eq!(facts(&s), &[other]);
}

#[test]
fn activation_selects_the_window_on_its_screen() {
    let mut s = state();
    s.observed(vec![win_of(7, "test", &launch_of("one")), win_of(8, "test", &launch_of("two"))]);
    s.applied(WindowEvent::Activate(8).into());
    s.applied(Event::Device(DeviceKey::WindowDown));
    s.applied(Event::Device(DeviceKey::BottomView));
    assert_eq!(s.applied(WindowEvent::Activate(7).into()), vec![Effect::PadRoute(crate::PadRoute::Game)]);
    assert_eq!(s.session().top_window().map(|w| w.id), Some(7));
    assert_eq!(s.applied(WindowEvent::Activate(8).into()), vec![Effect::Watch(crate::Watch::Metrics, false), Effect::PadActivity(true)]);
    assert_eq!(s.session().bottom_window().map(|w| w.id), Some(8));
    assert_eq!(s.session().keyboard_focus(), Some(8));
    assert_eq!(s.session().top_window().map(|w| w.id), Some(7));
    assert!(s.bottom().shows_windows(s.session()));
}

/// Window 7 of launch "top" in front and window 8 of launch "bottom" below.
fn top_and_bottom(s: &mut crate::State) {
    s.observed(vec![win_of(7, "top", &launch_of("top")), win_of(8, "bottom", &launch_of("bottom"))]);
    s.applied(WindowEvent::Activate(8).into());
    s.applied(Event::Device(DeviceKey::WindowDown));
    s.applied(WindowEvent::Activate(7).into());
}

#[test]
fn keyboard_focus_is_independent_of_both_screens_selected_content() {
    let mut s = state();
    top_and_bottom(&mut s);
    assert_eq!(s.session().keyboard_focus(), Some(7));
    for (target, effects) in [(Some(8), vec![Effect::PadActivity(true)]), (None, vec![]), (Some(7), vec![Effect::PadActivity(false)])] {
        assert_eq!(s.applied(WindowEvent::KeyboardFocus(target).into()), effects);
        assert_eq!(s.session().keyboard_focus(), target);
        assert_eq!(s.session().top_window().map(|w| w.id), Some(7));
        assert_eq!(s.session().bottom_window().map(|w| w.id), Some(8));
    }
    assert!(s.applied(WindowEvent::KeyboardFocus(Some(99)).into()).is_empty());
    assert_eq!(s.session().keyboard_focus(), Some(7));
    s.applied(WindowEvent::KeyboardFocus(Some(8)).into());
    s.applied(WindowEvent::Closed(8).into());
    assert_eq!(s.session().keyboard_focus(), None);
    assert_eq!(s.session().top_window().map(|w| w.id), Some(7));
    assert_eq!(s.session().bottom_window(), None);
}

#[test]
fn home_cancels_a_launchs_late_request_to_take_the_front() {
    let mut s = state();
    let instance = launch(&mut s, "zelda-lttp");
    assert!(s.session().front_launch().is_some());
    s.applied(Event::Home);
    assert!(s.session().front_launch().is_none());
    s.applied(WindowEvent::Opened(win_of(7, "zelda3", &instance)).into());
    assert_eq!(s.session().front(), crate::session::running::Front::Shelf);
}

#[test]
fn lifecycle_open_and_close_follow_launch_ownership() {
    let mut s = state();
    s.applied(Event::tick(100));
    let instance = launch(&mut s, "zelda-lttp");
    let window = win_of(7, "zelda3", &instance);
    assert_eq!(s.applied(WindowEvent::Opened(window.clone()).into()), vec![Effect::PadRoute(crate::PadRoute::Game)]);
    assert_eq!(s.session().windows_of("zelda-lttp").map(|w| &w.facts).collect::<Vec<_>>(), vec![&window]);
    assert_eq!(s.session().top_window().map(|w| w.id), Some(7));
    assert!(s.session().front_launch().is_none());
    assert_eq!(s.applied(WindowEvent::Closed(7).into()), vec![Effect::PadRoute(crate::PadRoute::Ui)]);
    assert!(s.session().windows_of("zelda-lttp").next().is_none());
    assert!(s.session().shelf_in_front());
}

#[test]
fn activating_another_window_cancels_a_launchs_late_request_to_take_the_front() {
    let mut s = state();
    s.observed(vec![win_of(7, "other", &launch_of("other"))]);
    s.applied(WindowEvent::Activate(7).into());
    let instance = launch(&mut s, "zelda-lttp");
    assert!(s.session().front_launch().is_some());
    assert!(s.applied(WindowEvent::Activate(7).into()).is_empty());
    assert!(s.session().front_launch().is_none());
    s.applied(WindowEvent::Opened(win_of(8, "zelda3", &instance)).into());
    assert_eq!(s.session().top_window().map(|w| w.id), Some(7));
}

#[test]
fn activation_thaws_a_suspended_owner_before_selecting_its_window() {
    let mut s = state();
    let instance = launch(&mut s, "zelda-lttp");
    s.observed(vec![win_of(7, "zelda3", &instance)]);
    s.applied(Event::Entry("zelda-lttp".into(), crate::EntryAct::Suspend));
    assert!(s.session().suspended("zelda-lttp"));
    // The client must be able to draw when the compositor shows it.
    assert_eq!(
        s.applied(WindowEvent::Activate(7).into()),
        vec![Effect::Freeze { slice: Instance::slice_of(&clamshell_library::key_of("zelda-lttp")), frozen: false }, Effect::PadRoute(crate::PadRoute::Game),]
    );
    assert!(!s.session().suspended("zelda-lttp"));
    assert_eq!(s.session().top_window().map(|w| w.id), Some(7));
    assert_eq!(s.session().keyboard_focus(), Some(7));
    assert!(s.applied(WindowEvent::Activate(7).into()).iter().all(|effect| !matches!(effect, Effect::Freeze { .. })));
}

#[test]
fn lifecycle_title_applies_a_bottom_rule_once() {
    use crate::session::below::{WindowRule, WindowRules};
    let rules = WindowRules::from_list(vec![WindowRule { owner: "zelda-lttp".into(), app_id: "zelda3".into(), title: Some("second".into()) }]);
    let mut s = started(crate::Init { window_rules: rules, ..init() });
    s.applied(Event::tick(100));
    let instance = launch(&mut s, "zelda-lttp");
    s.applied(WindowEvent::Opened(win_of(7, "zelda3", &instance)).into());
    let event = || WindowEvent::Title { window: 7, title: "second".into() }.into();
    let effects = s.applied(event());
    assert_eq!(effects, vec![Effect::Watch(crate::Watch::Metrics, false), Effect::PadRoute(crate::PadRoute::Ui)]);
    assert_eq!(s.session().bottom_window().map(|w| w.id), Some(7));
    assert!(s.session().shelf_in_front());
    assert_eq!(s.session().windows().first().map(|w| w.title.as_str()), Some("second"));
    assert!(s.bottom().shows_windows(s.session()));
    assert!(s.applied(event()).is_empty());
}

#[test]
fn a_launchs_window_and_its_rule_are_placed_before_its_unit_is_listed() {
    use crate::session::below::{WindowRule, WindowRules};
    let rules = WindowRules::from_list(vec![WindowRule { owner: "zelda-lttp".into(), app_id: "game".into(), title: None }]);
    let mut s = started(crate::Init { window_rules: rules, ..init() });
    let group = Group::entry(&clamshell_library::key_of("zelda-lttp"));
    let unit = Instance { key: clamshell_library::key_of("zelda-lttp"), secs: 1, n: 0 };
    assert_eq!(s.applied(WindowEvent::Opened(win_of(7, "game", &unit)).into()), vec![Effect::Watch(crate::Watch::Metrics, false)]);
    assert_eq!(s.session().place(7), Some(WindowPlace::Bottom));
    assert_eq!(s.session().window(7).and_then(|w| w.group.clone()), Some(group.clone()));
    assert!(s.session().units().is_empty());
    assert_eq!(s.session().bottom_window().map(|w| w.id), Some(7));
}

#[test]
fn stale_identity_facts_cannot_undo_a_placement() {
    let mut s = state();
    let instance = launch(&mut s, "zelda-lttp");
    let before = win_of(7, "game", &instance);
    s.applied(WindowEvent::Opened(before.clone()).into());
    s.applied(Event::Device(DeviceKey::WindowDown));
    let identity = WindowFacts { pids: vec![100, 50], ..before.clone() };
    assert!(s.applied(WindowEvent::Changed(identity).into()).is_empty());
    let window = s.session().window(7);
    assert_eq!(s.session().place(7), Some(WindowPlace::Bottom));
    assert_eq!(window.and_then(|w| w.group.clone()), Some(group_of(&instance)));
    assert_eq!(window.map(|w| w.pids.as_slice()), Some(&[100, 50][..]));
    assert_eq!(s.session().bottom_window().map(|w| w.id), Some(7));
}

#[test]
fn a_hidden_group_keeps_its_selection_and_closing_it_selects_the_next_window() {
    let mut s = state();
    let one = launch(&mut s, "zelda-lttp");
    let two = launch(&mut s, "ff6");
    s.observed(vec![win_of(7, "game", &one), win_of(8, "game", &one), win_of(9, "other", &two)]);
    s.applied(WindowEvent::Activate(7).into());
    show(&mut s, "ff6");
    assert_eq!(s.session().top_window().map(|w| w.id), Some(9));
    show(&mut s, "zelda-lttp");
    assert_eq!(s.session().top_window().map(|w| w.id), Some(7));
    s.applied(WindowEvent::Closed(7).into());
    assert_eq!(s.session().top_window().map(|w| w.id), Some(8));
}

#[test]
fn a_dialog_inherits_its_parent_and_survives_the_parent_closing() {
    let mut s = state();
    let instance = launch(&mut s, "zelda-lttp");
    s.applied(WindowEvent::Opened(win_of(7, "game", &instance)).into());
    let dialog = WindowFacts { parent: Some(7), unit: Some("portal.service".into()), ..win(8, "dialog") };
    assert!(s.applied(WindowEvent::Opened(dialog).into()).is_empty());
    assert_eq!(s.session().place(8), Some(WindowPlace::Top(Some(&group_of(&instance)))));
    assert_eq!(s.session().top_window().map(|w| w.id), Some(8));
    assert_eq!(s.session().window(8).and_then(|w| s.session().running_of(w)).map(|r| &*r.id), Some("zelda-lttp"));
    assert!(s.applied(WindowEvent::Closed(7).into()).is_empty());
    assert_eq!(s.session().top_window().map(|w| w.id), Some(8));
}

#[test]
/// A dialog of the window below (a file picker of an emulator's second-screen window) shows on the top
/// screen, where it can be answered, while its parent stays below; closed, the parent is still below.
fn a_dialog_of_a_window_below_shows_on_top_while_its_parent_stays_below() {
    let mut s = state();
    let instance = launch(&mut s, "zelda-lttp");
    s.applied(WindowEvent::Opened(win_of(7, "game", &instance)).into());
    s.applied(Event::Device(DeviceKey::WindowDown));
    let dialog = WindowFacts { parent: Some(7), ..win(8, "dialog") };
    s.applied(WindowEvent::Opened(dialog).into());
    assert_eq!(s.session().top_window().map(|w| w.id), Some(8));
    assert_eq!(s.session().keyboard_focus(), Some(8));
    assert_eq!(s.session().bottom_window().map(|w| w.id), Some(7), "the parent stays below");
    s.applied(WindowEvent::Closed(8).into());
    assert_eq!(s.session().bottom_window().map(|w| w.id), Some(7));
    assert_ne!(s.session().top_window().map(|w| w.id), Some(7));
}

#[test]
fn windows_know_their_instance() {
    let w = |unit: Option<&str>, app_id: &str| WindowFacts { unit: unit.map(Into::into), app_id: app_id.into(), ..WindowFacts::default() };
    let terminal = Instance { key: "foot_1".into(), secs: 1234, n: 7 };
    let foot = w(Some("clamshell-run-foot_1-1234-7.scope"), "foot");
    assert_eq!(foot.instance(), Some(terminal.clone()));
    // Firefox started from that terminal belongs to the terminal.
    assert_eq!(w(Some("clamshell-run-foot_1-1234-7.scope"), "firefox").instance(), Some(terminal));
    assert_eq!(w(Some("app-steam.scope"), "steam_app_620").instance(), None, "Steam's: placed by the program");
    assert_eq!(w(Some("app-steam.scope"), "steam").instance(), None, "not ours: left alone");
    // The same without allocating.
    for w in [foot, w(Some("app-steam.scope"), "steam"), w(None, "foot")] {
        assert_eq!(w.launched(), w.instance().is_some(), "{w:?}");
    }
}

#[test]
fn touching_a_home_arrow_consumes_the_release_but_direct_home_still_shows_shelf() {
    let mut s = state();
    let one = launch_of("one");
    s.observed(vec![win_of(7, "one", &one), win_of(8, "one", &one)]);
    s.applied(WindowEvent::Activate(7).into());
    s.applied(Event::HomeHeld(true));
    assert!(s.applied(Event::Device(DeviceKey::WindowNext)).is_empty());
    s.applied(Event::HomeHeld(false));
    assert!(s.applied(Event::HomeTap).is_empty());
    assert_eq!(s.session().top_window().map(|w| w.id), Some(8));
    s.applied(Event::Home);
    assert!(s.session().shelf_in_front());
    s.applied(WindowEvent::Activate(8).into());
    s.applied(Event::HomeHeld(true));
    s.applied(Event::HomeHeld(false));
    s.applied(Event::HomeTap);
    assert!(s.session().shelf_in_front());
}

#[test]
fn presentation_facts_and_colour_do_not_change_placement_or_selection() {
    use crate::colour::{ColourDescription, ColourPrimaries, TransferFunction};
    use crate::presentation::{WindowLayout, WindowLimits, WindowRole};
    let mut s = state();
    let one = launch_of("one");
    let window = win_of(7, "one", &one);
    s.applied(WindowEvent::Opened(window.clone()).into());
    s.applied(WindowEvent::Activate(7).into());
    let colour = ColourDescription {
        primaries: ColourPrimaries::Bt2020,
        transfer: TransferFunction::Pq,
        luminance: Some(crate::colour::ColourLuminance { min: 10, max: 1000, reference: 203 }),
        target: None,
    };
    let changed =
        WindowFacts { limits: WindowLimits { min: (400, 300), max: (400, 300) }, role: WindowRole::Utility, colour: Some(Arc::new(colour)), ..window };
    for _ in 0..2 {
        assert!(s.applied(WindowEvent::Changed(changed.clone()).into()).is_empty());
        let current = s.session().window(7);
        assert_eq!(s.session().place(7), Some(WindowPlace::Top(Some(&group_of(&one)))));
        assert_eq!(current.and_then(|w| w.colour.as_deref()).copied(), Some(colour));
        assert_eq!(s.session().top_window().map(|w| w.id), Some(7));
        assert_eq!(s.session().keyboard_focus(), Some(7));
        assert_eq!(s.window_presentation(7).map(|p| p.layout), Some(WindowLayout::Centred));
    }
}

#[test]
fn remembering_a_bottom_rule_does_not_move_existing_siblings_of_other_launches() {
    let mut s = state();
    let instance = crate::Instance { key: clamshell_library::key_of("desktop:foot"), secs: 100, n: 0 };
    let another = crate::Instance { key: clamshell_library::key_of("desktop:foot"), secs: 200, n: 1 };
    s.applied(Event::Units(vec![instance.clone(), another.clone()]));
    s.observed(vec![win_of(7, "foot", &instance), win_of(8, "foot", &another)]);
    s.applied(WindowEvent::Activate(7).into());
    s.applied(Event::Device(DeviceKey::WindowDown));
    assert_eq!(s.session().bottom_window().map(|w| w.id), Some(7));
    assert_eq!(s.session().top_window().map(|w| w.id), Some(8));
    // A later matching window does use that rule and displaces the bottom occupant.
    s.applied(WindowEvent::Opened(win_of(9, "foot", &another)).into());
    assert_eq!(s.session().bottom_window().map(|w| w.id), Some(9));
    assert_eq!(s.session().place(7), Some(WindowPlace::Top(Some(&group_of(&instance)))));
    assert_eq!(s.session().top_window().map(|w| w.id), Some(8));
}

#[test]
fn displacing_another_programs_bottom_window_does_not_activate_it() {
    let mut s = state();
    let (one, two) = (launch_of("one"), launch_of("two"));
    s.observed(vec![win_of(7, "one", &one), win_of(8, "two", &two), win_of(9, "one", &one)]);
    s.applied(WindowEvent::Activate(8).into());
    s.applied(Event::Device(DeviceKey::WindowDown));
    s.applied(WindowEvent::Activate(7).into());
    s.applied(Event::Device(DeviceKey::WindowDown));
    // The returned foreign program receives no focus.
    assert_eq!(s.session().place(8), Some(WindowPlace::Top(Some(&group_of(&two)))));
    assert_eq!(s.session().bottom_window().map(|w| w.id), Some(7));
    assert_eq!(s.session().top_window().map(|w| w.id), Some(9));
    assert_eq!(s.session().keyboard_focus(), Some(9));
}

#[test]
fn top_window_switching_wraps_within_the_program_and_skips_the_bottom_slot() {
    let mut s = state();
    let one = launch_of("one");
    s.observed(vec![win_of(7, "one", &one), win_of(8, "one", &one), win_of(9, "other", &launch_of("other"))]);
    s.applied(WindowEvent::Activate(7).into());
    assert!(s.session().has_other_top_window());
    for (action, id) in [(DeviceKey::WindowNext, 8), (DeviceKey::WindowNext, 7), (DeviceKey::WindowPrevious, 8)] {
        assert!(s.applied(Event::Device(action)).is_empty());
        assert_eq!(s.session().top_window().map(|w| w.id), Some(id));
    }
    s.applied(Event::Device(DeviceKey::WindowDown));
    assert!(!s.session().has_other_top_window());
    assert!(s.applied(Event::Device(DeviceKey::WindowNext)).is_empty());
    assert_eq!(s.session().top_window().map(|w| w.id), Some(7));
    assert_eq!(s.session().bottom_window().map(|w| w.id), Some(8));
}

#[test]
fn pad_activity_returns_keyboard_focus_to_top_without_moving_windows() {
    let mut s = state();
    top_and_bottom(&mut s);
    assert_eq!(s.applied(WindowEvent::KeyboardFocus(Some(8)).into()), vec![Effect::PadActivity(true)]);
    // Focus returns to the top window and the activity watch is disarmed; another focus loss can then
    // arm it.
    assert_eq!(s.applied(Event::PadActive), vec![Effect::PadActivity(false)]);
    assert_eq!(s.session().keyboard_focus(), Some(7));
    assert_eq!(s.session().bottom_window().map(|w| w.id), Some(8));
    assert!(s.applied(Event::PadActive).is_empty());
    for held in [true, true, false, false] {
        assert!(s.applied(Event::HomeHeld(held)).is_empty());
        assert_eq!(s.session().home_held(), held);
    }
}

#[test]
fn activating_a_program_reveals_its_bottom_window_without_moving_or_focusing_it() {
    let mut s = state();
    let instance = launch(&mut s, "zelda-lttp");
    s.observed(vec![win_of(7, "game", &instance), win_of(8, "second", &instance)]);
    s.applied(WindowEvent::Activate(8).into());
    s.applied(Event::Device(DeviceKey::WindowDown));
    s.applied(Event::Device(DeviceKey::BottomView));
    assert!(!s.bottom().shows_windows(s.session()));
    // Only the tab its window covers stops being watched.
    assert_eq!(show(&mut s, "zelda-lttp"), vec![Effect::Watch(crate::Watch::Metrics, false)]);
    assert_eq!(s.session().keyboard_focus(), Some(7));
    assert_eq!(s.session().top_window().map(|w| w.id), Some(7));
    assert_eq!(s.session().bottom_window().map(|w| w.id), Some(8));
    assert!(s.bottom().shows_windows(s.session()));
    s.applied(WindowEvent::Closed(8).into());
    assert_eq!(s.session().top_window().map(|w| w.id), Some(7));
    assert_eq!(s.session().bottom_window(), None);
    assert!(!s.bottom().shows_windows(s.session()));
}

#[test]
fn moving_the_top_window_does_not_require_keyboard_focus_there() {
    let mut s = state();
    top_and_bottom(&mut s);
    s.applied(WindowEvent::KeyboardFocus(Some(8)).into());
    assert_eq!(s.session().top_window().map(|w| w.id), Some(7));
    s.applied(Event::Device(DeviceKey::WindowDown));
    assert_eq!(s.session().bottom_window().map(|w| w.id), Some(7));
    assert_eq!(s.session().place(8), Some(WindowPlace::Top(Some(&group_of(&launch_of("bottom"))))));
    assert!(s.session().shelf_in_front());
}

/// A dialog that closes brings back its parent, not the window its group shows last: the parent is
/// what the user was in when the dialog opened.
#[test]
fn a_dialogs_close_brings_back_its_parent() {
    let mut s = state();
    let one = launch_of("one");
    s.observed(vec![win_of(7, "one", &one), win_of(8, "one", &one)]);
    s.applied(WindowEvent::Activate(7).into());
    s.applied(WindowEvent::Opened(WindowFacts { parent: Some(7), ..win_of(9, "one", &one) }).into());
    assert_eq!(s.session().top_window().map(|w| w.id), Some(9));
    s.applied(WindowEvent::Closed(9).into());
    assert_eq!(s.session().top_window().map(|w| w.id), Some(7), "the parent, not 8");
    assert_eq!(s.session().keyboard_focus(), Some(7));
}

/// A window sent down by its rule while it has the keyboard gives the keyboard to the window in front on
/// the top screen, which keeps it (see [`WindowCommand::Down`]). Here the compositor reports the keyboard
/// on a window that opened behind, in the batch that opened it, before its rule sends it down.
#[test]
fn a_window_sent_down_gives_the_keyboard_to_the_top_window() {
    use crate::session::below::{WindowRule, WindowRules};
    let rules = WindowRules::from_list(vec![WindowRule { owner: "zelda-lttp".into(), app_id: "zelda3".into(), title: None }]);
    let mut s = started(crate::Init { window_rules: rules, ..init() });
    s.applied(WindowEvent::Opened(foot()).into());
    assert_eq!(s.session().top_window().map(|w| w.id), Some(1001));
    let zelda = Instance { key: clamshell_library::key_of("zelda-lttp"), secs: 1, n: 0 };
    s.applied(Event::WindowBatch(vec![WindowEvent::Opened(win_of(9, "zelda3", &zelda)), WindowEvent::KeyboardFocus(Some(9))]));
    assert_eq!(s.session().bottom_window().map(|w| w.id), Some(9));
    assert_eq!(s.session().top_window().map(|w| w.id), Some(1001));
    assert_eq!(s.session().keyboard_focus(), Some(1001));
}

/// Whether a window is X11 is known when it opens; a later `Changed` doesn't carry it, so it is kept.
#[test]
fn changed_facts_keep_a_window_x11() {
    let mut s = state();
    let window = WindowFacts { x11: true, ..win_of(7, "one", &launch_of("one")) };
    s.applied(WindowEvent::Opened(window.clone()).into());
    s.applied(WindowEvent::Changed(WindowFacts { x11: false, pids: vec![100], ..window }).into());
    let kept = s.session().window(7);
    assert_eq!(kept.map(|w| (w.x11, w.pids.clone())), Some((true, vec![100])));
}

/// The window below closing empties the bottom screen's slot: a new window that reuses its id isn't
/// taken for the window below.
#[test]
fn closing_the_window_below_empties_its_slot() {
    let mut s = state();
    top_and_bottom(&mut s);
    s.applied(WindowEvent::Closed(8).into());
    assert!(!s.session().below(8));
    assert!(s.session().selections_valid());
    s.applied(WindowEvent::Opened(win(8, "foot")).into());
    assert!(matches!(s.session().place(8), Some(WindowPlace::Top(_))), "opened on top, not below");
    assert_eq!(s.session().bottom_window(), None);
}
