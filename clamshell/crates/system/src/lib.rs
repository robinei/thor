//! Threads that watch the system and carry out what the state asks for: the clock (with waking from
//! sleep), the bar's status, the readings the tabs show, the pad and touchpad paths (evdev, uinput),
//! sway, systemd, NetworkManager, BlueZ, PipeWire, UPower, logind and sysfs; and the files ClamShell
//! keeps. Nothing here waits for the UI. How a thread is built: [`os::worker`].

pub mod audio;
pub mod device;
pub mod display;
pub mod files;
pub mod handles;
pub mod input;
pub mod launch;
pub mod library;
pub mod network;
pub mod os;
pub mod paths;
pub mod power;
pub mod readings;
pub mod session;
pub mod store;
pub mod sway;
pub mod time;
pub mod waydroid;

use std::sync::mpsc;

use clamshell_core::Event;

pub use handles::Handles;
pub use os::worker::Closed;

/// Where system threads deliver events: the UI's event loop.
pub trait Sink: Send + 'static {
    fn send(&self, event: Event) -> Result<(), Closed>;
}

/// A plain channel as the sink (tests, tools).
impl Sink for mpsc::Sender<Event> {
    fn send(&self, event: Event) -> Result<(), Closed> {
        mpsc::Sender::send(self, event).map_err(|_| Closed)
    }
}
