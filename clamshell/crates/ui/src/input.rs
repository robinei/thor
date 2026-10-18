//! Presses: from touch, the mouse, the pad's A and Enter (on a desktop), each from its own source. A
//! pure state machine: it is told what happens (down, moved, up, a timer fired) and answers with what to
//! do: events for the state, and timers to arm or disarm (the caller runs them on its event loop).
//!
//! A press is a tap (it acts on release, if that is still on its target) until its hold timer fires: then it is held (keys repeat, the
//! rest get one long press) and the release only lifts it. A finger on volume or brightness may become
//! a slide (held there briefly, or moved sideways): the slider comes up under it and follows it. A finger
//! on a level row sets it where it is, and follows it. A finger on a scrolled list that moves up or down
//! past [`DRAG_START`] drags the list instead (the row under it is let go without a tap); let go while
//! moving, the list flings on, slowing down, until it stops or reaches an end. Last, a finger that would
//! still tap on the second screen ([`points`]) and moves past [`POINT_START`] ([`BLANK_POINT_START`] on
//! nothing, where there is no tap to keep) points (see clamshell-core's
//! `touch`): it and the other fingers tapping there are let go without a tap and become the touchpad's,
//! as are fingers that come down there while it points, until none has been on it for [`POINT_IDLE`].
//! Positions are in the surface's logical pixels; the target's track says what they mean.

use std::time::{Duration, Instant};

use clamshell_core::levels::Level;
use clamshell_core::slider::{Control, SLIDE_AFTER, SLIDE_MOVE};
use clamshell_core::touch::{ContactPhase, Pointing};
use clamshell_core::{Event, Hold, Id, LevelEvent, ListEvent, PadRoute, PointerEvent, Slide, State};

use crate::draw::Rect;
use crate::fling::{FLING_EVERY, Fling, Trail};
use crate::toolkit::scroll::InList;
use crate::toolkit::{Target, Track};

/// A finger on a list moved this far up or down (logical pixels) drags the list.
pub(crate) const DRAG_START: f32 = 12.0;

/// A finger that would tap moved this far (logical pixels) points.
pub(crate) const POINT_START: f32 = 24.0;
/// A finger on nothing (no tap to keep) moved this far points: just past a still finger's jitter.
pub(crate) const BLANK_POINT_START: f32 = 4.0;
/// How far off its target (logical pixels) a finger may lift and still tap it.
pub(crate) const TOUCH_SLACK: f32 = 12.0;

/// The keyboard points until no finger has pointed for this long.
pub(crate) const POINT_IDLE: Duration = Duration::from_millis(1000);

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
    /// A fling's next step.
    Fling,
    /// The keyboard's pointing goes idle (see [`POINT_IDLE`]).
    Pointing,
}

/// Whether a finger on `id` may point: on the second screen below the bar (not shelf's, on the main
/// screen, where a finger is where it means; not the bar's, nor the dark screen's).
pub(crate) fn points(id: &Id) -> bool {
    !matches!(id, Id::Shelf(_) | Id::Tab(_) | Id::Control(_) | Id::PadMouse | Id::BottomWindows | Id::Wake)
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
    /// A finger pointing on the keyboard: down, moved or up at this position (the caller makes it a
    /// contact for the virtual touchpad).
    Point(Source, ContactPhase, (f32, f32)),
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
    /// Dragging a list: `y` where the finger was last.
    Dragging { list: InList, y: f32 },
    /// Pointing on the keyboard: the touchpad's.
    Pointing,
}

/// A press in progress, on the target it started on: where it came down, and the list the target is
/// in; the finger's last positions (for a fling's speed).
#[derive(Debug, Clone, PartialEq)]
struct Press {
    id: Id,
    /// Where the target is (a finger's or the mouse's; none for the pad's A and Enter).
    rect: Option<Rect>,
    gesture: Gesture,
    down: (f32, f32),
    at: (f32, f32),
    list: Option<InList>,
    trail: Trail,
}

impl Press {
    fn new(id: Id, rect: Option<Rect>, gesture: Gesture, at: (f32, f32), list: Option<InList>, now: Instant) -> Press {
        Press { id, rect, gesture, down: at, at, list, trail: Trail::new(now, at.1) }
    }

