// These older tests assert shortcut actions. Gesture tests below also assert held/activity events.
fn actions(out: &super::Out) -> Vec<clamshell_state::Event> {
    out.events.iter().filter(|e| !matches!(e, clamshell_state::Event::HomeHeld(_) | clamshell_state::Event::PadActive)).cloned().collect()
}

use super::*;

fn router(route: PadRoute) -> (PadRouter, Out) {
    let ranges = Ranges { stick: Range::STICK, trigger: Range { min: 0, max: 1023 } };
    let axes = vec![Axis { code: AbsoluteAxisCode::ABS_X, value: 1000, rest: 0 }, Axis { code: AbsoluteAxisCode::ABS_RZ, value: 0, rest: 0 }];
    let mut out = Out::default();
    let router = PadRouter::new(ranges, PadSnapshot::new(axes), route, Keymap::default(), &mut out);
    (router, out)
}

fn key(code: KeyCode, press: Press) -> Raw {
    Raw::key(code, press)
}

/// The keys down after these events, as the kernel would keep them, by code.
fn held(events: &[Raw]) -> Vec<KeyCode> {
    let mut down = Vec::new();
    for (k, press) in events.iter().filter_map(|e| e.as_key()) {
        down.retain(|&d| d != k);
        if press.held() {
            down.push(k);
        }
    }
    down.sort_by_key(|k| k.0);
    down
}

#[test]
fn game_activity_is_reported_once_per_keyboard_focus_loss() {
    let (mut r, mut out) = router(PadRoute::Game);
    let now = Instant::now();
    out.events.clear();
    for _ in 0..100 {
        r.event(key(KeyCode::BTN_SOUTH, Press::Down), now, &mut out);
        r.event(key(KeyCode::BTN_SOUTH, Press::Up), now, &mut out);
        r.event(Raw::abs(AbsoluteAxisCode::ABS_X, 30000), now, &mut out);
    }
    assert!(out.events.is_empty(), "normal play sends no state activity events");
    r.watch_activity(true);
    for _ in 0..100 {
        r.event(Raw::abs(AbsoluteAxisCode::ABS_X, 30000), now, &mut out);
        r.event(key(KeyCode::BTN_SOUTH, Press::Down), now, &mut out);
        r.event(key(KeyCode::BTN_SOUTH, Press::Up), now, &mut out);
    }
    assert_eq!(out.events, vec![Event::PadActive]);
    r.watch_activity(false);
    r.watch_activity(true);
    r.event(key(KeyCode::BTN_SOUTH, Press::Up), now, &mut out);
    r.event(Raw::abs(AbsoluteAxisCode::ABS_X, 0), now, &mut out);
    assert_eq!(out.events, vec![Event::PadActive], "release and rest do not take focus");
    r.event(key(KeyCode::BTN_SOUTH, Press::Down), now, &mut out);
    assert_eq!(out.events, vec![Event::PadActive, Event::PadActive]);
}

#[test]
fn events_dropped_are_made_up_from_the_controller_as_it_is() {
    let (mut r, mut out) = router(PadRoute::Game);
    let now = Instant::now();
    r.event(key(KeyCode::BTN_SOUTH, Press::Down), now, &mut out);
    let mut events = Vec::new();
    r.resync(&[KeyCode::BTN_EAST], &[(AbsoluteAxisCode::ABS_X, 1000), (AbsoluteAxisCode::ABS_RZ, 500)], &mut events);
    assert_eq!(events, [key(KeyCode::BTN_SOUTH, Press::Up), key(KeyCode::BTN_EAST, Press::Down), Raw::abs(AbsoluteAxisCode::ABS_RZ, 500), Raw::REPORT]);
    for e in events {
        r.event(e, now, &mut out);
    }
    let mut again = Vec::new();
    r.resync(&[KeyCode::BTN_EAST], &[(AbsoluteAxisCode::ABS_X, 1000), (AbsoluteAxisCode::ABS_RZ, 500)], &mut again);
    assert_eq!(again, [], "in step");
}

