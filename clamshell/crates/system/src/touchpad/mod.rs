//! Touchpad mode: the second screen as a laptop touchpad.
//!
//! While on, the touchscreen is grabbed (sway sees none of its touches). Each finger is decided at
//! touch-down ([`router`]): one that starts on a region the UI keeps (the bar) is a press on that target
//! (moving, on the volume or brightness slider: [`bar`]); any other is copied, multitouch events as they
//! come, to a virtual touchpad the size of the second screen, in its pixels. sway's
//! libinput then does what it does for a laptop touchpad: pointer motion with acceleration, tap to
//! click, two-finger scroll, gestures, palm detection, all configurable with `input` lines. Our part is
//! only this forwarding.
//!
//! The keyboard's fingers pointing go to the same touchpad ([`pointing`]: the UI hands them over; the
//! touchscreen isn't grabbed). It is made once the UI has drawn the second screen, and kept: ready before
//! any finger. Whichever of the two ends lifts the fingers it left on it.

mod bar;
mod device;
mod pointing;
mod router;

use std::fmt;
use std::io;
use std::sync::mpsc::{Receiver, TryRecvError};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use clamshell_core::touch::TouchLayout;
use clamshell_core::{Event, Reading, TouchpadProblem};

use crate::input::Rect;
use crate::input::device::{DeviceSpec, Grabbed};
use crate::worker::{self, Ended, Next, Worker, lost_on};
use crate::{Closed, Sink};
use bar::BarGesture;
use router::{Routed, Router};

/// How long the UI may take to lay out the touchpad area (a few frames at most).
const LAYOUT_WAIT: Duration = Duration::from_secs(3);
/// The longest the thread waits for touches before looking at its commands (how long an Off waits).
const COMMANDS_EVERY: Duration = Duration::from_millis(100);

/// Starting and stopping touchpad mode (from ClamShell's mode changes); the keyboard's fingers.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Command {
    On,
    Off,
    Contact(clamshell_core::touch::Contact),
}

#[derive(Debug, Clone)]
pub struct Config {
    /// The second screen's touchscreen (CLAMSHELL_TOUCHSCREEN).
    pub device: DeviceSpec,
    /// The second screen's pixels per millimetre, so libinput's speeds and thresholds match its size.
    pub px_per_mm: f32,
}

#[derive(Debug)]
enum Error {
    Io(io::Error),
    /// The UI didn't lay out the touchpad area in time.
    NotLaidOut,
    /// The UI's side panicked while holding the touch layout.
    Poisoned,
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::Io(e) => write!(f, "{e}"),
            Error::NotLaidOut => write!(f, "the touchpad area wasn't laid out"),
            Error::Poisoned => write!(f, "touch layout lock poisoned"),
        }
    }
}

impl From<io::Error> for Error {
    fn from(e: io::Error) -> Self {
        Error::Io(e)
    }
}

lost_on!(Error: io::Error);

/// Why touchpad mode didn't work, for the touchpad tab to say.
fn problem(e: &Error) -> TouchpadProblem {
    log::warn!("touchpad mode: {e}");
    match e {
        Error::Io(e) if e.kind() == io::ErrorKind::NotFound => TouchpadProblem::NoTouchscreen,
        other => TouchpadProblem::Failed(other.to_string()),
    }
}

/// Runs touchpad mode whenever `On` arrives, until `Off`, and the keyboard's pointing between. `layout`:
/// the second screen's size, where the touchpad area and the bar's targets are, kept up to date by the UI.
pub fn spawn(config: Config, layout: Arc<Mutex<TouchLayout>>, sink: impl Sink) -> io::Result<Worker<Command>> {
    Worker::spawn("touchpad", move |commands| {
        let _ = TouchpadMode { config, layout, sink, gesture: BarGesture::default(), pad: None, fingers: pointing::Fingers::default() }.serve(&commands);
    })
}

struct TouchpadMode<S> {
    config: Config,
    layout: Arc<Mutex<TouchLayout>>,
    sink: S,
    gesture: BarGesture,
    /// The virtual touchpad, once the second screen's size is known.
    pad: Option<Pad>,
    /// The keyboard's fingers pointing.
    fingers: pointing::Fingers,
}

/// The virtual touchpad: the second screen's size, in its pixels.
struct Pad {
    device: evdev::uinput::VirtualDevice,
    size: (i32, i32),
}

/// The touchpad area (pixels), once the UI has laid it out (at start it may not have yet).
fn pad_area(layout: &Mutex<TouchLayout>) -> Option<Rect> {
    layout.lock().ok().map(|l| Rect::from_corners(l.pad)).filter(|area| !area.is_empty())
}

/// The second screen's size (pixels), once the UI has drawn it.
fn screen(layout: &Mutex<TouchLayout>) -> Option<(i32, i32)> {
    layout.lock().ok().map(|l| (l.screen.0 as i32, l.screen.1 as i32)).filter(|(w, h)| *w > 0 && *h > 0)
}

