use super::*;
use crate::input::Range;
use crate::pad::motion::Sample;
use crate::pad::route::{Axis, PadRouter};
use clamshell_core::PadRoute;
use clamshell_core::keyboard::Key;

const STICK_MAX: i32 = 32767;

fn router(keymap: Keymap, route: PadRoute) -> (PadRouter, Out) {
    let ranges = Ranges { stick: Range::STICK, trigger: Range { min: 0, max: 1023 } };
    let axis = |code| Axis { code, value: 0, rest: 0 };
    let axes = [
        AbsoluteAxisCode::ABS_X,
        AbsoluteAxisCode::ABS_Y,
        AbsoluteAxisCode::ABS_RX,
        AbsoluteAxisCode::ABS_RY,
        AbsoluteAxisCode::ABS_Z,
        AbsoluteAxisCode::ABS_RZ,
        AbsoluteAxisCode::ABS_HAT0X,
        AbsoluteAxisCode::ABS_HAT0Y,
    ]
    .map(axis)
    .to_vec();
    let mut out = Out::default();
    let r = PadRouter::new(ranges, PadSnapshot::new(axes), route, keymap, &mut out);
    (r, Out::default())
}

fn keymap(bindings: &[(Input, Binding)]) -> Keymap {
    let mut k = Keymap::new(1, "test");
    for (i, b) in bindings {
        assert!(k.set(*i, Some(*b)), "{i:?}");
    }
    k
}

fn to(output: Output) -> Binding {
    Binding::to(output)
}

fn key(code: KeyCode, press: Press) -> Raw {
    Raw::key(code, press)
}

fn press(r: &mut PadRouter, code: KeyCode, down: bool, out: &mut Out) {
    r.event(key(code, if down { Press::Down } else { Press::Up }), Instant::now(), out);
    r.event(Raw::REPORT, Instant::now(), out);
}

fn axis(r: &mut PadRouter, code: AbsoluteAxisCode, v: i32, out: &mut Out) {
    r.event(Raw::abs(code, v), Instant::now(), out);
    r.event(Raw::REPORT, Instant::now(), out);
}

/// The keys down after these events, as the kernel would keep them.
fn held(events: &[Raw]) -> Vec<KeyCode> {
    let mut down = Vec::new();
    for (k, p) in events.iter().filter_map(|e| e.as_key()) {
        down.retain(|&d| d != k);
        if p.held() {
            down.push(k);
        }
    }
    down.sort_by_key(|k| k.0);
    down
}

/// An axis' value after these events (every axis starts at 0 here).
fn last(events: &[Raw], code: AbsoluteAxisCode) -> Option<i32> {
    Some(events.iter().rev().find_map(|e| e.as_abs().filter(|(c, _)| *c == code).map(|(_, v)| v)).unwrap_or(0))
}

fn sum(events: &[Raw], code: RelativeAxisCode) -> i32 {
    events.iter().filter(|e| e.kind == evdev::EventType::RELATIVE && e.code == code.0).map(|e| e.value).sum()
}

fn chord(text: &str) -> Chord {
    Chord::parse(text).unwrap()
}

#[test]
fn a_button_to_another_button() {
    let (mut r, mut out) = router(clamshell_core::keymap::nintendo(1), PadRoute::Game);
    press(&mut r, KeyCode::BTN_SOUTH, true, &mut out);
    assert_eq!(held(&out.pad), vec![KeyCode::BTN_EAST], "A is B");
    press(&mut r, KeyCode::BTN_TL, true, &mut out);
    assert_eq!(held(&out.pad), vec![KeyCode::BTN_EAST, KeyCode::BTN_TL], "LB as it is");
    press(&mut r, KeyCode::BTN_SOUTH, false, &mut out);
    assert_eq!(held(&out.pad), vec![KeyCode::BTN_TL]);
}

#[test]
fn a_button_to_a_key_chord_held_while_held() {
    let k = keymap(&[(Input::Button(Button::A), to(Output::Key(chord("ctrl+x"))))]);
    let (mut r, mut out) = router(k, PadRoute::Game);
    press(&mut r, KeyCode::BTN_SOUTH, true, &mut out);
    let downs: Vec<Raw> = out.mouse.iter().copied().filter(|e| e.as_key().is_some()).collect();
    assert_eq!(downs, vec![key(KeyCode::KEY_LEFTCTRL, Press::Down), key(KeyCode::KEY_X, Press::Down)], "the modifier first");
    assert!(held(&out.pad).is_empty(), "nothing on the game's pad");
    out.mouse.clear();
    r.event(key(KeyCode::BTN_SOUTH, Press::Repeat), Instant::now(), &mut out);
    assert!(out.mouse.is_empty(), "held: no repeats from here (the compositor repeats)");
    press(&mut r, KeyCode::BTN_SOUTH, false, &mut out);
    let ups: Vec<Raw> = out.mouse.iter().copied().filter(|e| e.as_key().is_some()).collect();
    assert_eq!(ups, vec![key(KeyCode::KEY_X, Press::Up), key(KeyCode::KEY_LEFTCTRL, Press::Up)], "the modifier last");
}

#[test]
fn two_chords_share_a_modifier() {
    let k = keymap(&[(Input::Button(Button::A), to(Output::Key(chord("ctrl+x")))), (Input::Button(Button::B), to(Output::Key(chord("ctrl+c"))))]);
    let (mut r, mut out) = router(k, PadRoute::Game);
    press(&mut r, KeyCode::BTN_SOUTH, true, &mut out);
    press(&mut r, KeyCode::BTN_EAST, true, &mut out);
    press(&mut r, KeyCode::BTN_SOUTH, false, &mut out);
    assert_eq!(held(&out.mouse), vec![KeyCode::KEY_LEFTCTRL, KeyCode::KEY_C], "Ctrl stays for B's chord");
    press(&mut r, KeyCode::BTN_EAST, false, &mut out);
    assert!(held(&out.mouse).is_empty());
}

#[test]
fn buttons_to_the_mouse_and_the_wheel() {
    let k = keymap(&[
        (Input::Button(Button::RB), to(Output::Mouse(MouseButton::Right))),
        (Input::Dpad(Dir::Down), to(Output::Wheel(Dir::Down))),
        (Input::Button(Button::Y), to(Output::Nothing)),
    ]);
    let (mut r, mut out) = router(k, PadRoute::Game);
    press(&mut r, KeyCode::BTN_TR, true, &mut out);
    assert_eq!(held(&out.mouse), vec![KeyCode::BTN_RIGHT]);
    axis(&mut r, AbsoluteAxisCode::ABS_HAT0Y, 1, &mut out);
    axis(&mut r, AbsoluteAxisCode::ABS_HAT0X, 1, &mut out);
    assert_eq!(sum(&out.mouse, RelativeAxisCode::REL_WHEEL), -1, "one step a press");
    assert_eq!(last(&out.pad, AbsoluteAxisCode::ABS_HAT0Y), Some(0), "not on the game's D-pad");
    assert_eq!(last(&out.pad, AbsoluteAxisCode::ABS_HAT0X), Some(1), "right as it is");
    axis(&mut r, AbsoluteAxisCode::ABS_HAT0Y, 0, &mut out);
    axis(&mut r, AbsoluteAxisCode::ABS_HAT0Y, 1, &mut out);
    assert_eq!(sum(&out.mouse, RelativeAxisCode::REL_WHEEL), -2);
    assert_eq!(sum(&out.mouse, RelativeAxisCode::REL_WHEEL_HI_RES), -240);
    press(&mut r, KeyCode::BTN_NORTH, true, &mut out);
    assert!(!held(&out.pad).contains(&KeyCode::BTN_NORTH), "Y does nothing");
}

