//! Presses from touch, the mouse, the pad's A and, on a desktop, Enter, each from its own source. This is
//! a pure state machine. It is told what happens (down, moved, up, a timer fired) and answers with what to
//! do: events for the state, and timers to arm or disarm, which the caller runs on its event loop.
//!
//! A press is a tap until its hold timer fires. A tap acts on release, if the release is still on its
//! target. Once the timer fires, the press is held: keys repeat, other targets get one long press, and the
//! release only lifts it.
//!
//! A finger on volume or brightness may become a slide, by being held there briefly or moved sideways.
//! The slider then comes up under it and follows it. A finger on a level row sets the level where it is,
//! and follows it.
//!
//! A finger on a scrolled list that moves up or down past [`DRAG_START`] drags the list instead, and the
//! row under it is let go without a tap. If it is let go while moving, the list flings on, slowing down,
//! until it stops or reaches an end.
//!
//! Last, a finger on the second screen that would still tap ([`points`]) starts pointing when it moves
//! past [`POINT_START`] (see clamshell-state's `touch`). On nothing, where there is no tap to keep, the
//! distance is [`BLANK_POINT_START`]. That finger and the other fingers tapping there are let go without
//! a tap and go to the touchpad. So do fingers that come down there while it points, until no finger has
//! been on it for [`POINT_IDLE`].
//!
//! Positions are in the surface's logical pixels; the target's track says what they mean.

use std::time::{Duration, Instant};

use clamshell_state::bottom::touch::{ContactPhase, Pointing};
use clamshell_state::levels::Level;
use clamshell_state::levels::slider::{Control, SLIDE_AFTER, SLIDE_MOVE};
use clamshell_state::{Event, Hold, Id, LevelEvent, ListEvent, PadRoute, PointerEvent, Slide, State};

use crate::gfx::Rect;
use crate::input::fling::{FLING_EVERY, Fling, Trail};
use crate::toolkit::scroll::InList;
use crate::toolkit::{Target, Track};

/// A finger on a list that moves this far up or down, in logical pixels, drags the list.
pub(crate) const DRAG_START: f32 = 12.0;

/// A finger that would tap starts pointing when it moves this far, in logical pixels.
pub(crate) const POINT_START: f32 = 24.0;
/// A finger on nothing, with no tap to keep, starts pointing when it moves this far. This is just past a
/// still finger's jitter.
pub(crate) const BLANK_POINT_START: f32 = 4.0;
/// How far off its target, in logical pixels, a finger may lift and still tap it.
pub(crate) const TOUCH_SLACK: f32 = 12.0;

/// The keyboard points until no finger has pointed for this long.
pub(crate) const POINT_IDLE: Duration = Duration::from_millis(1000);

/// A press held this long is a hold: keys start repeating, modifiers lock.
pub(crate) const LONG_PRESS: Duration = Duration::from_millis(450);
/// The key repeat interval while held: 25 per second.
pub(crate) const REPEAT: Duration = Duration::from_millis(40);

/// Turns the compositor's event times, in milliseconds on its monotonic clock, into instants. It
/// measures from the earliest base seen, since an event handled late only looks later. So a batch of
/// events handled at once keeps the times they happened at, which a fling's speed needs.
#[derive(Debug, Default)]
pub(crate) struct EventClock {
    base: Option<(Instant, u32)>,
}

impl EventClock {
    pub fn at(&mut self, ms: u32) -> Instant {
        let now = Instant::now();
        if let Some((base, base_ms)) = self.base {
            let at = base + Duration::from_millis(u64::from(ms.wrapping_sub(base_ms)));
            if at <= now {
                return at;
            }
        }
        // This is the first event, or it came sooner than the base says, so the base was handled late.
        self.base = Some((now, ms));
        now
    }
}

/// Where a press comes from: each touch point, the mouse, the pad's A, or Enter on a desktop.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Source {
    Touch(i32),
    Mouse,
    PadA,
    Enter,
}

