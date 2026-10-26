//! The second screen as a touchpad (see clamshell-state's `touch`). The fingers the UI says are pointing,
//! on any tab, or any finger in the touchpad tab, go to a virtual touchpad. The compositor's libinput then
//! does what it does for a laptop touchpad: pointer motion with acceleration, tap to click, two-finger
//! scroll, tap and drag, and palm detection. Our part is only the forwarding.
//!
//! The touchpad is the size of the second screen, in its pixels, at its pixels per millimetre. The state
//! sends them as soon as the compositor has said them, and the touchpad is made then and kept: libinput
//! takes a moment to see a new device, so it is ready before any finger.

mod device;
mod pointing;

use std::io;

use clamshell_state::bottom::touch::{Contact, TouchLayout};

use crate::os::evdev::Rect;
use crate::os::worker::Worker;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Command {
    /// The second screen's size and resolution, or None while they aren't known.
    Layout(Option<TouchLayout>),
    /// The fingers pointing in one touch frame.
    Contacts(Vec<Contact>),
}

/// Forwards the fingers pointing to the virtual touchpad, made as the state says the second screen is.
pub(crate) fn spawn() -> io::Result<Worker<Command>> {
    Worker::spawn(clamshell_state::Thread::Touchpad.name(), move |commands| {
        let mut touchpad = Touchpad { layout: None, pad: None, fingers: pointing::Fingers::default() };
        for command in commands {
            match command {
                Command::Layout(layout) => touchpad.layout = layout,
                Command::Contacts(contacts) => touchpad.contacts(&contacts),
            }
            touchpad.make();
        }
    })
}

struct Touchpad {
    /// The layout the touchpad is to have.
    layout: Option<TouchLayout>,
    /// The virtual touchpad, and the layout it was made with.
    pad: Option<Pad>,
    fingers: pointing::Fingers,
}

struct Pad {
    device: evdev::uinput::VirtualDevice,
    layout: TouchLayout,
}

impl Touchpad {
    /// Makes the touchpad as the layout says, or drops it while there is none. It is made again when the
    /// layout changes, once no finger is on it.
    fn make(&mut self) {
        if self.pad.as_ref().map(|pad| pad.layout) == self.layout || self.fingers.any() {
            return;
        }
        self.pad = None;
        let Some(layout) = self.layout else {
            return;
        };
        let (w, h) = (layout.pixels.0 as f32, layout.pixels.1 as f32);
        let resolution = i32::try_from(layout.resolution).unwrap_or(i32::MAX);
        match device::virtual_touchpad(Rect::from_corners((0.0, 0.0, w, h)), (resolution, resolution)) {
            Ok(device) => self.pad = Some(Pad { device, layout }),
            Err(e) => log::warn!("the virtual touchpad: {e}"),
        }
    }

    /// Sends the fingers pointing to the touchpad as one frame. Without a touchpad, they are dropped.
    fn contacts(&mut self, contacts: &[Contact]) {
        let Some(Pad { device, layout }) = self.pad.as_mut() else {
            return;
        };
        let size = (i32::try_from(layout.pixels.0).unwrap_or(i32::MAX), i32::try_from(layout.pixels.1).unwrap_or(i32::MAX));
        let batch: Vec<evdev::InputEvent> = self.fingers.frame(contacts, size).into_iter().map(Into::into).collect();
        if let Err(e) = device.emit(&batch) {
            log::warn!("the virtual touchpad: {e}");
        }
    }
}
