use super::*;
use clamshell_state::bottom::keyboard::{self, Key, KeyPos};
use clamshell_state::{Init, ListId, Tab};

fn t0() -> Instant {
    *START.get_or_init(Instant::now)
}

static START: std::sync::OnceLock<Instant> = std::sync::OnceLock::new();

fn blank() -> State {
    State::new(Init::default())
}

fn target(id: Id, drag: Option<Drag>) -> Target {
    Target { rect: Rect::new(0.0, 0.0, 100.0, 50.0), id, drag, list: None, item: None, pad: crate::toolkit::PadUse::Moves, hold: Hold::Long }
}

fn tab() -> Target {
    target(Id::Tab(Tab::Wifi), None)
}

fn volume() -> Target {
    let track = Track { x0: 10.0, w: 600.0 };
    target(Id::Control(Control::Volume), Some(Drag::Slide { control: Control::Volume, track, at_once: false }))
}

fn level() -> Target {
    target(Id::Level(Level::Volume), Some(Drag::Level(Level::Volume, Track { x0: 100.0, w: 200.0 })))
}

/// What the input side asks in `tell`, as a list of its own.
fn act(tell: impl FnOnce(&mut Vec<Action>)) -> Vec<Action> {
    let mut out = Vec::new();
    tell(&mut out);
    out
}

fn events(actions: &[Action]) -> Vec<Event> {
    actions.iter().filter_map(|a| if let Action::Emit(e) = a { Some(e.clone()) } else { None }).collect()
}

fn key() -> Id {
    Id::Key(keyboard::find(Key::Char('a')).unwrap_or(KeyPos { row: 0, col: 0 }))
}

#[test]
fn a_tap_activates_on_release_and_a_hold_does_not() {
    let state = State::new(Init::default());
    let mut input = Input::default();
    let down = act(|o| input.touch(Source::Touch(1), Some(&tab()), true, (5.0, 0.0), t0(), o));
    assert_eq!(events(&down), vec![PointerEvent::HideFocus.into(), PointerEvent::Down(Id::Tab(Tab::Wifi), None).into()]);
    assert!(down.contains(&Action::Arm(Source::Touch(1), Timer::Hold, LONG_PRESS)));
    let up = act(|o| input.up(Source::Touch(1), End::Release, &blank(), t0(), o));
    assert_eq!(up[0], Action::Disarm(Source::Touch(1), Timer::Hold));
    assert_eq!(events(&up), vec![PointerEvent::Activate(Id::Tab(Tab::Wifi)).into(), PointerEvent::Up(Id::Tab(Tab::Wifi)).into()]);

    act(|o| input.touch(Source::Touch(1), Some(&tab()), true, (5.0, 0.0), t0(), o));
    let held = act(|o| input.timer(Source::Touch(1), Timer::Hold, &state, t0(), o));
    assert_eq!(events(&held), vec![PointerEvent::LongPress(Id::Tab(Tab::Wifi)).into()]);
    assert_eq!(events(&act(|o| input.up(Source::Touch(1), End::Release, &blank(), t0(), o))), vec![PointerEvent::Up(Id::Tab(Tab::Wifi)).into()]);
    // Cancelled: no tap.
    act(|o| input.touch(Source::Touch(1), Some(&tab()), true, (5.0, 0.0), t0(), o));
    assert_eq!(events(&act(|o| input.up(Source::Touch(1), End::Cancel, &blank(), t0(), o))), vec![PointerEvent::Up(Id::Tab(Tab::Wifi)).into()]);
}

/// A state with `n` networks, of which the Wi-Fi list shows 5.
fn networks(n: usize) -> State {
    use clamshell_state::radios::wifi::{Network, Wifi};
    let mut s = State::new(Init::default());
    let networks = (0..n).map(|i| Network { ssid: format!("n{i}"), ..Network::default() }).collect();
    s.apply(Event::Reading(clamshell_state::Reading::Wifi(Wifi { present: true, on: true, networks, ..Wifi::default() })), &mut Vec::new());
    s.apply(ListEvent::Visible(ListId::Wifi, 5.0).into(), &mut Vec::new());
    s
}

fn row(list: ListId, i: u16) -> Target {
    let id = list.row(i);
    Target {
        rect: Rect::new(0.0, 0.0, 100.0, 1000.0),
        id,
        drag: None,
        list: Some(InList { list, row_h: 50.0 }),
        item: None,
        pad: crate::toolkit::PadUse::Moves,
        hold: Hold::Long,
    }
}

