//! What games get from the pad's sticks: a deadzone (the rest of the range stretched to full). (Which
//! button is which is the keymap's: see [`super::remap`].)

use clamshell_core::settings::{Setting, Values};
use evdev::AbsoluteAxisCode;

use crate::input::{Range, Raw};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Shaping {
    /// Percent of the stick's travel.
    pub deadzone: u16,
}

impl Shaping {
    /// The shaping the settings in force ask for.
    pub fn from_settings(v: &Values) -> Shaping {
        Shaping { deadzone: v.get(Setting::Deadzone) }
    }

    /// The event a game gets for this one (`stick`: the sticks' range).
    pub fn apply(self, stick: Range, raw: Raw) -> Raw {
        let Some((axis, value)) = raw.as_abs() else {
            return raw;
        };
        let is_stick = matches!(axis, AbsoluteAxisCode::ABS_X | AbsoluteAxisCode::ABS_Y | AbsoluteAxisCode::ABS_RX | AbsoluteAxisCode::ABS_RY);
        if !is_stick || self.deadzone == 0 {
            return raw;
        }
        let dz = f32::from(self.deadzone.min(90)) / 100.0;
        let c = stick.centered(value);
        let shaped = if c.abs() < dz { 0.0 } else { c.signum() * (c.abs() - dz) / (1.0 - dz) };
        // The resting value: 0 for -32768..32767, 128 for 0..255.
        let mid = (stick.min + stick.max + 1) / 2;
        let half = if shaped < 0.0 { mid - stick.min } else { stick.max - mid };
        Raw::abs(axis, mid + (shaped * half as f32).round() as i32)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::input::Press;
    use evdev::KeyCode;

    #[test]
    fn applies_the_deadzone_to_the_sticks_only() {
        let stick = Range::STICK;
        let key = |code: KeyCode, press| Raw::key(code, press);
        let n = Shaping { deadzone: 10 };
        assert_eq!(n.apply(stick, key(KeyCode::BTN_SOUTH, Press::Down)), key(KeyCode::BTN_SOUTH, Press::Down));
        let x = |v| Raw::abs(AbsoluteAxisCode::ABS_X, v);
        assert_eq!(n.apply(stick, x(3000)), x(0), "inside the deadzone: centred");
        assert_eq!(n.apply(stick, x(32767)), x(32767), "full stays full");
        let half = n.apply(stick, x(18000)).value;
        assert!(half > 0 && half < 18000, "{half}: stretched from the deadzone's edge");
        let z = Raw::abs(AbsoluteAxisCode::ABS_Z, 100);
        assert_eq!(n.apply(stick, z), z, "triggers as they are");
    }
}
