//! Pointing: the second screen below the bar is a touchpad. A finger that would tap (a key, a button, a
//! row, nothing) and moves far enough points instead (what it was on let go without a tap), handed with
//! the other fingers then tapping to a virtual touchpad, so libinput moves the pointer, taps to click,
//! scrolls with two fingers, drags. What moves of its own takes the motion first: a slider sideways, a
//! list up and down (so on a list only a sideways move points), a held key nowhere. While it points the
//! area below the bar fades, and fingers that come down on it point too, until none has been on it a
//! moment or a text field takes focus. The touchpad tab is that area with nothing in it, always
//! pointing: every finger below the bar points at once. The UI decides which fingers point (its input
//! machine); the state keeps whether it points.

/// What the virtual touchpad needs of the second screen's layout: its size in pixels (the touchscreen
/// covers it). The UI updates it with every frame of the second screen.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct TouchLayout {
    pub screen: (f32, f32),
}

/// Fingers (slots) a touchpad tracks.
pub const SLOTS: i32 = 10;

/// A finger pointing on the keyboard: its slot (one per finger), what it did, and where (screen pixels).
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

/// Pointing: the fingers' contacts in one touch frame (pointing starts, or goes on), or the fingers idle
/// long enough that taps act again.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Pointing {
    Contacts(Vec<Contact>),
    Idle,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Effect, Event};

    #[test]
    fn it_points_until_idle_or_a_text_field_takes_focus() {
        let mut s = crate::testkit::state();
        let contact = Contact { slot: 1, phase: ContactPhase::Down, x: 10, y: 20 };
        let (dirty, fx) = s.apply(Pointing::Contacts(vec![contact]).into());
        assert!(s.bottom().pointing() && dirty.bottom);
        assert_eq!(fx, vec![Effect::Touchpad(vec![contact])]);
        s.apply(Pointing::Idle.into());
        assert!(!s.bottom().pointing());
        s.apply(Pointing::Contacts(vec![contact]).into());
        s.apply(Event::TextInput(true));
        assert!(!s.bottom().pointing(), "a field took focus: typing");
    }
}