/// Applies what the actions emit to `s`, and returns whether a timer was armed again.
fn run(s: &mut State, actions: &[Action]) -> bool {
    for e in events(actions) {
        s.apply(e, &mut Vec::new());
    }
    actions.iter().any(|a| matches!(a, Action::Arm(..)))
}

#[test]
fn a_still_finger_on_a_list_taps_and_one_moving_drags() {
    let state = networks(20);
    let mut input = Input::default();
    let t = t0();
    let wifi = row(ListId::Wifi, 2);
    act(|o| input.touch(Source::Touch(1), Some(&wifi), true, (5.0, 300.0), t, o));
    assert!(act(|o| input.moved(Source::Touch(1), (8.0, 300.0 - DRAG_START + 1.0), t, o)).is_empty(), "a little jitter: still a tap");
    let up = act(|o| input.up(Source::Touch(1), End::Release, &state, t, o));
    assert!(events(&up).contains(&PointerEvent::Activate(wifi.id.clone()).into()));
    // Moved past the threshold, the row is let go without a tap and the list follows the finger 1:1
    // in rows.
    act(|o| input.touch(Source::Touch(1), Some(&wifi), true, (5.0, 300.0), t, o));
    let moved = act(|o| input.moved(Source::Touch(1), (5.0, 275.0), t + Duration::from_millis(500), o));
    assert_eq!(moved[0], Action::Disarm(Source::Touch(1), Timer::Hold));
    assert_eq!(events(&moved), vec![PointerEvent::Up(wifi.id.clone()).into(), ListEvent::Scroll(ListId::Wifi, 0.5).into()]);
    assert_eq!(
        events(&act(|o| input.moved(Source::Touch(1), (5.0, 325.0), t + Duration::from_millis(600), o))),
        vec![ListEvent::Scroll(ListId::Wifi, -1.0).into()]
    );
    assert!(act(|o| input.timer(Source::Touch(1), Timer::Hold, &state, t, o)).is_empty(), "no long press while dragging");
    // Let go slowly, there is no tap and no fling.
    let up = act(|o| input.up(Source::Touch(1), End::Release, &state, t + Duration::from_secs(2), o));
    assert!(events(&up).is_empty() && !up.iter().any(|a| matches!(a, Action::Arm(..))), "{up:?}");
}

#[test]
fn a_fling_slows_down_and_stops_at_the_end() {
    let mut state = networks(20);
    let mut input = Input::default();
    let t = t0();
    let ms = |n| t + Duration::from_millis(n);
    let wifi = row(ListId::Wifi, 0);
    act(|o| input.touch(Source::Touch(1), Some(&wifi), true, (5.0, 400.0), t, o));
    let mut fx = act(|o| input.moved(Source::Touch(1), (5.0, 380.0), ms(10), o));
    fx.extend(act(|o| input.moved(Source::Touch(1), (5.0, 300.0), ms(30), o)));
    run(&mut state, &fx);
    let up = act(|o| input.up(Source::Touch(1), End::Release, &state, ms(35), o));
    assert!(up.contains(&Action::Arm(Source::Touch(1), Timer::Fling, FLING_EVERY)), "{up:?}");
    let mut at = 35;
    let mut steps = 0;
    let mut last = state.lists().scroll(ListId::Wifi).offset();
    let mut gaps = vec![];
    loop {
        at += 16;
        steps += 1;
        let fx = act(|o| input.timer(Source::Touch(1), Timer::Fling, &state, ms(at), o));
        let more = run(&mut state, &fx);
        let now = state.lists().scroll(ListId::Wifi).offset();
        gaps.push(now - last);
        last = now;
        if !more || steps > 1000 {
            break;
        }
    }
    assert!(steps < 1000, "it ends");
    assert!(gaps.windows(2).take(10).all(|w| w[1] <= w[0] + 1e-4), "slowing down: {gaps:?}");
    assert_eq!(state.lists().scroll(ListId::Wifi).offset(), 15.0, "at the end: {gaps:?}");
    // A touch stops a fling.
    act(|o| input.touch(Source::Touch(1), Some(&wifi), true, (5.0, 400.0), ms(at), o));
    let fx = act(|o| input.moved(Source::Touch(1), (5.0, 500.0), ms(at + 10), o));
    run(&mut state, &fx);
    act(|o| input.up(Source::Touch(1), End::Release, &state, ms(at + 12), o));
    let stop = act(|o| input.touch(Source::Touch(2), Some(&wifi), true, (5.0, 100.0), ms(at + 20), o));
    assert!(stop.contains(&Action::Disarm(Source::Touch(1), Timer::Fling)), "{stop:?}");
    assert!(act(|o| input.timer(Source::Touch(1), Timer::Fling, &state, ms(at + 40), o)).is_empty());
}