    /// Whether it is (still) on its target: the mouse exactly, a finger within [`TOUCH_SLACK`] of it (a
    /// finger rolls a little off a key as it lifts).
    fn on_target(&self, source: Source) -> bool {
        let slack = if matches!(source, Source::Touch(_)) { TOUCH_SLACK } else { 0.0 };
        self.rect.is_none_or(|r| Rect::new(r.x - slack, r.y - slack, r.w + 2.0 * slack, r.h + 2.0 * slack).contains(self.at.0, self.at.1))
    }

    /// A finger that would still tap where pointing is: it may point.
    fn may_point(&self, source: Source) -> bool {
        matches!(source, Source::Touch(_)) && self.gesture == Gesture::Tap && points(&self.id)
    }
}

/// The presses in progress, one per source, and a fling.
#[derive(Debug, Default)]
pub(crate) struct Input {
    presses: Vec<(Source, Press)>,
    /// A list moving on after a drag, on the timer of the source that dragged it.
    fling: Option<(Source, Fling)>,
    /// The keyboard's pointing going idle, on the timer of the finger that pointed last.
    idle: Option<Source>,
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
    pub fn press(&mut self, source: Source, id: Id, now: Instant) -> Vec<Action> {
        if self.pressed(source) {
            return vec![];
        }
        self.presses.push((source, Press::new(id.clone(), None, Gesture::Tap, (0.0, 0.0), None, now)));
        vec![Action::Emit(PointerEvent::Down(id).into()), Action::Arm(source, Timer::Hold, LONG_PRESS)]
    }

    /// A finger or the mouse comes down on `target` at (`x`, `y`). Touch hides the pad's focus ring, and
    /// stops a fling.
    pub fn touch(&mut self, source: Source, target: &Target, (x, y): (f32, f32), now: Instant) -> Vec<Action> {
        if self.pressed(source) {
            return vec![];
        }
        let mut actions = self.stop_fling();
        let gesture = match (&target.id, Level::of(&target.id), target.track) {
            (Id::Control(control), _, Some(track)) if !self.sliding() => {
                let at = track.along(x);
                Gesture::PendingSlide { control: *control, down_x: at, x: at, track }
            }
            (_, Some(level), Some(track)) => Gesture::LevelDrag { level, track },
            _ => Gesture::Tap,
        };
        let id = target.id.clone();
        self.presses.push((source, Press::new(id.clone(), Some(target.rect), gesture, (x, y), target.list, now)));
        actions.extend([Action::Emit(PointerEvent::HideFocus.into()), Action::Emit(PointerEvent::Down(id).into())]);
        match gesture {
            // A keymap parameter's row has nothing to tap: it slides at once, from where the finger came
            // down (so a touch without moving changes nothing).
            Gesture::PendingSlide { control: Control::Param(_), .. } => {
                if let Some((_, press)) = self.presses.last_mut() {
                    actions.extend(Self::start_slide(press));
                }
            }
            Gesture::PendingSlide { .. } => {
                actions.push(Action::Arm(source, Timer::Hold, LONG_PRESS));
                actions.push(Action::Arm(source, Timer::Slide, SLIDE_AFTER));
            }
            Gesture::LevelDrag { level, track } => actions.push(Action::Emit(LevelEvent::Set(level, track.value_at(x)).into())),
            _ => actions.push(Action::Arm(source, Timer::Hold, LONG_PRESS)),
        }
        actions
    }

    /// A finger comes down on `target` (a key) while the keyboard points: it points too.
    pub fn point(&mut self, source: Source, target: &Target, at: (f32, f32), now: Instant) -> Vec<Action> {
        if self.pressed(source) {
            return vec![];
        }
        let mut actions = self.stop_fling();
        actions.extend(self.idle.take().map(|s| Action::Disarm(s, Timer::Pointing)));
        self.presses.push((source, Press::new(target.id.clone(), Some(target.rect), Gesture::Pointing, at, None, now)));
        actions.push(Action::Point(source, ContactPhase::Down, at));
        actions
    }