#[test]
fn leaving_the_pad_mouse_lets_go_of_its_buttons() {
    let (mut r, mut out) = router(PadRoute::Mouse);
    let now = Instant::now();
    r.event(key(KeyCode::BTN_SOUTH, Press::Down), now, &mut out);
    r.event(Raw::abs(AbsoluteAxisCode::ABS_HAT0Y, -1), now, &mut out);
    r.event(Raw::abs(AbsoluteAxisCode::ABS_RZ, 1000), now, &mut out);
    assert_eq!(held(&out.mouse), vec![KeyCode::KEY_UP, KeyCode::BTN_LEFT]);
    r.set_route(PadRoute::Ui, &mut out);
    assert_eq!(held(&out.mouse), vec![], "no stuck keys");
    // Back on the pad mouse, the trigger is still held, but it counts only once pressed again.
    r.set_route(PadRoute::Mouse, &mut out);
    r.event(Raw::abs(AbsoluteAxisCode::ABS_RZ, 1000), now, &mut out);
    assert_eq!(held(&out.mouse), vec![KeyCode::BTN_LEFT]);
}

#[test]
fn leaving_typing_lets_go_of_its_keys() {
    let (mut r, mut out) = router(PadRoute::Keys);
    let now = Instant::now();
    r.event(key(KeyCode::BTN_EAST, Press::Down), now, &mut out);
    r.event(key(KeyCode::BTN_EAST, Press::Repeat), now, &mut out);
    r.event(Raw::abs(AbsoluteAxisCode::ABS_HAT0X, 1), now, &mut out);
    assert_eq!(held(&out.mouse), vec![KeyCode::KEY_BACKSPACE, KeyCode::KEY_RIGHT]);
    r.set_route(PadRoute::Game, &mut out);
    assert_eq!(held(&out.mouse), vec![]);
}

#[test]
fn leaving_navigation_stops_a_held_direction() {
    let (mut r, mut out) = router(PadRoute::Ui);
    let t0 = Instant::now();
    r.event(Raw::abs(AbsoluteAxisCode::ABS_HAT0Y, 1), t0, &mut out);
    assert_eq!(actions(&out), vec![Event::Pad(PadInput::Down)]);
    assert!(r.tick(t0, &mut out).is_some(), "repeats while held");
    r.set_route(PadRoute::Mouse, &mut out);
    r.set_route(PadRoute::Ui, &mut out);
    out.events.clear();
    assert_eq!(r.tick(t0 + std::time::Duration::from_secs(1), &mut out), None);
    assert!(actions(&out).is_empty(), "no repeat after coming back");
}

#[test]
fn the_virtual_pad_is_blanked_away_from_the_game_and_restored_after() {
    let (mut r, mut out) = router(PadRoute::Game);
    assert!(out.pad.is_empty(), "the game sees the controller");
    let now = Instant::now();
    r.event(key(KeyCode::BTN_TL, Press::Down), now, &mut out);
    assert_eq!(out.pad, vec![key(KeyCode::BTN_TL, Press::Down), Raw::REPORT], "passed through");
    out.pad.clear();
    r.set_route(PadRoute::Mouse, &mut out);
    let axis = |v| Raw::abs(AbsoluteAxisCode::ABS_X, v);
    let rz = Raw::abs(AbsoluteAxisCode::ABS_RZ, 0);
    assert_eq!(out.pad, vec![key(KeyCode::BTN_TL, Press::Up), axis(0), Raw::REPORT], "at rest (the trigger was)");
    out.pad.clear();
    r.set_route(PadRoute::Keys, &mut out);
    assert!(out.pad.is_empty(), "still away");
    r.set_route(PadRoute::Game, &mut out);
    let hat = |axis| Raw::abs(axis, 0);
    let (hx, hy) = (hat(AbsoluteAxisCode::ABS_HAT0X), hat(AbsoluteAxisCode::ABS_HAT0Y));
    assert_eq!(out.pad, vec![key(KeyCode::BTN_TL, Press::Down), axis(1000), rz, hx, hy, Raw::REPORT], "as it is (the D-pad's hat too)");
}

#[test]
fn home_chords_use_the_right_stick_and_not_select_or_the_dpad() {
    let (mut r, mut out) = router(PadRoute::Game);
    let now = Instant::now();
    r.event(key(KeyCode::BTN_MODE, Press::Down), now, &mut out);
    for code in [KeyCode::BTN_SELECT, KeyCode::BTN_DPAD_UP, KeyCode::BTN_DPAD_DOWN] {
        r.event(key(code, Press::Down), now, &mut out);
        r.event(key(code, Press::Up), now, &mut out);
    }
    for code in [KeyCode::BTN_THUMBR, KeyCode::BTN_START] {
        r.event(key(code, Press::Down), now, &mut out);
        r.event(key(code, Press::Up), now, &mut out);
    }
    r.event(key(KeyCode::BTN_MODE, Press::Up), now, &mut out);
    assert_eq!(actions(&out), vec![Event::Device(DeviceKey::BottomView), Event::Device(DeviceKey::BottomPower)]);
    assert!(!out.pad.iter().any(|raw| {
        raw.as_key().is_some_and(|(key, _)| matches!(key, KeyCode::BTN_SELECT | KeyCode::BTN_THUMBR | KeyCode::BTN_DPAD_UP | KeyCode::BTN_DPAD_DOWN))
    }));
}

