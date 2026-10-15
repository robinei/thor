//! What games get from the pad: which virtual pad ([`PadKind`]), the face buttons in Nintendo layout
//! (A/B and X/Y swapped), and a stick deadzone (the rest of the range stretched to full).

use clamshell_core::settings::{Setting, Values};
use evdev::{AbsoluteAxisCode, KeyCode};

use crate::input::{Range, Raw};

/// The virtual pad games see.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum PadKind {
    /// A wired Xbox 360 controller (uinput).
    #[default]
    Xbox,
    /// A USB DualSense (uhid, driven by the kernel's hid-playstation): motion sensors and rumble.
    DualSense,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Shaping {
    pub kind: PadKind,
    pub nintendo: bool,
    /// Percent of the stick's travel.
    pub deadzone: u16,
}

impl Shaping {
    /// The shaping the settings in force ask for.
    pub fn from_settings(v: &Values) -> Shaping {
        let pad = v.get(Setting::Pad);
        let kind = if pad == 2 { PadKind::DualSense } else { PadKind::Xbox };
        Shaping { kind, nintendo: pad == 1, deadzone: v.get(Setting::Deadzone) }
    }

    /// The event a game gets for this one (`stick`: the sticks' range).
    pub fn apply(self, stick: Range, raw: Raw) -> Raw {
        if let Some((key, _)) = raw.as_key() {
            if !self.nintendo {
                return raw;
            }
            let swapped = match key {
                KeyCode::BTN_SOUTH => KeyCode::BTN_EAST,
                KeyCode::BTN_EAST => KeyCode::BTN_SOUTH,
                KeyCode::BTN_NORTH => KeyCode::BTN_WEST,
                KeyCode::BTN_WEST => KeyCode::BTN_NORTH,
                other => other,
            };
            return Raw { code: swapped.0, ..raw };
        }
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

    #[test]
    fn swaps_face_buttons_and_applies_the_deadzone() {
        let stick = Range::STICK;
        let key = |code: KeyCode, press| Raw::key(code, press);
        let off = Shaping::default();
        assert_eq!(off.apply(stick, key(KeyCode::BTN_SOUTH, Press::Down)), key(KeyCode::BTN_SOUTH, Press::Down));
        let n = Shaping { nintendo: true, deadzone: 10, ..Shaping::default() };
        assert_eq!(n.apply(stick, key(KeyCode::BTN_SOUTH, Press::Down)), key(KeyCode::BTN_EAST, Press::Down));
        assert_eq!(n.apply(stick, key(KeyCode::BTN_WEST, Press::Up)), key(KeyCode::BTN_NORTH, Press::Up));
        assert_eq!(n.apply(stick, key(KeyCode::BTN_START, Press::Repeat)), key(KeyCode::BTN_START, Press::Repeat));
        let x = |v| Raw::abs(AbsoluteAxisCode::ABS_X, v);
        assert_eq!(n.apply(stick, x(3000)), x(0), "inside the deadzone: centred");
        assert_eq!(n.apply(stick, x(32767)), x(32767), "full stays full");
        let half = n.apply(stick, x(18000)).value;
        assert!(half > 0 && half < 18000, "{half}: stretched from the deadzone's edge");
        let z = Raw::abs(AbsoluteAxisCode::ABS_Z, 100);
        assert_eq!(n.apply(stick, z), z, "triggers as they are");
    }

    #[test]
    fn the_pad_setting_picks_the_kind_and_layout() {
        let mut v = Values::default();
        assert_eq!(Shaping::from_settings(&v), Shaping::default(), "Xbox by default");
        v.set(Setting::Pad, 1);
        assert_eq!(Shaping::from_settings(&v), Shaping { kind: PadKind::Xbox, nintendo: true, deadzone: 0 });
        v.set(Setting::Pad, 2);
        v.set(Setting::Deadzone, 8);
        assert_eq!(Shaping::from_settings(&v), Shaping { kind: PadKind::DualSense, nintendo: false, deadzone: 8 });
    }
}
