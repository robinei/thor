//! Threads that watch the system and carry out what the state asks for: the clock (with waking from
//! sleep), the bar's status, the readings the tabs show, the pad and touchpad paths (evdev, uinput),
//! sway, systemd, NetworkManager, BlueZ, PipeWire, UPower, logind and sysfs; and the files ClamShell
//! keeps. Nothing here waits for the UI. How a thread is built: [`os::worker`].

pub mod account;
pub mod archives;
pub mod battery;
pub mod bluetooth;
pub mod brightness;
pub mod clipboard;
pub mod clock;
pub mod desktop;
pub mod device;
pub mod files;
pub mod frames;
pub mod handles;
pub mod icons;
pub mod input;
pub mod job;
pub mod keyboard;
pub mod launcher;
pub mod library;
pub mod lid;
pub mod meta;
pub mod metrics;
pub mod mixer;
pub mod mpv;
pub mod os;
pub mod pad;
pub mod paths;
pub mod sleep;
pub mod sound;
pub mod status;
pub mod store;
pub mod sway;
pub mod touchpad;
pub mod tray;
pub mod tuning;
pub mod units;
pub mod wayland_filter;
pub mod wifi;

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
