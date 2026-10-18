//! The second screen as a touchpad (see clamshell-core's `touch`): the fingers the UI says point (on any
//! tab, or any in the touchpad tab) go to a virtual touchpad, and sway's libinput does what it does for a
//! laptop touchpad: pointer motion with acceleration, tap to click, two-finger scroll, tap and drag,
//! palm detection, all configurable with `input` lines. Our part is only this forwarding.
//!
//! The touchpad is the size of the second screen, in its pixels. It is made once the UI has drawn that
//! screen, and kept: libinput takes a moment to see a new device, so it is ready before any finger.

mod device;
mod pointing;

use std::io;
use std::sync::{Arc, Mutex};

use clamshell_core::touch::{Contact, TouchLayout};

use crate::input::Rect;
use crate::worker::{self, Next, Worker};

/// How often the thread looks for the second screen's size until it can make the touchpad.
const LAYOUT_EVERY: std::time::Duration = std::time::Duration::from_millis(100);

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Command {
    /// Fingers pointing: one touch frame's.
    Contacts(Vec<Contact>),
}

#[derive(Debug, Clone)]
pub struct Config {
    /// The second screen's pixels per millimetre, so libinput's speeds and thresholds match its size.
    pub px_per_mm: f32,
}

/// Forwards the fingers pointing to the virtual touchpad. `layout`: the second screen's size, kept up to
/// date by the UI.
pub fn spawn(config: Config, layout: Arc<Mutex<TouchLayout>>) -> io::Result<Worker<Command>> {
    Worker::spawn("touchpad", move |commands| {
        let mut touchpad = Touchpad { config, layout, pad: None, fingers: pointing::Fingers::default() };
        loop {
            // (Until the touchpad is made, looking for the screen's size now and then.)
            let wait = touchpad.pad.is_none().then_some(LAYOUT_EVERY);
            let next = worker::next(&commands, wait);
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
    config: Config,
    layout: Arc<Mutex<TouchLayout>>,
    /// The virtual touchpad, and its size, once the second screen's size is known.
    pad: Option<(evdev::uinput::VirtualDevice, (i32, i32))>,
    fingers: pointing::Fingers,
}

impl Touchpad {
    /// The touchpad made the second screen's size; made again if that size changed while no finger is on
    /// it.
    fn make(&mut self) {
        let size = self.layout.lock().ok().map(|l| (l.screen.0 as i32, l.screen.1 as i32)).filter(|(w, h)| *w > 0 && *h > 0);
        let Some(size) = size.filter(|size| self.pad.as_ref().is_none_or(|(_, s)| s != size)) else {
            return;
        };
        if self.pad.is_some() && self.fingers.any() {
            return;
        }
        match device::virtual_touchpad(Rect::from_corners((0.0, 0.0, size.0 as f32, size.1 as f32)), self.config.px_per_mm) {
            Ok(device) => self.pad = Some((device, size)),
            Err(e) => log::warn!("the virtual touchpad: {e}"),
        }
    }

    /// Fingers pointing, to the touchpad as one frame (none yet: dropped).
    fn contacts(&mut self, contacts: &[Contact]) {
        let Some((pad, size)) = self.pad.as_mut() else {
            return;
        };
        let batch: Vec<evdev::InputEvent> = self.fingers.frame(contacts, *size).into_iter().map(Into::into).collect();
        if let Err(e) = pad.emit(&batch) {
            log::warn!("the virtual touchpad: {e}");
        }
    }
}
