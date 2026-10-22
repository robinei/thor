//! Drawing one frame of a screen, the same for a live view (straight into the compositor's shared buffer)
//! and a snapshot (into a pixmap of its own).
//!
//! Coordinates, from the inside out:
//! - Layout units: logical pixels from the top left of the area inside the burn-in margin. The views
//!   draw and place their targets in these.
//! - Surface pixels: logical pixels of the surface (what Wayland reports for touch and the pointer).
//!   Layout units moved by the burn-in offset; targets are moved into these once the frame is drawn.
//! - Buffer pixels: surface pixels times the scale (what the pixmap has).
//! - Screen pixels: the touchscreen's, for the virtual touchpad: buffer pixels of the second screen,
//!   which fills its output.

use clamshell_core::State;
use tiny_skia::PixmapMut;

use crate::gfx::images::Images;
use crate::gfx::{Canvas, DrawCache, Rect};
use crate::toolkit::{Frame, FrameOut, Scratch, Target};
use crate::views;
use crate::{Error, Screen};

/// OLED burn-in protection: every screen is laid out inside a black margin of MARGIN buffer pixels on
/// each side, and drawn moved within it by an offset that drifts slowly (about a pixel a minute, along an
/// irregular path within ±MARGIN), so static elements like the bar's icons and the clock don't sit on the
/// same pixels for hours, and nothing ever reaches the edge or is cut off. Tap areas move with the
/// picture.
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

/// The layout's size (inside the margin, in layout units), and the buffer-pixel offset to draw it at
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

/// What a screen keeps from its last frame: what can be tapped in it (in surface pixels), and kept text.
#[derive(Default)]
pub(crate) struct Retained {
    pub targets: Vec<Target>,
    pub scratch: Scratch,
}

/// Draws `screen` for `state` into `pixmap` (`at`'s size), and moves its targets and the touchpad area
/// into surface pixels. The whole frame is drawn, onto black.
pub(crate) fn render(
    mut pixmap: PixmapMut,
    retained: &mut Retained,
    at: Placement,
    screen: Screen,
    state: &State,
    fonts: &mut DrawCache,
    images: &mut Images,
) -> Result<FrameOut, Error> {
    let (w, h) = at.size_px;
    if (pixmap.width(), pixmap.height()) != (w, h) {
        return Err(format!("a {}x{} pixmap for a {w}x{h} frame", pixmap.width(), pixmap.height()).into());
    }
    // Black, all of it: the views paint on black (no background of their own), and a buffer reused
    // holds an older frame.
    pixmap.fill(crate::toolkit::style::BG.into());
    let scale = at.scale.max(1);
    let ((lw, lh), offset) = inset((w, h), scale, at.drift);
    let mut out = {
        let mut canvas = Canvas::new(pixmap, scale as f32, fonts).with_offset(offset);
        let mut f = Frame::new(&mut canvas, state, images, &mut retained.scratch, &mut retained.targets);
        let layout = Rect::new(0.0, 0.0, lw, lh);
        match screen {
            Screen::Top => views::top(&mut f, layout),
            Screen::Bottom => views::bottom(&mut f, layout),
        }
        f.finish()
    };
    if screen == Screen::Top {
        images.end_frame();
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
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

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
    //! How long a frame takes to draw, at the Thor's size: straight into a buffer (what a live view does),
    //! and drawn into a pixmap of its own then copied into a buffer (what it did before). Run by hand:
    //! `cargo test --release -p clamshell-ui frame_times -- --ignored --nocapture`.

    use super::*;
    use clamshell_core::{Event, Id, Init, PointerEvent, Tab};
    use std::time::{Duration, Instant};

    fn state(events: Vec<Event>) -> State {
        let mut s = State::new(Init { library: clamshell_library::demo::library(), ..Init::default() });
        s.apply(Event::tick(1_700_000_000));
        for e in events {
            s.apply(e);
        }
        s
    }

    fn median(mut v: Vec<Duration>) -> Duration {
        v.sort();
        v[v.len() / 2]
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
            let mut pixmap = tiny_skia::Pixmap::new(size.0, size.1).unwrap();
            let rounds = 200;
            let (mut direct, mut copied) = (Vec::new(), Vec::new());
            for round in 0..rounds + 20 {
                let t = Instant::now();
                let target = tiny_skia::PixmapMut::from_bytes(&mut buffer, size.0, size.1).unwrap();
                render(target, &mut retained, at, screen, &s, &mut fonts, &mut images).unwrap();
                let d = t.elapsed();
                let t = Instant::now();
                render(pixmap.as_mut(), &mut retained, at, screen, &s, &mut fonts, &mut images).unwrap();
                buffer.copy_from_slice(pixmap.data());
                let c = t.elapsed();
                // (The first rounds warm the caches.)
                if round >= 20 {
                    direct.push(d);
                    copied.push(c);
                }
            }
            println!("{name:18} direct {:>7.3} ms   drawn then copied {:>7.3} ms", median(direct).as_secs_f64() * 1e3, median(copied).as_secs_f64() * 1e3);
        }
    }
}