#[test]
fn a_stick_to_the_pointer_to_keys_and_to_the_other_stick() {
    let k = keymap(&[
        (Input::Stick(Side::Right), to(Output::Pointer)),
        (Input::Stick(Side::Left), to(Output::Keys([chord("w"), chord("a"), chord("s"), chord("d")]))),
    ]);
    let (mut r, mut out) = router(k, PadRoute::Game);
    let t0 = Instant::now();
    axis(&mut r, AbsoluteAxisCode::ABS_RX, STICK_MAX, &mut out);
    assert_eq!(last(&out.pad, AbsoluteAxisCode::ABS_RX), Some(0), "the game's stick stays at rest");
    let mut t = t0;
    let next = r.tick(t, &mut out);
    assert!(next.is_some(), "ticks while deflected");
    for _ in 0..125 {
        t += POINTER_EVERY;
        r.tick(t, &mut out);
    }
    let moved = sum(&out.mouse, RelativeAxisCode::REL_X);
    assert!((moved - 1400).abs() < 30, "about the stock speed for a second: {moved}");
    axis(&mut r, AbsoluteAxisCode::ABS_RX, 0, &mut out);
    r.tick(t + POINTER_EVERY, &mut out);
    assert_eq!(r.tick(t + POINTER_EVERY * 2, &mut out), None, "at rest: no ticks");
    // The left stick: W while up past half, A with left.
    axis(&mut r, AbsoluteAxisCode::ABS_Y, -20000, &mut out);
    axis(&mut r, AbsoluteAxisCode::ABS_X, -20000, &mut out);
    assert_eq!(held(&out.mouse), vec![KeyCode::KEY_W, KeyCode::KEY_A]);
    axis(&mut r, AbsoluteAxisCode::ABS_Y, -5000, &mut out);
    assert_eq!(held(&out.mouse), vec![KeyCode::KEY_A]);
    // The left stick to the right one instead.
    r.set_keymap(keymap(&[(Input::Stick(Side::Left), to(Output::Stick(Side::Right)))]), &mut out);
    assert!(held(&out.mouse).is_empty(), "the keys let go with the old keymap");
    assert_eq!((last(&out.pad, AbsoluteAxisCode::ABS_RX), last(&out.pad, AbsoluteAxisCode::ABS_X)), (Some(-20000), Some(0)));
    // Its up direction bound: that half is a button now.
    r.set_keymap(keymap(&[(Input::StickDir(Side::Left, Dir::Left), to(Output::Pad(PadOut::Button(Button::LB))))]), &mut out);
    assert_eq!((last(&out.pad, AbsoluteAxisCode::ABS_X), last(&out.pad, AbsoluteAxisCode::ABS_RX)), (Some(0), Some(0)));
    assert_eq!(held(&out.pad), vec![KeyCode::BTN_TL]);
}

#[test]
fn a_stick_to_the_dpad() {
    let k = keymap(&[(Input::Stick(Side::Left), to(Output::Dpad))]);
    let (mut r, mut out) = router(k, PadRoute::Game);
    axis(&mut r, AbsoluteAxisCode::ABS_X, STICK_MAX, &mut out);
    assert_eq!((last(&out.pad, AbsoluteAxisCode::ABS_HAT0X), last(&out.pad, AbsoluteAxisCode::ABS_X)), (Some(1), Some(0)));
}

#[test]
fn a_trigger_past_its_threshold_and_to_the_other_trigger() {
    let k = keymap(&[
        (Input::Trigger(Side::Left), Binding { threshold: 75, ..to(Output::Key(chord("shift"))) }),
        (Input::Trigger(Side::Right), to(Output::Trigger(Side::Left))),
    ]);
    let (mut r, mut out) = router(k, PadRoute::Game);
    axis(&mut r, AbsoluteAxisCode::ABS_Z, 600, &mut out);
    assert!(held(&out.mouse).is_empty(), "past half, short of 75 %");
    axis(&mut r, AbsoluteAxisCode::ABS_Z, 800, &mut out);
    assert_eq!(held(&out.mouse), vec![KeyCode::KEY_LEFTSHIFT]);
    assert_eq!(last(&out.pad, AbsoluteAxisCode::ABS_Z), Some(0), "not on the game's trigger");
    axis(&mut r, AbsoluteAxisCode::ABS_RZ, 512, &mut out);
    assert_eq!((last(&out.pad, AbsoluteAxisCode::ABS_Z), last(&out.pad, AbsoluteAxisCode::ABS_RZ)), (Some(512), Some(0)));
}

fn gyro(r: &mut PadRouter, ms: u64, yaw: f32, out: &mut Out) {
    r.motion(Sample { rate: super::super::motion::Rate { pitch: 0.0, yaw, roll: 0.0 }, at: Duration::from_millis(ms) }, out);
}

#[test]
fn the_gyro_to_the_pointer_while_a_button_is_held() {
    let k = keymap(&[(Input::Gyro, Binding { hold: Some(Input::Trigger(Side::Left)), ..to(Output::Pointer) })]);
    let (mut r, mut out) = router(k, PadRoute::Game);
    for i in 0..=100 {
        gyro(&mut r, i * 4, 50.0, &mut out);
    }
    assert_eq!(sum(&out.mouse, RelativeAxisCode::REL_X), 0, "not held: nothing");
    axis(&mut r, AbsoluteAxisCode::ABS_Z, 1023, &mut out);
    assert_eq!(last(&out.pad, AbsoluteAxisCode::ABS_Z), Some(1023), "LT keeps its own mapping");
    for i in 101..=350 {
        gyro(&mut r, i * 4, -20.0, &mut out);
    }
    // A second at -20 °/s (turning right) at 15 px per degree: 300 px right.
    let moved = sum(&out.mouse, RelativeAxisCode::REL_X);
    assert!((moved - 300).abs() <= 2, "{moved}");
    // A sensor's offset at rest (0.5 °/s): tightened to a crawl, then learnt and taken off.
    let before = sum(&out.mouse, RelativeAxisCode::REL_X);
    for i in 351..=850 {
        gyro(&mut r, i * 4, 0.5, &mut out);
    }
    let learning = sum(&out.mouse, RelativeAxisCode::REL_X) - before;
    assert!(learning.abs() <= 5, "a few pixels while it learns: {learning}");
    let learnt = sum(&out.mouse, RelativeAxisCode::REL_X);
    for i in 851..=1100 {
        gyro(&mut r, i * 4, 0.5, &mut out);
    }
    assert_eq!(sum(&out.mouse, RelativeAxisCode::REL_X), learnt, "then nothing");
}

