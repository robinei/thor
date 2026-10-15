//! Threads that watch the system and carry out what the state asks for: the clock (with waking from
//! sleep), the bar's status, the readings the tabs show, the pad and touchpad paths (evdev, uinput),
//! sway, systemd, NetworkManager, BlueZ, PipeWire, UPower, logind and sysfs; and the files ClamShell
//! keeps. Nothing here waits for the UI. How a thread is built: [`worker`].

pub mod battery;
pub mod bluetooth;
pub mod brightness;
pub mod cgroup;
pub mod clock;
pub mod command;
pub mod dbus;
pub mod desktop;
pub mod drm;
pub mod frames;
pub mod handles;
pub mod input;
pub mod job;
pub mod keyboard;
pub mod launcher;
pub mod library;
pub mod metrics;
pub mod mixer;
pub mod pad;
pub mod paths;
pub mod sleep;
pub mod sound;
pub mod status;
pub mod sway;
pub mod sys;
pub mod system;
pub mod touchpad;
pub mod tuning;
pub mod units;
pub mod wifi;
pub mod worker;

use std::sync::mpsc;

use clamshell_core::Event;

pub use handles::Handles;

/// The UI is gone: a thread whose events can't be delivered ends.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Closed;

/// Where policy threads deliver events: the UI's event loop.
pub trait Sink: Send + 'static {
    fn send(&self, event: Event) -> Result<(), Closed>;
}

/// A plain channel as the sink (tests, tools).
impl Sink for mpsc::Sender<Event> {
    fn send(&self, event: Event) -> Result<(), Closed> {
        mpsc::Sender::send(self, event).map_err(|_| Closed)
    }
}
