//! Typing into the app in front with the pad while the on-screen keyboard is open. The D-pad gives
//! arrows, A gives Enter and B gives Backspace, on the virtual mouse's keys.

use std::time::Instant;

use evdev::KeyCode;

use super::route::{Mapping, Out, release_mouse_keys};
use crate::os::evdev::{Press, Raw, hat_keys};
use clamshell_state::bottom::keyboard::KEY_HELD_AT_LEAST;

/// The keys the pad types, each held at least [`KEY_HELD_AT_LEAST`], so a game that reads the keys' state once a
/// frame sees a tap: a release sooner waits for the tick.
const KEYS: [KeyCode; 6] = [KeyCode::KEY_UP, KeyCode::KEY_DOWN, KeyCode::KEY_LEFT, KeyCode::KEY_RIGHT, KeyCode::KEY_ENTER, KeyCode::KEY_BACKSPACE];

#[derive(Debug, Clone, Copy, Default)]
pub(crate) struct Typing {
    /// Each of [`KEYS`]: when it went down, if it is down, and when its release is due, if it waits.
    held: [(Option<Instant>, Option<Instant>); KEYS.len()],
}

impl Typing {
    /// Sends `raw`, one of [`KEYS`] or a report, at `now`, holding back a release that comes too soon.
    fn send(&mut self, raw: Raw, now: Instant, out: &mut Out) {
        let Some((code, press)) = raw.as_key() else { return out.mouse.push(raw) };
        let Some(held) = KEYS.iter().position(|&k| k == code).and_then(|i| self.held.get_mut(i)) else { return out.mouse.push(raw) };
        match (press, *held) {
            // Pressed again while its release waits: it goes up first.
            (Press::Down, (_, Some(_))) => out.mouse.extend([Raw::key(code, Press::Up), Raw::REPORT, raw]),
            (Press::Up, (Some(down), None)) if now < down + KEY_HELD_AT_LEAST => {
                held.1 = Some(down + KEY_HELD_AT_LEAST);
                return;
            }
            // A release while already waiting, or of a key not down: nothing new.
            (Press::Up, (_, Some(_)) | (None, None)) => return,
            _ => out.mouse.push(raw),
        }
        *held = match press {
            Press::Down => (Some(now), None),
            _ => (None, None),
        };
    }
}

impl Mapping for Typing {
    fn event(&mut self, raw: Raw, now: Instant, out: &mut Out) {
        if let Some((axis, value)) = raw.as_abs() {
            for raw in hat_keys(axis, value).into_iter().flatten() {
                self.send(raw, now, out);
            }
            return;
        }
        let key = match raw.as_key() {
            Some((KeyCode::BTN_SOUTH, press)) => (KeyCode::KEY_ENTER, press),
            Some((KeyCode::BTN_EAST, press)) => (KeyCode::KEY_BACKSPACE, press),
            _ => return,
        };
        // (The merge drops the controller's autorepeat: a held B keeps deleting by the client's key repeat.)
        self.send(Raw::key(key.0, key.1), now, out);
        out.mouse.push(Raw::REPORT);
    }

    fn tick(&mut self, now: Instant, out: &mut Out) -> Option<Instant> {
        let mut released = false;
        for (code, held) in KEYS.iter().zip(self.held.iter_mut()) {
            if held.1.is_some_and(|at| at <= now) {
                *held = (None, None);
                out.mouse.push(Raw::key(*code, Press::Up));
                released = true;
            }
        }
        if released {
            out.mouse.push(Raw::REPORT);
        }
        self.held.iter().filter_map(|held| held.1).min()
    }

    fn release(&mut self, out: &mut Out) {
        self.held = Default::default();
        release_mouse_keys(out);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use evdev::AbsoluteAxisCode;

    #[test]
    fn maps_the_dpad_a_and_b() {
        let mut out = Out::default();
        let now = Instant::now();
        let mut typing = Typing::default();
        typing.event(Raw::abs(AbsoluteAxisCode::ABS_HAT0Y, -1), now, &mut out);
        assert_eq!(out.mouse, vec![Raw::key(KeyCode::KEY_UP, Press::Down), Raw::REPORT], "(down not being down, it isn't let go)");
        out.mouse.clear();
        typing.event(Raw::key(KeyCode::BTN_SOUTH, Press::Down), now, &mut out);
        typing.event(Raw::key(KeyCode::BTN_EAST, Press::Down), now, &mut out);
        typing.event(Raw::key(KeyCode::BTN_NORTH, Press::Down), now, &mut out);
        assert_eq!(out.mouse, vec![Raw::key(KeyCode::KEY_ENTER, Press::Down), Raw::REPORT, Raw::key(KeyCode::KEY_BACKSPACE, Press::Down), Raw::REPORT]);
    }

    /// A goes down and up with the button, but up no sooner than KEY_HELD_AT_LEAST after it went down, so a
    /// game reading the keys once a frame sees it.
    #[test]
    fn a_key_goes_up_with_its_button_but_not_too_soon() {
        let mut out = Out::default();
        let now = Instant::now();
        let mut typing = Typing::default();
        typing.event(Raw::key(KeyCode::BTN_SOUTH, Press::Down), now, &mut out);
        assert_eq!(out.mouse, vec![Raw::key(KeyCode::KEY_ENTER, Press::Down), Raw::REPORT], "down at once");
        out.mouse.clear();
        typing.event(Raw::key(KeyCode::BTN_SOUTH, Press::Up), now + KEY_HELD_AT_LEAST / 4, &mut out);
        assert!(!out.mouse.contains(&Raw::key(KeyCode::KEY_ENTER, Press::Up)), "too soon: held");
        assert_eq!(typing.tick(now + KEY_HELD_AT_LEAST / 2, &mut out), Some(now + KEY_HELD_AT_LEAST));
        out.mouse.clear();
        assert_eq!(typing.tick(now + KEY_HELD_AT_LEAST, &mut out), None);
        assert_eq!(out.mouse, vec![Raw::key(KeyCode::KEY_ENTER, Press::Up), Raw::REPORT], "up once held long enough");
        // Held longer, it goes up with the button.
        out.mouse.clear();
        let later = now + KEY_HELD_AT_LEAST * 3;
        typing.event(Raw::key(KeyCode::BTN_SOUTH, Press::Down), later, &mut out);
        typing.event(Raw::key(KeyCode::BTN_SOUTH, Press::Up), later + KEY_HELD_AT_LEAST, &mut out);
        assert_eq!(out.mouse, vec![Raw::key(KeyCode::KEY_ENTER, Press::Down), Raw::REPORT, Raw::key(KeyCode::KEY_ENTER, Press::Up), Raw::REPORT]);
        // Pressed again while its release waits, it goes up first.
        out.mouse.clear();
        let again = later + KEY_HELD_AT_LEAST * 3;
        typing.event(Raw::key(KeyCode::BTN_SOUTH, Press::Down), again, &mut out);
        typing.event(Raw::key(KeyCode::BTN_SOUTH, Press::Up), again, &mut out);
        typing.event(Raw::key(KeyCode::BTN_SOUTH, Press::Down), again, &mut out);
        let enter = |p| Raw::key(KeyCode::KEY_ENTER, p);
        assert_eq!(out.mouse, vec![enter(Press::Down), Raw::REPORT, Raw::REPORT, enter(Press::Up), Raw::REPORT, enter(Press::Down), Raw::REPORT]);
    }
}
