//! evdev events as values, shared by the pad, the touchpad and the keyboard: an event ([`Raw`]), a key's
//! press ([`Press`]), an axis' range ([`Range`]), an area of the screen ([`Rect`]), the D-pad as arrow
//! keys ([`hat_keys`]); and the devices themselves ([`device`]).

pub mod device;
pub mod watch;

use evdev::{AbsoluteAxisCode, AttributeSet, EventType, InputEvent, KeyCode, RelativeAxisCode};

/// One evdev event as the kernel reports it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Raw {
    pub kind: EventType,
    pub code: u16,
    pub value: i32,
}

impl Raw {
    /// The end of a report (SYN_REPORT): what came before it belongs together.
    pub const REPORT: Raw = Raw { kind: EventType::SYNCHRONIZATION, code: 0, value: 0 };

    pub fn key(code: KeyCode, press: Press) -> Raw {
        Raw { kind: EventType::KEY, code: code.0, value: press.value() }
    }

    pub fn abs(code: AbsoluteAxisCode, value: i32) -> Raw {
        Raw { kind: EventType::ABSOLUTE, code: code.0, value }
    }

    pub fn rel(code: RelativeAxisCode, value: i32) -> Raw {
        Raw { kind: EventType::RELATIVE, code: code.0, value }
    }

    /// A key event's key and press.
    pub fn as_key(self) -> Option<(KeyCode, Press)> {
        (self.kind == EventType::KEY).then(|| (KeyCode(self.code), Press::from_value(self.value)))
    }

    /// An absolute axis event's axis and value.
    pub fn as_abs(self) -> Option<(AbsoluteAxisCode, i32)> {
        (self.kind == EventType::ABSOLUTE).then_some((AbsoluteAxisCode(self.code), self.value))
    }

    pub fn is_report(self) -> bool {
        self == Raw::REPORT
    }
}

impl From<InputEvent> for Raw {
    fn from(e: InputEvent) -> Raw {
        Raw { kind: e.event_type(), code: e.code(), value: e.value() }
    }
}

impl From<Raw> for InputEvent {
    fn from(r: Raw) -> InputEvent {
        InputEvent::new(r.kind.0, r.code, r.value)
    }
}

/// A key event's value.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Press {
    Up,
    Down,
    /// The kernel's autorepeat of a held key.
    Repeat,
}

impl Press {
    /// 0 up, 1 down, 2 (or more) repeated.
    pub fn from_value(value: i32) -> Press {
        match value {
            ..=0 => Press::Up,
            1 => Press::Down,
            _ => Press::Repeat,
        }
    }

    pub fn value(self) -> i32 {
        match self {
            Press::Up => 0,
            Press::Down => 1,
            Press::Repeat => 2,
        }
    }

    /// Down, or held down (repeated).
    pub fn held(self) -> bool {
        self != Press::Up
    }
}

/// An absolute axis' raw range.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Range {
    pub min: i32,
    pub max: i32,
}

impl Range {
    /// A stick's full range on most controllers (the range assumed when a controller doesn't say).
    pub const STICK: Range = Range { min: -32768, max: 32767 };

    /// A value as -1..1 from the middle (sticks).
    pub fn centered(self, v: i32) -> f32 {
        let mid = (self.min as f32 + self.max as f32) / 2.0;
        let half = ((self.max - self.min) as f32 / 2.0).max(1.0);
        ((v as f32 - mid) / half).clamp(-1.0, 1.0)
    }

    /// How far along the range a value is: 0 at its minimum, 1 at its maximum, and beyond either
    /// outside it (a touch at the screen's very edge).
    pub fn fraction(self, v: i32) -> f32 {
        (v - self.min) as f32 / (self.max - self.min).max(1) as f32
    }

    /// A value as 0..1 (triggers).
    pub fn unit(self, v: i32) -> f32 {
        self.fraction(v).clamp(0.0, 1.0)
    }
}

/// An area of a screen in pixels: from (x0, y0) to (x1, y1).
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct Rect {
    pub x0: f32,
    pub y0: f32,
    pub x1: f32,
    pub y1: f32,
}

impl Rect {
    /// From corners as the touch layout gives them: (x0, y0, x1, y1).
    pub fn from_corners((x0, y0, x1, y1): (f32, f32, f32, f32)) -> Rect {
        Rect { x0, y0, x1, y1 }
    }

    /// Whether it has no area (not laid out yet).
    pub fn is_empty(&self) -> bool {
        !(self.x1 > self.x0 && self.y1 > self.y0)
    }

    pub fn width(&self) -> f32 {
        self.x1 - self.x0
    }

    pub fn height(&self) -> f32 {
        self.y1 - self.y0
    }
}

/// The D-pad (a hat axis: -1, 0 or 1) as arrow keys: the key of its direction down, the other up, and
/// the report; `None` for any other axis. Letting go reports 0, which lifts both.
pub fn hat_keys(axis: AbsoluteAxisCode, value: i32) -> Option<[Raw; 3]> {
    let (minus, plus) = match axis {
        AbsoluteAxisCode::ABS_HAT0X => (KeyCode::KEY_LEFT, KeyCode::KEY_RIGHT),
        AbsoluteAxisCode::ABS_HAT0Y => (KeyCode::KEY_UP, KeyCode::KEY_DOWN),
        _ => return None,
    };
    let press = |down: bool| if down { Press::Down } else { Press::Up };
    Some([Raw::key(minus, press(value < 0)), Raw::key(plus, press(value > 0)), Raw::REPORT])
}

/// A device's keys, as uinput takes them.
pub fn keyset(keys: &[KeyCode]) -> AttributeSet<KeyCode> {
    keys.iter().copied().collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn key_values_as_presses() {
        assert_eq!([0, 1, 2, -1].map(Press::from_value), [Press::Up, Press::Down, Press::Repeat, Press::Up]);
        assert_eq!(Raw::key(KeyCode::BTN_SOUTH, Press::Repeat).as_key(), Some((KeyCode::BTN_SOUTH, Press::Repeat)));
        assert_eq!(Raw::abs(AbsoluteAxisCode::ABS_X, 5).as_key(), None);
        assert!(Raw::REPORT.is_report());
    }

    #[test]
    fn ranges() {
        let stick = Range::STICK;
        assert_eq!((stick.centered(-32768), stick.centered(32767)), (-1.0, 1.0));
        let trigger = Range { min: 0, max: 1023 };
        assert_eq!((trigger.unit(2000), trigger.fraction(2046)), (1.0, 2.0));
    }

    #[test]
    fn the_dpad_as_arrows() {
        let up = |down: bool| Raw::key(KeyCode::KEY_UP, if down { Press::Down } else { Press::Up });
        let down = Raw::key(KeyCode::KEY_DOWN, Press::Up);
        assert_eq!(hat_keys(AbsoluteAxisCode::ABS_HAT0Y, -1), Some([up(true), down, Raw::REPORT]));
        assert_eq!(hat_keys(AbsoluteAxisCode::ABS_HAT0Y, 0), Some([up(false), down, Raw::REPORT]));
        assert_eq!(hat_keys(AbsoluteAxisCode::ABS_X, 1), None);
    }

    #[test]
    fn rects() {
        let r = Rect::from_corners((0.0, 100.0, 1000.0, 800.0));
        assert!(!r.is_empty() && Rect::default().is_empty());
    }
}