    /// Fingers tapping keys point (one moved far enough): each let go without typing, and down on the
    /// touchpad where it is.
    fn start_pointing(&mut self) -> Vec<Action> {
        let mut actions: Vec<Action> = self.idle.take().map(|s| Action::Disarm(s, Timer::Pointing)).into_iter().collect();
        for (source, press) in self.presses.iter_mut().filter(|(s, p)| p.may_point(*s)) {
            press.gesture = Gesture::Pointing;
            actions.push(Action::Disarm(*source, Timer::Hold));
            actions.push(Action::Emit(PointerEvent::Up(press.id.clone()).into()));
            actions.push(Action::Point(*source, ContactPhase::Down, press.at));
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

    /// Ends a fling, if one is on.
    fn stop_fling(&mut self) -> Vec<Action> {
        self.fling.take().map(|(source, _)| Action::Disarm(source, Timer::Fling)).into_iter().collect()
    }

    /// A finger or the mouse moved to (`x`, `y`): a pending slide may start; the slider or a level row
    /// follows; a tap on a list far enough up or down becomes a drag, which the list follows.
    pub fn moved(&mut self, source: Source, (x, y): (f32, f32), now: Instant) -> Vec<Action> {
        let Some(press) = self.get(source) else {
            return vec![];
        };
        press.trail.follow(now, y);
        press.at = (x, y);
        if let (Gesture::Tap, Some(list)) = (press.gesture, press.list)
            && (y - press.down.1).abs() >= DRAG_START
        {
            press.gesture = Gesture::Dragging { list, y };
            let rows = -(y - press.down.1) / list.row_h;
            return vec![
                Action::Disarm(source, Timer::Hold),
                Action::Emit(PointerEvent::Up(press.id.clone()).into()),
                Action::Emit(ListEvent::Scroll(list.list, rows).into()),
            ];
        }
        // (Last: what moves of its own took the motion first.)
        let start = if press.id == Id::Blank { BLANK_POINT_START } else { POINT_START };
        if press.may_point(source) && (x - press.down.0).hypot(y - press.down.1) >= start {
            return self.start_pointing();
        }
        match &mut press.gesture {
            Gesture::Dragging { list, y: last } => {
                let rows = -(y - *last) / list.row_h;
                *last = y;
                vec![Action::Emit(ListEvent::Scroll(list.list, rows).into())]
            }
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
            Gesture::Pointing => vec![Action::Point(source, ContactPhase::Move, (x, y))],
            Gesture::Tap | Gesture::Held => vec![],
        }
    }

    /// The source's `timer` fired at `now`. A hold repeats keys (`state` says which repeat) and
    /// long-presses the rest; the slide timer starts the slide; a fling steps.
    pub fn timer(&mut self, source: Source, timer: Timer, state: &State, now: Instant) -> Vec<Action> {
        if timer == Timer::Fling {
            return self.fling_step(state, now);
        }
        if timer == Timer::Pointing {
            self.idle = None;
            let pointing = self.presses.iter().any(|(_, p)| p.gesture == Gesture::Pointing);
            return if pointing { vec![] } else { vec![Action::Emit(Pointing::Idle.into())] };
        }
        let Some(press) = self.get(source) else {
            return vec![];
        };
        let mut actions = vec![];
        match timer {
            Timer::Slide => return Self::start_slide(press),
            Timer::Fling | Timer::Pointing => return actions,
            // (The slide's timer is the shorter: this comes first only if that one couldn't run.)
            Timer::Hold if matches!(press.gesture, Gesture::PendingSlide { .. }) => {
                actions.push(Action::Disarm(source, Timer::Slide));
                actions.extend(Self::start_slide(press));
            }
            Timer::Hold if matches!(press.gesture, Gesture::LevelDrag { .. } | Gesture::Dragging { .. } | Gesture::Pointing) => return actions,
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

    /// A fling's step (see [`Fling::step`]).
    fn fling_step(&mut self, state: &State, now: Instant) -> Vec<Action> {
        let Some((source, fling)) = self.fling.as_mut() else {
            return vec![];
        };
        let (rows, goes_on) = fling.step(state, now);
        let mut actions = vec![Action::Emit(ListEvent::Scroll(fling.list, rows).into())];
        if goes_on {
            actions.push(Action::Arm(*source, Timer::Fling, FLING_EVERY));
        } else {
            self.fling = None;
        }
        actions
    }

    /// A press ends at `now`: a tap activates (unless cancelled); a hold already acted; a slide ends; a
    /// drag let go while moving flings on.
    pub fn up(&mut self, source: Source, end: End, state: &State, now: Instant) -> Vec<Action> {
        let Some(at) = self.presses.iter().position(|(s, _)| *s == source) else {
            return vec![];
        };
        let (_, press) = self.presses.remove(at);
        let mut actions = vec![Action::Disarm(source, Timer::Hold)];
        // (The key was let go when it began pointing.) The last finger off: idle after a while.
        if press.gesture == Gesture::Pointing {
            actions.push(Action::Point(source, ContactPhase::Up, press.at));
            if !self.presses.iter().any(|(_, p)| p.gesture == Gesture::Pointing) {
                self.idle = Some(source);
                actions.push(Action::Arm(source, Timer::Pointing, POINT_IDLE));
            }
            return actions;
        }
        if let Gesture::Dragging { list, .. } = press.gesture {
            // (The row was let go when the drag began.)
            if let Some(fling) = Fling::after_drag(list, &press.trail, end == End::Release, state, now) {
                self.fling = Some((source, fling));
                actions.push(Action::Arm(source, Timer::Fling, FLING_EVERY));
            }
            return actions;
        }
        let activates = match press.gesture {
            Gesture::Sliding { .. } => {
                actions.push(Action::Emit(Slide::End.into()));
                false
            }
            Gesture::PendingSlide { .. } => {
                actions.push(Action::Disarm(source, Timer::Slide));
                true
            }
            // (A tap let go off its target is no tap.)
            Gesture::Tap => press.on_target(source),
            Gesture::LevelDrag { .. } => true,
            Gesture::Held | Gesture::Dragging { .. } | Gesture::Pointing => false,
        };
        if activates && end == End::Release {
            actions.push(Action::Emit(PointerEvent::Activate(press.id.clone()).into()));
        }
        actions.push(Action::Emit(PointerEvent::Up(press.id).into()));
        actions
    }

    /// All touch points gone without a tap (cancelled, or the touch device went away).
    pub fn cancel_touches(&mut self, state: &State) -> Vec<Action> {
        let touches: Vec<Source> = self.presses.iter().map(|(s, _)| *s).filter(|s| matches!(s, Source::Touch(_))).collect();
        let now = Instant::now();
        touches.into_iter().flat_map(|source| self.up(source, End::Cancel, state, now)).collect()
    }

    /// The pad went `route`: if that is away from ClamShell's screens while A is held on a target, its
    /// release won't come here, so the press ends now (else the target stays pressed, a key repeating).
    pub fn pad_route(&mut self, route: PadRoute, state: &State) -> Vec<Action> {
        if route != PadRoute::Ui && self.pressed(Source::PadA) { self.up(Source::PadA, End::Cancel, state, Instant::now()) } else { vec![] }
    }
}

#[cfg(test)]
#[allow(clippy::indexing_slicing)]
mod tests {
    use super::*;
    use clamshell_core::keyboard::{self, Key, KeyPos};
    use clamshell_core::{Init, ListId, Tab};

    fn t0() -> Instant {
        *START.get_or_init(Instant::now)
    }

    static START: std::sync::OnceLock<Instant> = std::sync::OnceLock::new();

    fn blank() -> State {
        State::new(Init::default())
    }

    fn target(id: Id, track: Option<Track>) -> Target {
        Target { rect: Rect::new(0.0, 0.0, 100.0, 50.0), id, track, list: None }
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
        assert_eq!(events(&down), vec![PointerEvent::HideFocus.into(), PointerEvent::Down(Id::Tab(Tab::Wifi)).into()]);
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
        use clamshell_core::wifi::{Network, Wifi};
        let mut s = State::new(Init::default());
        let networks = (0..n).map(|i| Network { ssid: format!("n{i}"), ..Network::default() }).collect();
        s.apply(Event::Reading(clamshell_core::Reading::Wifi(Wifi { device: true, enabled: true, networks, ..Wifi::default() })));
        s.apply(ListEvent::Visible(ListId::Wifi, 5.0).into());
        s
    }

    fn row(list: ListId, i: u16) -> Target {
        let id = list.row(i);
        Target { rect: Rect::new(0.0, 0.0, 100.0, 1000.0), id, track: None, list: Some(InList { list, row_h: 50.0 }) }
    }

    /// Applies what the actions emit to `s`; whether a timer was armed again.
    fn run(s: &mut State, actions: &[Action]) -> bool {
        for e in events(actions) {
            s.apply(e);
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
        // Moved past the threshold: the row is let go without a tap, the list follows 1:1 in rows.
        input.touch(Source::Touch(1), &wifi, (5.0, 300.0), t);
        let moved = input.moved(Source::Touch(1), (5.0, 275.0), t + Duration::from_millis(500));
        assert_eq!(moved[0], Action::Disarm(Source::Touch(1), Timer::Hold));
        assert_eq!(events(&moved), vec![PointerEvent::Up(wifi.id.clone()).into(), ListEvent::Scroll(ListId::Wifi, 0.5).into()]);
        assert_eq!(events(&input.moved(Source::Touch(1), (5.0, 325.0), t + Duration::from_millis(600))), vec![ListEvent::Scroll(ListId::Wifi, -1.0).into()]);
        assert!(input.timer(Source::Touch(1), Timer::Hold, &state, t).is_empty(), "no long press while dragging");
        // Let go slowly: no tap, no fling.
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
        let mut last = state.list_offset(ListId::Wifi);
        let mut gaps = vec![];
        loop {
            at += 16;
            steps += 1;
            let fx = input.timer(Source::Touch(1), Timer::Fling, &state, ms(at));
            let more = run(&mut state, &fx);
            let now = state.list_offset(ListId::Wifi);
            gaps.push(now - last);
            last = now;
            if !more || steps > 1000 {
                break;
            }
        }
        assert!(steps < 1000, "it ends");
        assert!(gaps.windows(2).take(10).all(|w| w[1] <= w[0] + 1e-4), "slowing down: {gaps:?}");
        assert_eq!(state.list_offset(ListId::Wifi), 15.0, "at the end: {gaps:?}");
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
        let mut state = State::new(Init { library: clamshell_library::demo_library(), ..Init::default() });
        state.apply(clamshell_core::ShelfEvent::Rows(3).into());
        let mut input = Input::default();
        let t = t0();
        let ms = |n| t + Duration::from_millis(n);
        let game = Target { list: Some(InList { list: ListId::Shelf, row_h: 100.0 }), ..row(ListId::Shelf, 0) };
        input.touch(Source::Touch(1), &game, (5.0, 400.0), t);
        let fx = input.moved(Source::Touch(1), (5.0, 260.0), ms(500));
        run(&mut state, &fx);
        assert_eq!(state.list_offset(ListId::Shelf), 1.4);
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
        assert_eq!(state.list_offset(ListId::Shelf), 1.0, "the nearest row");
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
        assert_eq!(
            events(&input.timer(Source::Mouse, Timer::Slide, &state, t0())),
            vec![Slide::Start { control: Control::Volume, x: 290.0, width: 600.0 }.into()]
        );
        // Lifted before either: a tap (it opens the tab).
        input.up(Source::Mouse, End::Release, &blank(), t0());
        input.touch(Source::Mouse, &volume(), (300.0, 0.0), t0());
        let up = input.up(Source::Mouse, End::Release, &blank(), t0());
        assert!(up.contains(&Action::Disarm(Source::Mouse, Timer::Slide)));
        assert!(events(&up).contains(&PointerEvent::Activate(Id::Control(Control::Volume)).into()));
    }

    #[test]
    fn a_keymap_parameter_slides_at_once() {
        use clamshell_core::keymap::Param;
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
        // Ctrl tapping, a key held (repeating), a finger moving: Ctrl and the mover point, the held key stays.
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
        // Lifted: the last pointing finger off arms the idle.
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
        // Diagonal: the list takes it (12 up) before pointing could (24 along).
        input.touch(finger, &wifi, (5.0, 300.0), t);
        let moved = input.moved(finger, (5.0 + 20.0, 300.0 - DRAG_START), t);
        assert!(points(&moved).is_empty() && events(&moved).iter().any(|e| matches!(e, Event::List(ListEvent::Scroll(..)))), "{moved:?}");
        assert!(points(&input.moved(finger, (5.0 + 60.0, 300.0 - DRAG_START), t)).is_empty(), "dragging: a list's");
        input.up(finger, End::Release, &state, t);
        // Sideways: it points.
        input.touch(finger, &wifi, (5.0, 300.0), t);
        let moved = input.moved(finger, (5.0 + POINT_START, 300.0 - DRAG_START + 1.0), t);
        assert_eq!(points(&moved), vec![(finger, ContactPhase::Down)]);
        input.up(finger, End::Release, &state, t);
        // Nowhere on shelf (the main screen), nor by the mouse.
        let game = target(Id::Shelf(clamshell_core::ShelfId::Game("g".into())), None);
        input.touch(Source::Touch(2), &game, (0.0, 0.0), t);
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
        // A key keeps its tap that long.
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
}
