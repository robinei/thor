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
//! - Buffer pixels are surface pixels times the scale, as the pixmap has them.
//! - Screen pixels are the touchscreen's, used for the virtual touchpad. They are the buffer pixels of
//!   the second screen, which fills its output.

#[cfg(test)]
#[allow(unsafe_code, clippy::unwrap_used, clippy::expect_used)]
mod no_alloc;

#[cfg(any(test, feature = "dev"))]
use clamshell_state::ShelfEvent;
use clamshell_state::{Event, ListEvent, ListId, State};

#[cfg(any(test, feature = "dev"))]
use crate::Error;
use crate::gfx::images::Images;
use crate::gfx::paint::play;
use crate::gfx::record::{Bounds, Recording};
use crate::gfx::{Canvas, DrawCache, Rect};
use crate::toolkit::{Frame, FrameOut, Scratch, Target};
use crate::views;
use crate::{Layer, Screen};

/// OLED burn-in protection. Every screen is laid out inside a black margin of MARGIN buffer pixels on
/// each side, and drawn moved within it by an offset that drifts slowly, about a pixel a minute along an
/// irregular path within ±MARGIN. This keeps static elements such as the bar's icons and the clock from
/// sitting on the same pixels for hours, while nothing reaches the edge or is cut off. Tap areas move
/// with the picture.
pub(crate) const MARGIN: u32 = 4;

/// The burn-in drift at `unix_secs`, in buffer pixels.
pub(crate) fn orbit(unix_secs: u64) -> (i32, i32) {
    let minutes = (unix_secs / 60) as f32;
    let tau = std::f32::consts::TAU;
    let reach = MARGIN as f32;
    let dx = (reach * (minutes * tau / 17.0).sin()).round() as i32;
    let dy = (reach * (minutes * tau / 23.0).sin()).round() as i32;
    (dx, dy)
}

/// The layout's size inside the margin, in layout units, and the offset in buffer pixels to draw it at
/// for a drift of `(dx, dy)`.
pub(crate) fn inset(size_px: (u32, u32), scale: u32, (dx, dy): (i32, i32)) -> ((f32, f32), (i32, i32)) {
    let scale = scale.max(1) as f32;
    let w = size_px.0.saturating_sub(2 * MARGIN) as f32 / scale;
    let h = size_px.1.saturating_sub(2 * MARGIN) as f32 / scale;
    ((w, h), (MARGIN as i32 + dx, MARGIN as i32 + dy))
}

/// Where a frame goes: the buffer's size in pixels, its scale, and the burn-in drift.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Placement {
    pub size_px: (u32, u32),
    pub scale: u32,
    pub drift: (i32, i32),
}

/// What a screen keeps from its last frame: what can be tapped in it, in surface pixels, kept text, the
/// recording of the frame drawn last and the one recorded after it.
#[derive(Default)]
pub(crate) struct Retained {
    pub targets: Vec<Target>,
    pub scratch: Scratch,
    pub drawn: Recording,
    pub next: Recording,
}

impl Retained {
    /// What the frame recorded changes from the one drawn last, or `None` if it looks the same.
    pub(crate) fn damage(&self) -> Option<Bounds> {
        self.next.damage(&self.drawn)
    }

    /// The frame recorded has been drawn.
    pub(crate) fn drew(&mut self) {
        std::mem::swap(&mut self.drawn, &mut self.next);
    }
}

/// Records `layer` for `state` into `retained.next`, and moves its targets and the touchpad area into
/// surface pixels. A screen's frame starts with the whole buffer black, an overlay's clear.
pub(crate) fn record(retained: &mut Retained, at: Placement, layer: Layer, state: &State, fonts: &mut DrawCache, images: &mut Images) -> FrameOut {
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
            Layer::Screen(Screen::Top) => views::top(&mut f, layout),
            Layer::Screen(Screen::Bottom) => views::bottom(&mut f, layout),
            Layer::Overlay(screen) => views::overlay(&mut f, layout, screen),
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
        if let Some(track) = &mut target.track {
            track.x0 += sx;
        }
    }
    out.touchpad_area = out.touchpad_area.map(|r| r.shifted(sx, sy));
    out
}

