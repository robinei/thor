//! The pad driving ClamShell's screens ([`Event::Pad`]): buttons as they come, the D-pad and left stick
//! as directions that repeat while held.

use std::time::{Duration, Instant};

use clamshell_core::{Event, PadInput};
use evdev::{AbsoluteAxisCode, KeyCode};

use super::route::{Mapping, Out};
use crate::input::{Press, Range, Raw};

/// A direction held this long starts repeating, this often.
pub const REPEAT_AFTER: Duration = Duration::from_millis(350);
pub const REPEAT_EVERY: Duration = Duration::from_millis(90);
/// The left stick counts as a direction past this (of full).
const STICK_DIRECTION: f32 = 0.6;
/// A trigger counts as pulled past this (of full).
const TRIGGER_PULLED: f32 = 0.5;

#[derive(Debug)]
pub struct Navigator {
    stick_range: Range,
    trigger_range: Range,
    /// The left and right triggers pulled.
    pulled: (bool, bool),
    hat: (i32, i32),
    stick: (f32, f32),
    held: Option<PadInput>,
    next: Option<Instant>,
}

impl Navigator {
    pub fn new(stick_range: Range, trigger_range: Range) -> Self {
        Navigator { stick_range, trigger_range, pulled: (false, false), hat: (0, 0), stick: (0.0, 0.0), held: None, next: None }
    }

    /// A trigger's value: `Trigger` as it is pulled past [`TRIGGER_PULLED`] (not again until let go).
    fn trigger(&mut self, right: bool, v: i32, out: &mut Out) {
        let pulled = self.trigger_range.unit(v) >= TRIGGER_PULLED;
        let was = if right { &mut self.pulled.1 } else { &mut self.pulled.0 };
        if pulled && !*was {
            out.events.push(Event::Pad(PadInput::Trigger(if right { 1 } else { -1 })));
        }
        *was = pulled;
    }

    /// The direction held now: the D-pad's, else the stick's (its larger axis).
    fn direction(&self) -> Option<PadInput> {
        let from = |(x, y): (f32, f32), threshold: f32| match (x.abs() >= y.abs(), x, y) {
            (true, x, _) if x <= -threshold => Some(PadInput::Left),
            (true, x, _) if x >= threshold => Some(PadInput::Right),
            (false, _, y) if y <= -threshold => Some(PadInput::Up),
            (false, _, y) if y >= threshold => Some(PadInput::Down),
            _ => None,
        };
        from((self.hat.0 as f32, self.hat.1 as f32), 0.5).or_else(|| from(self.stick, STICK_DIRECTION))
    }

    /// A button's meaning: A down and up (it is held like a press); the others when pressed (not on
    /// release or autorepeat).
    fn button(key: KeyCode, press: Press) -> Option<PadInput> {
        match (key, press) {
            (KeyCode::BTN_SOUTH, Press::Down | Press::Up) => Some(PadInput::A(press == Press::Down)),
            (_, Press::Up | Press::Repeat) => None,
            (KeyCode::BTN_EAST, _) => Some(PadInput::B),
            (KeyCode::BTN_NORTH, _) => Some(PadInput::Y),
            (KeyCode::BTN_WEST, _) => Some(PadInput::X),
            (KeyCode::BTN_START, _) => Some(PadInput::Start),
            (KeyCode::BTN_TL, _) => Some(PadInput::Shoulder(-1)),
            (KeyCode::BTN_TR, _) => Some(PadInput::Shoulder(1)),
            (KeyCode::BTN_TL2, _) => Some(PadInput::Trigger(-1)),
            (KeyCode::BTN_TR2, _) => Some(PadInput::Trigger(1)),
            (KeyCode::BTN_SELECT, _) => Some(PadInput::Switch),
            _ => None,
        }
    }
}

