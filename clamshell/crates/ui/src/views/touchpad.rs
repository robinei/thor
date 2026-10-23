//! The touchpad tab: the empty area below the bar, where touches always move the pointer (see
//! clamshell-state's `touch`). It is black, with a faint cross through the middle to show where it is.

use crate::gfx::Rgb;
use crate::toolkit::Frame;

/// The cross's colour, barely above black.
const CROSS: Rgb = Rgb(0x18181c);
/// How far the cross stops short of the area's edges and the bar above, as a fraction of the area's
/// height.
const CLEAR: f32 = 0.05;

pub(super) fn draw(f: &mut Frame) {
    let body = f.body();
    let (cx, cy) = body.center();
    let m = body.h * CLEAR;
    f.c.hline(body.x + m, body.x + body.w - m, cy, 2, CROSS);
    f.c.vline(cx, body.y + m, body.y + body.h - m, 2, CROSS);
}
