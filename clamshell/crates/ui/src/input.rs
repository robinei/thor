//! Presses: from touch, the mouse, the pad's A and Enter (on a desktop), each from its own source. A
//! pure state machine: it is told what happens (down, moved, up, a timer fired) and answers with what to
//! do: events for the state, and timers to arm or disarm (the caller runs them on its event loop).
//!
//! A press is a tap (it acts on release) until its hold timer fires: then it is held (keys repeat, the
//! rest get one long press) and the release only lifts it. A finger on volume or brightness may become
//! a slide (held there briefly, or moved sideways): the slider comes up under it and follows it. A finger
//! on a level row sets it where it is, and follows it. Positions are in the surface's logical pixels;
//! the target's track says what they mean.

use std::time::Duration;

use clamshell_core::levels::Level;
use clamshell_core::slider::{Control, SLIDE_AFTER, SLIDE_MOVE};
use clamshell_core::{Event, Hold, Id, LevelEvent, PadRoute, PointerEvent, Slide, State};

use crate::views::{Target, Track};

/// A press held this long is a hold: keys start repeating, modifiers lock.
pub(crate) const LONG_PRESS: Duration = Duration::from_millis(450);
/// Key repeat interval while held (25 per second).
pub(crate) const REPEAT: Duration = Duration::from_millis(40);

/// Where a press comes from: each touch point, the mouse, the pad's A, or Enter (on a desktop).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) enum Source {
    Touch(i32),
    Mouse,
    PadA,
    Enter,
}

/// A press's timers: the hold (long press, key repeat), and the wait before a slide.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) enum Timer {
    Hold,
    Slide,
}

/// What the caller does.
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum Action {
    /// Applies the event to the state.
    Emit(Event),
    /// Starts (or restarts) the source's timer, to call [`Input::timer`] after the duration.
    Arm(Source, Timer, Duration),
    /// Stops the source's timer, if it runs.
    Disarm(Source, Timer),
}

/// How a press ends: let go (a tap acts), or cancelled (the touch was taken away, the pad left).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum End {
    Release,
    Cancel,
}

/// What a press is doing.
#[derive(Debug, Clone, Copy, PartialEq)]
enum Gesture {
    /// Acts on release.
    Tap,
    /// The hold timer acted (a long press, or repeating): the release only lifts.
    Held,
    /// On volume or brightness: becomes a slide when held or moved sideways, else a tap. `down_x` and
    /// `x` are where it came down and is now, along the track.
    PendingSlide { control: Control, down_x: f32, x: f32, track: Track },
    /// Moving the slider: the release ends the slide.
    Sliding { track: Track },
    /// Setting a level row: holding it still doesn't long-press, the release focuses it as a tap does.
    LevelDrag { level: Level, track: Track },
}

/// A press in progress, on the target it started on.
#[derive(Debug, Clone, PartialEq)]
struct Press {
    id: Id,
    gesture: Gesture,
}

/// The presses in progress, one per source.
#[derive(Debug, Default)]
pub(crate) struct Input {
    presses: Vec<(Source, Press)>,
}

impl Input {
    fn get(&mut self, source: Source) -> Option<&mut Press> {
        self.presses.iter_mut().find(|(s, _)| *s == source).map(|(_, p)| p)
    }

    pub fn pressed(&self, source: Source) -> bool {
        self.presses.iter().any(|(s, _)| *s == source)
    }

    /// A slide is pending or moving (there is one slider: another finger on a control meanwhile taps).
    fn sliding(&self) -> bool {
        self.presses.iter().any(|(_, p)| matches!(p.gesture, Gesture::PendingSlide { .. } | Gesture::Sliding { .. }))
    }

    /// The pad's A or Enter on the focused target: a plain press.
    pub fn press(&mut self, source: Source, id: Id) -> Vec<Action> {
        if self.pressed(source) {
            return vec![];
        }
        self.presses.push((source, Press { id: id.clone(), gesture: Gesture::Tap }));
        vec![Action::Emit(PointerEvent::Down(id).into()), Action::Arm(source, Timer::Hold, LONG_PRESS)]
    }