/// The timers: the hold, for a long press or key repeat; the wait before a slide; a fling's steps; and
/// pointing going idle.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) enum Timer {
    Hold,
    Slide,
    /// A fling's next step.
    Fling,
    /// The keyboard's pointing goes idle (see [`POINT_IDLE`]).
    Pointing,
}

/// Whether a finger on `id` may point. That is anywhere on the second screen below the bar. It excludes
/// shelf's targets on the main screen, where a finger is where it means, and the bar's and the dark
/// screen's targets.
pub(crate) fn points(id: &Id) -> bool {
    !matches!(id, Id::Shelf(_) | Id::Tab(_) | Id::Control(_) | Id::PadMouse | Id::Wake)
}

/// What the caller does.
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum Action {
    /// Applies the event to the state.
    Emit(Event),
    /// Starts or restarts the source's timer, to call [`Input::timer`] after the duration.
    Arm(Source, Timer, Duration),
    /// Stops the source's timer, if it runs.
    Disarm(Source, Timer),
    /// A finger pointing on the keyboard came down, moved or lifted at this position. The caller makes it
    /// a contact for the virtual touchpad.
    Point(Source, ContactPhase, (f32, f32)),
}

/// How a press ends. On release a tap acts. A press is cancelled when the touch was taken away or the pad
/// left.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum End {
    Release,
    Cancel,
}

/// What a press is doing.
#[derive(Debug, Clone, Copy, PartialEq)]
enum Gesture {
    /// Acts on release.
    Tap,
    /// The hold timer acted with a long press or a repeat, so the release only lifts.
    Held,
    /// On volume or brightness. It becomes a slide when held or moved sideways, and otherwise a tap.
    /// `down_x` and `x` are where it came down and where it is now, along the track.
    PendingSlide { control: Control, down_x: f32, x: f32, track: Track },
    /// Moving the slider. The release ends the slide.
    Sliding { track: Track },
    /// Setting a level row. Holding it still doesn't long-press, and the release focuses it as a tap
    /// does.
    LevelDrag { level: Level, track: Track },
    /// Dragging a list. `y` is where the finger was last.
    Dragging { list: InList, y: f32 },
    /// Pointing on the keyboard, as a touchpad finger.
    Pointing,
}

/// A press in progress on the target it started on. It keeps where it came down, the list the target is
/// in, and the finger's last positions for a fling's speed.
#[derive(Debug, Clone, PartialEq)]
struct Press {
    id: Id,
    /// Where the target is, for a finger or the mouse. It is `None` for the pad's A and Enter.
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

    /// Whether it is still on its target. The mouse must be exactly on it, and a finger within
    /// [`TOUCH_SLACK`] of it, since a finger rolls a little off a key as it lifts.
    fn on_target(&self, source: Source) -> bool {
        let slack = if matches!(source, Source::Touch(_)) { TOUCH_SLACK } else { 0.0 };
        self.rect.is_none_or(|r| Rect::new(r.x - slack, r.y - slack, r.w + 2.0 * slack, r.h + 2.0 * slack).contains(self.at.0, self.at.1))
    }

    /// Whether it may point: a finger that would still tap, where pointing is allowed.
    fn may_point(&self, source: Source) -> bool {
        matches!(source, Source::Touch(_)) && self.gesture == Gesture::Tap && points(&self.id)
    }
}

/// The presses in progress, one per source, and a fling.
#[derive(Debug, Default)]
pub(crate) struct Input {
    presses: Vec<(Source, Press)>,
    /// A list moving on after a drag. It uses the timer of the source that dragged it.
    fling: Option<(Source, Fling)>,
    /// The keyboard's pointing going idle. It uses the timer of the finger that pointed last.
    idle: Option<Source>,
}

impl Input {
    fn get(&mut self, source: Source) -> Option<&mut Press> {
        self.presses.iter_mut().find(|(s, _)| *s == source).map(|(_, p)| p)
    }

