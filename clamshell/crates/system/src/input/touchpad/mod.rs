//! The second screen as a touchpad (see clamshell-state's `touch`). The fingers the UI says are pointing,
//! on any tab, or any finger in the touchpad tab, go to a virtual touchpad. The compositor's libinput then
//! does what it does for a laptop touchpad: pointer motion with acceleration, tap to click, two-finger
//! scroll, tap and drag, and palm detection. Our part is only the forwarding.
//!
//! The touchpad is the size of the second screen, in its pixels. It is made once the UI has drawn that
//! screen, and kept: libinput takes a moment to see a new device, so it is ready before any finger.

mod device;
mod pointing;

use std::io;
use std::sync::{Arc, Mutex};

use clamshell_state::bottom::touch::{Contact, TouchLayout};

use crate::os::evdev::Rect;
use crate::os::worker::{self, Next, Worker};

/// How often the thread looks for the second screen's size until it can make the touchpad.
const LAYOUT_EVERY: std::time::Duration = std::time::Duration::from_millis(100);

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Command {
    /// The fingers pointing in one touch frame.
    Contacts(Vec<Contact>),
}

/// Forwards the fingers pointing to the virtual touchpad. `layout` holds the second screen's size, kept
/// up to date by the UI.
pub fn spawn(layout: Arc<Mutex<TouchLayout>>) -> io::Result<Worker<Command>> {
    Worker::spawn(clamshell_state::Thread::Touchpad.name(), move |commands| {
        let mut touchpad = Touchpad { layout: layout.clone(), pad: None, fingers: pointing::Fingers::default() };
        touchpad.make();
        loop {
            // Wait for the first layout. Unknown physical density creates no continuing poll.
            let wait = (touchpad.pad.is_none() && crate::os::lock(&touchpad.layout).screen == (0.0, 0.0)).then_some(LAYOUT_EVERY);
            let next = worker::next(commands, wait);
            touchpad.make();
            match next {
                Next::Command(Command::Contacts(contacts)) => touchpad.contacts(&contacts),
                Next::Elapsed => {}
                Next::Disconnected => return,
            }
        }
    })
}

struct Touchpad {
    layout: Arc<Mutex<TouchLayout>>,
    /// The virtual touchpad, and its size, once the second screen's size is known.
    pad: Option<Pad>,
    fingers: pointing::Fingers,
}

struct Pad {
    device: evdev::uinput::VirtualDevice,
    size: (i32, i32),
    resolution: (i32, i32),
}

impl Touchpad {
    /// Makes the touchpad the second screen's size. It is made again if that size changes while no finger
    /// is on it.
    fn make(&mut self) {
        let layout = crate::os::lock(&self.layout).clone();
        let screen = layout.screen;
        let Some(density) = resolution(layout.pixels_per_mm) else {
            if !self.fingers.any() {
                self.pad = None;
            }
            return;
        };
        let size = Some((screen.0 as i32, screen.1 as i32)).filter(|(w, h)| *w > 0 && *h > 0);
        let Some(size) = size.filter(|size| self.pad.as_ref().is_none_or(|pad| pad.size != *size || pad.resolution != density)) else {
            return;
        };
        if self.pad.is_some() && self.fingers.any() {
            return;
        }
        match device::virtual_touchpad(Rect::from_corners((0.0, 0.0, size.0 as f32, size.1 as f32)), density) {
            Ok(device) => self.pad = Some(Pad { device, size, resolution: density }),
            Err(e) => log::warn!("the virtual touchpad: {e}"),
        }
    }

    /// Sends the fingers pointing to the touchpad as one frame. Without a touchpad yet, they are dropped.
    fn contacts(&mut self, contacts: &[Contact]) {
        let Some(Pad { device: pad, size, .. }) = self.pad.as_mut() else {
            return;
        };
        let batch: Vec<evdev::InputEvent> = self.fingers.frame(contacts, *size).into_iter().map(Into::into).collect();
        if let Err(e) = pad.emit(&batch) {
            log::warn!("the virtual touchpad: {e}");
        }
    }
}

fn resolution(measured: Option<(f32, f32)>) -> Option<(i32, i32)> {
    measured.filter(|(x, y)| x.is_finite() && y.is_finite() && *x > 0.0 && *y > 0.0).map(|(x, y)| (x.round().max(1.0) as i32, y.round().max(1.0) as i32))
}
#[cfg(test)]
mod resolution_tests {
    use super::*;
    #[test]
    fn both_reported_axes_are_kept_and_unknown_resolution_is_not_guessed() {
        assert_eq!(resolution(Some((4.0, 5.0))), Some((4, 5)));
        assert_eq!(resolution(None), None);
        assert_eq!(resolution(Some((0.0, f32::NAN))), None);
        assert_eq!(resolution(Some((0.0, 0.0))), None);
    }
}