#[test]
fn home_with_b_closes_the_window_held_quits_and_the_game_sees_no_b() {
    let (mut r, mut out) = router(PadRoute::Game);
    let now = Instant::now();
    let b = |out: &Out| out.pad.iter().any(|e| e.as_key().is_some_and(|(k, _)| k == KeyCode::BTN_EAST));
    // Letting go soon closes the window.
    r.event(key(KeyCode::BTN_MODE, Press::Down), now, &mut out);
    r.event(key(KeyCode::BTN_EAST, Press::Down), now, &mut out);
    assert_eq!(r.tick(now, &mut out), Some(now + QUIT_HOLD), "ticked at the hold's end");
    r.event(key(KeyCode::BTN_EAST, Press::Up), now + QUIT_HOLD / 2, &mut out);
    r.event(key(KeyCode::BTN_MODE, Press::Up), now + QUIT_HOLD / 2, &mut out);
    assert_eq!(actions(&out), vec![Event::Device(DeviceKey::CloseWindow)], "not Home as well");
    // Holding quits once the hold is reached, and letting go after that does nothing more.
    out.events.clear();
    r.event(key(KeyCode::BTN_MODE, Press::Down), now, &mut out);
    r.event(key(KeyCode::BTN_EAST, Press::Down), now, &mut out);
    r.tick(now + QUIT_HOLD, &mut out);
    r.event(key(KeyCode::BTN_EAST, Press::Up), now + QUIT_HOLD * 2, &mut out);
    r.event(key(KeyCode::BTN_MODE, Press::Up), now + QUIT_HOLD * 2, &mut out);
    assert_eq!(actions(&out), vec![Event::Device(DeviceKey::Quit)]);
    assert!(!b(&out), "the game saw no B: {:?}", out.pad);
}

#[test]
fn home_with_rb_is_a_screenshot_and_the_game_sees_neither() {
    let (mut r, mut out) = router(PadRoute::Game);
    let now = Instant::now();
    r.event(key(KeyCode::BTN_MODE, Press::Down), now, &mut out);
    r.event(key(KeyCode::BTN_TR, Press::Down), now, &mut out);
    r.event(key(KeyCode::BTN_MODE, Press::Up), now, &mut out);
    r.event(key(KeyCode::BTN_TR, Press::Up), now, &mut out);
    assert_eq!(actions(&out), vec![Event::Device(DeviceKey::Screenshot)], "not Home as well");
    assert!(out.pad.is_empty(), "RB stays ClamShell's: {:?}", out.pad);
    // RB on its own is the game's again.
    r.event(key(KeyCode::BTN_TR, Press::Down), now, &mut out);
    assert_eq!(out.pad, vec![key(KeyCode::BTN_TR, Press::Down), Raw::REPORT]);
}

const BACK: KeyCode = KeyCode::BTN_BACK;

