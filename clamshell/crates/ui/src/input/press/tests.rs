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

fn target(id: Id, track: Option<Track>) -> Target {
    Target { rect: Rect::new(0.0, 0.0, 100.0, 50.0), id, track, list: None, item: None }
}

fn tab() -> Target {
    target(Id::Tab(Tab::Wifi), None)
}

fn volume() -> Target {
    target(Id::Control(Control::Volume), Some(Track { x0: 10.0, w: 600.0 }))
}

fn level() -> Target {
    target(Id::Level(Level::Volume), Some(Track { x0: 100.0, w: 200.0 }))
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
    let down = input.touch(Source::Touch(1), &tab(), (5.0, 0.0), t0());
    assert_eq!(events(&down), vec![PointerEvent::HideFocus.into(), PointerEvent::Down(Id::Tab(Tab::Wifi), None).into()]);
    assert!(down.contains(&Action::Arm(Source::Touch(1), Timer::Hold, LONG_PRESS)));
    let up = input.up(Source::Touch(1), End::Release, &blank(), t0());
    assert_eq!(up[0], Action::Disarm(Source::Touch(1), Timer::Hold));
    assert_eq!(events(&up), vec![PointerEvent::Activate(Id::Tab(Tab::Wifi)).into(), PointerEvent::Up(Id::Tab(Tab::Wifi)).into()]);

    input.touch(Source::Touch(1), &tab(), (5.0, 0.0), t0());
    let held = input.timer(Source::Touch(1), Timer::Hold, &state, t0());
    assert_eq!(events(&held), vec![PointerEvent::LongPress(Id::Tab(Tab::Wifi)).into()]);
    assert_eq!(events(&input.up(Source::Touch(1), End::Release, &blank(), t0())), vec![PointerEvent::Up(Id::Tab(Tab::Wifi)).into()]);
    // Cancelled: no tap.
    input.touch(Source::Touch(1), &tab(), (5.0, 0.0), t0());
    assert_eq!(events(&input.up(Source::Touch(1), End::Cancel, &blank(), t0())), vec![PointerEvent::Up(Id::Tab(Tab::Wifi)).into()]);
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
    Target { rect: Rect::new(0.0, 0.0, 100.0, 1000.0), id, track: None, list: Some(InList { list, row_h: 50.0 }), item: None }
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
    input.touch(Source::Touch(1), &wifi, (5.0, 300.0), t);
    assert!(input.moved(Source::Touch(1), (8.0, 300.0 - DRAG_START + 1.0), t).is_empty(), "a little jitter: still a tap");
    let up = input.up(Source::Touch(1), End::Release, &state, t);
    assert!(events(&up).contains(&PointerEvent::Activate(wifi.id.clone()).into()));
    // Moved past the threshold, the row is let go without a tap and the list follows the finger 1:1
    // in rows.
    input.touch(Source::Touch(1), &wifi, (5.0, 300.0), t);
    let moved = input.moved(Source::Touch(1), (5.0, 275.0), t + Duration::from_millis(500));
    assert_eq!(moved[0], Action::Disarm(Source::Touch(1), Timer::Hold));
    assert_eq!(events(&moved), vec![PointerEvent::Up(wifi.id.clone()).into(), ListEvent::Scroll(ListId::Wifi, 0.5).into()]);
    assert_eq!(events(&input.moved(Source::Touch(1), (5.0, 325.0), t + Duration::from_millis(600))), vec![ListEvent::Scroll(ListId::Wifi, -1.0).into()]);
    assert!(input.timer(Source::Touch(1), Timer::Hold, &state, t).is_empty(), "no long press while dragging");
    // Let go slowly, there is no tap and no fling.
    let up = input.up(Source::Touch(1), End::Release, &state, t + Duration::from_secs(2));
    assert!(events(&up).is_empty() && !up.iter().any(|a| matches!(a, Action::Arm(..))), "{up:?}");
}