#[test]
fn library_view_settles_on_a_whole_row() {
    let mut state = State::new(Init { library: Some(clamshell_library::demo::library()), ..Init::default() });
    state.apply(clamshell_state::LibraryViewEvent::Rows(3).into(), &mut Vec::new());
    let mut input = Input::default();
    let t = t0();
    let ms = |n| t + Duration::from_millis(n);
    let entry = Target { list: Some(InList { list: ListId::LibraryView, row_h: 100.0 }), ..row(ListId::LibraryView, 0) };
    act(|o| input.touch(Source::Touch(1), Some(&entry), true, (5.0, 400.0), t, o));
    let fx = act(|o| input.moved(Source::Touch(1), (5.0, 260.0), ms(500), o));
    run(&mut state, &fx);
    assert_eq!(state.lists().scroll(ListId::LibraryView).offset(), 1.4);
    let up = act(|o| input.up(Source::Touch(1), End::Release, &state, ms(2000), o));
    assert!(run(&mut state, &up), "settling");
    let mut at = 2000;
    loop {
        let fx = act(|o| input.timer(Source::Touch(1), Timer::Fling, &state, ms(at), o));
        if !run(&mut state, &fx) || at > 10_000 {
            break;
        }
        at += 16;
    }
    assert_eq!(state.lists().scroll(ListId::LibraryView).offset(), 1.0, "the nearest row");
    assert!(at < 2500, "a short animation: {} ms", at - 2000);
}

#[test]
fn a_held_key_repeats() {
    let state = State::new(Init::default());
    let mut input = Input::default();
    act(|o| input.press(Source::PadA, key(), Hold::Repeat, t0(), o));
    // (Typed as A went down; held, it repeats.)
    let held = act(|o| input.timer(Source::PadA, Timer::Hold, &state, t0(), o));
    assert_eq!(held, vec![Action::Emit(PointerEvent::Activate(key()).into()), Action::Arm(Source::PadA, Timer::Hold, REPEAT)]);
    assert_eq!(events(&act(|o| input.up(Source::PadA, End::Release, &blank(), t0(), o))), vec![PointerEvent::Up(key()).into()]);
}

#[test]
fn a_slide_starts_when_moved_far_enough_or_held() {
    let state = State::new(Init::default());
    let mut input = Input::default();
    let down = act(|o| input.touch(Source::Touch(1), Some(&volume()), true, (300.0, 0.0), t0(), o));
    assert!(down.contains(&Action::Arm(Source::Touch(1), Timer::Slide, SLIDE_AFTER)));
    // Not far enough yet: nothing.
    assert!(act(|o| input.moved(Source::Touch(1), (300.0 + SLIDE_MOVE - 1.0, 0.0), t0(), o)).is_empty());
    let moved = act(|o| input.moved(Source::Touch(1), (300.0 + SLIDE_MOVE, 0.0), t0(), o));
    assert_eq!(moved[0], Action::Disarm(Source::Touch(1), Timer::Slide));
    // The control is let go as the slider comes up, so it isn't shown pressed under it.
    let start = Slide::Start { control: Control::Volume, x: 290.0 + SLIDE_MOVE, width: 600.0 };
    assert_eq!(events(&moved), vec![PointerEvent::Up(Id::Control(Control::Volume)).into(), start.into()]);
    assert_eq!(events(&act(|o| input.moved(Source::Touch(1), (400.0, 0.0), t0(), o))), vec![Slide::To { x: 390.0 }.into()]);
    let up = act(|o| input.up(Source::Touch(1), End::Release, &blank(), t0(), o));
    assert_eq!(events(&up), vec![Slide::End.into()]);

    // Held still: the timer starts it.
    act(|o| input.touch(Source::Mouse, Some(&volume()), true, (300.0, 0.0), t0(), o));
    let start = Slide::Start { control: Control::Volume, x: 290.0, width: 600.0 };
    assert_eq!(
        events(&act(|o| input.timer(Source::Mouse, Timer::Slide, &state, t0(), o))),
        vec![PointerEvent::Up(Id::Control(Control::Volume)).into(), start.into()]
    );
    // Lifted before either, it is a tap, which opens the tab.
    act(|o| input.up(Source::Mouse, End::Release, &blank(), t0(), o));
    act(|o| input.touch(Source::Mouse, Some(&volume()), true, (300.0, 0.0), t0(), o));
    let up = act(|o| input.up(Source::Mouse, End::Release, &blank(), t0(), o));
    assert!(up.contains(&Action::Disarm(Source::Mouse, Timer::Slide)));
    assert!(events(&up).contains(&PointerEvent::Activate(Id::Control(Control::Volume)).into()));
}

