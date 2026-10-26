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

/// What the virtual touchpad needs of the second screen: its size in pixels, which the touchscreen
/// covers, and its resolution, the whole pixels per millimetre (see
/// [`crate::presentation::Panel::pixels_per_mm`]), which libinput's acceleration goes by.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TouchLayout {
    pub pixels: (u32, u32),
    pub resolution: u32,
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

    /// The touchpad is made as soon as the second screen's size in pixels and millimetres are known, and
    /// again only when they change.
    #[test]
    fn the_touchpad_is_sized_when_the_screens_are_known() {
        use crate::presentation::{Panel, Screens};
        let mut s = crate::testkit::state();
        let screens = |pixels, millimetres| Screens { top: Panel::default(), bottom: Panel { pixels, millimetres, ..Panel::default() } };
        let read = |s: &mut crate::State, screens| s.applied(crate::Reading::Screens(Box::new(screens)).into());
        assert!(read(&mut s, screens((1240, 1080), None)).is_empty(), "no millimetres: no touchpad");
        let fx = read(&mut s, screens((1240, 1080), Some((75, 65))));
        assert_eq!(fx, vec![Effect::SizeTouchpad(Some(TouchLayout { pixels: (1240, 1080), resolution: 17 }))]);
        assert!(read(&mut s, screens((1240, 1080), Some((75, 65)))).is_empty());
        let fx = read(&mut s, screens((620, 540), Some((75, 65))));
        assert_eq!(fx, vec![Effect::SizeTouchpad(Some(TouchLayout { pixels: (620, 540), resolution: 8 }))], "a nested window made smaller");
        assert_eq!(read(&mut s, screens((620, 540), None)), vec![Effect::SizeTouchpad(None)]);
    }
}