fn turning(r: &mut PadRouter, from: u64, ms: u64, yaw: f32, roll: f32, out: &mut Out) -> i32 {
    let before = sum(&out.mouse, RelativeAxisCode::REL_X);
    for i in from..=from + ms / 4 {
        r.motion(Sample { rate: super::super::motion::Rate { pitch: 0.0, yaw, roll }, at: Duration::from_millis(i * 4) }, out);
    }
    sum(&out.mouse, RelativeAxisCode::REL_X) - before
}

#[test]
fn gyro_acceleration_tightening_and_tilting() {
    let pointer = |b: Binding| keymap(&[(Input::Gyro, Binding { ..b })]);
    // No acceleration: a second at 20 °/s and at 100 °/s go 15 px per degree.
    let (mut r, mut out) = router(pointer(to(Output::Pointer)), PadRoute::Game);
    assert!((turning(&mut r, 0, 1000, -20.0, 0.0, &mut out) - 300).abs() <= 3);
    assert!((turning(&mut r, 251, 1000, -100.0, 0.0, &mut out) - 1500).abs() <= 15);
    // Full acceleration, fastest at 60 °/s: slow turns as before, 100 °/s three times as far.
    let (mut r, mut out) = router(pointer(Binding { accel: 100, accel_at: 60, ..to(Output::Pointer) }), PadRoute::Game);
    let slow = turning(&mut r, 0, 1000, -6.0, 0.0, &mut out);
    assert!((slow - 108).abs() <= 3, "6 °/s: a tenth of the way to 60, so x1.2: {slow}");
    let fast = turning(&mut r, 251, 1000, -100.0, 0.0, &mut out);
    assert!((fast - 4500).abs() <= 45, "{fast}");
    // Tightening 2 °/s: 1 °/s goes at half (a quarter of the way), 4 °/s in full.
    let (mut r, mut out) = router(pointer(Binding { tighten: 20, ..to(Output::Pointer) }), PadRoute::Game);
    let tight = turning(&mut r, 0, 1000, -1.0, 0.0, &mut out);
    assert!((tight - 7).abs() <= 1, "{tight}");
    // Tilting turns too, with the binding's switch; not without.
    let (mut r, mut out) = router(pointer(to(Output::Pointer)), PadRoute::Game);
    assert_eq!(turning(&mut r, 0, 500, 0.0, -20.0, &mut out), 0);
    let (mut r, mut out) = router(pointer(Binding { twist: true, ..to(Output::Pointer) }), PadRoute::Game);
    assert!((turning(&mut r, 0, 1000, 0.0, -20.0, &mut out) - 300).abs() <= 3);
}

#[test]
fn the_gyro_off_while_held_and_a_stick_from_its_minimum() {
    // A ratchet: on, but off while LB is held.
    let k = keymap(&[(Input::Gyro, Binding { hold: Some(Input::Button(Button::LB)), ratchet: true, ..to(Output::Pointer) })]);
    let (mut r, mut out) = router(k, PadRoute::Game);
    assert!(turning(&mut r, 0, 500, -20.0, 0.0, &mut out) > 140);
    press(&mut r, KeyCode::BTN_TL, true, &mut out);
    assert_eq!(turning(&mut r, 126, 500, -20.0, 0.0, &mut out), 0, "held: off");
    press(&mut r, KeyCode::BTN_TL, false, &mut out);
    assert!(turning(&mut r, 252, 500, -20.0, 0.0, &mut out) > 140);
    // A stick from 30 %: a slow turn starts there, full stays full.
    let k = keymap(&[(Input::Gyro, Binding { stick_min: 30, ..to(Output::Stick(Side::Right)) })]);
    let (mut r, mut out) = router(k, PadRoute::Game);
    gyro(&mut r, 0, -5.0, &mut out);
    let v = last(&out.pad, AbsoluteAxisCode::ABS_RX).unwrap();
    assert!((v - STICK_MAX * 335 / 1000).abs() < 400, "30 % + 70 % of 5 % of full: {v}");
    gyro(&mut r, 4, -500.0, &mut out);
    assert_eq!(last(&out.pad, AbsoluteAxisCode::ABS_RX), Some(STICK_MAX));
}

#[test]
fn the_gyro_to_a_stick() {
    let k = keymap(&[(Input::Gyro, Binding { speed: 50, ..to(Output::Stick(Side::Right)) })]);
    let (mut r, mut out) = router(k, PadRoute::Game);
    gyro(&mut r, 0, -50.0, &mut out);
    let half = last(&out.pad, AbsoluteAxisCode::ABS_RX).unwrap();
    assert!((half - STICK_MAX / 2).abs() < 400, "50 of 100 °/s: half way: {half}");
    gyro(&mut r, 4, -500.0, &mut out);
    assert_eq!(last(&out.pad, AbsoluteAxisCode::ABS_RX), Some(STICK_MAX), "clamped");
    gyro(&mut r, 8, 0.0, &mut out);
    assert_eq!(last(&out.pad, AbsoluteAxisCode::ABS_RX), Some(0));
}

#[test]
fn only_the_game_route_is_remapped_and_nothing_stays_down() {
    let k = keymap(&[
        (Input::Button(Button::A), to(Output::Key(chord("ctrl+x")))),
        (Input::Button(Button::B), to(Output::Pad(PadOut::Button(Button::X)))),
        (Input::Button(Button::X), to(Output::Mouse(MouseButton::Left))),
        (Input::Stick(Side::Right), to(Output::Pointer)),
    ]);
    let (mut r, mut out) = router(k.clone(), PadRoute::Ui);
    press(&mut r, KeyCode::BTN_SOUTH, true, &mut out);
    assert!(held(&out.mouse).is_empty() && out.events.contains(&clamshell_core::Event::Pad(clamshell_core::PadInput::A(true))), "the screens' A");
    // Into the game with A held: its chord goes down; B and X too.
    r.set_route(PadRoute::Game, &mut out);
    press(&mut r, KeyCode::BTN_EAST, true, &mut out);
    press(&mut r, KeyCode::BTN_WEST, true, &mut out);
    assert_eq!(held(&out.mouse), vec![KeyCode::KEY_LEFTCTRL, KeyCode::KEY_X, KeyCode::BTN_LEFT]);
    assert_eq!(held(&out.pad), vec![KeyCode::BTN_WEST]);
    // Away from the game: everything let go, keys, mouse buttons and the pad.
    r.set_route(PadRoute::Mouse, &mut out);
    assert!(held(&out.mouse).is_empty(), "{:?}", held(&out.mouse));
    assert!(held(&out.pad).is_empty());
    // Back, then a new keymap mid-press: the old outputs go up, the new ones down.
    r.set_route(PadRoute::Game, &mut out);
    assert_eq!(held(&out.pad), vec![KeyCode::BTN_WEST]);
    r.set_keymap(keymap(&[(Input::Button(Button::A), to(Output::Key(Chord::of(Key::Char('z')))))]), &mut out);
    assert_eq!(held(&out.mouse), vec![KeyCode::KEY_Z]);
    assert_eq!(held(&out.pad), vec![KeyCode::BTN_EAST, KeyCode::BTN_WEST], "B and X as themselves now");
    r.set_keymap(Keymap::default(), &mut out);
    assert!(held(&out.mouse).is_empty());
    assert_eq!(held(&out.pad), vec![KeyCode::BTN_SOUTH, KeyCode::BTN_EAST, KeyCode::BTN_WEST]);
    // The controller lost: nothing held on the keyboard.
    r.set_keymap(k, &mut out);
    r.lose(&mut out);
    assert!(held(&out.mouse).is_empty());
}