#[test]
fn a_keymap_parameter_slides_at_once() {
    use clamshell_state::keymap::Param;
    let control = Control::Param(Param::Speed);
    let track = Track { x0: 10.0, w: 600.0 };
    let row = Target { id: Id::Control(control), drag: Some(Drag::Slide { control, track, at_once: true }), ..volume() };
    let mut input = Input::default();
    let down = act(|o| input.touch(Source::Touch(1), Some(&row), true, (300.0, 0.0), t0(), o));
    assert_eq!(events(&down)[2..], [PointerEvent::Up(Id::Control(control)).into(), Slide::Start { control, x: 290.0, width: 600.0 }.into()], "no wait");
    assert!(!down.iter().any(|a| matches!(a, Action::Arm(..))), "no timers: {down:?}");
    assert_eq!(events(&act(|o| input.moved(Source::Touch(1), (310.0, 0.0), t0(), o))), vec![Slide::To { x: 300.0 }.into()]);
    let up = act(|o| input.up(Source::Touch(1), End::Release, &blank(), t0(), o));
    assert_eq!(events(&up), vec![Slide::End.into()]);
    assert!(!events(&up).contains(&PointerEvent::Activate(Id::Control(control)).into()), "not a tap");
}

#[test]
fn a_second_finger_on_a_control_meanwhile_is_a_tap() {
    let mut input = Input::default();
    act(|o| input.touch(Source::Touch(1), Some(&volume()), true, (300.0, 0.0), t0(), o));
    let second = act(|o| input.touch(Source::Touch(2), Some(&volume()), true, (100.0, 0.0), t0(), o));
    assert!(!second.iter().any(|a| matches!(a, Action::Arm(_, Timer::Slide, _))));
    assert!(act(|o| input.moved(Source::Touch(2), (200.0, 0.0), t0(), o)).is_empty());
    // The same source again is ignored until it lifts.
    assert!(act(|o| input.touch(Source::Touch(1), Some(&tab()), true, (0.0, 0.0), t0(), o)).is_empty());
}

#[test]
fn a_level_row_follows_the_finger_and_the_release_focuses_it() {
    let state = State::new(Init::default());
    let mut input = Input::default();
    let down = act(|o| input.touch(Source::Touch(1), Some(&level()), true, (200.0, 0.0), t0(), o));
    assert!(events(&down).contains(&LevelEvent::Set(Level::Volume, 50).into()));
    assert!(!down.iter().any(|a| matches!(a, Action::Arm(..))));
    assert_eq!(events(&act(|o| input.moved(Source::Touch(1), (350.0, 0.0), t0(), o))), vec![LevelEvent::Set(Level::Volume, 100).into()]);
    assert!(act(|o| input.timer(Source::Touch(1), Timer::Hold, &state, t0(), o)).is_empty());
    let up = act(|o| input.up(Source::Touch(1), End::Release, &blank(), t0(), o));
    assert_eq!(events(&up), vec![PointerEvent::Activate(Id::Level(Level::Volume)).into(), PointerEvent::Up(Id::Level(Level::Volume)).into()]);
    assert!(act(|o| input.moved(Source::Touch(1), (100.0, 0.0), t0(), o)).is_empty());
}

#[test]
fn pad_a_is_released_when_the_pad_leaves_the_screens() {
    let mut input = Input::default();
    act(|o| input.press(Source::PadA, Id::Tab(Tab::Wifi), Hold::Long, t0(), o));
    assert!(act(|o| input.pad_route(PadRoute::Ui, &blank(), o)).is_empty());
    let released = act(|o| input.pad_route(PadRoute::Game, &blank(), o));
    assert_eq!(events(&released), vec![PointerEvent::Up(Id::Tab(Tab::Wifi)).into()]);
    assert!(!input.pressed(Source::PadA));
    assert!(act(|o| input.pad_route(PadRoute::Game, &blank(), o)).is_empty());
}