#[test]
fn home_then_back_with_a_game_in_front_is_its_guide_while_both_are_held() {
    let (mut r, mut out) = router(PadRoute::Game);
    let t0 = Instant::now();
    r.event(key(KeyCode::BTN_MODE, Press::Down), t0, &mut out);
    assert_eq!(r.tick(t0 + std::time::Duration::from_secs(2), &mut out), None, "held long: nothing");
    assert!(out.pad.is_empty());
    r.event(key(BACK, Press::Down), t0, &mut out);
    assert_eq!(out.pad, vec![key(KeyCode::BTN_MODE, Press::Down), Raw::REPORT]);
    // RB pressed in the meantime is the game's, as a Steam chord, not a screenshot.
    r.event(key(KeyCode::BTN_TR, Press::Down), t0, &mut out);
    assert_eq!(held(&out.pad), vec![KeyCode::BTN_TR, KeyCode::BTN_MODE]);
    // Back let go first puts Guide up. Home's release isn't Home, and Back's release isn't the game's.
    r.event(key(BACK, Press::Up), t0, &mut out);
    assert_eq!(held(&out.pad), vec![KeyCode::BTN_TR]);
    r.event(key(KeyCode::BTN_MODE, Press::Up), t0, &mut out);
    assert!(actions(&out).is_empty(), "not Home: {:?}", out.events);
    assert!(!out.pad.iter().any(|e| e.as_key().is_some_and(|(k, _)| k == BACK)), "Back never reaches the game");
    // Home let go first puts Guide up, and Back's release after that stays ours.
    out = Out::default();
    r.event(key(KeyCode::BTN_MODE, Press::Down), t0, &mut out);
    r.event(key(BACK, Press::Down), t0, &mut out);
    r.event(key(KeyCode::BTN_MODE, Press::Up), t0, &mut out);
    assert!(!held(&out.pad).contains(&KeyCode::BTN_MODE));
    let before = out.pad.len();
    r.event(key(BACK, Press::Up), t0, &mut out);
    assert_eq!(out.pad.len(), before);
    assert!(actions(&out).is_empty());
}

#[test]
fn home_with_another_button_is_a_combination_not_home() {
    let (mut r, mut out) = router(PadRoute::Game);
    let t0 = Instant::now();
    r.event(key(KeyCode::BTN_MODE, Press::Down), t0, &mut out);
    r.event(key(KeyCode::BTN_SOUTH, Press::Down), t0, &mut out);
    r.event(key(KeyCode::BTN_MODE, Press::Up), t0, &mut out);
    r.event(key(KeyCode::BTN_SOUTH, Press::Up), t0, &mut out);
    assert!(actions(&out).is_empty(), "not Home");
    assert!(
        held(&out.pad).is_empty() && !out.pad.iter().any(|e| e.as_key().is_some_and(|(k, _)| k == KeyCode::BTN_SOUTH)),
        "A, pressed with Home held, never reaches the game: {:?}",
        out.pad
    );
    // Pressed again afterwards, it is the game's.
    r.event(key(KeyCode::BTN_SOUTH, Press::Down), t0, &mut out);
    assert_eq!(held(&out.pad), vec![KeyCode::BTN_SOUTH]);
    // Away from the game, Home then Back is neither Guide nor Home.
    let (mut r, mut out) = router(PadRoute::Ui);
    out.pad.clear();
    r.event(key(KeyCode::BTN_MODE, Press::Down), t0, &mut out);
    r.event(key(BACK, Press::Down), t0, &mut out);
    r.event(key(BACK, Press::Up), t0, &mut out);
    r.event(key(KeyCode::BTN_MODE, Press::Up), t0, &mut out);
    assert!(actions(&out).is_empty() && out.pad.is_empty(), "{out:?}");
    // Home alone, held long, is Home on release.
    r.event(key(KeyCode::BTN_MODE, Press::Down), t0, &mut out);
    r.event(key(KeyCode::BTN_MODE, Press::Up), t0 + std::time::Duration::from_secs(2), &mut out);
    assert_eq!(actions(&out), vec![Event::HomeTap]);
}

#[test]
fn back_on_clamshells_screens_is_back_and_nothing_with_the_mouse() {
    let (mut r, mut out) = router(PadRoute::Ui);
    out.pad.clear();
    r.event(key(BACK, Press::Down), Instant::now(), &mut out);
    r.event(key(BACK, Press::Up), Instant::now(), &mut out);
    assert_eq!(actions(&out), vec![Event::Pad(PadInput::Back)]);
    assert!(out.pad.is_empty());
    // While a keymap's edit screen listens, Back is an input to remap.
    let (mut r, mut out) = router(PadRoute::Listen);
    out.pad.clear();
    r.event(key(BACK, Press::Down), Instant::now(), &mut out);
    r.event(Raw::REPORT, Instant::now(), &mut out);
    assert_eq!(actions(&out), vec![Event::Listened(clamshell_state::keymap::Input::Button(clamshell_state::keymap::Button::Back))]);
    let (mut r, mut out) = router(PadRoute::Mouse);
    out = Out { pad: vec![], ..out };
    r.event(key(BACK, Press::Down), Instant::now(), &mut out);
    assert!(actions(&out).is_empty() && out.mouse.is_empty() && out.pad.is_empty());
}