#[test]
fn the_default_keymap_passes_everything_through() {
    let (mut r, mut out) = router(Keymap::default(), PadRoute::Game);
    press(&mut r, KeyCode::BTN_SOUTH, true, &mut out);
    axis(&mut r, AbsoluteAxisCode::ABS_X, 12345, &mut out);
    axis(&mut r, AbsoluteAxisCode::ABS_RZ, 77, &mut out);
    axis(&mut r, AbsoluteAxisCode::ABS_HAT0X, -1, &mut out);
    r.event(key(KeyCode::BTN_C, Press::Down), Instant::now(), &mut out);
    r.event(Raw::REPORT, Instant::now(), &mut out);
    assert_eq!(held(&out.pad), vec![KeyCode::BTN_SOUTH, KeyCode::BTN_C], "an unknown button passes as it is");
    assert_eq!((last(&out.pad, AbsoluteAxisCode::ABS_X), last(&out.pad, AbsoluteAxisCode::ABS_RZ)), (Some(12345), Some(77)));
    assert_eq!(last(&out.pad, AbsoluteAxisCode::ABS_HAT0X), Some(-1));
    assert!(out.mouse.is_empty());
}

#[test]
fn the_ayn_button_is_the_games_guide_unless_mapped() {
    let (mut r, mut out) = router(Keymap::default(), PadRoute::Game);
    press(&mut r, KeyCode::KEY_F24, true, &mut out);
    assert_eq!(held(&out.pad), vec![KeyCode::BTN_MODE], "as on the pad: Guide");
    press(&mut r, KeyCode::KEY_F24, false, &mut out);
    assert!(held(&out.pad).is_empty());
    let (mut r, mut out) = router(keymap(&[(Input::Button(Button::Ayn), to(Output::Pad(PadOut::Button(Button::A))))]), PadRoute::Game);
    press(&mut r, KeyCode::KEY_F24, true, &mut out);
    assert_eq!(held(&out.pad), vec![KeyCode::BTN_SOUTH], "mapped: what it is mapped to, not Guide");
    let (mut r, mut out) = router(keymap(&[(Input::Button(Button::B), to(Output::Pad(PadOut::Button(Button::Ayn))))]), PadRoute::Game);
    press(&mut r, KeyCode::BTN_EAST, true, &mut out);
    assert_eq!(held(&out.pad), vec![KeyCode::BTN_MODE], "another button to AYN: Guide");
}

/// `input` bound in a layer on while Back is held (made the first time), as old Back layers load.
fn back_layer(k: &mut Keymap, input: Input, b: Binding) {
    let held = k.layers().find(|(_, n)| *n == "held").map(|(l, _)| l);
    let layer = held.or_else(|| k.add_layer("held")).unwrap();
    k.set(Input::Button(Button::Back), Some(to(Output::Layer(layer))));
    assert!(k.set_in(layer, input, Some(b)));
}

#[test]
fn the_back_layer_while_back_is_held_and_nothing_stuck_letting_go_mid_press() {
    let mut k = keymap(&[(Input::Button(Button::A), to(Output::Key(chord("ctrl+x")))), (Input::Button(Button::B), to(Output::Key(chord("z"))))]);
    back_layer(&mut k, Input::Button(Button::A), to(Output::Key(chord("ctrl+y"))));
    let (mut r, mut out) = router(k, PadRoute::Game);
    press(&mut r, KeyCode::BTN_SOUTH, true, &mut out);
    assert_eq!(held(&out.mouse), vec![KeyCode::KEY_LEFTCTRL, KeyCode::KEY_X], "main");
    press(&mut r, KeyCode::BTN_SOUTH, false, &mut out);
    press(&mut r, KeyCode::BTN_BACK, true, &mut out);
    assert!(out.pad.is_empty() && held(&out.mouse).is_empty(), "Back alone sends nothing: {out:?}");
    press(&mut r, KeyCode::BTN_SOUTH, true, &mut out);
    press(&mut r, KeyCode::BTN_EAST, true, &mut out);
    assert_eq!(held(&out.mouse), vec![KeyCode::KEY_Y, KeyCode::KEY_LEFTCTRL, KeyCode::KEY_Z], "the Back layer's, B falling through");
    // Back let go while A is held: Ctrl+Y up, A is main's Ctrl+X now.
    press(&mut r, KeyCode::BTN_BACK, false, &mut out);
    assert_eq!(held(&out.mouse), vec![KeyCode::KEY_LEFTCTRL, KeyCode::KEY_Z, KeyCode::KEY_X]);
    press(&mut r, KeyCode::BTN_SOUTH, false, &mut out);
    press(&mut r, KeyCode::BTN_EAST, false, &mut out);
    assert!(held(&out.mouse).is_empty(), "nothing stuck");
    assert!(!out.pad.iter().any(|e| e.as_key().is_some_and(|(k, _)| k == KeyCode::BTN_BACK)), "the game never sees Back");
    // Back held when the pad leaves the game: everything let go.
    press(&mut r, KeyCode::BTN_BACK, true, &mut out);
    press(&mut r, KeyCode::BTN_SOUTH, true, &mut out);
    r.set_route(PadRoute::Ui, &mut out);
    assert!(held(&out.mouse).is_empty() && held(&out.pad).is_empty());
}

#[test]
fn a_hold_to_a_layer_is_decided_by_an_input_the_layer_maps() {
    // LB: X tapped, held a layer where A is Y.
    let mut k = keymap(&[(Input::Button(Button::LB), to(Output::Key(chord("x"))))]);
    let aim = k.add_layer("aim").unwrap();
    k.set_of(Layer::BASE, Input::Button(Button::LB), Actuation::Hold, Some(to(Output::Layer(aim))));
    k.set_in(aim, Input::Button(Button::A), Some(to(Output::Key(chord("y")))));
    let (mut r, mut out) = router(k, PadRoute::Game);
    let t0 = Instant::now();
    // B (not in the layer) meanwhile: as on the pad, LB still deciding.
    press_at(&mut r, KeyCode::BTN_TL, true, t0, &mut out);
    press_at(&mut r, KeyCode::BTN_EAST, true, t0 + ms(20), &mut out);
    assert!(held(&out.pad).contains(&KeyCode::BTN_EAST) && held(&out.mouse).is_empty());
    // A soon after: the hold, at once; A is Y.
    press_at(&mut r, KeyCode::BTN_SOUTH, true, t0 + ms(50), &mut out);
    assert_eq!(held(&out.mouse), vec![KeyCode::KEY_Y]);
    for (code, t) in [(KeyCode::BTN_SOUTH, 80), (KeyCode::BTN_EAST, 90), (KeyCode::BTN_TL, 100)] {
        press_at(&mut r, code, false, t0 + ms(t), &mut out);
    }
    assert!(!out.mouse.iter().any(|e| e.as_key().is_some_and(|(k, _)| k == KeyCode::KEY_X)), "never X");
    // Alone and short: X.
    let t1 = t0 + ms(1000);
    press_at(&mut r, KeyCode::BTN_TL, true, t1, &mut out);
    press_at(&mut r, KeyCode::BTN_TL, false, t1 + ms(100), &mut out);
    assert_eq!(held(&out.mouse), vec![KeyCode::KEY_X]);
    assert_eq!(out.buzz, None, "a held layer: no buzz");
}