impl<S: Sink> TouchpadMode<S> {
    /// Runs touchpad mode each time it is turned on, and the keyboard's fingers meanwhile, until the
    /// commands or the UI go.
    fn serve(&mut self, commands: &Receiver<Command>) -> Result<(), Closed> {
        loop {
            // (Until the touchpad is made, looking for the screen's size now and then.)
            let wait = self.pad.is_none().then_some(COMMANDS_EVERY);
            let next = worker::next(commands, wait);
            self.make_pad();
            match next {
                Next::Command(Command::On) => {}
                Next::Command(Command::Contact(contact)) => {
                    let events = self.fingers.contact(contact, self.pad.as_ref().map_or((0, 0), |p| p.size));
                    self.emit(events);
                    continue;
                }
                Next::Command(Command::Off) | Next::Elapsed => continue,
                Next::Disconnected => return Ok(()),
            }
            log::info!("touchpad mode on");
            // (The keyboard's fingers, if any are left down, are done.)
            let lifted = self.fingers.lift();
            self.emit(lifted);
            let result = match self.run(commands) {
                Ok(()) => Ok(()),
                Err(Ended::Closed) => return Err(Closed),
                Err(Ended::Lost(e)) => Err(e),
            };
            // What the UI had under fingers ends (no tap), and whether it worked, for the tab to say.
            let mut events = Vec::new();
            self.gesture.cancel(&mut events);
            events.push(Reading::TouchpadProblem(result.as_ref().err().map(problem)).into());
            for event in events {
                self.sink.send(event)?;
            }
            log::info!("touchpad mode off");
        }
    }

    /// The touchpad made the second screen's size, ahead of any finger (libinput takes a moment to see
    /// a new device); made again if that size changed while no finger is on it.
    fn make_pad(&mut self) {
        let Some(size) = screen(&self.layout).filter(|size| self.pad.as_ref().is_none_or(|p| p.size != *size)) else {
            return;
        };
        if self.pad.is_some() && self.fingers.any() {
            return;
        }
        let area = Rect::from_corners((0.0, 0.0, size.0 as f32, size.1 as f32));
        match device::virtual_touchpad(area, self.config.px_per_mm) {
            Ok(device) => self.pad = Some(Pad { device, size }),
            Err(e) => log::warn!("the virtual touchpad: {e}"),
        }
    }

    /// Events for the touchpad (none without one).
    fn emit(&mut self, events: Vec<crate::input::Raw>) {
        let Some(pad) = self.pad.as_mut().filter(|_| !events.is_empty()) else {
            return;
        };
        let batch: Vec<evdev::InputEvent> = events.into_iter().map(Into::into).collect();
        if let Err(e) = pad.device.emit(&batch) {
            log::warn!("the virtual touchpad: {e}");
        }
    }

    /// Runs touchpad mode until `Off` (or the commands go); its fingers lifted then.
    fn run(&mut self, commands: &Receiver<Command>) -> Result<(), Ended<Error>> {
        let device = device::find(&self.config.device)?;
        let axes = device::axes(&device)?;
        let waited = Instant::now();
        while pad_area(&self.layout).is_none() || self.pad.is_none() {
            if waited.elapsed() > LAYOUT_WAIT {
                return Err(Ended::Lost(Error::NotLaidOut));
            }
            if matches!(commands.try_recv(), Ok(Command::Off) | Err(TryRecvError::Disconnected)) {
                return Ok(());
            }
            thread::sleep(Duration::from_millis(20));
            self.make_pad();
        }
        let mut grabbed = Grabbed::new(device)?;
        self.sink.send(Reading::TouchpadProblem(None).into())?;
        let mut router = Router::new(axes);
        let result = self.route(commands, &mut grabbed, &mut router);
        self.emit(router.lift());
        result
    }

    /// The touchscreen's fingers to the bar or the touchpad, until `Off`.
    fn route(&mut self, commands: &Receiver<Command>, grabbed: &mut Grabbed, router: &mut Router) -> Result<(), Ended<Error>> {
        let (mut raws, mut routed, mut events) = (Vec::new(), Routed::default(), Vec::<Event>::new());
        loop {
            if matches!(commands.try_recv(), Ok(Command::Off) | Err(TryRecvError::Disconnected)) {
                return Ok(());
            }
            // Wakes for touches at once.
            grabbed.wait(COMMANDS_EVERY)?;
            grabbed.fetch(&mut raws)?;
            // The layout as it is now, for this batch: where the bar's targets are, and the slider's
            // positions.
            let layout = self.layout.lock().map_err(|_| Error::Poisoned)?.clone();
            for raw in raws.drain(..) {
                router.feed(raw, &layout, &mut routed);
            }
            let now = Instant::now();
            self.gesture.tick(now, layout.width, &mut events);
            for touch in routed.ui.drain(..) {
                self.gesture.on(touch, now, &layout, &mut events);
            }
            for event in events.drain(..) {
                self.sink.send(event)?;
            }
            let pad = std::mem::take(&mut routed.pad);
            self.emit(pad);
        }
    }
}