#[test]
fn cancelled_touches_end_without_a_tap() {
    let mut input = Input::default();
    act(|o| input.touch(Source::Touch(1), Some(&tab()), true, (0.0, 0.0), t0(), o));
    act(|o| input.touch(Source::Touch(2), Some(&volume()), true, (300.0, 0.0), t0(), o));
    act(|o| input.touch(Source::Mouse, Some(&tab()), true, (0.0, 0.0), t0(), o));
    let cancelled = act(|o| input.cancel_touches(&blank(), o));
    assert!(!events(&cancelled).iter().any(|e| matches!(e, Event::Pointer(PointerEvent::Activate(_)))));
    assert!(input.pressed(Source::Mouse) && !input.pressed(Source::Touch(1)) && !input.pressed(Source::Touch(2)));
}

fn points(actions: &[Action]) -> Vec<(Source, ContactPhase)> {
    actions.iter().filter_map(|a| if let Action::Point(s, phase, _) = a { Some((*s, *phase)) } else { None }).collect()
}

#[test]
fn a_finger_moving_on_a_key_points_and_takes_the_other_tapping_fingers_along() {
    let state = blank();
    let mut input = Input::default();
    let (a, ctrl, held) = (Source::Touch(1), Source::Touch(2), Source::Touch(3));
    let key_at = |k| target(Id::Key(keyboard::find(k).unwrap_or(KeyPos { row: 0, col: 0 })), None);
    // A little jitter still types.
    act(|o| input.touch(a, Some(&key_at(Key::Char('a'))), true, (10.0, 10.0), t0(), o));
    assert!(act(|o| input.moved(a, (10.0 + POINT_START - 1.0, 10.0), t0(), o)).is_empty());
    assert!(events(&act(|o| input.up(a, End::Release, &state, t0(), o))).contains(&PointerEvent::Activate(key()).into()));
    // Ctrl tapping, a key held and repeating, and a finger moving: Ctrl and the moving finger point,
    // and the held key stays a key.
    act(|o| input.touch(ctrl, Some(&key_at(Key::Mod(keyboard::Modifier::Ctrl))), true, (0.0, 50.0), t0(), o));
    act(|o| input.touch(held, Some(&key_at(Key::Named(keyboard::Named::Backspace))), true, (90.0, 30.0), t0(), o));
    act(|o| input.timer(held, Timer::Hold, &state, t0(), o));
    act(|o| input.touch(a, Some(&key_at(Key::Char('a'))), true, (10.0, 10.0), t0(), o));
    let moved = act(|o| input.moved(a, (10.0, 10.0 + POINT_START), t0(), o));
    assert_eq!(points(&moved).len(), 2, "{moved:?}");
    assert!(points(&moved).iter().all(|(s, phase)| *s != held && *phase == ContactPhase::Down));
    assert!(!events(&moved).iter().any(|e| matches!(e, Event::Pointer(PointerEvent::Activate(_)))), "let go without typing");
    assert_eq!(points(&act(|o| input.moved(a, (20.0, 40.0), t0(), o))), vec![(a, ContactPhase::Move)]);
    assert!(points(&act(|o| input.moved(held, (0.0, 0.0), t0(), o))).is_empty(), "held: a key wherever it goes");
    // Lifting the last pointing finger arms the idle timer.
    let up = act(|o| input.up(ctrl, End::Release, &state, t0(), o));
    assert_eq!((points(&up), up.iter().any(|a| matches!(a, Action::Arm(_, Timer::Pointing, _)))), (vec![(ctrl, ContactPhase::Up)], false));
    let up = act(|o| input.up(a, End::Release, &state, t0(), o));
    assert!(up.contains(&Action::Arm(a, Timer::Pointing, POINT_IDLE)) && events(&up).is_empty(), "{up:?}");
    // A finger down meanwhile points at once and stops the idle.
    let b = Source::Touch(4);
    let again = act(|o| input.point(b, (5.0, 5.0), t0(), o));
    assert!(again.contains(&Action::Disarm(a, Timer::Pointing)) && points(&again) == vec![(b, ContactPhase::Down)], "{again:?}");
    let up = act(|o| input.up(b, End::Release, &state, t0(), o));
    assert!(up.contains(&Action::Arm(b, Timer::Pointing, POINT_IDLE)));
    assert_eq!(events(&act(|o| input.timer(b, Timer::Pointing, &state, t0(), o))), vec![Pointing::Idle.into()], "idle: typing is back");
}

