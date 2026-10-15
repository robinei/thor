//! The pad's state as the HID pads report it, kept from the controller's evdev events: buttons by
//! position, the D-pad, the sticks (-1 to 1) and the triggers (0 to 1). Each pad encodes it in its own
//! reports.

use evdev::{AbsoluteAxisCode, KeyCode};

use crate::input::Raw;
use crate::pad::route::Ranges;

/// A button, by where it is: the face buttons as compass points.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Button {
    South,
    East,
    West,
    North,
    L1,
    R1,
    L2,
    R2,
    /// View / Create / Minus.
    Select,
    /// Menu / Options / Plus.
    Start,
    L3,
    R3,
}

impl Button {
    /// The button an evdev key is. The incoming pad is Xbox-style, read as xpad reports it: BTN_X
    /// (= BTN_NORTH's code) is the left face button, BTN_Y (= BTN_WEST's) the top one. BTN_MODE is
    /// none: the home button is ClamShell's.
    fn of(key: KeyCode) -> Option<Button> {
        Some(match key {
            KeyCode::BTN_SOUTH => Button::South,
            KeyCode::BTN_EAST => Button::East,
            KeyCode::BTN_NORTH => Button::West,
            KeyCode::BTN_WEST => Button::North,
            KeyCode::BTN_TL => Button::L1,
            KeyCode::BTN_TR => Button::R1,
            KeyCode::BTN_TL2 => Button::L2,
            KeyCode::BTN_TR2 => Button::R2,
            KeyCode::BTN_SELECT => Button::Select,
            KeyCode::BTN_START => Button::Start,
            KeyCode::BTN_THUMBL => Button::L3,
            KeyCode::BTN_THUMBR => Button::R3,
            _ => return None,
        })
    }

    fn bit(self) -> u16 {
        1 << self as u16
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct PadState {
    buttons: u16,
    /// The hat axes (-1, 0, 1).
    hat: (i32, i32),
    /// D-pad buttons, for controllers that have them instead of a hat: up, down, left, right.
    dpad: [bool; 4],
    /// Left x, left y, right x, right y, from -1 to 1; y is positive down.
    pub sticks: [f32; 4],
    /// Left, right, from 0 to 1.
    pub triggers: [f32; 2],
}

impl PadState {
    /// One controller event (a key or an axis; the rest change nothing).
    pub fn apply(&mut self, raw: Raw, ranges: Ranges) {
        if let Some((key, press)) = raw.as_key() {
            let down = press.held();
            let dpad = match key {
                KeyCode::BTN_DPAD_UP => Some(0),
                KeyCode::BTN_DPAD_DOWN => Some(1),
                KeyCode::BTN_DPAD_LEFT => Some(2),
                KeyCode::BTN_DPAD_RIGHT => Some(3),
                _ => None,
            };
            if let Some(slot) = dpad.and_then(|i| self.dpad.get_mut(i)) {
                *slot = down;
            } else if let Some(button) = Button::of(key) {
                self.set(button, down);
            }
            return;
        }
        let Some((axis, v)) = raw.as_abs() else {
            return;
        };
        let (stick, trigger) = (ranges.stick.centered(v), ranges.trigger.unit(v));
        let (slot, value) = match axis {
            AbsoluteAxisCode::ABS_X => (self.sticks.get_mut(0), stick),
            AbsoluteAxisCode::ABS_Y => (self.sticks.get_mut(1), stick),
            AbsoluteAxisCode::ABS_RX => (self.sticks.get_mut(2), stick),
            AbsoluteAxisCode::ABS_RY => (self.sticks.get_mut(3), stick),
            AbsoluteAxisCode::ABS_Z => (self.triggers.get_mut(0), trigger),
            AbsoluteAxisCode::ABS_RZ => (self.triggers.get_mut(1), trigger),
            AbsoluteAxisCode::ABS_HAT0X => {
                self.hat.0 = v.signum();
                return;
            }
            AbsoluteAxisCode::ABS_HAT0Y => {
                self.hat.1 = v.signum();
                return;
            }
            _ => return,
        };
        if let Some(slot) = slot {
            *slot = value;
        }
    }

    pub fn set(&mut self, button: Button, down: bool) {
        if down {
            self.buttons |= button.bit();
        } else {
            self.buttons &= !button.bit();
        }
    }

    pub fn pressed(&self, button: Button) -> bool {
        self.buttons & button.bit() != 0
    }

    /// The D-pad: x (-1 left, 1 right) and y (-1 up, 1 down).
    pub fn dpad(&self) -> (i32, i32) {
        let [up, down, left, right] = self.dpad;
        let x = if self.hat.0 != 0 { self.hat.0 } else { i32::from(right) - i32::from(left) };
        let y = if self.hat.1 != 0 { self.hat.1 } else { i32::from(down) - i32::from(up) };
        (x, y)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::input::{Press, Range};

    #[test]
    fn keys_and_axes_by_position() {
        let ranges = Ranges { stick: Range::STICK, trigger: Range { min: 0, max: 1023 } };
        let mut s = PadState::default();
        s.apply(Raw::key(KeyCode::BTN_NORTH, Press::Down), ranges);
        s.apply(Raw::key(KeyCode::BTN_MODE, Press::Down), ranges);
        assert!(s.pressed(Button::West), "xpad's X: the left face button");
        s.apply(Raw::key(KeyCode::BTN_NORTH, Press::Up), ranges);
        assert_eq!(s, PadState::default(), "home changes nothing");
        s.apply(Raw::abs(AbsoluteAxisCode::ABS_Y, 32767), ranges);
        s.apply(Raw::abs(AbsoluteAxisCode::ABS_RZ, 1023), ranges);
        assert_eq!((s.sticks[1], s.triggers[1]), (1.0, 1.0));
        s.apply(Raw::key(KeyCode::BTN_DPAD_LEFT, Press::Down), ranges);
        s.apply(Raw::abs(AbsoluteAxisCode::ABS_HAT0Y, 1), ranges);
        assert_eq!(s.dpad(), (-1, 1));
    }
}
