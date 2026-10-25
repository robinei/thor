//! The in-process Wayland server. The nested host presents both screens in one desktop window.
//!
//! Clients are told their size and place and nothing waits on them: a window ignoring its size is
//! scaled. A client's error ends that client, never the server. X11 windows become the same window
//! facts as Wayland ones in one adapter. See `docs/compositor-mvp.md`, "Keeping the app tail small".

mod activation;
mod base;
mod colour;
mod cursor;
mod dmabuf;
mod drm;
mod frames;
mod geometry;
mod handlers;
mod idle;
mod input;
mod keyboard;
mod nested;
mod pointer;
mod popup;
mod presentation;
mod render;
mod server;
mod surface;
mod touch;
pub mod trace;
mod x11;

pub use drm::run as run_drm;
pub use nested::run;
pub use server::Process;

#[cfg(test)]
mod tests;

/// Where clients reach the compositor, for the system threads once it listens.
pub struct Displays {
    /// The Wayland socket's path, and its name (WAYLAND_DISPLAY).
    pub socket: std::path::PathBuf,
    pub wayland: String,
    /// Xwayland's display (DISPLAY).
    pub x11: String,
}

/// An opaque SDR capture of the presented scene, with the top row first.
pub struct Capture {
    pub size: (u32, u32),
    pub rgba: Vec<u8>,
}
