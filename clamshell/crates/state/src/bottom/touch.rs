//! Pointing: the second screen below the bar works as a touchpad.
//!
//! A finger that would tap something (a key, a button, a row, or nothing) and then moves far enough
//! points instead, and what it was on is released without a tap. That finger, and any other fingers
//! tapping at the time, are handed to a virtual touchpad, so libinput moves the pointer, taps to click,
//! scrolls with two fingers and drags.
//!
//! Anything that moves by itself takes the motion first: a slider takes sideways motion and a list takes
//! up and down, so on a list only a sideways move points. A held key never points.
//!
//! While it is pointing, the area below the bar fades, and fingers that come down on it point too. This
//! lasts until no finger has been on it for a moment, or a text field takes focus. The touchpad tab is
//! that area with nothing in it, always pointing, so every finger below the bar points at once.
//!
//! The UI decides which fingers point, in its input state machine. The state keeps whether it is
//! pointing.

/// What the virtual touchpad needs from the second screen's layout: its size in pixels, which the
/// touchscreen covers. The UI updates it with every frame of the second screen.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct TouchLayout {
    pub screen: (f32, f32),
    /// Measured buffer pixels per millimetre, when the host reports physical dimensions.
    pub pixels_per_mm: Option<(f32, f32)>,
}

/// How many fingers (slots) a touchpad tracks.
pub const SLOTS: i32 = 10;

/// A finger pointing on the second screen: its slot (one per finger), what it did, and where it is in
/// screen pixels.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Contact {
    pub slot: u8,
    pub phase: ContactPhase,
    pub x: i32,
    pub y: i32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ContactPhase {
    Down,
    Move,
    Up,
}

/// Either the fingers' contacts in one touch frame, as pointing starts or goes on, or `Idle` once the
/// fingers have been idle long enough that taps act again.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Pointing {
    Contacts(Vec<Contact>),
    Idle,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testkit::Applied;
    use crate::{Effect, Event};

    #[test]
    fn it_points_until_idle_or_a_text_field_takes_focus() {
        let mut s = crate::testkit::state();
        let contact = Contact { slot: 1, phase: ContactPhase::Down, x: 10, y: 20 };
        let fx = s.applied(Pointing::Contacts(vec![contact]).into());
        assert!(s.bottom().pointing());
        assert_eq!(fx, vec![Effect::Touchpad(vec![contact])]);
        s.applied(Pointing::Idle.into());
        assert!(!s.bottom().pointing());
        s.applied(Pointing::Contacts(vec![contact]).into());
        s.applied(Event::TextInput(true));
        assert!(!s.bottom().pointing(), "a field took focus: typing");
    }
}