impl Mapping for Navigator {
    fn event(&mut self, raw: Raw, now: Instant, out: &mut Out) {
        match (raw.as_abs(), raw.as_key()) {
            (Some((AbsoluteAxisCode::ABS_HAT0X, v)), _) => self.hat.0 = v.signum(),
            (Some((AbsoluteAxisCode::ABS_HAT0Y, v)), _) => self.hat.1 = v.signum(),
            (Some((AbsoluteAxisCode::ABS_X, v)), _) => self.stick.0 = self.stick_range.centered(v),
            (Some((AbsoluteAxisCode::ABS_Y, v)), _) => self.stick.1 = self.stick_range.centered(v),
            (Some((AbsoluteAxisCode::ABS_Z, v)), _) => self.trigger(false, v, out),
            (Some((AbsoluteAxisCode::ABS_RZ, v)), _) => self.trigger(true, v, out),
            (_, Some((key, press))) => out.events.extend(Navigator::button(key, press).map(Event::Pad)),
            _ => {}
        }
        let direction = self.direction();
        if direction != self.held {
            self.held = direction;
            self.next = direction.map(|d| {
                out.events.push(Event::Pad(d));
                now + REPEAT_AFTER
            });
        }
    }

    /// Repeats a held direction when due; ticks wanted while one is held.
    fn tick(&mut self, now: Instant, out: &mut Out) -> Option<Instant> {
        let (Some(direction), Some(next)) = (self.held, self.next) else {
            return None;
        };
        if now >= next {
            out.events.push(Event::Pad(direction));
            self.next = Some(now + REPEAT_EVERY);
        }
        self.next
    }

    /// Nothing is held down outside ClamShell: only the directions are forgotten.
    fn release(&mut self, _: &mut Out) {
        *self = Navigator::new(self.stick_range, self.trigger_range);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn repeats_held_directions_and_passes_buttons() {
        let mut nav = Navigator::new(Range::STICK, Range { min: 0, max: 255 });
        let t0 = Instant::now();
        let mut out = Out::default();
        let pad = |inputs: &[PadInput]| inputs.iter().map(|&i| Event::Pad(i)).collect::<Vec<_>>();
        nav.event(Raw::abs(AbsoluteAxisCode::ABS_HAT0Y, 1), t0, &mut out);
        assert_eq!(out.events, pad(&[PadInput::Down]));
        out.events.clear();
        assert!(nav.tick(t0 + Duration::from_millis(100), &mut out).is_some());
        assert!(out.events.is_empty(), "not yet");
        nav.tick(t0 + REPEAT_AFTER, &mut out);
        nav.tick(t0 + REPEAT_AFTER + REPEAT_EVERY, &mut out);
        assert_eq!(out.events, pad(&[PadInput::Down, PadInput::Down]));
        out.events.clear();
        nav.event(Raw::abs(AbsoluteAxisCode::ABS_HAT0Y, 0), t0, &mut out);
        assert_eq!(nav.tick(t0 + Duration::from_secs(5), &mut out), None);
        // The stick past its threshold, sideways.
        nav.event(Raw::abs(AbsoluteAxisCode::ABS_X, 30000), t0, &mut out);
        nav.event(Raw::abs(AbsoluteAxisCode::ABS_X, 0), t0, &mut out);
        for (code, press) in [
            (KeyCode::BTN_SOUTH, Press::Down),
            (KeyCode::BTN_SOUTH, Press::Repeat),
            (KeyCode::BTN_SOUTH, Press::Up),
            (KeyCode::BTN_EAST, Press::Down),
            (KeyCode::BTN_EAST, Press::Up),
            (KeyCode::BTN_SELECT, Press::Down),
            (KeyCode::BTN_TR, Press::Down),
        ] {
            nav.event(Raw::key(code, press), t0, &mut out);
        }
        assert_eq!(out.events, pad(&[PadInput::Right, PadInput::A(true), PadInput::A(false), PadInput::B, PadInput::Switch, PadInput::Shoulder(1)]));
        // A trigger pulled past half: once, until let go.
        out.events.clear();
        for v in [100, 200, 250, 30, 220] {
            nav.event(Raw::abs(AbsoluteAxisCode::ABS_RZ, v), t0, &mut out);
        }
        assert_eq!(out.events, pad(&[PadInput::Trigger(1), PadInput::Trigger(1)]));
    }
}