    /// A finger or the mouse comes down on `target` at `x`. Touch hides the pad's focus ring.
    pub fn touch(&mut self, source: Source, target: &Target, x: f32) -> Vec<Action> {
        if self.pressed(source) {
            return vec![];
        }
        let gesture = match (&target.id, target.track) {
            (Id::Control(control), Some(track)) if !self.sliding() => {
                let at = track.along(x);
                Gesture::PendingSlide { control: *control, down_x: at, x: at, track }
            }
            (Id::Level(level), Some(track)) => Gesture::LevelDrag { level: *level, track },
            _ => Gesture::Tap,
        };
        let id = target.id.clone();
        self.presses.push((source, Press { id: id.clone(), gesture }));
        let mut actions = vec![Action::Emit(PointerEvent::HideFocus.into()), Action::Emit(PointerEvent::Down(id).into())];
        match gesture {
            Gesture::PendingSlide { .. } => {
                actions.push(Action::Arm(source, Timer::Hold, LONG_PRESS));
                actions.push(Action::Arm(source, Timer::Slide, SLIDE_AFTER));
            }
            Gesture::LevelDrag { level, track } => actions.push(Action::Emit(LevelEvent::Set(level, track.value_at(x)).into())),
            _ => actions.push(Action::Arm(source, Timer::Hold, LONG_PRESS)),
        }
        actions
    }

    /// A pending slide begins: the slider comes up under the finger.
    fn start_slide(press: &mut Press) -> Vec<Action> {
        let Gesture::PendingSlide { control, x, track, .. } = press.gesture else {
            return vec![];
        };
        press.gesture = Gesture::Sliding { track };
        vec![Action::Emit(Slide::Start { control, x, width: track.w }.into())]
    }

    /// A finger or the mouse moved to `x`: a pending slide may start; the slider or a level row follows.
    pub fn moved(&mut self, source: Source, x: f32) -> Vec<Action> {
        let Some(press) = self.get(source) else {
            return vec![];
        };
        match &mut press.gesture {
            Gesture::PendingSlide { down_x, x: at, track, .. } => {
                *at = track.along(x);
                if (*at - *down_x).abs() < SLIDE_MOVE {
                    return vec![];
                }
                let mut actions = vec![Action::Disarm(source, Timer::Slide)];
                actions.extend(Self::start_slide(press));
                actions
            }
            Gesture::Sliding { track } => vec![Action::Emit(Slide::To { x: track.along(x) }.into())],
            Gesture::LevelDrag { level, track } => vec![Action::Emit(LevelEvent::Set(*level, track.value_at(x)).into())],
            Gesture::Tap | Gesture::Held => vec![],
        }
    }

    /// The source's `timer` fired. A hold repeats keys (`state` says which repeat) and long-presses the
    /// rest; the slide timer starts the slide.
    pub fn timer(&mut self, source: Source, timer: Timer, state: &State) -> Vec<Action> {
        let Some(press) = self.get(source) else {
            return vec![];
        };
        let mut actions = vec![];
        match timer {
            Timer::Slide => return Self::start_slide(press),
            // (The slide's timer is the shorter: this comes first only if that one couldn't run.)
            Timer::Hold if matches!(press.gesture, Gesture::PendingSlide { .. }) => {
                actions.push(Action::Disarm(source, Timer::Slide));
                actions.extend(Self::start_slide(press));
            }
            Timer::Hold if matches!(press.gesture, Gesture::LevelDrag { .. }) => return actions,
            Timer::Hold => {}
        }
        if press.gesture == Gesture::Tap {
            press.gesture = Gesture::Held;
        }
        match state.hold(&press.id) {
            Hold::Repeat => {
                actions.push(Action::Emit(PointerEvent::Activate(press.id.clone()).into()));
                actions.push(Action::Arm(source, Timer::Hold, REPEAT));
            }
            Hold::Long => actions.push(Action::Emit(PointerEvent::LongPress(press.id.clone()).into())),
        }
        actions
    }