#[test]
fn a_once_layer_tapped_is_on_until_an_input_it_maps_has_acted() {
    // LB: a Once layer where A is Enter and B Esc.
    let mut k = keymap(&[]);
    let menu = k.add_layer("menu").unwrap();
    k.set_layer_once(menu, true);
    k.set(Input::Button(Button::LB), Some(to(Output::Layer(menu))));
    k.set_in(menu, Input::Button(Button::A), Some(to(Output::Key(chord("enter")))));
    k.set_in(menu, Input::Button(Button::B), Some(to(Output::Key(chord("esc")))));
    let (mut r, mut out) = router(k, PadRoute::Game);
    let tap = |r: &mut PadRouter, code, out: &mut Out| {
        press(r, code, true, out);
        press(r, code, false, out);
    };
    tap(&mut r, KeyCode::BTN_TL, &mut out);
    assert_eq!(out.buzz.take(), Some(Buzz::On), "armed");
    // X (not in it): as on the pad; still armed.
    tap(&mut r, KeyCode::BTN_WEST, &mut out);
    assert_eq!(out.buzz.take(), None);
    press(&mut r, KeyCode::BTN_SOUTH, true, &mut out);
    assert_eq!(held(&out.mouse), vec![KeyCode::KEY_ENTER], "A in the layer");
    press(&mut r, KeyCode::BTN_SOUTH, false, &mut out);
    assert_eq!(out.buzz.take(), Some(Buzz::Off), "used: off");
    press(&mut r, KeyCode::BTN_SOUTH, true, &mut out);
    assert!(held(&out.pad).contains(&KeyCode::BTN_SOUTH) && held(&out.mouse).is_empty(), "Base again");
    press(&mut r, KeyCode::BTN_SOUTH, false, &mut out);
    // Tapped twice: armed, then off.
    tap(&mut r, KeyCode::BTN_TL, &mut out);
    tap(&mut r, KeyCode::BTN_TL, &mut out);
    assert_eq!(out.buzz.take(), Some(Buzz::Off));
    press(&mut r, KeyCode::BTN_SOUTH, true, &mut out);
    assert!(held(&out.mouse).is_empty(), "off");
    press(&mut r, KeyCode::BTN_SOUTH, false, &mut out);
    // Held: as any layer, used as often as wanted; off on letting go, without a buzz.
    press(&mut r, KeyCode::BTN_TL, true, &mut out);
    tap(&mut r, KeyCode::BTN_SOUTH, &mut out);
    press(&mut r, KeyCode::BTN_EAST, true, &mut out);
    assert_eq!(held(&out.mouse), vec![KeyCode::KEY_ESC], "still on");
    press(&mut r, KeyCode::BTN_EAST, false, &mut out);
    press(&mut r, KeyCode::BTN_TL, false, &mut out);
    assert_eq!(out.buzz.take(), None);
    press(&mut r, KeyCode::BTN_SOUTH, true, &mut out);
    assert!(held(&out.mouse).is_empty(), "off");
    press(&mut r, KeyCode::BTN_SOUTH, false, &mut out);
}

#[test]
fn a_toggled_once_layer_goes_off_with_its_toggle_once_used() {
    let mut k = keymap(&[]);
    let menu = k.add_layer("menu").unwrap();
    k.set_layer_once(menu, true);
    k.set(Input::Button(Button::LB), Some(Binding { toggle: true, ..to(Output::Layer(menu)) }));
    k.set_in(menu, Input::Button(Button::A), Some(to(Output::Key(chord("enter")))));
    let (mut r, mut out) = router(k, PadRoute::Game);
    for _ in 0..2 {
        press(&mut r, KeyCode::BTN_TL, true, &mut out);
        press(&mut r, KeyCode::BTN_TL, false, &mut out);
        assert_eq!(out.buzz.take(), Some(Buzz::On), "toggled on");
        press(&mut r, KeyCode::BTN_SOUTH, true, &mut out);
        assert_eq!(held(&out.mouse), vec![KeyCode::KEY_ENTER]);
        press(&mut r, KeyCode::BTN_SOUTH, false, &mut out);
        assert_eq!(out.buzz.take(), Some(Buzz::Off), "used: off, and its toggle (so the next press turns it on)");
    }
    // A plain toggled layer: a buzz on and off.
    let mut k = keymap(&[]);
    let aim = k.add_layer("aim").unwrap();
    k.set(Input::Button(Button::LB), Some(Binding { toggle: true, ..to(Output::Layer(aim)) }));
    let (mut r, mut out) = router(k, PadRoute::Game);
    press(&mut r, KeyCode::BTN_TL, true, &mut out);
    assert_eq!(out.buzz.take(), Some(Buzz::On));
    press(&mut r, KeyCode::BTN_TL, false, &mut out);
    press(&mut r, KeyCode::BTN_TL, true, &mut out);
    assert_eq!(out.buzz.take(), Some(Buzz::Off));
}

