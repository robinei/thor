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
//! - `core` is all of it without a backend: the state, the event queue, input by screen, timers as
//!   deadlines, and each screen drawn into a pixmap ([`Core`]);
//! - `platform` hosts the core as a Wayland client, with its event loop.

mod core;
mod gfx;
mod input;
mod platform;
mod render;
mod screens;
mod toolkit;
mod views;
pub mod window_arrows;

pub use crate::core::{Core, TextField};
pub use gfx::record::Bounds;
pub use input::{End, Source};
pub use platform::run;
pub use screens::Screen;

use std::sync::{Arc, Mutex};

use clamshell_state::bottom::touch::TouchLayout;
use clamshell_state::{Event, State};
use smithay_client_toolkit::reexports::calloop::channel;

use gfx::DrawCache;
use gfx::images::Images;
use render::{Placement, Retained};

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
    /// A host's channel for events from system threads.
    pub fn channel() -> (Self, channel::Channel<Event>) {
        let (sender, receiver) = channel::channel();
        (Self(sender), receiver)
    }

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
    frames: render::ScreenFrames,
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
            frames: render::ScreenFrames::default(),
            buffers: [buffer()?, buffer()?],
            latest: 1,
            fonts: DrawCache::load(),
            images: Images::synchronous(),
        })
    }

    /// Draws `state` as the next frame. Returns the pixels shown, premultiplied RGBA, and the events a
    /// live frame would send the state.
    pub fn draw(&mut self, state: &State) -> (&[u8], Vec<Event>) {
        let out = self.frames.record(self.at, self.screen, state, &mut self.fonts, &mut self.images);
        if let Some(damage) = self.frames.damage() {
            let next = (self.latest + 1) % self.buffers.len();
            if let Some((pixmap, held)) = self.buffers.get_mut(next) {
                *held = self.frames.draw(pixmap.as_mut(), *held, damage, &mut self.fonts, &self.images);
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

/// Records both screens without Wayland and answers what the pad does on them, by the same functions a
/// live view uses (`input::focus`). The snapshot test maps the pad's moves with it.
pub struct PadFrames {
    at: screens::Screens<Placement>,
    retained: screens::Screens<Retained>,
    fonts: DrawCache,
    images: Images,
}

/// Both screens' targets as one frame of each placed them.
pub struct PadView {
    targets: screens::Screens<Vec<toolkit::Target>>,
    sizes: screens::Screens<(f32, f32)>,
}

/// A direction the pad moves the focus in.
pub use input::nav::Dir;

/// A target drawn on a screen: its id and its rect (x, y, width, height) in the screen's pixels.
#[derive(Debug, Clone, PartialEq)]
pub struct PadTarget {
    pub screen: Screen,
    pub id: clamshell_state::Id,
    pub rect: [f32; 4],
}

impl PadTarget {
    fn of(screen: Screen, t: &toolkit::Target) -> PadTarget {
        PadTarget { screen, id: t.id.clone(), rect: [t.rect.x, t.rect.y, t.rect.w, t.rect.h] }
    }
}

impl PadFrames {
    /// For screens of `top` and `bottom` pixels.
    pub fn new(top: (u32, u32), bottom: (u32, u32)) -> PadFrames {
        let at = |size_px| Placement { size_px, scale: 1, drift: (0, 0) };
        PadFrames {
            at: screens::Screens { top: at(top), bottom: at(bottom) },
            retained: screens::Screens::default(),
            fonts: DrawCache::load(),
            images: Images::synchronous(),
        }
    }

    /// Records both screens for `state`.
    pub fn record(&mut self, state: &State) -> PadView {
        let mut view = PadView { targets: screens::Screens::default(), sizes: screens::Screens::default() };
        for screen in Screen::BOTH {
            let at = self.at[screen];
            render::record(&mut self.retained[screen], at, screen, state, &mut self.fonts, &mut self.images);
            view.targets[screen] = self.retained[screen].targets.clone();
            view.sizes[screen] = (at.size_px.0 as f32, at.size_px.1 as f32);
        }
        view
    }
}

impl input::focus::Drawn for PadView {
    fn targets(&self, screen: Screen) -> &[toolkit::Target] {
        &self.targets[screen]
    }

    fn size(&self, screen: Screen) -> (f32, f32) {
        self.sizes[screen]
    }
}

impl PadView {
    /// Every target drawn on `screen`, in the order drawn.
    pub fn drawn(&self, screen: Screen) -> Vec<PadTarget> {
        self.targets[screen].iter().map(|t| PadTarget::of(screen, t)).collect()
    }

    /// The targets the pad may move between in `state`.
    pub fn focusable(&self, state: &State) -> Vec<PadTarget> {
        input::focus::candidates(state, self).into_iter().map(|(screen, t)| PadTarget::of(screen, t)).collect()
    }

    /// What moving the pad in `dir` does, with nothing remembered of where the focus was on each screen.
    pub fn navigate(&self, state: &State, dir: Dir) -> Option<Event> {
        input::focus::navigate(state, self, &screens::Screens::default(), dir)
    }

    /// Where the pad's focus is when it is next used: the focus if it is drawn where the pad may go,
    /// else where A brings it.
    pub fn start(&self, state: &State) -> Option<clamshell_state::Id> {
        match input::focus::press(state, self, &screens::Screens::default())? {
            input::focus::PadPress::Press(id) => Some(id),
            input::focus::PadPress::Focus(Event::Pointer(clamshell_state::PointerEvent::Focus(id))) => Some(id),
            input::focus::PadPress::Focus(_) => None,
        }
    }

    /// Whether the pad may go to both screens in `state`, rather than to the second screen alone.
    pub fn both_screens(state: &State) -> bool {
        input::focus::scope(state) != input::focus::PadScope::BottomOnly
    }

    /// What the pad's B does in `state`.
    pub fn back(state: &State) -> Option<Event> {
        input::focus::back(state)
    }
}
