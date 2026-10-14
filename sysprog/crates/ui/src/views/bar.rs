//! The bar along the top of the second screen: one icon per mode (the active one lit), and the clock.

use sysprog_core::{Id, Mode};

use super::{ACCENT, DIM, Frame, Look, Scratch, TEXT, label};
use crate::draw::{Rect, rgb};
use crate::icons::Icon;

/// The bar's height, of the screen's (at least 20 pixels).
const BAR_H: f32 = 0.075;
/// Icon and clock text sizes, of the bar's height.
const ICON: f32 = 0.62;
const CLOCK_TEXT: f32 = 0.42;
/// Room around the clock text, of the bar's height.
const CLOCK_PAD: f32 = 0.8;

fn icon(mode: Mode) -> Icon {
    match mode {
        Mode::Follow => Icon::Gamepad,
        Mode::Launcher => Icon::Apps,
        Mode::Keyboard => Icon::Keyboard,
        Mode::Touchpad => Icon::Touchpad,
        Mode::Sysinfo => Icon::Sysinfo,
    }
}

/// Draws the bar; returns its height.
pub(super) fn draw(f: &mut Frame, w: f32, h: f32, scratch: &mut Scratch) -> f32 {
    let bar_h = (h * BAR_H).max(20.0);
    let side = bar_h * ICON;
    let clock = scratch.clock(f.s.unix_secs);
    let size = bar_h * CLOCK_TEXT;
    let clock_w = f.c.text_width(size, clock) + bar_h * CLOCK_PAD;
    // The mode icons share the width left of the clock.
    let slot = (w - clock_w) / Mode::ALL.len() as f32;
    let look = Look { radius: 8.0, fill: None, inset: 2.0, ring_inset: 0.0 };
    for (i, mode) in Mode::ALL.into_iter().enumerate() {
        let r = Rect::new(i as f32 * slot, 0.0, slot, bar_h);
        let color = if f.s.mode == mode { ACCENT } else { DIM };
        f.button(Id::Mode(mode), r, look, |c| {
            let (cx, cy) = r.center();
            c.icon(icon(mode), Rect::new(cx - side / 2.0, cy - side / 2.0, side, side), rgb(color));
        });
    }
    label(f.c, Rect::new(w - clock_w, 0.0, clock_w, bar_h), size, TEXT, clock);
    bar_h
}