#[test]
fn on_a_list_up_and_down_scrolls_and_only_sideways_points() {
    let state = networks(20);
    let mut input = Input::default();
    let (t, finger) = (t0(), Source::Touch(1));
    let wifi = row(ListId::Wifi, 2);
    // Moving diagonally, the list takes it at 12 up before pointing could at 24 along.
    act(|o| input.touch(finger, Some(&wifi), true, (5.0, 300.0), t, o));
    let moved = act(|o| input.moved(finger, (5.0 + 20.0, 300.0 - DRAG_START), t, o));
    assert!(points(&moved).is_empty() && events(&moved).iter().any(|e| matches!(e, Event::List(ListEvent::Scroll(..)))), "{moved:?}");
    assert!(points(&act(|o| input.moved(finger, (5.0 + 60.0, 300.0 - DRAG_START), t, o))).is_empty(), "dragging: a list's");
    act(|o| input.up(finger, End::Release, &state, t, o));
    // Moving sideways, it points.
    act(|o| input.touch(finger, Some(&wifi), true, (5.0, 300.0), t, o));
    let moved = act(|o| input.moved(finger, (5.0 + POINT_START, 300.0 - DRAG_START + 1.0), t, o));
    assert_eq!(points(&moved), vec![(finger, ContactPhase::Down)]);
    act(|o| input.up(finger, End::Release, &state, t, o));
    // No pointing outside the screen's point area, such as on the Library on the main screen or a tab on
    // the bar, nor with the mouse.
    for (i, outside) in [target(Id::LibraryView(clamshell_state::LibraryViewId::Entry("g".into())), None), tab()].iter().enumerate() {
        let finger = Source::Touch(2 + i as i32);
        act(|o| input.touch(finger, Some(outside), false, (0.0, 0.0), t, o));
        assert!(points(&act(|o| input.moved(finger, (100.0, 0.0), t, o))).is_empty(), "{:?}", outside.id);
    }
    act(|o| input.touch(Source::Mouse, None, true, (0.0, 0.0), t, o));
    assert!(points(&act(|o| input.moved(Source::Mouse, (100.0, 0.0), t, o))).is_empty());
}

#[test]
fn on_nothing_a_finger_points_as_soon_as_it_moves() {
    let state = blank();
    let mut input = Input::default();
    let (t, finger) = (t0(), Source::Touch(1));
    act(|o| input.touch(finger, None, true, (50.0, 50.0), t, o));
    assert!(points(&act(|o| input.moved(finger, (50.0 + BLANK_POINT_START - 1.0, 50.0), t, o))).is_empty(), "jitter");
    assert_eq!(points(&act(|o| input.moved(finger, (50.0 + BLANK_POINT_START, 50.0), t, o))), vec![(finger, ContactPhase::Down)]);
    act(|o| input.up(finger, End::Release, &state, t, o));
    // A key keeps its tap over that distance.
    let key_at = target(key(), None);
    act(|o| input.touch(finger, Some(&key_at), true, (50.0, 50.0), t, o));
    assert!(points(&act(|o| input.moved(finger, (50.0 + BLANK_POINT_START, 50.0), t, o))).is_empty());
}

#[test]
fn a_tap_let_go_off_its_target_does_nothing() {
    let state = blank();
    let mut input = Input::default();
    let t = t0();
    act(|o| input.touch(Source::Mouse, Some(&tab()), true, (5.0, 5.0), t, o));
    act(|o| input.moved(Source::Mouse, (150.0, 5.0), t, o));
    let up = act(|o| input.up(Source::Mouse, End::Release, &state, t, o));
    assert_eq!(events(&up), vec![PointerEvent::Up(Id::Tab(Tab::Wifi)).into()], "off it: no tap");
    act(|o| input.touch(Source::Mouse, Some(&tab()), true, (5.0, 5.0), t, o));
    act(|o| input.moved(Source::Mouse, (150.0, 5.0), t, o));
    act(|o| input.moved(Source::Mouse, (50.0, 5.0), t, o));
    let up = act(|o| input.up(Source::Mouse, End::Release, &state, t, o));
    assert!(events(&up).contains(&PointerEvent::Activate(Id::Tab(Tab::Wifi)).into()), "back on it: a tap");
    // A finger may lift a little off it.
    let finger = Source::Touch(1);
    act(|o| input.touch(finger, Some(&tab()), true, (95.0, 5.0), t, o));
    act(|o| input.moved(finger, (100.0 + TOUCH_SLACK - 1.0, 5.0), t, o));
    assert!(events(&act(|o| input.up(finger, End::Release, &state, t, o))).contains(&PointerEvent::Activate(Id::Tab(Tab::Wifi)).into()));
}

