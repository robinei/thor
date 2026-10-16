//! Drawing one frame of a screen into its picture, the same for a live view and a snapshot.
//!
//! Coordinates, from the inside out:
//! - Layout units: logical pixels from the top left of the area inside the burn-in margin. The views
//!   draw and place their targets in these.
//! - Surface pixels: logical pixels of the surface (what Wayland reports for touch and the pointer).
//!   Layout units moved by the burn-in offset; targets are moved into these once the frame is drawn.
//! - Buffer pixels: surface pixels times the scale (what the pixmap has).
//! - Screen pixels: the touchscreen's, for the touchpad thread (`TouchLayout`): buffer pixels of the
//!   second screen, which fills its output.

use clamshell_core::State;
use tiny_skia::Pixmap;

use crate::draw::{Canvas, DrawCache, Rect};
use crate::images::Images;
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

/// What a screen keeps from frame to frame: the picture, what can be tapped in it (in surface pixels),
/// and kept text.
#[derive(Default)]
pub(crate) struct Picture {
    pub pixmap: Option<Pixmap>,
    pub targets: Vec<Target>,
    pub scratch: Scratch,
}

/// What a frame reports, with where its layout starts in the surface (the burn-in offset, in surface
/// pixels).
#[derive(Debug, Clone, Copy)]
pub(crate) struct Rendered {
    pub out: FrameOut,
    pub origin: (f32, f32),
}

/// Draws `screen` for `state` into `picture` (sized to `at` first), and moves its targets and the
/// touchpad area into surface pixels.
pub(crate) fn render(
    picture: &mut Picture,
    at: Placement,
    screen: Screen,
    state: &State,
    fonts: &mut DrawCache,
    images: &mut Images,
) -> Result<Rendered, Error> {
    let (w, h) = at.size_px;
    if picture.pixmap.as_ref().map(|p| (p.width(), p.height())) != Some((w, h)) {
        picture.pixmap = Pixmap::new(w, h);
    }
    let Some(pixmap) = picture.pixmap.as_mut() else {
        return Err(format!("can't allocate a {w}x{h} pixmap").into());
    };
    let scale = at.scale.max(1);
    let ((lw, lh), offset) = inset((w, h), scale, at.drift);
    let mut out = {
        let mut canvas = Canvas::new(pixmap, scale as f32, fonts).with_offset(offset);
        let mut f = Frame::new(&mut canvas, state, images, &mut picture.scratch, &mut picture.targets);
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
    for target in &mut picture.targets {
        target.rect.x += sx;
        target.rect.y += sy;
        if let Some(track) = &mut target.track {
            track.x0 += sx;
        }
    }
    out.touchpad_area = out.touchpad_area.map(|r| r.shifted(sx, sy));
    Ok(Rendered { out, origin: (sx, sy) })
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