/// Draws `layer` for `state` into `pixmap`, which must be `at`'s size, as [`record`] and then the
/// whole recording played.
#[cfg(any(test, feature = "dev"))]
pub(crate) fn render<const Q: bool, const B: bool>(
    pixmap: crate::Pixels<Q, B>,
    retained: &mut Retained,
    at: Placement,
    layer: Layer,
    state: &State,
    fonts: &mut DrawCache,
    images: &mut Images,
) -> Result<FrameOut, Error> {
    let (w, h) = at.size_px;
    if (pixmap.width(), pixmap.height()) != (w, h) {
        return Err(format!("a {}x{} pixmap for a {w}x{h} frame", pixmap.width(), pixmap.height()).into());
    }
    let out = record(retained, at, layer, state, fonts, images);
    play(pixmap, &retained.next, Bounds::of_size(at.size_px), fonts, images);
    retained.drew();
    Ok(out)
}

/// Brings `pixmap`, a buffer that holds frame `held` of `history` (0 if none), up to the frame recorded
/// last in `retained` (its `drawn`). Only the commands that meet what changed since `held` are played,
/// within it. Returns the frame the buffer now holds and the area drawn.
pub(crate) fn draw_damaged<const Q: bool, const B: bool>(
    pixmap: crate::Pixels<Q, B>,
    held: u64,
    retained: &Retained,
    history: &History,
    fonts: &mut DrawCache,
    images: &Images,
) -> (u64, Bounds) {
    let area = history.since(held, Bounds::of_size(retained.drawn.size()));
    if !area.is_empty() {
        play(pixmap, &retained.drawn, area, fonts, images);
    }
    (history.frame, area)
}

/// One screen drawn frame after frame, live or not: what its last frame left behind and what changed in
/// the last frames. Each frame is recorded and compared with the last one, and what changed is played
/// into a buffer, which may be one of several used in turn, and drawn by more than one consumer (the
/// compositor's texture and its scanout buffers), each catching up from the frame it holds.
#[derive(Default)]
pub(crate) struct ScreenFrames {
    pub retained: Retained,
    history: History,
    /// What the frame recorded last changed from the one before.
    damage: Option<Bounds>,
    /// What the frames recorded since one was last drawn changed, for the history once one is.
    undrawn: Option<Bounds>,
}

impl ScreenFrames {
    /// Records the next frame (see [`record`]).
    pub(crate) fn record(&mut self, at: Placement, layer: Layer, state: &State, fonts: &mut DrawCache, images: &mut Images) -> FrameOut {
        let out = record(&mut self.retained, at, layer, state, fonts, images);
        self.damage = self.retained.damage();
        if let Some(damage) = self.damage {
            self.undrawn = Some(self.undrawn.map_or(damage, |undrawn| undrawn.union(damage)));
        }
        // The frame recorded is the one to draw from now on.
        self.retained.drew();
        out
    }

    /// What the frame recorded last changes from the one before, or `None` if it looks the same.
    pub(crate) fn damage(&self) -> Option<Bounds> {
        self.damage
    }

    /// Has every buffer draw all of the frame, `all` (after the layer's size changed).
    pub(crate) fn redraw_all(&mut self, all: Bounds) {
        self.undrawn = Some(all);
    }

    /// Draws the frame recorded last into `pixmap`, which holds frame `held` (0 for none): what changed
    /// since (see [`draw_damaged`]). The first buffer to draw after frames were recorded adds what they
    /// changed to the history. Returns the frame the pixmap now holds and the area drawn.
    pub(crate) fn draw<const Q: bool, const B: bool>(
        &mut self,
        pixmap: crate::Pixels<Q, B>,
        held: u64,
        fonts: &mut DrawCache,
        images: &Images,
    ) -> (u64, Bounds) {
        if let Some(undrawn) = self.undrawn.take() {
            self.history.push(undrawn);
        }
        draw_damaged(pixmap, held, &self.retained, &self.history, fonts, images)
    }

    /// Whether a buffer that holds frame `held` has something to draw.
    pub(crate) fn behind(&self, held: u64) -> bool {
        self.undrawn.is_some() || held != self.history.frame
    }
}

/// How many frames back [`History`] remembers what changed.
const AGES: usize = 4;

/// What changed in each of the last frames drawn, to bring a buffer drawn a few frames ago up to date.
#[derive(Debug, Default)]
pub(crate) struct History {
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
        if held == 0 || held > self.frame || self.frame - held >= AGES as u64 {
            return all;
        }
        (held + 1..=self.frame).fold(Bounds::EMPTY, |d, f| d.union(self.damage.get(f as usize % AGES).copied().unwrap_or(all)))
    }
}

/// What the state doesn't know yet about the lists a frame drew: how many rows each one shows. A count
/// the state already has, give or take rounding, is not reported.
pub(crate) fn list_changes(state: &State, out: &FrameOut) -> impl Iterator<Item = Event> {
    ListId::ALL.into_iter().filter_map(move |list| {
        let rows = out.lists.get(list.index()).copied().flatten()?;
        let known = state.lists().scroll(list).visible().is_some_and(|v| (v - rows).abs() < 1e-3);
        (!known).then(|| ListEvent::Visible(list, rows).into())
    })
}

