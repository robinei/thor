//! The Wayland side. There are two views, the main (top) screen and the second (bottom) screen, both
//! drawn on the CPU into shm buffers. After every event each visible view is recorded and compared with
//! its last frame; only one that changed is drawn, only where it changed, at most once per frame
//! callback. Between events the program sleeps.
//!
//! The top view is an xdg toplevel, which sway rules make fullscreen in its own workspace. The bottom
//! view is a layer-shell surface on the second screen's output when that output exists (`DSI-1` on the
//! Thor). Otherwise it is a panel in a corner of the desktop or a second ordinary window, so it runs on
//! any desktop.
//!
//! The crate's layers:
//! - `gfx` draws geometry, the canvas, text, icons and images, and holds no state;
//! - `toolkit` is the immediate-mode kit the screens are drawn with;
//! - `views` are the screens themselves, and `render` draws one frame of them;
//! - `input` turns presses and the pad into events (the pure parts);
//! - `platform` connects all of it to Wayland and the event loop.

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

use clamshell_state::bottom::touch::TouchLayout;
use clamshell_state::{Event, State};
use smithay_client_toolkit::reexports::calloop::channel;

use gfx::DrawCache;
use gfx::images::Images;
use render::{History, Placement, Retained};

pub type Error = Box<dyn std::error::Error>;

pub struct Config {
    /// Output that gets the bottom view as a layer-shell surface; without it, the bottom view is a window.
    pub bottom_output: String,
    /// Logical sizes when shown as windows (on a desktop).
    pub top_size: (u32, u32),
    pub bottom_size: (u32, u32),
    /// When the second screen's output is missing (on a desktop), show the bottom view as a panel in a
    /// corner instead of a window. Tapping its keyboard then leaves the keyboard focus on the app being
    /// typed into.
    pub bottom_panel: bool,
    /// Where the touchpad area and the bar's targets are, for the touchpad tab's input side. It is
    /// updated with every frame of the second screen.
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
/// For checking layouts and for automated tests. Returns the events a live frame would send the state,
/// such as the shelf's rows and the lists' rows.
pub fn snapshot(state: &State, screen: Screen, size: (u32, u32), scale: u32, path: &std::path::Path) -> Result<Vec<Event>, Error> {
    let at = Placement { size_px: (size.0 * scale, size.1 * scale), scale, drift: (0, 0) };
    let mut pixmap = tiny_skia::Pixmap::new(at.size_px.0, at.size_px.1).ok_or("no pixmap")?;
    let mut retained = Retained::default();
    let rendered = render::render(pixmap.as_mut(), &mut retained, at, screen, state, &mut DrawCache::load(), &mut Images::synchronous())?;
    pixmap.save_png(path)?;
    Ok(render::frame_events(state, &rendered))
}

/// Draws one screen frame after frame without Wayland, as a live view does: each frame is recorded and
/// compared with the last, and only what changed is played, into two buffers used in turn. Each buffer
/// is brought up to date with what changed since it was last drawn into. Tests use it to check that
/// this gives the pixels a whole frame does.
pub struct Frames {
    screen: Screen,
    at: Placement,
    retained: Retained,
    history: History,
    /// The buffers, each with the frame it holds (0 for none).
    buffers: [(tiny_skia::Pixmap, u64); 2],
    /// The buffer that holds the latest frame.
    latest: usize,
    fonts: DrawCache,
    images: Images,
}

impl Frames {
    /// For `screen` at `size` pixels.
    pub fn new(screen: Screen, size: (u32, u32)) -> Result<Frames, Error> {
        let buffer = || tiny_skia::Pixmap::new(size.0, size.1).map(|p| (p, 0)).ok_or("no pixmap");
        Ok(Frames {
            screen,
            at: Placement { size_px: size, scale: 1, drift: (0, 0) },
            retained: Retained::default(),
            history: History::default(),
            buffers: [buffer()?, buffer()?],
            latest: 1,
            fonts: DrawCache::load(),
            images: Images::synchronous(),
        })
    }

    /// Draws `state` as the next frame. Returns the pixels shown, premultiplied RGBA, and the events a
    /// live frame would send the state.
    pub fn draw(&mut self, state: &State) -> (&[u8], Vec<Event>) {
        let out = render::record(&mut self.retained, self.at, self.screen, state, &mut self.fonts, &mut self.images);
        if let Some(damage) = self.retained.damage() {
            let next = (self.latest + 1) % self.buffers.len();
            if let Some((pixmap, held)) = self.buffers.get_mut(next) {
                *held = render::draw_damaged(pixmap.as_mut(), *held, damage, &mut self.retained, &mut self.history, &mut self.fonts, &self.images);
                self.latest = next;
            }
        }
        (self.buffers.get(self.latest).map_or(&[], |(p, _)| p.data()), render::frame_events(state, &out))
    }

    /// Draws `state` whole, each command cut only by the clip it was recorded in, not by its bounds, to
    /// check frames against: they match only if every command's bounds hold all it draws. Returns its
    /// pixels.
    pub fn whole(&mut self, state: &State) -> Result<&[u8], Error> {
        let (pixmap, _) = self.buffers.first_mut().ok_or("no buffer")?;
        let mut retained = Retained::default();
        render::record(&mut retained, self.at, self.screen, state, &mut self.fonts, &mut self.images);
        gfx::paint::play_unbounded(pixmap.as_mut(), &retained.next, &mut self.fonts, &self.images);
        Ok(pixmap.data())
    }
}