#[test]
fn a_fling_slows_down_and_stops_at_the_end() {
    let mut state = networks(20);
    let mut input = Input::default();
    let t = t0();
    let ms = |n| t + Duration::from_millis(n);
    let wifi = row(ListId::Wifi, 0);
    input.touch(Source::Touch(1), &wifi, (5.0, 400.0), t);
    let mut fx = input.moved(Source::Touch(1), (5.0, 380.0), ms(10));
    fx.extend(input.moved(Source::Touch(1), (5.0, 300.0), ms(30)));
    run(&mut state, &fx);
    let up = input.up(Source::Touch(1), End::Release, &state, ms(35));
    assert!(up.contains(&Action::Arm(Source::Touch(1), Timer::Fling, FLING_EVERY)), "{up:?}");
    let mut at = 35;
    let mut steps = 0;
    let mut last = state.lists().scroll(ListId::Wifi).offset();
    let mut gaps = vec![];
    loop {
        at += 16;
        steps += 1;
        let fx = input.timer(Source::Touch(1), Timer::Fling, &state, ms(at));
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
    input.touch(Source::Touch(1), &wifi, (5.0, 400.0), ms(at));
    let fx = input.moved(Source::Touch(1), (5.0, 500.0), ms(at + 10));
    run(&mut state, &fx);
    input.up(Source::Touch(1), End::Release, &state, ms(at + 12));
    let stop = input.touch(Source::Touch(2), &wifi, (5.0, 100.0), ms(at + 20));
    assert!(stop.contains(&Action::Disarm(Source::Touch(1), Timer::Fling)), "{stop:?}");
    assert!(input.timer(Source::Touch(1), Timer::Fling, &state, ms(at + 40)).is_empty());
}

#[test]
fn shelf_settles_on_a_whole_row() {
    let mut state = State::new(Init { library: clamshell_library::demo::library(), ..Init::default() });
    state.apply(clamshell_state::ShelfEvent::Rows(3).into(), &mut Vec::new());
    let mut input = Input::default();
    let t = t0();
    let ms = |n| t + Duration::from_millis(n);
    let entry = Target { list: Some(InList { list: ListId::Shelf, row_h: 100.0 }), ..row(ListId::Shelf, 0) };
    input.touch(Source::Touch(1), &entry, (5.0, 400.0), t);
    let fx = input.moved(Source::Touch(1), (5.0, 260.0), ms(500));
    run(&mut state, &fx);
    assert_eq!(state.lists().scroll(ListId::Shelf).offset(), 1.4);
    let up = input.up(Source::Touch(1), End::Release, &state, ms(2000));
    assert!(run(&mut state, &up), "settling");
    let mut at = 2000;
    loop {
        let fx = input.timer(Source::Touch(1), Timer::Fling, &state, ms(at));
        if !run(&mut state, &fx) || at > 10_000 {
            break;
        }
        at += 16;
    }
    assert_eq!(state.lists().scroll(ListId::Shelf).offset(), 1.0, "the nearest row");
    assert!(at < 2500, "a short animation: {} ms", at - 2000);
}

#[test]
fn a_held_key_repeats() {
    let state = State::new(Init::default());
    let mut input = Input::default();
    input.press(Source::PadA, key(), t0());
    let held = input.timer(Source::PadA, Timer::Hold, &state, t0());
    assert_eq!(held, vec![Action::Emit(PointerEvent::Activate(key()).into()), Action::Arm(Source::PadA, Timer::Hold, REPEAT)]);
    assert_eq!(events(&input.up(Source::PadA, End::Release, &blank(), t0())), vec![PointerEvent::Up(key()).into()]);
}

#[test]
fn a_slide_starts_when_moved_far_enough_or_held() {
    let state = State::new(Init::default());
    let mut input = Input::default();
    let down = input.touch(Source::Touch(1), &volume(), (300.0, 0.0), t0());
    assert!(down.contains(&Action::Arm(Source::Touch(1), Timer::Slide, SLIDE_AFTER)));
    // Not far enough yet: nothing.
    assert!(input.moved(Source::Touch(1), (300.0 + SLIDE_MOVE - 1.0, 0.0), t0()).is_empty());
    let moved = input.moved(Source::Touch(1), (300.0 + SLIDE_MOVE, 0.0), t0());
    assert_eq!(moved[0], Action::Disarm(Source::Touch(1), Timer::Slide));
    assert_eq!(events(&moved), vec![Slide::Start { control: Control::Volume, x: 290.0 + SLIDE_MOVE, width: 600.0 }.into()]);
    assert_eq!(events(&input.moved(Source::Touch(1), (400.0, 0.0), t0())), vec![Slide::To { x: 390.0 }.into()]);
    let up = input.up(Source::Touch(1), End::Release, &blank(), t0());
    assert_eq!(events(&up), vec![Slide::End.into(), PointerEvent::Up(Id::Control(Control::Volume)).into()]);

    // Held still: the timer starts it.
    input.touch(Source::Mouse, &volume(), (300.0, 0.0), t0());
    assert_eq!(events(&input.timer(Source::Mouse, Timer::Slide, &state, t0())), vec![Slide::Start { control: Control::Volume, x: 290.0, width: 600.0 }.into()]);
    // Lifted before either, it is a tap, which opens the tab.
    input.up(Source::Mouse, End::Release, &blank(), t0());
    input.touch(Source::Mouse, &volume(), (300.0, 0.0), t0());
    let up = input.up(Source::Mouse, End::Release, &blank(), t0());
    assert!(up.contains(&Action::Disarm(Source::Mouse, Timer::Slide)));
    assert!(events(&up).contains(&PointerEvent::Activate(Id::Control(Control::Volume)).into()));
}

#[test]
fn a_keymap_parameter_slides_at_once() {
    use clamshell_state::keymap::Param;
    let control = Control::Param(Param::Speed);
    let row = Target { id: Id::Control(control), ..volume() };
    let mut input = Input::default();
    let down = input.touch(Source::Touch(1), &row, (300.0, 0.0), t0());
    assert_eq!(events(&down)[2..], [Slide::Start { control, x: 290.0, width: 600.0 }.into()], "no wait");
    assert!(!down.iter().any(|a| matches!(a, Action::Arm(..))), "no timers: {down:?}");
    assert_eq!(events(&input.moved(Source::Touch(1), (310.0, 0.0), t0())), vec![Slide::To { x: 300.0 }.into()]);
    let up = input.up(Source::Touch(1), End::Release, &blank(), t0());
    assert!(events(&up).contains(&Slide::End.into()));
    assert!(!events(&up).contains(&PointerEvent::Activate(Id::Control(control)).into()), "not a tap");
}

#[test]
fn a_second_finger_on_a_control_meanwhile_is_a_tap() {
    let mut input = Input::default();
    input.touch(Source::Touch(1), &volume(), (300.0, 0.0), t0());
    let second = input.touch(Source::Touch(2), &volume(), (100.0, 0.0), t0());
    assert!(!second.iter().any(|a| matches!(a, Action::Arm(_, Timer::Slide, _))));
    assert!(input.moved(Source::Touch(2), (200.0, 0.0), t0()).is_empty());
    // The same source again is ignored until it lifts.
    assert!(input.touch(Source::Touch(1), &tab(), (0.0, 0.0), t0()).is_empty());
}

#[test]
fn a_level_row_follows_the_finger_and_the_release_focuses_it() {
    let state = State::new(Init::default());
    let mut input = Input::default();
    let down = input.touch(Source::Touch(1), &level(), (200.0, 0.0), t0());
    assert!(events(&down).contains(&LevelEvent::Set(Level::Volume, 50).into()));
    assert!(!down.iter().any(|a| matches!(a, Action::Arm(..))));
    assert_eq!(events(&input.moved(Source::Touch(1), (350.0, 0.0), t0())), vec![LevelEvent::Set(Level::Volume, 100).into()]);
    assert!(input.timer(Source::Touch(1), Timer::Hold, &state, t0()).is_empty());
    let up = input.up(Source::Touch(1), End::Release, &blank(), t0());
    assert_eq!(events(&up), vec![PointerEvent::Activate(Id::Level(Level::Volume)).into(), PointerEvent::Up(Id::Level(Level::Volume)).into()]);
    assert!(input.moved(Source::Touch(1), (100.0, 0.0), t0()).is_empty());
}

#[test]
fn pad_a_is_released_when_the_pad_leaves_the_screens() {
    let mut input = Input::default();
    input.press(Source::PadA, Id::Tab(Tab::Wifi), t0());
    assert!(input.pad_route(PadRoute::Ui, &blank()).is_empty());
    let released = input.pad_route(PadRoute::Game, &blank());
    assert_eq!(events(&released), vec![PointerEvent::Up(Id::Tab(Tab::Wifi)).into()]);
    assert!(!input.pressed(Source::PadA));
    assert!(input.pad_route(PadRoute::Game, &blank()).is_empty());
}

#[test]
fn cancelled_touches_end_without_a_tap() {
    let mut input = Input::default();
    input.touch(Source::Touch(1), &tab(), (0.0, 0.0), t0());
    input.touch(Source::Touch(2), &volume(), (300.0, 0.0), t0());
    input.touch(Source::Mouse, &tab(), (0.0, 0.0), t0());
    let cancelled = input.cancel_touches(&blank());
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
    input.touch(a, &key_at(Key::Char('a')), (10.0, 10.0), t0());
    assert!(input.moved(a, (10.0 + POINT_START - 1.0, 10.0), t0()).is_empty());
    assert!(events(&input.up(a, End::Release, &state, t0())).contains(&PointerEvent::Activate(key()).into()));
    // Ctrl tapping, a key held and repeating, and a finger moving: Ctrl and the moving finger point,
    // and the held key stays a key.
    input.touch(ctrl, &key_at(Key::Mod(keyboard::Modifier::Ctrl)), (0.0, 50.0), t0());
    input.touch(held, &key_at(Key::Named(keyboard::Named::Backspace)), (90.0, 30.0), t0());
    input.timer(held, Timer::Hold, &state, t0());
    input.touch(a, &key_at(Key::Char('a')), (10.0, 10.0), t0());
    let moved = input.moved(a, (10.0, 10.0 + POINT_START), t0());
    assert_eq!(points(&moved).len(), 2, "{moved:?}");
    assert!(points(&moved).iter().all(|(s, phase)| *s != held && *phase == ContactPhase::Down));
    assert!(!events(&moved).iter().any(|e| matches!(e, Event::Pointer(PointerEvent::Activate(_)))), "let go without typing");
    assert_eq!(points(&input.moved(a, (20.0, 40.0), t0())), vec![(a, ContactPhase::Move)]);
    assert!(points(&input.moved(held, (0.0, 0.0), t0())).is_empty(), "held: a key wherever it goes");
    // Lifting the last pointing finger arms the idle timer.
    let up = input.up(ctrl, End::Release, &state, t0());
    assert_eq!((points(&up), up.iter().any(|a| matches!(a, Action::Arm(_, Timer::Pointing, _)))), (vec![(ctrl, ContactPhase::Up)], false));
    let up = input.up(a, End::Release, &state, t0());
    assert!(up.contains(&Action::Arm(a, Timer::Pointing, POINT_IDLE)) && events(&up).is_empty(), "{up:?}");
    // A finger down meanwhile points at once and stops the idle.
    let b = Source::Touch(4);
    let again = input.point(b, &key_at(Key::Char('q')), (5.0, 5.0), t0());
    assert!(again.contains(&Action::Disarm(a, Timer::Pointing)) && points(&again) == vec![(b, ContactPhase::Down)], "{again:?}");
    let up = input.up(b, End::Release, &state, t0());
    assert!(up.contains(&Action::Arm(b, Timer::Pointing, POINT_IDLE)));
    assert_eq!(events(&input.timer(b, Timer::Pointing, &state, t0())), vec![Pointing::Idle.into()], "idle: typing is back");
}

#[test]
fn on_a_list_up_and_down_scrolls_and_only_sideways_points() {
    let state = networks(20);
    let mut input = Input::default();
    let (t, finger) = (t0(), Source::Touch(1));
    let wifi = row(ListId::Wifi, 2);
    // Moving diagonally, the list takes it at 12 up before pointing could at 24 along.
    input.touch(finger, &wifi, (5.0, 300.0), t);
    let moved = input.moved(finger, (5.0 + 20.0, 300.0 - DRAG_START), t);
    assert!(points(&moved).is_empty() && events(&moved).iter().any(|e| matches!(e, Event::List(ListEvent::Scroll(..)))), "{moved:?}");
    assert!(points(&input.moved(finger, (5.0 + 60.0, 300.0 - DRAG_START), t)).is_empty(), "dragging: a list's");
    input.up(finger, End::Release, &state, t);
    // Moving sideways, it points.
    input.touch(finger, &wifi, (5.0, 300.0), t);
    let moved = input.moved(finger, (5.0 + POINT_START, 300.0 - DRAG_START + 1.0), t);
    assert_eq!(points(&moved), vec![(finger, ContactPhase::Down)]);
    input.up(finger, End::Release, &state, t);
    // No pointing on shelf, on the main screen, nor with the mouse.
    let entry = target(Id::Shelf(clamshell_state::ShelfId::Entry("g".into())), None);
    input.touch(Source::Touch(2), &entry, (0.0, 0.0), t);
    assert!(points(&input.moved(Source::Touch(2), (100.0, 0.0), t)).is_empty());
    input.touch(Source::Mouse, &tab_free(), (0.0, 0.0), t);
    assert!(points(&input.moved(Source::Mouse, (100.0, 0.0), t)).is_empty());
}

#[test]
fn on_nothing_a_finger_points_as_soon_as_it_moves() {
    let state = blank();
    let mut input = Input::default();
    let (t, finger) = (t0(), Source::Touch(1));
    input.touch(finger, &tab_free(), (50.0, 50.0), t);
    assert!(points(&input.moved(finger, (50.0 + BLANK_POINT_START - 1.0, 50.0), t)).is_empty(), "jitter");
    assert_eq!(points(&input.moved(finger, (50.0 + BLANK_POINT_START, 50.0), t)), vec![(finger, ContactPhase::Down)]);
    input.up(finger, End::Release, &state, t);
    // A key keeps its tap over that distance.
    let key_at = target(key(), None);
    input.touch(finger, &key_at, (50.0, 50.0), t);
    assert!(points(&input.moved(finger, (50.0 + BLANK_POINT_START, 50.0), t)).is_empty());
}

fn tab_free() -> Target {
    target(Id::Blank, None)
}

#[test]
fn a_tap_let_go_off_its_target_does_nothing() {
    let state = blank();
    let mut input = Input::default();
    let t = t0();
    input.touch(Source::Mouse, &tab(), (5.0, 5.0), t);
    input.moved(Source::Mouse, (150.0, 5.0), t);
    let up = input.up(Source::Mouse, End::Release, &state, t);
    assert_eq!(events(&up), vec![PointerEvent::Up(Id::Tab(Tab::Wifi)).into()], "off it: no tap");
    input.touch(Source::Mouse, &tab(), (5.0, 5.0), t);
    input.moved(Source::Mouse, (150.0, 5.0), t);
    input.moved(Source::Mouse, (50.0, 5.0), t);
    let up = input.up(Source::Mouse, End::Release, &state, t);
    assert!(events(&up).contains(&PointerEvent::Activate(Id::Tab(Tab::Wifi)).into()), "back on it: a tap");
    // A finger may lift a little off it.
    let finger = Source::Touch(1);
    input.touch(finger, &tab(), (95.0, 5.0), t);
    input.moved(finger, (100.0 + TOUCH_SLACK - 1.0, 5.0), t);
    assert!(events(&input.up(finger, End::Release, &state, t)).contains(&PointerEvent::Activate(Id::Tab(Tab::Wifi)).into()));
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