#[test]
fn event_times_keep_the_spacing_they_happened_with() {
    let mut clock = EventClock::default();
    let first = clock.at(1000);
    // Handled later, all at once, they are still 8 ms apart.
    std::thread::sleep(Duration::from_millis(20));
    let second = clock.at(1008);
    assert_eq!(second.duration_since(first), Duration::from_millis(8));
}

/// A level row in a scrolled list, such as an app's volume in the Apps list.
fn level_in_list() -> Target {
    let list = Some(InList { list: ListId::Streams, row_h: 50.0 });
    Target { list, ..target(Id::Level(Level::Stream(0)), Some(Drag::Level(Level::Stream(0), Track { x0: 100.0, w: 200.0 }))) }
}

/// In a list, a finger on a level row waits for its first move: up or down drags the list and leaves the
/// level as it was; sideways sets the level and follows; let go still, it sets the level where it came
/// down.
#[test]
fn a_level_row_in_a_list_scrolls_with_a_vertical_swipe_and_sets_with_a_sideways_one() {
    let (finger, row) = (Source::Touch(1), level_in_list());
    let set = |v| Event::from(LevelEvent::Set(Level::Stream(0), v));
    let mut input = Input::default();
    let down = act(|o| input.touch(finger, Some(&row), true, (150.0, 25.0), t0(), o));
    assert!(!events(&down).iter().any(|e| matches!(e, Event::Level(_))), "nothing set yet: {down:?}");
    let swiped = act(|o| input.moved(finger, (152.0, 25.0 - DRAG_START), t0(), o));
    assert_eq!(events(&swiped), vec![PointerEvent::Up(row.id.clone()).into(), ListEvent::Scroll(ListId::Streams, DRAG_START / 50.0).into()]);
    assert_eq!(events(&act(|o| input.moved(finger, (180.0, -75.0), t0(), o))), vec![ListEvent::Scroll(ListId::Streams, (100.0 - DRAG_START) / 50.0).into()]);
    assert!(
        !events(&act(|o| input.up(finger, End::Release, &blank(), t0(), o)))
            .iter()
            .any(|e| matches!(e, Event::Level(_) | Event::Pointer(PointerEvent::Activate(_))))
    );

    let mut input = Input::default();
    act(|o| input.touch(finger, Some(&row), true, (150.0, 25.0), t0(), o));
    assert_eq!(events(&act(|o| input.moved(finger, (150.0 + DRAG_START, 27.0), t0(), o))), vec![set(31)]);
    assert_eq!(events(&act(|o| input.moved(finger, (200.0, 60.0), t0(), o))), vec![set(50)], "set, it follows the finger, whatever the height");
    let up = act(|o| input.up(finger, End::Release, &blank(), t0(), o));
    assert_eq!(events(&up), vec![PointerEvent::Activate(row.id.clone()).into(), PointerEvent::Up(row.id.clone()).into()]);

    let mut input = Input::default();
    act(|o| input.touch(finger, Some(&row), true, (150.0, 25.0), t0(), o));
    assert_eq!(act(|o| input.timer(finger, Timer::Hold, &blank(), t0(), o)), vec![], "held still, it doesn't long-press");
    let up = act(|o| input.up(finger, End::Release, &blank(), t0(), o));
    assert_eq!(events(&up), vec![set(25), PointerEvent::Activate(row.id.clone()).into(), PointerEvent::Up(row.id.clone()).into()]);
}

/// A key's target, as the keyboard draws it: it repeats when held.
fn key_target(k: Key) -> Target {
    Target { hold: Hold::Repeat, ..target(Id::Key(keyboard::find(k).unwrap_or(KeyPos { row: 0, col: 0 })), None) }
}

fn activated(actions: &[Action]) -> bool {
    events(actions).iter().any(|e| matches!(e, Event::Pointer(PointerEvent::Activate(_))))
}

/// A tap on a key types it at the lift, after its Down showed it pressed as the finger landed.
#[test]
fn a_tapped_key_is_typed_at_the_lift() {
    let mut input = Input::default();
    let finger = Source::Touch(1);
    let down = act(|o| input.touch(finger, Some(&key_target(Key::Char('a'))), true, (10.0, 10.0), t0(), o));
    assert!(events(&down).contains(&PointerEvent::Down(key(), None).into()), "pressed at once");
    assert!(!activated(&down) && down.contains(&Action::Arm(finger, Timer::KeyHold, KEY_HOLD)), "{down:?}");
    let up = act(|o| input.up(finger, End::Release, &blank(), t0(), o));
    assert!(up.contains(&Action::Disarm(finger, Timer::KeyHold)));
    assert_eq!(events(&up), vec![PointerEvent::Activate(key()).into(), PointerEvent::Up(key()).into()]);
}

