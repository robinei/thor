//! What games get from the pad: which virtual pad ([`PadKind`]), and a stick deadzone (the rest of the
//! range stretched to full).

use clamshell_core::settings::{Setting, Values};
use evdev::AbsoluteAxisCode;

use crate::input::{Range, Raw};

/// The virtual pad games see.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum PadKind {
    /// A wired Xbox 360 controller (uinput).
    #[default]
    Xbox,
    /// A USB DualSense (uhid, driven by the kernel's hid-playstation): motion sensors and rumble.
    PlayStation,
    /// A Bluetooth Switch Pro Controller (uhid, driven by hid-nintendo): motion sensors and rumble.
    Nintendo,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Shaping {
    pub kind: PadKind,
    /// Percent of the stick's travel.
    pub deadzone: u16,
}

impl Shaping {
    /// The shaping the settings in force ask for.
    pub fn from_settings(v: &Values) -> Shaping {
        let kind = match v.get(Setting::Pad) {
            1 => PadKind::PlayStation,
            2 => PadKind::Nintendo,
            _ => PadKind::Xbox,
        };
        Shaping { kind, deadzone: v.get(Setting::Deadzone) }
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
        let a = Raw::key(KeyCode::BTN_SOUTH, Press::Down);
        assert_eq!(Shaping::default().apply(stick, a), a, "buttons as they are");
        let n = Shaping { deadzone: 10, ..Shaping::default() };
        assert_eq!(n.apply(stick, a), a);
        let x = |v| Raw::abs(AbsoluteAxisCode::ABS_X, v);
        assert_eq!(n.apply(stick, x(3000)), x(0), "inside the deadzone: centred");
        assert_eq!(n.apply(stick, x(32767)), x(32767), "full stays full");
        let half = n.apply(stick, x(18000)).value;
        assert!(half > 0 && half < 18000, "{half}: stretched from the deadzone's edge");
        let z = Raw::abs(AbsoluteAxisCode::ABS_Z, 100);
        assert_eq!(n.apply(stick, z), z, "triggers as they are");
    }

    #[test]
    fn the_pad_setting_picks_the_kind() {
        let mut v = Values::default();
        assert_eq!(Shaping::from_settings(&v), Shaping::default(), "Xbox by default");
        v.set(Setting::Pad, 1);
        assert_eq!(Shaping::from_settings(&v).kind, PadKind::PlayStation);
        v.set(Setting::Pad, 2);
        v.set(Setting::Deadzone, 8);
        assert_eq!(Shaping::from_settings(&v), Shaping { kind: PadKind::Nintendo, deadzone: 8 });
    }
}
