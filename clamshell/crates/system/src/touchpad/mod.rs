//! Touchpad mode: the second screen as a laptop touchpad.
//!
//! While on, the touchscreen is grabbed (sway sees none of its touches). Each finger is decided at
//! touch-down ([`router`]): one that starts on a region the UI keeps (the bar) is a press on that target
//! (moving, on the volume or brightness slider: [`bar`]); any other is copied, multitouch events as they
//! come, to a virtual touchpad the size of the area below the bar (made again when that changes). sway's
//! libinput then does what it does for a laptop touchpad: pointer motion with acceleration, tap to
//! click, two-finger scroll, gestures, palm detection, all configurable with `input` lines. Our part is
//! only this forwarding.

mod bar;
mod device;
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

/// Starting and stopping touchpad mode (from ClamShell's mode changes).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Command {
    On,
    Off,
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

/// Runs touchpad mode whenever `On` arrives, until `Off`. `layout`: where the touchpad area and the
/// bar's targets are, kept up to date by the UI.
pub fn spawn(config: Config, layout: Arc<Mutex<TouchLayout>>, sink: impl Sink) -> io::Result<Worker<Command>> {
    Worker::spawn("touchpad", move |commands| {
        let _ = TouchpadMode { config, layout, sink, gesture: BarGesture::default() }.serve(&commands);
    })
}

struct TouchpadMode<S> {
    config: Config,
    layout: Arc<Mutex<TouchLayout>>,
    sink: S,
    gesture: BarGesture,
}

/// The touchpad area (pixels), once the UI has laid it out (at start it may not have yet).
fn pad_area(layout: &Mutex<TouchLayout>) -> Option<Rect> {
    layout.lock().ok().map(|l| Rect::from_corners(l.pad)).filter(|area| !area.is_empty())
}

impl<S: Sink> TouchpadMode<S> {
    /// Runs touchpad mode each time it is turned on, until the commands or the UI go.
    fn serve(&mut self, commands: &Receiver<Command>) -> Result<(), Closed> {
        loop {
            match worker::next(commands, None) {
                Next::Command(Command::On) => {}
                Next::Command(Command::Off) | Next::Elapsed => continue,
                Next::Disconnected => return Ok(()),
            }
            log::info!("touchpad mode on");
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

    /// Runs touchpad mode until `Off` (or the commands go).
    fn run(&mut self, commands: &Receiver<Command>) -> Result<(), Ended<Error>> {
        let device = device::find(&self.config.device)?;
        let axes = device::axes(&device)?;
        let waited = Instant::now();
        let mut area = loop {
            if let Some(area) = pad_area(&self.layout) {
                break area;
            }
            if waited.elapsed() > LAYOUT_WAIT {
                return Err(Ended::Lost(Error::NotLaidOut));
            }
            if matches!(commands.try_recv(), Ok(Command::Off) | Err(TryRecvError::Disconnected)) {
                return Ok(());
            }
            thread::sleep(Duration::from_millis(20));
        };
        let mut touchpad = device::virtual_touchpad(area, self.config.px_per_mm)?;
        let mut grabbed = Grabbed::new(device)?;
        self.sink.send(Reading::TouchpadProblem(None).into())?;
        let mut router = Router::new(axes);
        let (mut raws, mut routed, mut events) = (Vec::new(), Routed::default(), Vec::<Event>::new());
        loop {
            if matches!(commands.try_recv(), Ok(Command::Off) | Err(TryRecvError::Disconnected)) {
                return Ok(());
            }
            // The area changed (a new layout): a touchpad its new size, between touches.
            if let Some(now) = pad_area(&self.layout).filter(|now| *now != area)
                && !router.pad_fingers()
            {
                log::info!("touchpad mode: the area changed, the touchpad follows");
                touchpad = device::virtual_touchpad(now, self.config.px_per_mm)?;
                area = now;
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
            if !routed.pad.is_empty() {
                let batch: Vec<evdev::InputEvent> = routed.pad.drain(..).map(Into::into).collect();
                touchpad.emit(&batch)?;
            }
        }
    }
}
