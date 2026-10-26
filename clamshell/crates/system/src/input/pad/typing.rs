//! Typing into the app in front with the pad while the on-screen keyboard is open. The D-pad gives
//! arrows, A gives Enter and B gives Backspace, on the virtual mouse's keys.

use std::time::Instant;

use evdev::KeyCode;

use super::route::{Mapping, Out, release_mouse_keys};
use crate::os::evdev::{Raw, hat_keys};

#[derive(Debug, Clone, Copy, Default)]
pub(crate) struct Typing;

impl Mapping for Typing {
    fn event(&mut self, raw: Raw, _: Instant, out: &mut Out) {
        if let Some((axis, value)) = raw.as_abs() {
            out.mouse.extend(hat_keys(axis, value).into_iter().flatten());
            return;
        }
        let key = match raw.as_key() {
            Some((KeyCode::BTN_SOUTH, press)) => (KeyCode::KEY_ENTER, press),
            Some((KeyCode::BTN_EAST, press)) => (KeyCode::KEY_BACKSPACE, press),
            _ => return,
        };
        // Autorepeat is passed on, so a held B keeps deleting.
        out.mouse.extend([Raw::key(key.0, key.1), Raw::REPORT]);
    }

    fn tick(&mut self, _: Instant, _: &mut Out) -> Option<Instant> {
        None
    }

    fn release(&mut self, out: &mut Out) {
        release_mouse_keys(out);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::os::evdev::Press;
    use evdev::AbsoluteAxisCode;

    #[test]
    fn maps_the_dpad_a_and_b() {
        let mut out = Out::default();
        let now = Instant::now();
        Typing.event(Raw::abs(AbsoluteAxisCode::ABS_HAT0Y, -1), now, &mut out);
        assert_eq!(out.mouse, vec![Raw::key(KeyCode::KEY_UP, Press::Down), Raw::key(KeyCode::KEY_DOWN, Press::Up), Raw::REPORT]);
        out.mouse.clear();
        Typing.event(Raw::key(KeyCode::BTN_SOUTH, Press::Down), now, &mut out);
        Typing.event(Raw::key(KeyCode::BTN_EAST, Press::Repeat), now, &mut out);
        Typing.event(Raw::key(KeyCode::BTN_NORTH, Press::Down), now, &mut out);
        assert_eq!(out.mouse, vec![Raw::key(KeyCode::KEY_ENTER, Press::Down), Raw::REPORT, Raw::key(KeyCode::KEY_BACKSPACE, Press::Repeat), Raw::REPORT]);
    }
}