    /// A press ends: a tap activates (unless cancelled); a hold already acted; a slide ends.
    pub fn up(&mut self, source: Source, end: End) -> Vec<Action> {
        let Some(at) = self.presses.iter().position(|(s, _)| *s == source) else {
            return vec![];
        };
        let (_, press) = self.presses.remove(at);
        let mut actions = vec![Action::Disarm(source, Timer::Hold)];
        let activates = match press.gesture {
            Gesture::Sliding { .. } => {
                actions.push(Action::Emit(Slide::End.into()));
                false
            }
            Gesture::PendingSlide { .. } => {
                actions.push(Action::Disarm(source, Timer::Slide));
                true
            }
            Gesture::Tap | Gesture::LevelDrag { .. } => true,
            Gesture::Held => false,
        };
        if activates && end == End::Release {
            actions.push(Action::Emit(PointerEvent::Activate(press.id.clone()).into()));
        }
        actions.push(Action::Emit(PointerEvent::Up(press.id).into()));
        actions
    }

    /// All touch points gone without a tap (cancelled, or the touch device went away).
    pub fn cancel_touches(&mut self) -> Vec<Action> {
        let touches: Vec<Source> = self.presses.iter().map(|(s, _)| *s).filter(|s| matches!(s, Source::Touch(_))).collect();
        touches.into_iter().flat_map(|source| self.up(source, End::Cancel)).collect()
    }

    /// The pad went `route`: if that is away from ClamShell's screens while A is held on a target, its
    /// release won't come here, so the press ends now (else the target stays pressed, a key repeating).
    pub fn pad_route(&mut self, route: PadRoute) -> Vec<Action> {
        if route != PadRoute::Ui && self.pressed(Source::PadA) { self.up(Source::PadA, End::Cancel) } else { vec![] }
    }
}

#[cfg(test)]
#[allow(clippy::indexing_slicing)]
mod tests {
    use super::*;
    use crate::draw::Rect;
    use clamshell_core::keyboard::{self, Key, KeyPos};
    use clamshell_core::{Init, Tab};

    fn target(id: Id, track: Option<Track>) -> Target {
        Target { rect: Rect::new(0.0, 0.0, 100.0, 50.0), id, track }
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
        let down = input.touch(Source::Touch(1), &tab(), 5.0);
        assert_eq!(events(&down), vec![PointerEvent::HideFocus.into(), PointerEvent::Down(Id::Tab(Tab::Wifi)).into()]);
        assert!(down.contains(&Action::Arm(Source::Touch(1), Timer::Hold, LONG_PRESS)));
        let up = input.up(Source::Touch(1), End::Release);
        assert_eq!(up[0], Action::Disarm(Source::Touch(1), Timer::Hold));
        assert_eq!(events(&up), vec![PointerEvent::Activate(Id::Tab(Tab::Wifi)).into(), PointerEvent::Up(Id::Tab(Tab::Wifi)).into()]);

        input.touch(Source::Touch(1), &tab(), 5.0);
        let held = input.timer(Source::Touch(1), Timer::Hold, &state);
        assert_eq!(events(&held), vec![PointerEvent::LongPress(Id::Tab(Tab::Wifi)).into()]);
        assert_eq!(events(&input.up(Source::Touch(1), End::Release)), vec![PointerEvent::Up(Id::Tab(Tab::Wifi)).into()]);
        // Cancelled: no tap.
        input.touch(Source::Touch(1), &tab(), 5.0);
        assert_eq!(events(&input.up(Source::Touch(1), End::Cancel)), vec![PointerEvent::Up(Id::Tab(Tab::Wifi)).into()]);
    }

    #[test]
    fn a_held_key_repeats() {
        let state = State::new(Init::default());
        let mut input = Input::default();
        input.press(Source::PadA, key());
        let held = input.timer(Source::PadA, Timer::Hold, &state);
        assert_eq!(held, vec![Action::Emit(PointerEvent::Activate(key()).into()), Action::Arm(Source::PadA, Timer::Hold, REPEAT)]);
        assert_eq!(events(&input.up(Source::PadA, End::Release)), vec![PointerEvent::Up(key()).into()]);
    }