#[test]
fn guide_is_let_go_when_the_pad_leaves_the_game() {
    let (mut r, mut out) = router(PadRoute::Game);
    let t0 = Instant::now();
    r.event(key(KeyCode::BTN_MODE, Press::Down), t0, &mut out);
    r.event(key(BACK, Press::Down), t0, &mut out);
    r.set_route(PadRoute::Ui, &mut out);
    assert!(!held(&out.pad).contains(&KeyCode::BTN_MODE), "no stuck Guide");
    out.events.clear();
    r.event(key(KeyCode::BTN_MODE, Press::Up), t0, &mut out);
    r.event(key(BACK, Press::Up), t0, &mut out);
    assert!(actions(&out).is_empty(), "letting go afterwards isn't Home, nor back");
}

#[test]
fn home_is_always_clamshells_and_view_with_the_mouse_or_typing() {
    let (mut r, mut out) = router(PadRoute::Game);
    let now = Instant::now();
    r.event(key(KeyCode::BTN_MODE, Press::Down), now, &mut out);
    assert!(actions(&out).is_empty(), "Home acts on release");
    r.event(key(KeyCode::BTN_MODE, Press::Up), now, &mut out);
    r.event(key(KeyCode::BTN_SELECT, Press::Down), now, &mut out);
    assert_eq!(actions(&out), vec![Event::HomeTap]);
    assert_eq!(out.pad, vec![key(KeyCode::BTN_SELECT, Press::Down), Raw::REPORT], "View is the game's");
    r.set_route(PadRoute::Mouse, &mut out);
    out = Out::default();
    r.event(key(KeyCode::BTN_SELECT, Press::Down), now, &mut out);
    r.event(key(KeyCode::BTN_SELECT, Press::Up), now, &mut out);
    assert_eq!(out, Out { events: vec![Event::Pad(PadInput::Switch)], ..Out::default() });
}

#[test]
fn home_flicks_once_and_neutralises_the_games_stick_until_it_returns() {
    let (mut r, mut out) = router(PadRoute::Game);
    for code in [AbsoluteAxisCode::ABS_RX, AbsoluteAxisCode::ABS_RY] {
        r.snapshot.axes.push(Axis { code, value: 0, rest: 0 });
    }
    let now = Instant::now();
    r.event(Raw::abs(AbsoluteAxisCode::ABS_RX, 30000), now, &mut out);
    out = Out::default();
    r.event(key(KeyCode::BTN_MODE, Press::Down), now, &mut out);
    assert_eq!(out.events, vec![Event::HomeHeld(true)]);
    assert!(out.pad.contains(&Raw::abs(AbsoluteAxisCode::ABS_RX, 0)), "Home must centre the stick the game already saw: {:?}", out.pad);
    out.pad.clear();
    // Already off-centre when Home went down: it must return before this can be a flick.
    for value in [32000, 0, -30000, -32000, 10000, -30000, 0, 30000] {
        r.event(Raw::abs(AbsoluteAxisCode::ABS_RX, value), now, &mut out);
    }
    assert_eq!(out.events, vec![Event::HomeHeld(true), Event::Device(DeviceKey::WindowPrevious), Event::Device(DeviceKey::WindowNext)]);
    assert!(!out.pad.iter().any(|raw| raw.as_abs().is_some_and(|(axis, value)| axis == AbsoluteAxisCode::ABS_RX && value != 0)));
    r.event(key(KeyCode::BTN_MODE, Press::Up), now, &mut out);
    r.event(Raw::abs(AbsoluteAxisCode::ABS_RX, 31000), now, &mut out);
    assert!(!out.pad.iter().any(|raw| raw.as_abs().is_some_and(|(axis, value)| axis == AbsoluteAxisCode::ABS_RX && value != 0)));
    assert_eq!(out.events.last(), Some(&Event::HomeHeld(false)));
    r.event(Raw::abs(AbsoluteAxisCode::ABS_RX, 0), now, &mut out);
    r.event(Raw::abs(AbsoluteAxisCode::ABS_RX, 30000), now, &mut out);
    assert!(out.pad.contains(&Raw::abs(AbsoluteAxisCode::ABS_RX, 30000)));
    assert_eq!(out.events.last(), Some(&Event::HomeHeld(false)));
}

#[test]
fn losing_the_controller_releases_home_for_the_state() {
    let (mut r, mut out) = router(PadRoute::Game);
    r.event(key(KeyCode::BTN_MODE, Press::Down), Instant::now(), &mut out);
    r.lose(&mut out);
    assert_eq!(out.events, vec![Event::HomeHeld(true), Event::HomeHeld(false)]);
}

