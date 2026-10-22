//! The Wayland side: two views, the main (top) screen and the second (bottom) screen, drawn on the CPU
//! into shm buffers. A view is redrawn only when the state changed for it, at most once per frame
//! callback; between events the program sleeps.
//!
//! The top view is an xdg toplevel (sway rules make it fullscreen in its own workspace). The bottom view
//! is a layer-shell surface on the second screen's output when that output exists (the Thor: `DSI-1`),
//! else a panel in a desktop's corner, or a second ordinary window, so it runs on any desktop.
//!
//! The crate, by layers: `gfx` draws (geometry, the canvas, text, icons, images; no state), `toolkit`
//! is the immediate-mode kit the screens are drawn with, `views` the screens themselves, `render` one
//! frame of them; `input` turns presses and the pad into events (pure parts), and `platform` connects
//! all of it to Wayland and the event loop.

mod gfx;
mod input;
mod platform;
mod render;
mod screens;
mod toolkit;
mod views;

pub use platform::run;
pub use screens::Screen;

use std::sync::{Arc, Mutex};

use clamshell_core::bottom::touch::TouchLayout;
use clamshell_core::{Event, State};
use smithay_client_toolkit::reexports::calloop::channel;

use gfx::DrawCache;
use render::{Placement, Retained};

pub type Error = Box<dyn std::error::Error>;

pub struct Config {
    /// Output that gets the bottom view as a layer-shell surface; without it, the bottom view is a window.
    pub bottom_output: String,
    /// Logical sizes when shown as windows (on a desktop).
    pub top_size: (u32, u32),
    pub bottom_size: (u32, u32),
    /// Without the second screen's output (a desktop): show the bottom view as a panel in a corner
    /// instead of a window, so tapping its keyboard leaves the keyboard focus on the app being typed into.
    pub bottom_panel: bool,
    /// Where the touchpad area and the bar's targets are, for the touchpad tab's input side; updated
    /// with every frame of the second screen.
    pub touch_layout: Arc<Mutex<TouchLayout>>,
}

/// Sends events into the UI's event loop from other threads.
#[derive(Clone)]
pub struct EventSender(channel::Sender<Event>);

impl EventSender {
    /// False once the UI has exited.
    pub fn send(&self, event: Event) -> bool {
        self.0.send(event).is_ok()
    }
}

/// Renders one screen without Wayland, at `size` logical pixels and integer `scale`, to a PNG file.
/// For checking layouts and for automated tests. Returns what a live frame would tell the state (shelf's
/// rows, the lists' rows).
pub fn snapshot(state: &State, screen: Screen, size: (u32, u32), scale: u32, path: &std::path::Path) -> Result<Vec<Event>, Error> {
    let at = Placement { size_px: (size.0 * scale, size.1 * scale), scale, drift: (0, 0) };
    let mut pixmap = tiny_skia::Pixmap::new(at.size_px.0, at.size_px.1).ok_or("no pixmap")?;
    let mut retained = Retained::default();
    let rendered = render::render(pixmap.as_mut(), &mut retained, at, screen, state, &mut DrawCache::load(), &mut crate::gfx::images::Images::synchronous())?;
    pixmap.save_png(path)?;
    Ok(render::frame_events(state, &rendered))
}
