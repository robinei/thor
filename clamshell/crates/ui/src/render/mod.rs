//! Drawing one frame of a screen. The views are recorded ([`record`]), the recording is compared with
//! the last one drawn, and what changed is played into a buffer. It works the same for a live view,
//! which draws straight into the compositor's shared buffer, and for a snapshot, which draws into a
//! pixmap of its own.
//!
//! Coordinates, from the inside out:
//! - Layout units are logical pixels from the top left of the area inside the burn-in margin. The views
//!   draw and place their targets in these.
//! - Surface pixels are the surface's logical pixels, which Wayland reports for touch and the pointer.
//!   They are layout units moved by the burn-in offset. Targets are moved into these once the frame is
//!   drawn.
//! - Buffer pixels are surface pixels times the scale, as the pixmap has them. A display buffer of a
//!   panel mounted sideways holds them turned; they are still counted the screen's way up.
//! - Screen pixels are the touchscreen's, used for the virtual touchpad. They are the buffer pixels of
//!   the second screen, which fills its output.

use clamshell_state::{Event, ListEvent, ListId, State};

#[cfg(any(test, feature = "dev"))]
use crate::Error;
use crate::gfx::images::Images;
use crate::gfx::paint::play;
use crate::gfx::record::{Bounds, Recording};
use crate::gfx::{Canvas, DrawCache, Rect};
use crate::toolkit::{Frame, FrameOut, Scratch, Target};
use crate::{Layer, Screen};

/// OLED burn-in protection. Every screen is laid out inside a black margin of MARGIN buffer pixels on
/// each side, and drawn moved within it by an offset that drifts slowly, about a pixel a minute along an
/// irregular path within ±MARGIN. This keeps static elements such as the bar's icons and the clock from
/// sitting on the same pixels for hours, while nothing reaches the edge or is cut off. Tap areas move
/// with the picture.
pub const MARGIN: u32 = 4;

/// The burn-in drift at `unix_secs`, in buffer pixels.
pub fn orbit(unix_secs: u64) -> (i32, i32) {
    let minutes = (unix_secs / 60) as f32;
    let tau = std::f32::consts::TAU;
    let reach = MARGIN as f32;
    let dx = (reach * (minutes * tau / 17.0).sin()).round() as i32;
    let dy = (reach * (minutes * tau / 23.0).sin()).round() as i32;
    (dx, dy)
}

/// The layout's size inside the margin, in layout units, and the offset in buffer pixels to draw it at
/// for a drift of `(dx, dy)`.
pub fn inset(size_px: (u32, u32), scale: u32, (dx, dy): (i32, i32)) -> ((f32, f32), (i32, i32)) {
    let scale = scale.max(1) as f32;
    let w = size_px.0.saturating_sub(2 * MARGIN) as f32 / scale;
    let h = size_px.1.saturating_sub(2 * MARGIN) as f32 / scale;
    ((w, h), (MARGIN as i32 + dx, MARGIN as i32 + dy))
}

/// Where a frame goes: the buffer's size in pixels, its scale, and the burn-in drift.
#[derive(Debug, Clone, Copy)]
pub struct Placement {
    pub size_px: (u32, u32),
    pub scale: u32,
    pub drift: (i32, i32),
}

/// What a screen keeps from its last frame: what can be tapped in it, in surface pixels, kept text, the
/// latest frame's recording, and the one the next frame is recorded into (the one before, until then).
#[derive(Default)]
pub struct Retained {
    pub targets: Vec<Target>,
    pub scratch: Scratch,
    pub latest: Recording,
    pub next: Recording,
}

impl Retained {
    /// What the frame recorded into `next` changes from the latest, or `None` if it looks the same.
    pub fn damage(&self) -> Option<Bounds> {
        self.next.damage(&self.latest)
    }

    /// The frame recorded into `next` becomes the latest.
    pub fn advance(&mut self) {
        std::mem::swap(&mut self.latest, &mut self.next);
    }
}