/// A keymap with every way of turning layers on, layered over each other, driven by random presses
/// and pauses (a fixed seed, so a failure repeats): whatever happened, with everything let go and
/// the timers run out nothing stays down, and only keys some binding sends ever were.
#[test]
fn random_presses_over_every_kind_of_layer_leave_nothing_down() {
    use clamshell_core::keymap::Layer;
    let key = |c: &str| to(Output::Key(chord(c)));
    let mut k = keymap(&[]);
    let menu = k.add_layer("menu").unwrap();
    let aim = k.add_layer("aim").unwrap();
    k.set_layer_once(menu, true);
    let b = |x| Input::Button(x);
    // LB: menu (Once). RB: R pressed, aim held. X: aim toggled. Y: T pressed, menu twice.
    k.set(b(Button::LB), Some(to(Output::Layer(menu))));
    k.set(b(Button::RB), Some(key("r")));
    k.set_of(Layer::BASE, b(Button::RB), Actuation::Hold, Some(to(Output::Layer(aim))));
    k.set(b(Button::X), Some(Binding { toggle: true, ..to(Output::Layer(aim)) }));
    k.set(b(Button::Y), Some(key("t")));
    k.set_of(Layer::BASE, b(Button::Y), Actuation::Double, Some(to(Output::Layer(menu))));
    k.set(b(Button::A), Some(key("x")));
    // menu: A Enter, B Esc. aim: A Ctrl+Y, LB Z, B held H.
    k.set_in(menu, b(Button::A), Some(key("enter")));
    k.set_in(menu, b(Button::B), Some(key("esc")));
    k.set_in(aim, b(Button::A), Some(key("ctrl+y")));
    k.set_in(aim, b(Button::LB), Some(key("z")));
    k.set_of(aim, b(Button::B), Actuation::Hold, Some(key("h")));
    let sent = [
        KeyCode::KEY_R,
        KeyCode::KEY_T,
        KeyCode::KEY_X,
        KeyCode::KEY_ENTER,
        KeyCode::KEY_ESC,
        KeyCode::KEY_LEFTCTRL,
        KeyCode::KEY_Y,
        KeyCode::KEY_Z,
        KeyCode::KEY_H,
    ];
    let codes = [KeyCode::BTN_SOUTH, KeyCode::BTN_EAST, KeyCode::BTN_WEST, KeyCode::BTN_NORTH, KeyCode::BTN_TL, KeyCode::BTN_TR];
    // (xorshift64: a few lines, and the same run every time.)
    let mut seed = 0x9e37_79b9_7f4a_7c15_u64;
    let mut next = |n: u64| {
        seed ^= seed << 13;
        seed ^= seed >> 7;
        seed ^= seed << 17;
        seed % n
    };
    for run in 0..300 {
        let (mut r, mut out) = router(k.clone(), PadRoute::Game);
        let mut t = Instant::now();
        let mut down = [false; 6];
        for _ in 0..40 {
            let i = next(codes.len() as u64) as usize;
            if let (Some(&code), Some(d)) = (codes.get(i), down.get_mut(i)) {
                *d = !*d;
                press_at(&mut r, code, *d, t, &mut out);
            }
            // A pause: none, within a tap or a double's window, past a hold's time.
            t += ms([0, 30, 120, 250, 450][next(5) as usize]);
            r.tick(t, &mut out);
            let wrong: Vec<_> = held(&out.mouse).into_iter().filter(|c| !sent.contains(c)).collect();
            assert!(wrong.is_empty(), "run {run}: {wrong:?} down");
        }
        for (i, &code) in codes.iter().enumerate() {
            if down.get(i) == Some(&true) {
                press_at(&mut r, code, false, t, &mut out);
            }
        }
        for _ in 0..4 {
            t += ms(500);
            r.tick(t, &mut out);
        }
        assert!(held(&out.mouse).is_empty(), "run {run}: {:?} still down", held(&out.mouse));
        assert!(held(&out.pad).iter().all(|c| !codes.contains(c)), "run {run}: {:?} still down on the pad", held(&out.pad));
    }
}

#[test]
fn a_toggled_layer_and_a_layer_key_the_layer_remaps() {
    let mut k = keymap(&[]);
    let aim = k.add_layer("aim").unwrap();
    let menu = k.add_layer("menu").unwrap();
    // LB toggles aim; in aim, A is Ctrl+Y and LB itself is a key (it still turns aim off).
    k.set(Input::Button(Button::LB), Some(Binding { toggle: true, ..to(Output::Layer(aim)) }));
    k.set_in(aim, Input::Button(Button::A), Some(to(Output::Key(chord("ctrl+y")))));
    k.set_in(aim, Input::Button(Button::LB), Some(to(Output::Key(chord("z")))));
    // RB held: menu, on top of aim; in it B is Q.
    k.set(Input::Button(Button::RB), Some(to(Output::Layer(menu))));
    k.set_in(menu, Input::Button(Button::B), Some(to(Output::Key(chord("q")))));
    let (mut r, mut out) = router(k, PadRoute::Game);
    press(&mut r, KeyCode::BTN_TL, true, &mut out);
    press(&mut r, KeyCode::BTN_TL, false, &mut out);
    assert!(held(&out.mouse).is_empty() && out.pad.iter().all(|e| e.as_key().is_none_or(|(k, _)| k != KeyCode::BTN_TL)), "LB is the layer key: nothing else");
    press(&mut r, KeyCode::BTN_SOUTH, true, &mut out);
    assert_eq!(held(&out.mouse), vec![KeyCode::KEY_Y, KeyCode::KEY_LEFTCTRL], "aim, toggled on");
    press(&mut r, KeyCode::BTN_SOUTH, false, &mut out);
    // RB held: menu over aim; B is Q there, A still aim's.
    press(&mut r, KeyCode::BTN_TR, true, &mut out);
    press(&mut r, KeyCode::BTN_EAST, true, &mut out);
    assert_eq!(held(&out.mouse), vec![KeyCode::KEY_Q]);
    press(&mut r, KeyCode::BTN_EAST, false, &mut out);
    press(&mut r, KeyCode::BTN_TR, false, &mut out);
    // LB again: aim off (its own binding there, Z, isn't what LB does: it turned aim on).
    press(&mut r, KeyCode::BTN_TL, true, &mut out);
    press(&mut r, KeyCode::BTN_TL, false, &mut out);
    assert!(!held(&out.mouse).contains(&KeyCode::KEY_Z));
    press(&mut r, KeyCode::BTN_SOUTH, true, &mut out);
    assert!(held(&out.mouse).is_empty() && held(&out.pad).contains(&KeyCode::BTN_SOUTH), "Base again: A as on the pad");
}

#[test]
fn the_gyro_differs_by_layer() {
    let mut k = keymap(&[]);
    back_layer(&mut k, Input::Gyro, to(Output::Pointer));
    let (mut r, mut out) = router(k, PadRoute::Game);
    for i in 0..=50 {
        gyro(&mut r, i * 4, -20.0, &mut out);
    }
    assert_eq!(sum(&out.mouse, RelativeAxisCode::REL_X), 0, "off in the main layer");
    press(&mut r, KeyCode::BTN_BACK, true, &mut out);
    for i in 51..=300 {
        gyro(&mut r, i * 4, -20.0, &mut out);
    }
    assert!(sum(&out.mouse, RelativeAxisCode::REL_X) > 250, "{}", sum(&out.mouse, RelativeAxisCode::REL_X));
}

#[test]
fn speeds_between_the_stock_steps() {
    assert_eq!(step(&STICK_POINTER, 50), 1400.0);
    assert_eq!(step(&STICK_POINTER, 100), 3000.0);
    assert_eq!(step(&STICK_POINTER, 0), 500.0);
    assert!((step(&STICK_POINTER, 60) - 1680.0).abs() < 0.01, "two fifths from 1400 to 2100");
}