/// A finger starting on a key and moving past POINT_START before KEY_HOLD points and types nothing.
#[test]
fn a_drag_from_a_key_points_and_types_nothing() {
    let mut input = Input::default();
    let finger = Source::Touch(1);
    act(|o| input.touch(finger, Some(&key_target(Key::Named(keyboard::Named::Esc))), true, (10.0, 10.0), t0(), o));
    let moved = act(|o| input.moved(finger, (10.0 + POINT_START, 10.0), t0(), o));
    assert_eq!(points(&moved), vec![(finger, ContactPhase::Down)]);
    assert!(moved.contains(&Action::Disarm(finger, Timer::KeyHold)) && !activated(&moved), "{moved:?}");
    assert!(!activated(&act(|o| input.up(finger, End::Release, &blank(), t0(), o))));
}

/// A finger kept on a key for KEY_HOLD types it then, before the lift, and from then never points; the lift
/// lets it go without typing it again. ClamShell's own repeat still comes at LONG_PRESS, for what takes text.
#[test]
fn a_held_key_is_typed_after_key_hold_and_held_until_the_lift() {
    let state = blank();
    let mut input = Input::default();
    let finger = Source::Touch(1);
    act(|o| input.touch(finger, Some(&key_target(Key::Char('a'))), true, (10.0, 10.0), t0(), o));
    assert_eq!(events(&act(|o| input.timer(finger, Timer::KeyHold, &state, t0(), o))), vec![PointerEvent::Activate(key()).into()]);
    assert!(points(&act(|o| input.moved(finger, (10.0 + POINT_START * 2.0, 10.0), t0(), o))).is_empty(), "a held key never points");
    assert_eq!(events(&act(|o| input.up(finger, End::Release, &state, t0(), o))), vec![PointerEvent::Up(key()).into()]);
}

/// Each finger is its own key: one held while another taps.
#[test]
fn one_key_held_while_another_is_tapped() {
    let state = blank();
    let mut input = Input::default();
    let (held, tapping) = (Source::Touch(1), Source::Touch(2));
    let w = Id::Key(keyboard::find(Key::Char('w')).unwrap_or(KeyPos { row: 0, col: 0 }));
    act(|o| input.touch(held, Some(&key_target(Key::Char('w'))), true, (10.0, 10.0), t0(), o));
    act(|o| input.timer(held, Timer::KeyHold, &state, t0(), o));
    act(|o| input.touch(tapping, Some(&key_target(Key::Char('a'))), true, (60.0, 10.0), t0(), o));
    assert_eq!(events(&act(|o| input.up(tapping, End::Release, &state, t0(), o))), vec![PointerEvent::Activate(key()).into(), PointerEvent::Up(key()).into()]);
    assert!(input.pressed(held));
    assert_eq!(events(&act(|o| input.up(held, End::Release, &state, t0(), o))), vec![PointerEvent::Up(w).into()]);
}

/// A modifier isn't typed at KEY_HOLD: held, it long-presses (locks) at LONG_PRESS as before.
#[test]
fn a_modifier_held_still_locks_at_the_long_press() {
    let state = blank();
    let mut input = Input::default();
    let ctrl = target(Id::Key(keyboard::find(Key::Mod(keyboard::Modifier::Ctrl)).unwrap_or(KeyPos { row: 0, col: 0 })), None);
    let down = act(|o| input.touch(Source::Touch(1), Some(&ctrl), true, (10.0, 10.0), t0(), o));
    assert!(!down.iter().any(|a| matches!(a, Action::Arm(_, Timer::KeyHold, _))), "{down:?}");
    assert_eq!(events(&act(|o| input.timer(Source::Touch(1), Timer::Hold, &state, t0(), o))), vec![PointerEvent::LongPress(ctrl.id.clone()).into()]);
}

/// The pad's A types a key as it goes down, and its release only lets the key go.
#[test]
fn the_pads_a_types_a_key_as_it_goes_down() {
    let mut input = Input::default();
    let down = act(|o| input.press(Source::PadA, key(), Hold::Repeat, t0(), o));
    assert_eq!(events(&down), vec![PointerEvent::Down(key(), None).into(), PointerEvent::Activate(key()).into()]);
    assert_eq!(events(&act(|o| input.up(Source::PadA, End::Release, &blank(), t0(), o))), vec![PointerEvent::Up(key()).into()]);
}