/// Records `layer` for `state` into `retained.next`, and moves its targets and the touchpad area into
/// surface pixels. A screen's frame starts with the whole buffer black, an overlay's clear.
pub fn record(
    retained: &mut Retained,
    at: Placement,
    layer: Layer,
    state: &State,
    fonts: &mut DrawCache,
    images: &mut Images,
    views: &crate::Views,
) -> FrameOut {
    let scale = at.scale.max(1);
    let ((lw, lh), offset) = inset(at.size_px, scale, at.drift);
    let mut out = {
        let mut canvas = Canvas::new(&mut retained.next, at.size_px, scale as f32, fonts).with_offset(offset);
        // The views paint on black and have no background of their own; an overlay shows what is under
        // it.
        canvas.clear(if matches!(layer, Layer::Overlay(_)) { tiny_skia::Color::TRANSPARENT } else { crate::toolkit::style::BG.into() });
        let mut f = Frame::new(&mut canvas, state, images, &mut retained.scratch, &mut retained.targets);
        let layout = Rect::new(0.0, 0.0, lw, lh);
        match layer {
            Layer::Screen(Screen::Top) => (views.top)(&mut f, layout),
            Layer::Screen(Screen::Bottom) => (views.bottom)(&mut f, layout),
            Layer::Overlay(screen) => (views.overlay)(&mut f, layout, screen),
        }
        f.finish()
    };
    if let Layer::Screen(screen) = layer {
        images.end_frame(screen);
    }
    let (sx, sy) = (offset.0 as f32 / scale as f32, offset.1 as f32 / scale as f32);
    for target in &mut retained.targets {
        target.rect.x += sx;
        target.rect.y += sy;
        if let Some(crate::toolkit::Drag::Level(_, track) | crate::toolkit::Drag::Slide { track, .. }) = &mut target.drag {
            track.x0 += sx;
        }
    }
    out.content = out.content.map(|r| r.shifted(sx, sy));
    out.point_area = out.point_area.map(|r| r.shifted(sx, sy));
    out
}

/// Draws `layer` for `state` into `pixmap`, which must be `at`'s size, as [`record`] and then the
/// whole recording played.
#[cfg(any(test, feature = "dev"))]
#[allow(clippy::too_many_arguments)]
pub fn render(
    pixmap: &mut tiny_skia::Pixmap,
    retained: &mut Retained,
    at: Placement,
    layer: Layer,
    state: &State,
    fonts: &mut DrawCache,
    images: &mut Images,
    views: &crate::Views,
) -> Result<FrameOut, Error> {
    let (w, h) = at.size_px;
    if (pixmap.width(), pixmap.height()) != (w, h) {
        return Err(format!("a {}x{} pixmap for a {w}x{h} frame", pixmap.width(), pixmap.height()).into());
    }
    let out = record(retained, at, layer, state, fonts, images, views);
    play(pixmap, &retained.next, Bounds::of_size(at.size_px), fonts, images);
    retained.advance();
    Ok(out)
}

/// Brings `pixmap`, a buffer that holds frame `held` of `history` (0 if none), up to the frame recorded
/// last in `retained` (its `latest`). Only the commands that meet what changed since `held` are played,
/// within it. Returns the frame the buffer now holds and the area drawn.
pub fn draw_damaged(
    pixmap: &mut tiny_skia::Pixmap,
    held: u64,
    retained: &Retained,
    history: &History,
    fonts: &mut DrawCache,
    images: &Images,
) -> (u64, Bounds) {
    let area = history.since(held, Bounds::of_size(retained.latest.size()));
    if !area.is_empty() {
        play(pixmap, &retained.latest, area, fonts, images);
    }
    (history.frame, area)
}

/// One screen drawn frame after frame, live or not: what its last frame left behind and what changed in
/// the last frames. Each frame is recorded and compared with the last one, and what changed is played
/// into a buffer, which may be one of several used in turn, and drawn by more than one consumer (the
/// compositor's texture and its scanout buffers), each catching up from the frame it holds.
#[derive(Default)]
pub struct ScreenFrames {
    pub retained: Retained,
    history: History,
    /// What the frame recorded last changed from the one before.
    damage: Option<Bounds>,
    /// What the frames recorded since one was last drawn changed, for the history once one is.
    undrawn: Option<Bounds>,
}

impl ScreenFrames {
    /// Records the next frame (see [`record`]).
    pub fn record(&mut self, at: Placement, layer: Layer, state: &State, fonts: &mut DrawCache, images: &mut Images, views: &crate::Views) -> FrameOut {
        let out = record(&mut self.retained, at, layer, state, fonts, images, views);
        self.damage = self.retained.damage();
        if let Some(damage) = self.damage {
            self.undrawn = Some(self.undrawn.map_or(damage, |undrawn| undrawn.union(damage)));
        }
        // The frame recorded is the one to draw from now on.
        self.retained.advance();
        out
    }

    /// What the frame recorded last changes from the one before, or `None` if it looks the same.
    pub fn damage(&self) -> Option<Bounds> {
        self.damage
    }

    /// Has every buffer draw all of the frame, `all` (after the layer's size changed).
    pub fn redraw_all(&mut self, all: Bounds) {
        self.undrawn = Some(all);
    }

