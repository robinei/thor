//! What the input side needs to know about the second screen's layout in touchpad mode: where the
//! touchpad area is, and which regions stay the UI's (the bar), in the screen's pixels. The UI updates it
//! with every frame of the second screen; the touchpad thread reads it at each touch-down.
//!
//! And pointing: the second screen below the bar is a touchpad too, without touchpad mode. A finger that
//! would tap (a key, a button, a row, nothing) and moves far enough points instead (what it was on let go
//! without a tap), handed with the other fingers then tapping to the virtual touchpad, so libinput moves
//! the pointer, taps to click, scrolls with two fingers. What moves of its own takes the motion first: a
//! slider sideways, a list up and down (so on a list only a sideways move points), a held key nowhere.
//! While it points the area below the bar fades, and fingers that come down on it point too, until none
//! has been on it a moment or a text field takes focus. The UI decides which fingers point (its input
//! machine); the state keeps whether it points.

use crate::Id;

/// A rectangle in screen pixels that belongs to a UI target.
#[derive(Debug, Clone, PartialEq)]
pub struct Region {
    pub x0: f32,
    pub y0: f32,
    pub x1: f32,
    pub y1: f32,
    pub id: Id,
}

impl Region {
    pub fn contains(&self, x: f32, y: f32) -> bool {
        x >= self.x0 && x < self.x1 && y >= self.y0 && y < self.y1
    }
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct TouchLayout {
    /// The second screen's size in pixels (the touchscreen covers it).
    pub screen: (f32, f32),
    /// Pixels per layout unit, where the layout starts on screen (in layout units), and its width: so
    /// a position on the bar's sliders can be given in the layout's units.
    pub scale: f32,
    pub origin_x: f32,
    pub width: f32,
    /// The touchpad area: x0, y0, x1, y1 in screen pixels.
    pub pad: (f32, f32, f32, f32),
    /// Targets that stay the UI's in touchpad mode.
    pub regions: Vec<Region>,
}

impl TouchLayout {
    pub fn region_at(&self, x: f32, y: f32) -> Option<&Id> {
        self.regions.iter().find(|r| r.contains(x, y)).map(|r| &r.id)
    }

    /// A horizontal screen position (pixels) in layout units.
    pub fn layout_x(&self, x: f32) -> f32 {
        x / self.scale.max(1.0) - self.origin_x
    }
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

/// The keyboard's pointing: a finger's contact (pointing starts, or goes on), or the fingers idle long
/// enough that typing is back.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Pointing {
    Contact(Contact),
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
        let (dirty, fx) = s.apply(Pointing::Contact(contact).into());
        assert!(s.bottom().pointing() && dirty.bottom);
        assert_eq!(fx, vec![Effect::Touchpad(contact)]);
        s.apply(Pointing::Idle.into());
        assert!(!s.bottom().pointing());
        s.apply(Pointing::Contact(contact).into());
        s.apply(Event::TextInput(true));
        assert!(!s.bottom().pointing(), "a field took focus: typing");
    }
}
