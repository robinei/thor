//! The fingers the UI says point (see clamshell-core's `touch`), as touchpad events. Pure: contacts in,
//! events out.

use clamshell_core::touch::{Contact, ContactPhase};
use evdev::{AbsoluteAxisCode, KeyCode};

use crate::input::{Press, Raw};

/// Slots (fingers) tracked.
pub const SLOTS: usize = clamshell_core::touch::SLOTS as usize;
/// BTN_TOOL_FINGER, _DOUBLETAP, _TRIPLETAP, _QUADTAP: how many fingers are on the touchpad.
const TOOLS: [KeyCode; 4] = [KeyCode::BTN_TOOL_FINGER, KeyCode::BTN_TOOL_DOUBLETAP, KeyCode::BTN_TOOL_TRIPLETAP, KeyCode::BTN_TOOL_QUADTAP];

/// BTN_TOUCH and the tool key for `fingers` fingers on the touchpad, `before` before.
fn finger_keys(before: usize, fingers: usize, out: &mut Vec<Raw>) {
    if fingers == before {
        return;
    }
    let press = |down: bool| if down { Press::Down } else { Press::Up };
    if (fingers > 0) != (before > 0) {
        out.push(Raw::key(KeyCode::BTN_TOUCH, press(fingers > 0)));
    }
    let tool = |n: usize| n.checked_sub(1).and_then(|i| TOOLS.get(i.min(TOOLS.len() - 1)).copied());
    if let Some(code) = tool(before) {
        out.push(Raw::key(code, Press::Up));
    }
    if let Some(code) = tool(fingers) {
        out.push(Raw::key(code, Press::Down));
    }
}

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

    fn count(&self) -> usize {
        self.down.iter().flatten().count()
    }

    fn first(&self) -> Option<(i32, i32)> {
        self.down.iter().flatten().next().copied()
    }

    /// One touch frame's contacts as one touchpad frame (on a touchpad of `size` pixels): each slot's
    /// events, then the touch and finger-count keys and the first finger's position as they changed.
    /// Nothing for a frame of fingers that weren't down moving or lifting.
    pub fn frame(&mut self, contacts: &[Contact], size: (i32, i32)) -> Vec<crate::input::Raw> {
        let (before, first) = (self.count(), self.first());
        let mut out = Vec::new();
        for &c in contacts {
            self.contact(c, size, &mut out);
        }
        if out.is_empty() {
            return out;
        }
        finger_keys(before, self.count(), &mut out);
        if let Some((x, y)) = self.first().filter(|now| Some(*now) != first) {
            out.push(Raw::abs(AbsoluteAxisCode::ABS_X, x));
            out.push(Raw::abs(AbsoluteAxisCode::ABS_Y, y));
        }
        out.push(Raw::REPORT);
        out
    }

    /// Contact `c`'s slot events, to `out`.
    fn contact(&mut self, c: Contact, size: (i32, i32), out: &mut Vec<Raw>) {
        let slot = usize::from(c.slot).min(SLOTS - 1);
        let at = (c.x.clamp(0, size.0), c.y.clamp(0, size.1));
        let Some(down) = self.down.get_mut(slot) else {
            return;
        };
        let start = out.len();
        out.push(Raw::abs(AbsoluteAxisCode::ABS_MT_SLOT, slot as i32));
        match c.phase {
            ContactPhase::Down => {
                self.last_id = (self.last_id + 1) % 65535;
                out.push(Raw::abs(AbsoluteAxisCode::ABS_MT_TRACKING_ID, self.last_id));
                *down = Some(at);
            }
            ContactPhase::Move if down.is_some() => *down = Some(at),
            ContactPhase::Up if down.take().is_some() => out.push(Raw::abs(AbsoluteAxisCode::ABS_MT_TRACKING_ID, -1)),
            ContactPhase::Move | ContactPhase::Up => return out.truncate(start),
        }
        if c.phase != ContactPhase::Up {
            out.push(Raw::abs(AbsoluteAxisCode::ABS_MT_POSITION_X, at.0));
            out.push(Raw::abs(AbsoluteAxisCode::ABS_MT_POSITION_Y, at.1));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
            f.frame(&[contact(0, ContactPhase::Down, 100, 900)], size),
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
        let second = f.frame(&[contact(3, ContactPhase::Down, 200, 200)], size);
        assert!(second.contains(&Raw::key(KeyCode::BTN_TOOL_FINGER, Press::Up)) && second.contains(&Raw::key(KeyCode::BTN_TOOL_DOUBLETAP, Press::Down)));
        assert_eq!(f.frame(&[contact(3, ContactPhase::Move, 210, 200)], size), vec![Raw::abs(SLOT, 3), Raw::abs(X, 210), Raw::abs(Y, 200), Raw::REPORT]);
        f.frame(&[contact(3, ContactPhase::Up, 210, 200)], size);
        let up = f.frame(&[contact(0, ContactPhase::Up, 100, 800)], size);
        assert_eq!(
            up,
            vec![Raw::abs(SLOT, 0), Raw::abs(ID, -1), Raw::key(KeyCode::BTN_TOUCH, Press::Up), Raw::key(KeyCode::BTN_TOOL_FINGER, Press::Up), Raw::REPORT]
        );
        assert!(f.frame(&[contact(0, ContactPhase::Move, 1, 1)], size).is_empty(), "not down");
        // Two fingers moving in one frame: one report.
        f.frame(&[contact(0, ContactPhase::Down, 10, 10), contact(1, ContactPhase::Down, 20, 10)], size);
        let both = f.frame(&[contact(0, ContactPhase::Move, 10, 30), contact(1, ContactPhase::Move, 20, 30)], size);
        assert_eq!(
            both,
            vec![
                Raw::abs(SLOT, 0),
                Raw::abs(X, 10),
                Raw::abs(Y, 30),
                Raw::abs(SLOT, 1),
                Raw::abs(X, 20),
                Raw::abs(Y, 30),
                Raw::abs(AbsoluteAxisCode::ABS_X, 10),
                Raw::abs(AbsoluteAxisCode::ABS_Y, 30),
                Raw::REPORT
            ]
        );
    }
}