/// What a frame drawn without Wayland reports back to the state, as a live frame would: shelf's rows and
/// the lists' visible rows.
#[cfg(any(test, feature = "dev"))]
pub(crate) fn frame_events(state: &State, out: &FrameOut) -> Vec<Event> {
    let mut events: Vec<Event> = out.list_rows.filter(|&r| r != state.shelf().rows()).map(|r| ShelfEvent::Rows(r).into()).into_iter().collect();
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
        assert_eq!(h.since(5, all), all, "too long ago");
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

#[cfg(test)]
#[allow(clippy::expect_used, clippy::unwrap_used, clippy::indexing_slicing)]
mod frame_times {
    //! How long a frame takes to draw at the Thor's size: a whole frame recorded and played straight into
    //! a buffer, as a live view draws a frame that changed everywhere; only the playing; and recording a
    //! frame and finding it unchanged, which is what every event costs a visible screen. Run it by hand:
    //! `cargo test --release -p clamshell-ui frame_times -- --ignored --nocapture`.

    use super::*;
    use clamshell_state::{Event, Id, Init, PointerEvent, Tab};
    use std::time::{Duration, Instant};

    fn state(events: Vec<Event>) -> State {
        let mut s = State::new(Init { library: Some(clamshell_library::demo::library()), ..Init::default() });
        s.apply(Event::tick(1_700_000_000), &mut Vec::new());
        for e in events {
            s.apply(e, &mut Vec::new());
        }
        s
    }

    fn median(mut v: Vec<Duration>) -> Duration {
        v.sort();
        v[v.len() / 2]
    }

    /// Two buffers drawing one screen's frames: one draws nearly each frame, the other now and then; both
    /// hold what a whole frame draws, also after a frame no buffer drew.
    #[test]
    fn a_buffer_that_draws_now_and_then_catches_up() {
        let (w, h) = (480u32, 270u32);
        let at = Placement { size_px: (w, h), scale: 1, drift: (0, 0) };
        let (mut fonts, mut images) = (DrawCache::load(), Images::synchronous());
        let mut frames = ScreenFrames::default();
        let (mut every, mut sometimes) = (tiny_skia::Pixmap::new(w, h).unwrap(), tiny_skia::Pixmap::new(w, h).unwrap());
        let (mut held_every, mut held_sometimes) = (0, 0);
        for step in 0..7 {
            // The shelf's selection a row further down each frame.
            let s = state(vec![Event::Shelf(ShelfEvent::ListStep(step + 1))]);
            frames.record(at, Screen::Top.into(), &s, &mut fonts, &mut images);
            // At step 4 no buffer draws: what it changed is kept for the next draw.
            if step != 4 {
                held_every = frames.draw((&mut every).into(), held_every, &mut fonts, &images).0;
            }
            if step % 3 == 2 {
                held_sometimes = frames.draw((&mut sometimes).into(), held_sometimes, &mut fonts, &images).0;
                let mut whole = tiny_skia::Pixmap::new(w, h).unwrap();
                render((&mut whole).into(), &mut Retained::default(), at, Screen::Top.into(), &s, &mut fonts, &mut images).unwrap();
                assert!(every.data() == whole.data() && sometimes.data() == whole.data(), "step {step}");
            }
        }
    }

    /// A screen drawn turned, as into a panel's buffer mounted sideways, holds the very pixels drawn
    /// unturned, turned: shelf, the dock and the keyboard, turned and not, in a buffer with padded rows,
    /// its bytes red first or blue first.
    #[test]
    fn a_turned_frame_is_the_frame_turned() {
        let tab = |t| vec![Event::Pointer(PointerEvent::Activate(Id::Tab(t)))];
        let cases = [(Screen::Top, (480u32, 270u32), vec![]), (Screen::Bottom, (310, 270), tab(Tab::Dock)), (Screen::Bottom, (310, 270), tab(Tab::Keyboard))];
        let (mut fonts, mut images) = (DrawCache::load(), Images::synchronous());
        for (screen, (w, h), events) in cases {
            let s = state(events);
            let at = Placement { size_px: (w, h), scale: 1, drift: (1, -2) };
            let mut plain = tiny_skia::Pixmap::new(w, h).unwrap();
            render((&mut plain).into(), &mut Retained::default(), at, screen.into(), &s, &mut fonts, &mut images).unwrap();
            let check = |buffer: &[u8], stride: usize, quarter: bool, bgra: bool| {
                for y in 0..h {
                    for x in 0..w {
                        let (bx, by) = if quarter { (h - 1 - y, x) } else { (x, y) };
                        let i = (by as usize * stride + bx as usize) * 4;
                        let j = (y * w + x) as usize * 4;
                        let [r, g, b, a] = [plain.data()[j], plain.data()[j + 1], plain.data()[j + 2], plain.data()[j + 3]];
                        let want = if bgra { [b, g, r, a] } else { [r, g, b, a] };
                        assert_eq!(buffer[i..i + 4], want, "{screen:?} quarter {quarter} bgra {bgra} at ({x}, {y})");
                    }
                }
            };
            let stride = w.max(h) as usize + 7;
            let mut buffer = vec![0u8; stride * w.max(h) as usize * 4];
            let target = crate::Pixels::<true, false>::new(&mut buffer, (w, h), stride).unwrap();
            render(target, &mut Retained::default(), at, screen.into(), &s, &mut fonts, &mut images).unwrap();
            check(&buffer, stride, true, false);
            let target = crate::Pixels::<true, true>::new(&mut buffer, (w, h), stride).unwrap();
            render(target, &mut Retained::default(), at, screen.into(), &s, &mut fonts, &mut images).unwrap();
            check(&buffer, stride, true, true);
            let target = crate::Pixels::<false, true>::new(&mut buffer, (w, h), stride).unwrap();
            render(target, &mut Retained::default(), at, screen.into(), &s, &mut fonts, &mut images).unwrap();
            check(&buffer, stride, false, true);
        }
    }

    #[test]
    #[ignore = "a measurement, by hand in release"]
    fn frame_times() {
        let tab = |t| vec![Event::Pointer(PointerEvent::Activate(Id::Tab(t)))];
        let cases = [
            ("top: shelf", Screen::Top, (1920u32, 1080u32), vec![]),
            ("bottom: dock", Screen::Bottom, (1240, 1080), tab(Tab::Dock)),
            ("bottom: settings", Screen::Bottom, (1240, 1080), tab(Tab::Settings)),
            ("bottom: keyboard", Screen::Bottom, (1240, 1080), tab(Tab::Keyboard)),
        ];
        let (mut fonts, mut images) = (DrawCache::load(), Images::synchronous());
        for (name, screen, size, events) in cases {
            let s = state(events);
            let at = Placement { size_px: size, scale: 1, drift: (1, -2) };
            let mut retained = Retained::default();
            let len = size.0 as usize * size.1 as usize * 4;
            let mut buffer = vec![0u8; len];
            let rounds = 200;
            let (mut full, mut checked, mut played, mut turned) = (Vec::new(), Vec::new(), Vec::new(), Vec::new());
            for round in 0..rounds + 20 {
                // A whole frame: recorded and played into the buffer.
                let t = Instant::now();
                let target = crate::Pixels::<false, false>::new(&mut buffer, size, size.0 as usize).unwrap();
                render(target, &mut retained, at, screen.into(), &s, &mut fonts, &mut images).unwrap();
                let f = t.elapsed();
                // Recorded again and found unchanged, which is all an event that changes nothing costs.
                let t = Instant::now();
                record(&mut retained, at, screen.into(), &s, &mut fonts, &mut images);
                assert_eq!(retained.damage(), None);
                let c = t.elapsed();
                // Only played.
                let t = Instant::now();
                let target = crate::Pixels::<false, false>::new(&mut buffer, size, size.0 as usize).unwrap();
                play(target, &retained.next, Bounds::of_size(size), &mut fonts, &images);
                let p = t.elapsed();
                // Only played, into a buffer turned a quarter, as a panel mounted sideways takes it.
                let t = Instant::now();
                let target = crate::Pixels::<true, false>::new(&mut buffer, size, size.1 as usize).unwrap();
                play(target, &retained.next, Bounds::of_size(size), &mut fonts, &images);
                let q = t.elapsed();
                // The first rounds warm the caches.
                if round >= 20 {
                    full.push(f);
                    checked.push(c);
                    played.push(p);
                    turned.push(q);
                }
            }
            let ms = |v: Vec<Duration>| median(v).as_secs_f64() * 1e3;
            println!(
                "{name:18} whole frame {:>7.3} ms   played {:>7.3} ms   played turned {:>7.3} ms   recorded and compared {:>7.3} ms",
                ms(full),
                ms(played),
                ms(turned),
                ms(checked),
            );
        }
    }
}