    #[test]
    fn a_slide_starts_when_moved_far_enough_or_held() {
        let state = State::new(Init::default());
        let mut input = Input::default();
        let down = input.touch(Source::Touch(1), &volume(), 300.0);
        assert!(down.contains(&Action::Arm(Source::Touch(1), Timer::Slide, SLIDE_AFTER)));
        // Not far enough yet: nothing.
        assert!(input.moved(Source::Touch(1), 300.0 + SLIDE_MOVE - 1.0).is_empty());
        let moved = input.moved(Source::Touch(1), 300.0 + SLIDE_MOVE);
        assert_eq!(moved[0], Action::Disarm(Source::Touch(1), Timer::Slide));
        assert_eq!(events(&moved), vec![Slide::Start { control: Control::Volume, x: 290.0 + SLIDE_MOVE, width: 600.0 }.into()]);
        assert_eq!(events(&input.moved(Source::Touch(1), 400.0)), vec![Slide::To { x: 390.0 }.into()]);
        let up = input.up(Source::Touch(1), End::Release);
        assert_eq!(events(&up), vec![Slide::End.into(), PointerEvent::Up(Id::Control(Control::Volume)).into()]);

        // Held still: the timer starts it.
        input.touch(Source::Mouse, &volume(), 300.0);
        assert_eq!(events(&input.timer(Source::Mouse, Timer::Slide, &state)), vec![Slide::Start { control: Control::Volume, x: 290.0, width: 600.0 }.into()]);
        // Lifted before either: a tap (it opens the tab).
        input.up(Source::Mouse, End::Release);
        input.touch(Source::Mouse, &volume(), 300.0);
        let up = input.up(Source::Mouse, End::Release);
        assert!(up.contains(&Action::Disarm(Source::Mouse, Timer::Slide)));
        assert!(events(&up).contains(&PointerEvent::Activate(Id::Control(Control::Volume)).into()));
    }

    #[test]
    fn a_second_finger_on_a_control_meanwhile_is_a_tap() {
        let mut input = Input::default();
        input.touch(Source::Touch(1), &volume(), 300.0);
        let second = input.touch(Source::Touch(2), &volume(), 100.0);
        assert!(!second.iter().any(|a| matches!(a, Action::Arm(_, Timer::Slide, _))));
        assert!(input.moved(Source::Touch(2), 200.0).is_empty());
        // The same source again is ignored until it lifts.
        assert!(input.touch(Source::Touch(1), &tab(), 0.0).is_empty());
    }

    #[test]
    fn a_level_row_follows_the_finger_and_the_release_focuses_it() {
        let state = State::new(Init::default());
        let mut input = Input::default();
        let down = input.touch(Source::Touch(1), &level(), 200.0);
        assert!(events(&down).contains(&LevelEvent::Set(Level::Volume, 50).into()));
        assert!(!down.iter().any(|a| matches!(a, Action::Arm(..))));
        assert_eq!(events(&input.moved(Source::Touch(1), 350.0)), vec![LevelEvent::Set(Level::Volume, 100).into()]);
        assert!(input.timer(Source::Touch(1), Timer::Hold, &state).is_empty());
        let up = input.up(Source::Touch(1), End::Release);
        assert_eq!(events(&up), vec![PointerEvent::Activate(Id::Level(Level::Volume)).into(), PointerEvent::Up(Id::Level(Level::Volume)).into()]);
        assert!(input.moved(Source::Touch(1), 100.0).is_empty());
    }

    #[test]
    fn pad_a_is_released_when_the_pad_leaves_the_screens() {
        let mut input = Input::default();
        input.press(Source::PadA, Id::Tab(Tab::Wifi));
        assert!(input.pad_route(PadRoute::Ui).is_empty());
        let released = input.pad_route(PadRoute::Game);
        assert_eq!(events(&released), vec![PointerEvent::Up(Id::Tab(Tab::Wifi)).into()]);
        assert!(!input.pressed(Source::PadA));
        assert!(input.pad_route(PadRoute::Game).is_empty());
    }

    #[test]
    fn cancelled_touches_end_without_a_tap() {
        let mut input = Input::default();
        input.touch(Source::Touch(1), &tab(), 0.0);
        input.touch(Source::Touch(2), &volume(), 300.0);
        input.touch(Source::Mouse, &tab(), 0.0);
        let cancelled = input.cancel_touches();
        assert!(!events(&cancelled).iter().any(|e| matches!(e, Event::Pointer(PointerEvent::Activate(_)))));
        assert!(input.pressed(Source::Mouse) && !input.pressed(Source::Touch(1)) && !input.pressed(Source::Touch(2)));
    }
}
