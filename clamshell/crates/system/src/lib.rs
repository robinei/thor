//! Threads that watch the system and carry out what the state asks for, and the files ClamShell keeps.
//! Nothing here waits for the UI. [`wiring`] starts the threads and routes the state's effects to them;
//! each area is a directory:
//!
//! - `input`: the controller's path (evdev, uinput, the gyro), the virtual touchpad and keyboard, the
//!   devices watched for idle and hardware keys;
//! - [`launch`]: launches in systemd scopes, Steam's games, the freezer, mpv;
//! - [`library`]: reading the library's sources, and fetching metadata;
//! - `files`: the Files tab's browsing and operations, the trash, mounts and archives;
//! - `audio`, `display`, `power`, [`time`], `network`, [`session`]: PipeWire; backlights, screenshots and
//!   the panel's rates; tuning, sleep and the lid; the clock and timedated; NetworkManager and BlueZ;
//!   the tray, clipboard, account and notifications;
//! - [`readings`]: what the bar and tabs show (status, metrics, the battery, frame rates, MangoHud's files);
//! - `sway`, [`waydroid`]: the compositor, and the Wayland proxy for Android's windows;
//! - [`store`] and [`paths`]: ClamShell's own files, and where they are;
//! - [`os`]: the operating system's plumbing, knowing nothing of the rest (how a thread is built:
//!   [`os::worker`]).

pub(crate) mod audio;
pub(crate) mod device;
pub(crate) mod display;
pub(crate) mod files;
pub(crate) mod input;
pub mod launch;
pub mod library;
pub(crate) mod network;
pub mod os;
pub mod paths;
pub(crate) mod power;
pub mod readings;
pub mod session;
pub mod store;
pub(crate) mod sway;
pub mod time;
pub mod waydroid;
pub mod wiring;

use std::sync::mpsc;

use clamshell_core::Event;

pub use os::worker::Closed;
pub use wiring::Handles;

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