    pub fn pressed(&self, source: Source) -> bool {
        self.presses.iter().any(|(s, _)| *s == source)
    }

    /// Whether a slide is pending or moving. There is only one slider, so another finger on a control
    /// meanwhile taps.
    fn sliding(&self) -> bool {
        self.presses.iter().any(|(_, p)| matches!(p.gesture, Gesture::PendingSlide { .. } | Gesture::Sliding { .. }))
    }

    /// The pad's A or Enter on the focused target, which is a plain press.
    pub fn press(&mut self, source: Source, id: Id, now: Instant) -> Vec<Action> {
        if self.pressed(source) {
            return vec![];
        }
        self.presses.push((source, Press::new(id.clone(), None, Gesture::Tap, (0.0, 0.0), None, now)));
        vec![Action::Emit(PointerEvent::Down(id, None).into()), Action::Arm(source, Timer::Hold, LONG_PRESS)]
    }

    /// A finger or the mouse comes down on `target` at (`x`, `y`). Touch hides the pad's focus ring and
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
        actions.extend([Action::Emit(PointerEvent::HideFocus.into()), Action::Emit(PointerEvent::Down(id, target.item).into())]);
        match gesture {
            // A keymap parameter's row has nothing to tap, so it slides at once from where the finger
            // came down. A touch without moving changes nothing.
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

    /// A finger comes down on `target`, a key, while the keyboard points. The finger points too.
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

    /// The fingers tapping keys start pointing, because one moved far enough. Each is let go without
    /// typing and comes down on the touchpad where it is.
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

    /// Starts a pending slide. The slider comes up under the finger.
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

    /// A finger or the mouse moved to (`x`, `y`). A pending slide may start, and the slider or a level
    /// row follows. A tap on a list that moves far enough up or down becomes a drag, which the list
    /// follows.
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
        // Pointing is checked last, after the gestures that move something of their own.
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

    /// The source's `timer` fired at `now`. A hold repeats targets such as keys (`state` says which) and
    /// long-presses the rest. The slide timer starts the slide, and the fling timer steps a fling.
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
            // The slide's timer is the shorter one, so the hold comes first only if that one couldn't run.
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
        match state.keyboard().hold(&press.id) {
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

    /// A press ends at `now`. A tap activates unless it was cancelled, a hold has already acted, a slide
    /// ends, and a drag let go while moving flings on.
    pub fn up(&mut self, source: Source, end: End, state: &State, now: Instant) -> Vec<Action> {
        let Some(at) = self.presses.iter().position(|(s, _)| *s == source) else {
            return vec![];
        };
        let (_, press) = self.presses.remove(at);
        let mut actions = vec![Action::Disarm(source, Timer::Hold)];
        // The key was let go when it began pointing. Once the last finger is off, pointing goes idle
        // after a while.
        if press.gesture == Gesture::Pointing {
            actions.push(Action::Point(source, ContactPhase::Up, press.at));
            if !self.presses.iter().any(|(_, p)| p.gesture == Gesture::Pointing) {
                self.idle = Some(source);
                actions.push(Action::Arm(source, Timer::Pointing, POINT_IDLE));
            }
            return actions;
        }
        if let Gesture::Dragging { list, .. } = press.gesture {
            // The row was let go when the drag began.
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
            // A tap let go off its target doesn't count.
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

    /// The pad was routed to `route`. If that is away from ClamShell's screens while A is held on a
    /// target, A's release won't come here, so the press ends now. Otherwise the target would stay
    /// pressed, and a key would keep repeating.
    pub fn pad_route(&mut self, route: PadRoute, state: &State) -> Vec<Action> {
        if route != PadRoute::Ui && self.pressed(Source::PadA) { self.up(Source::PadA, End::Cancel, state, Instant::now()) } else { vec![] }
    }
}

#[cfg(test)]
#[allow(clippy::indexing_slicing)]
mod tests;
