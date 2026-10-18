//! The keyboard's touchpad (see clamshell-core's `keyboard`): the fingers the UI says point, as touchpad
//! events. Pure: contacts in, events out.

use clamshell_core::touch::{Contact, ContactPhase};
use evdev::AbsoluteAxisCode;

use super::router::{SLOTS, finger_keys};
use crate::input::Raw;

/// The fingers pointing, by slot (where each is), and the last tracking id given.
#[derive(Debug, Default)]
pub struct Fingers {
    down: [Option<(i32, i32)>; SLOTS],
    last_id: i32,
}

impl Fingers {
    /// Whether a finger is down.
    pub fn any(&self) -> bool {
        self.count() > 0
    }

    /// Every finger lifted (touchpad mode starts with fingers down).
    pub fn lift(&mut self) -> Vec<Raw> {
        let before = self.count();
        let mut out = Vec::new();
        for (slot, down) in self.down.iter_mut().enumerate() {
            if down.take().is_some() {
                out.push(Raw::abs(AbsoluteAxisCode::ABS_MT_SLOT, slot as i32));
                out.push(Raw::abs(AbsoluteAxisCode::ABS_MT_TRACKING_ID, -1));
            }
        }
        finger_keys(before, 0, &mut out);
        if !out.is_empty() {
            out.push(Raw::REPORT);
        }
        out
    }

    fn count(&self) -> usize {
        self.down.iter().flatten().count()
    }

    fn first(&self) -> Option<(i32, i32)> {
        self.down.iter().flatten().next().copied()
    }

    /// Contact `c` as touchpad events (on a touchpad of `size` pixels): its slot's, then the touch and
    /// finger-count keys and the first finger's position as they change. Nothing for a finger that
    /// isn't down moving or lifting.
    pub fn contact(&mut self, c: Contact, size: (i32, i32)) -> Vec<crate::input::Raw> {
        let (before, first) = (self.count(), self.first());
        let slot = usize::from(c.slot).min(SLOTS - 1);
        let at = (c.x.clamp(0, size.0), c.y.clamp(0, size.1));
        let Some(down) = self.down.get_mut(slot) else {
            return vec![];
        };
        let mut out = vec![Raw::abs(AbsoluteAxisCode::ABS_MT_SLOT, slot as i32)];
        match c.phase {
            ContactPhase::Down => {
                self.last_id = (self.last_id + 1) % 65535;
                out.push(Raw::abs(AbsoluteAxisCode::ABS_MT_TRACKING_ID, self.last_id));
                *down = Some(at);
            }
            ContactPhase::Move if down.is_some() => *down = Some(at),
            ContactPhase::Up if down.take().is_some() => out.push(Raw::abs(AbsoluteAxisCode::ABS_MT_TRACKING_ID, -1)),
            ContactPhase::Move | ContactPhase::Up => return vec![],
        }
        if c.phase != ContactPhase::Up {
            out.push(Raw::abs(AbsoluteAxisCode::ABS_MT_POSITION_X, at.0));
            out.push(Raw::abs(AbsoluteAxisCode::ABS_MT_POSITION_Y, at.1));
        }
        finger_keys(before, self.count(), &mut out);
        if let Some((x, y)) = self.first().filter(|now| Some(*now) != first) {
            out.push(Raw::abs(AbsoluteAxisCode::ABS_X, x));
            out.push(Raw::abs(AbsoluteAxisCode::ABS_Y, y));
        }
        out.push(Raw::REPORT);
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::input::Press;
    use evdev::KeyCode;

    const SLOT: AbsoluteAxisCode = AbsoluteAxisCode::ABS_MT_SLOT;
    const ID: AbsoluteAxisCode = AbsoluteAxisCode::ABS_MT_TRACKING_ID;
    const X: AbsoluteAxisCode = AbsoluteAxisCode::ABS_MT_POSITION_X;
    const Y: AbsoluteAxisCode = AbsoluteAxisCode::ABS_MT_POSITION_Y;

    fn contact(slot: u8, phase: ContactPhase, x: i32, y: i32) -> Contact {
        Contact { slot, phase, x, y }
    }

    #[test]
    fn contacts_are_a_touchpads_fingers() {
        let mut f = Fingers::default();
        let size = (1000, 800);
        assert_eq!(
            f.contact(contact(0, ContactPhase::Down, 100, 900), size),
            vec![
                Raw::abs(SLOT, 0),
                Raw::abs(ID, 1),
                Raw::abs(X, 100),
                Raw::abs(Y, 800),
                Raw::key(KeyCode::BTN_TOUCH, Press::Down),
                Raw::key(KeyCode::BTN_TOOL_FINGER, Press::Down),
                Raw::abs(AbsoluteAxisCode::ABS_X, 100),
                Raw::abs(AbsoluteAxisCode::ABS_Y, 800),
                Raw::REPORT
            ],
            "on the touchpad"
        );
        let second = f.contact(contact(3, ContactPhase::Down, 200, 200), size);
        assert!(second.contains(&Raw::key(KeyCode::BTN_TOOL_FINGER, Press::Up)) && second.contains(&Raw::key(KeyCode::BTN_TOOL_DOUBLETAP, Press::Down)));
        assert_eq!(f.contact(contact(3, ContactPhase::Move, 210, 200), size), vec![Raw::abs(SLOT, 3), Raw::abs(X, 210), Raw::abs(Y, 200), Raw::REPORT]);
        f.contact(contact(3, ContactPhase::Up, 210, 200), size);
        let up = f.contact(contact(0, ContactPhase::Up, 100, 800), size);
        assert_eq!(
            up,
            vec![Raw::abs(SLOT, 0), Raw::abs(ID, -1), Raw::key(KeyCode::BTN_TOUCH, Press::Up), Raw::key(KeyCode::BTN_TOOL_FINGER, Press::Up), Raw::REPORT]
        );
        assert!(f.contact(contact(0, ContactPhase::Move, 1, 1), size).is_empty(), "not down");
    }
}