#[test]
fn a_stick_as_on_the_pad_tuned() {
    let own = to(Output::Stick(Side::Left));
    let k = keymap(&[(Input::Stick(Side::Left), Binding { deadzone: 20, full: 80, invert_x: true, ..own })]);
    let (mut r, mut out) = router(k, PadRoute::Game);
    axis(&mut r, AbsoluteAxisCode::ABS_X, STICK_MAX / 10, &mut out);
    assert_eq!(last(&out.pad, AbsoluteAxisCode::ABS_X), Some(0), "inside the deadzone");
    axis(&mut r, AbsoluteAxisCode::ABS_X, STICK_MAX * 8 / 10, &mut out);
    assert!(last(&out.pad, AbsoluteAxisCode::ABS_X).unwrap() <= -STICK_MAX + 2, "full at 80 %, and inverted: {:?}", last(&out.pad, AbsoluteAxisCode::ABS_X));
    axis(&mut r, AbsoluteAxisCode::ABS_X, STICK_MAX / 2, &mut out);
    let half = last(&out.pad, AbsoluteAxisCode::ABS_X).unwrap();
    assert!((half + STICK_MAX / 2).abs() < 400, "(50-20)/(80-20): half way: {half}");
    // The right stick untouched: its exact values.
    axis(&mut r, AbsoluteAxisCode::ABS_RX, 1234, &mut out);
    assert_eq!(last(&out.pad, AbsoluteAxisCode::ABS_RX), Some(1234));
}

#[test]
fn a_stick_curve_and_tuning_before_mapping() {
    let k = keymap(&[(Input::Stick(Side::Left), Binding { curve: 100, ..to(Output::Stick(Side::Left)) })]);
    let (mut r, mut out) = router(k, PadRoute::Game);
    axis(&mut r, AbsoluteAxisCode::ABS_X, STICK_MAX / 2, &mut out);
    let v = last(&out.pad, AbsoluteAxisCode::ABS_X).unwrap();
    assert!((v - STICK_MAX / 8).abs() < 200, "cubed: an eighth: {v}");
    // A stick on keys with a deadzone of 50: its directions count past it.
    let k = keymap(&[(Input::Stick(Side::Left), Binding { deadzone: 50, ..to(Output::Keys([chord("w"), chord("a"), chord("s"), chord("d")])) })]);
    let (mut r, mut out) = router(k, PadRoute::Game);
    axis(&mut r, AbsoluteAxisCode::ABS_X, STICK_MAX * 7 / 10, &mut out);
    assert!(held(&out.mouse).is_empty(), "70 % is under half once past the deadzone");
    axis(&mut r, AbsoluteAxisCode::ABS_X, STICK_MAX * 8 / 10, &mut out);
    assert_eq!(held(&out.mouse), vec![KeyCode::KEY_D]);
}

#[test]
fn a_trigger_tuned_as_on_the_pad_and_on_a_key() {
    let k = keymap(&[
        (Input::Trigger(Side::Left), Binding { deadzone: 10, full: 60, ..to(Output::Trigger(Side::Left)) }),
        (Input::Trigger(Side::Right), Binding { full: 50, threshold: 90, ..to(Output::Key(chord("x"))) }),
    ]);
    let (mut r, mut out) = router(k, PadRoute::Game);
    axis(&mut r, AbsoluteAxisCode::ABS_Z, 80, &mut out);
    assert_eq!(last(&out.pad, AbsoluteAxisCode::ABS_Z), Some(0), "inside its deadzone");
    axis(&mut r, AbsoluteAxisCode::ABS_Z, 700, &mut out);
    assert_eq!(last(&out.pad, AbsoluteAxisCode::ABS_Z), Some(1023), "past its full point");
    axis(&mut r, AbsoluteAxisCode::ABS_RZ, 470, &mut out);
    assert_eq!(held(&out.mouse), vec![KeyCode::KEY_X], "46 % is 92 % of the way to full at 50 %");
}

#[test]
fn a_stick_on_the_pointer_moves_as_before_by_default_and_without_a_deadzone_at_0() {
    // How the pointer moved before the parameters: a radial deadzone of 15 %, then squared.
    let old = |v: f32| ((v - 0.15) / 0.85).max(0.0).powi(2) * step(&STICK_POINTER, DEFAULT_SPEED_FOR_TEST);
    let moved = |b: Binding, deflection: f32| {
        let (mut r, mut out) = router(keymap(&[(Input::Stick(Side::Right), b)]), PadRoute::Game);
        axis(&mut r, AbsoluteAxisCode::ABS_RX, (deflection * STICK_MAX as f32) as i32, &mut out);
        let mut t = Instant::now();
        r.tick(t, &mut out);
        for _ in 0..125 {
            t += POINTER_EVERY;
            r.tick(t, &mut out);
        }
        sum(&out.mouse, RelativeAxisCode::REL_X) as f32
    };
    let default = Binding::of(Input::Stick(Side::Right), Output::Pointer);
    assert_eq!((default.deadzone, default.curve), (15, 50), "the panel shows what applies");
    for v in [0.1, 0.3, 0.5, 0.8, 1.0] {
        let (now, before) = (moved(default, v), old(v));
        assert!((now - before).abs() <= before * 0.02 + 2.0, "{v}: {now} px, before {before} px");
    }
    let plain = Binding { deadzone: 0, curve: 0, ..default };
    let slight = moved(plain, 0.1);
    assert!((slight - 140.0).abs() < 5.0, "no deadzone, linear: a tenth of the speed: {slight}");
}

const DEFAULT_SPEED_FOR_TEST: u8 = clamshell_core::keymap::DEFAULT_SPEED;

/// A button down or up at `t`.
fn press_at(r: &mut PadRouter, code: KeyCode, down: bool, t: Instant, out: &mut Out) {
    r.event(key(code, if down { Press::Down } else { Press::Up }), t, out);
    r.event(Raw::REPORT, t, out);
}

fn ms(n: u64) -> Duration {
    Duration::from_millis(n)
}

/// A on X when pressed, plus `act`'s binding.
fn with(act: Actuation, b: Binding) -> Keymap {
    let mut k = keymap(&[(Input::Button(Button::A), to(Output::Key(chord("x"))))]);
    assert!(k.set_of(clamshell_core::keymap::Layer::BASE, Input::Button(Button::A), act, Some(b)));
    k
}

#[test]
fn a_plain_press_acts_at_once_with_no_timers() {
    let (mut r, mut out) = router(keymap(&[(Input::Button(Button::A), to(Output::Key(chord("x"))))]), PadRoute::Game);
    let t0 = Instant::now();
    press_at(&mut r, KeyCode::BTN_SOUTH, true, t0, &mut out);
    assert_eq!(held(&out.mouse), vec![KeyCode::KEY_X], "on the press itself");
    assert_eq!(r.tick(t0, &mut out), None, "nothing waits for time");
}

