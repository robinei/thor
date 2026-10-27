//! ClamShell's Wayland compositor: it hosts the UI (`clamshell_ui::Core`) and runs the event loop, with the
//! clients' windows and Xwayland, on the machine's own screens ([`run_drm`]) or in a window on a desktop
//! ([`run_nested`]), where both screens are shown side by side.
//!
//! Clients are told their size and place and nothing waits on them: a window ignoring its size is
//! scaled. A client's error ends that client, never the server. X11 windows become the same window
//! facts as Wayland ones in one adapter. See `docs/architecture.md`, "Living with what apps do".

mod activation;
mod base;
mod clients;
mod colour;
mod cursor;
mod dmabuf;
mod drm;
mod frames;
mod geometry;
mod handlers;
mod hold;
mod idle;
mod input;
mod keyboard;
mod nested;
mod notify;
mod pacing;
mod plane_layers;
mod pointer;
mod popup;
mod presenter;
mod protocols;
mod render;
mod scanout;
mod scene;
mod screens;
mod server;
mod software;
mod surface;
mod touch;
pub mod trace;
mod ui_scanout;
mod window_scanout;
mod windows;
mod x11;

use smithay::reexports::calloop::channel;

pub use clients::Process;
pub use drm::run as run_drm;
pub use nested::run as run_nested;
pub use notify::{Held, Readiness};

#[cfg(test)]
mod tests;

/// SIGTERM and SIGINT, as systemd stops ClamShell at a poweroff or a restart: blocked for the thread
/// that made this and every thread it starts afterwards, and read by the event loop, which quits as the
/// state's Quit does (its last saves made, the store's writes finished).
pub struct QuitSignals(calloop::signals::Signals);

/// Makes [`QuitSignals`]. It is made first, before any thread starts, so that no thread (a driver's
/// among them) takes the signal and ends ClamShell on the spot.
pub fn quit_signals() -> std::io::Result<QuitSignals> {
    use calloop::signals::{Signal, Signals};
    Signals::new(&[Signal::SIGTERM, Signal::SIGINT]).map(QuitSignals).map_err(std::io::Error::other)
}

/// Sends events into the compositor's event loop from other threads.
#[derive(Clone)]
pub struct EventSender(channel::Sender<clamshell_state::Event>);

impl EventSender {
    /// The event loop's channel for events from system threads.
    pub(crate) fn channel() -> (Self, channel::Channel<clamshell_state::Event>) {
        let (sender, receiver) = channel::channel();
        (Self(sender), receiver)
    }

    /// False once the compositor has exited.
    pub fn send(&self, event: clamshell_state::Event) -> bool {
        self.0.send(event).is_ok()
    }
}

/// Where clients reach the compositor, for the system threads once it listens.
pub struct Displays {
    /// The Wayland socket's path, and its name (WAYLAND_DISPLAY).
    pub socket: std::path::PathBuf,
    pub wayland: String,
    /// Xwayland's display (DISPLAY).
    pub x11: String,
    /// What systemd's `READY=1` waits for: setup that services started after ClamShell need can hold it.
    pub readiness: Readiness,
}