    /// Draws the frame recorded last into `pixmap`, which holds frame `held` (0 for none): what changed
    /// since (see [`draw_damaged`]). The first buffer to draw after frames were recorded adds what they
    /// changed to the history. Returns the frame the pixmap now holds and the area drawn.
    pub fn draw(&mut self, pixmap: &mut tiny_skia::Pixmap, held: u64, fonts: &mut DrawCache, images: &Images) -> (u64, Bounds) {
        if let Some(undrawn) = self.undrawn.take() {
            self.history.push(undrawn);
        }
        draw_damaged(pixmap, held, &self.retained, &self.history, fonts, images)
    }

    /// What a buffer holding frame `held` lacks of the latest frame: what changed since, or all of it.
    /// Frames recorded and not yet drawn by any buffer are not counted: call it after a buffer has drawn.
    pub fn since(&self, held: u64) -> Bounds {
        self.history.since(held, Bounds::of_size(self.retained.latest.size()))
    }

    /// Whether a buffer that holds frame `held` has something to draw.
    pub fn behind(&self, held: u64) -> bool {
        self.undrawn.is_some() || held != self.history.frame
    }
}

/// How many frames back [`History`] remembers what changed.
const AGES: usize = 4;

/// What changed in each of the last frames drawn, to bring a buffer drawn a few frames ago up to date.
#[derive(Debug, Default)]
pub struct History {
    /// How many frames have been drawn. Frame 0 is none.
    frame: u64,
    damage: [Bounds; AGES],
}

impl History {
    /// A frame that changed `damage` is drawn. Returns its number.
    fn push(&mut self, damage: Bounds) -> u64 {
        self.frame += 1;
        if let Some(d) = self.damage.get_mut(self.frame as usize % AGES) {
            *d = damage;
        }
        self.frame
    }

    /// What a buffer that holds frame `held` (0 for none) needs drawn to hold the latest frame: what
    /// changed in each frame since, or `all` if that is too long ago.
    fn since(&self, held: u64, all: Bounds) -> Bounds {
        if held == 0 || held > self.frame || self.frame - held > AGES as u64 {
            return all;
        }
        (held + 1..=self.frame).fold(Bounds::EMPTY, |d, f| d.union(self.damage.get(f as usize % AGES).copied().unwrap_or(all)))
    }
}

/// What the state doesn't know yet about the lists a frame drew: how many rows each one shows. A count
/// the state already has, give or take rounding, is not reported.
pub fn list_changes(state: &State, out: &FrameOut) -> impl Iterator<Item = Event> {
    ListId::ALL.into_iter().filter_map(move |list| {
        let rows = out.lists.get(list.index()).copied().flatten()?;
        let known = state.lists().scroll(list).visible().is_some_and(|v| (v - rows).abs() < 1e-3);
        (!known).then(|| ListEvent::Visible(list, rows).into())
    })
}

/// What a frame drawn without Wayland reports back to the state, as a live frame would: what the views
/// reported and the lists' visible rows.
#[cfg(any(test, feature = "dev"))]
pub fn frame_events(state: &State, out: &FrameOut) -> Vec<Event> {
    let mut events: Vec<Event> = out.report.clone().into_iter().collect();
    events.extend(list_changes(state, out));
    events
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_buffer_is_brought_up_to_date_with_what_changed_since_it_was_drawn() {
        let all = Bounds::new(0, 0, 100, 100);
        let mut h = History::default();
        let row = |y| Bounds::new(0, y, 100, y + 1);
        assert_eq!(h.push(row(1)), 1);
        assert_eq!(h.push(row(2)), 2);
        assert_eq!(h.push(row(3)), 3);
        assert_eq!(h.since(0, all), all, "a new buffer");
        assert_eq!(h.since(2, all), row(3));
        assert_eq!(h.since(1, all), Bounds::new(0, 2, 100, 4));
        for y in 4..10 {
            h.push(row(y));
        }
        assert_eq!(h.since(9, all), Bounds::EMPTY);
        assert_eq!(h.since(6, all), Bounds::new(0, 7, 100, 10));
        assert_eq!(h.since(5, all), Bounds::new(0, 6, 100, 10), "as far back as it remembers");
        assert_eq!(h.since(4, all), all, "too long ago");
    }

    #[test]
    fn orbit_stays_in_the_margin_and_moves_slowly() {
        let mut last = orbit(0);
        for minute in 1..2000u64 {
            let (dx, dy) = orbit(minute * 60);
            assert!(dx.unsigned_abs() <= MARGIN && dy.unsigned_abs() <= MARGIN);
            assert!((dx - last.0).abs() <= 2 && (dy - last.1).abs() <= 2);
            last = (dx, dy);
        }
    }
}