#[test]
fn a_hold_acts_at_its_time_and_a_short_press_taps_on_letting_go() {
    let k = with(Actuation::Hold, Binding { hold_time: 40, ..to(Output::Key(chord("esc"))) });
    let (mut r, mut out) = router(k, PadRoute::Game);
    let t0 = Instant::now();
    press_at(&mut r, KeyCode::BTN_SOUTH, true, t0, &mut out);
    assert!(held(&out.mouse).is_empty(), "undecided");
    assert_eq!(r.tick(t0, &mut out), Some(t0 + ms(400)), "wakes at the hold's time");
    r.tick(t0 + ms(399), &mut out);
    assert!(held(&out.mouse).is_empty());
    r.tick(t0 + ms(400), &mut out);
    assert_eq!(held(&out.mouse), vec![KeyCode::KEY_ESC], "a hold");
    press_at(&mut r, KeyCode::BTN_SOUTH, false, t0 + ms(700), &mut out);
    assert!(held(&out.mouse).is_empty() && !out.mouse.iter().any(|e| e.as_key().is_some_and(|(k, _)| k == KeyCode::KEY_X)), "never X");
    // Short: X tapped on letting go, for a moment.
    let t1 = t0 + ms(2000);
    press_at(&mut r, KeyCode::BTN_SOUTH, true, t1, &mut out);
    press_at(&mut r, KeyCode::BTN_SOUTH, false, t1 + ms(100), &mut out);
    assert_eq!(held(&out.mouse), vec![KeyCode::KEY_X], "the press, on letting go");
    assert_eq!(r.tick(t1 + ms(100), &mut out), Some(t1 + ms(100) + TAP));
    r.tick(t1 + ms(100) + TAP, &mut out);
    assert!(held(&out.mouse).is_empty(), "a tap");
    assert_eq!(r.tick(t1 + ms(200), &mut out), None);
}

#[test]
fn a_double_and_a_single_press_with_a_double() {
    let k = with(Actuation::Double, to(Output::Key(chord("tab"))));
    let (mut r, mut out) = router(k, PadRoute::Game);
    let t0 = Instant::now();
    press_at(&mut r, KeyCode::BTN_SOUTH, true, t0, &mut out);
    press_at(&mut r, KeyCode::BTN_SOUTH, false, t0 + ms(50), &mut out);
    assert!(held(&out.mouse).is_empty(), "waiting for a second press");
    press_at(&mut r, KeyCode::BTN_SOUTH, true, t0 + ms(200), &mut out);
    assert_eq!(held(&out.mouse), vec![KeyCode::KEY_TAB], "a double, held while held");
    press_at(&mut r, KeyCode::BTN_SOUTH, false, t0 + ms(400), &mut out);
    assert!(held(&out.mouse).is_empty());
    // Once: X after the window, as a tap.
    let t1 = t0 + ms(2000);
    press_at(&mut r, KeyCode::BTN_SOUTH, true, t1, &mut out);
    press_at(&mut r, KeyCode::BTN_SOUTH, false, t1 + ms(50), &mut out);
    r.tick(t1 + ms(50) + DOUBLE_WINDOW, &mut out);
    assert_eq!(held(&out.mouse), vec![KeyCode::KEY_X]);
    r.tick(t1 + ms(50) + DOUBLE_WINDOW + TAP, &mut out);
    assert!(held(&out.mouse).is_empty());
    // Held past the window (no hold binding): a press, held.
    let t2 = t1 + ms(2000);
    press_at(&mut r, KeyCode::BTN_SOUTH, true, t2, &mut out);
    r.tick(t2 + DOUBLE_WINDOW, &mut out);
    assert_eq!(held(&out.mouse), vec![KeyCode::KEY_X]);
    press_at(&mut r, KeyCode::BTN_SOUTH, false, t2 + ms(900), &mut out);
    assert!(held(&out.mouse).is_empty());
}

#[test]
fn a_trigger_pulls_and_clicks_at_full_pull() {
    let mut k = keymap(&[]);
    let full = Binding { threshold: 95, ..to(Output::Mouse(MouseButton::Left)) };
    assert!(k.set_of(clamshell_core::keymap::Layer::BASE, Input::Trigger(Side::Right), Actuation::FullPull, Some(full)));
    let (mut r, mut out) = router(k, PadRoute::Game);
    axis(&mut r, AbsoluteAxisCode::ABS_RZ, 500, &mut out);
    assert_eq!(last(&out.pad, AbsoluteAxisCode::ABS_RZ), Some(500), "the pull as it is");
    assert!(held(&out.mouse).is_empty());
    axis(&mut r, AbsoluteAxisCode::ABS_RZ, 1000, &mut out);
    assert_eq!(last(&out.pad, AbsoluteAxisCode::ABS_RZ), Some(1000), "still the game's trigger");
    assert_eq!(held(&out.mouse), vec![KeyCode::BTN_LEFT], "and a click past 95 %");
    axis(&mut r, AbsoluteAxisCode::ABS_RZ, 600, &mut out);
    assert!(held(&out.mouse).is_empty());
}

#[test]
fn toggle_and_turbo() {
    let k = keymap(&[
        (Input::Button(Button::A), Binding { toggle: true, ..to(Output::Key(chord("x"))) }),
        (Input::Button(Button::B), Binding { turbo: 10, ..to(Output::Key(chord("z"))) }),
    ]);
    let (mut r, mut out) = router(k, PadRoute::Game);
    let t0 = Instant::now();
    press_at(&mut r, KeyCode::BTN_SOUTH, true, t0, &mut out);
    press_at(&mut r, KeyCode::BTN_SOUTH, false, t0 + ms(50), &mut out);
    assert_eq!(held(&out.mouse), vec![KeyCode::KEY_X], "on after one press");
    press_at(&mut r, KeyCode::BTN_SOUTH, true, t0 + ms(500), &mut out);
    assert!(held(&out.mouse).is_empty(), "off at the next press");
    press_at(&mut r, KeyCode::BTN_SOUTH, false, t0 + ms(550), &mut out);
    assert!(held(&out.mouse).is_empty(), "and stays off");
    // Turbo at 10 a second: on 50 ms, off 50 ms, while held.
    let t1 = t0 + ms(1000);
    press_at(&mut r, KeyCode::BTN_EAST, true, t1, &mut out);
    assert_eq!(held(&out.mouse), vec![KeyCode::KEY_Z]);
    assert_eq!(r.tick(t1, &mut out), Some(t1 + ms(50)));
    r.tick(t1 + ms(50), &mut out);
    assert!(held(&out.mouse).is_empty());
    r.tick(t1 + ms(100), &mut out);
    assert_eq!(held(&out.mouse), vec![KeyCode::KEY_Z]);
    press_at(&mut r, KeyCode::BTN_EAST, false, t1 + ms(120), &mut out);
    assert!(held(&out.mouse).is_empty());
    assert_eq!(r.tick(t1 + ms(130), &mut out), None, "let go: no more beats");
}

#[test]
fn a_parameter_slid_applies_at_once() {
    let own = to(Output::Stick(Side::Left));
    let (mut r, mut out) = router(Keymap::default(), PadRoute::Game);
    axis(&mut r, AbsoluteAxisCode::ABS_X, STICK_MAX / 2, &mut out);
    assert_eq!(last(&out.pad, AbsoluteAxisCode::ABS_X), Some(STICK_MAX / 2));
    // The slider sends the keymap as it moves (see core's keymap_screens): the stick held still follows.
    for dz in [10, 30, 50] {
        r.set_keymap(keymap(&[(Input::Stick(Side::Left), Binding { deadzone: dz, ..own })]), &mut out);
    }
    assert_eq!(last(&out.pad, AbsoluteAxisCode::ABS_X), Some(0), "half way is inside a deadzone of 50");
}