/// The D-pad's left and right with Home held are Home's too, as any button is: the game never sees them,
/// and letting Home go isn't Home.
#[test]
fn home_with_the_dpad_is_a_combination_the_game_doesnt_see() {
    let (mut r, _) = router(PadRoute::Game);
    let now = Instant::now();
    for axis in [AbsoluteAxisCode::ABS_HAT0X, AbsoluteAxisCode::ABS_HAT0Y] {
        let mut out = Out::default();
        r.event(key(KeyCode::BTN_MODE, Press::Down), now, &mut out);
        r.event(Raw::abs(axis, 1), now, &mut out);
        r.event(Raw::REPORT, now, &mut out);
        r.event(key(KeyCode::BTN_MODE, Press::Up), now, &mut out);
        assert!(actions(&out).is_empty(), "not Home: {:?}", out.events);
        assert!(!out.pad.contains(&Raw::abs(axis, 1)), "{:?}", out.pad);
    }
}

/// Home pressed away from the game gives the mapping in force the right stick at rest, since while Home is
/// held the stick is Home's for its flicks: the pad mouse stops scrolling rather than scrolling on.
#[test]
fn home_away_from_the_game_centres_the_right_stick_for_the_mapping() {
    let (mut r, mut out) = router(PadRoute::Mouse);
    for code in [AbsoluteAxisCode::ABS_RX, AbsoluteAxisCode::ABS_RY] {
        r.snapshot.axes.push(Axis { code, value: 0, rest: 0 });
    }
    let t0 = Instant::now();
    r.event(Raw::abs(AbsoluteAxisCode::ABS_RY, 32767), t0, &mut out);
    assert!(r.tick(t0, &mut out).is_some(), "scrolling");
    r.event(key(KeyCode::BTN_MODE, Press::Down), t0, &mut out);
    out.mouse.clear();
    assert_eq!(r.tick(t0 + std::time::Duration::from_secs(1), &mut out), None, "stopped");
    assert!(out.mouse.is_empty(), "{:?}", out.mouse);
}

/// Activity is a button pressed or an axis moved past 4096 of its 32767, so that a stick resting a little
/// off centre doesn't take the keyboard focus back.
#[test]
fn game_activity_is_an_axis_past_a_threshold() {
    let (mut r, mut out) = router(PadRoute::Game);
    let now = Instant::now();
    out.events.clear();
    r.watch_activity(true);
    r.event(Raw::abs(AbsoluteAxisCode::ABS_X, 4096), now, &mut out);
    r.event(Raw::abs(AbsoluteAxisCode::ABS_X, -4096), now, &mut out);
    assert!(out.events.is_empty(), "{:?}", out.events);
    r.event(Raw::abs(AbsoluteAxisCode::ABS_X, -4097), now, &mut out);
    assert_eq!(out.events, vec![Event::PadActive]);
}

/// After dropped events, Home's press is made up before the other buttons', so that they are Home's
/// combination as they were; and its release after theirs, so that a button Home took is let go while
/// still Home's.
#[test]
fn a_resync_presses_home_first_and_lets_it_go_last() {
    let (mut r, mut out) = router(PadRoute::Game);
    let now = Instant::now();
    let mut events = Vec::new();
    r.resync(&[KeyCode::BTN_SOUTH, KeyCode::BTN_MODE], &[], &mut events);
    assert_eq!(events, [key(KeyCode::BTN_MODE, Press::Down), key(KeyCode::BTN_SOUTH, Press::Down), Raw::REPORT]);
    for e in events {
        r.event(e, now, &mut out);
    }
    assert!(!held(&out.pad).contains(&KeyCode::BTN_SOUTH), "A is Home's");
    let mut events = Vec::new();
    r.resync(&[], &[], &mut events);
    assert_eq!(events, [key(KeyCode::BTN_SOUTH, Press::Up), key(KeyCode::BTN_MODE, Press::Up), Raw::REPORT]);
    out.events.clear();
    for e in events {
        r.event(e, now, &mut out);
    }
    assert!(actions(&out).is_empty(), "not Home: {:?}", out.events);
    assert!(out.pad.iter().all(|e| e.as_key().is_none_or(|(k, _)| k != KeyCode::BTN_SOUTH)), "{:?}", out.pad);
}
