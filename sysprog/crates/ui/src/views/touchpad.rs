//! Touchpad mode: the area below the bar is a touchpad (the input side forwards its touches to a virtual
//! touchpad; see the policy crate). Black, with a faint cross through the middle to show where it is.

use super::Frame;
use crate::draw::{Rect, rgb};

/// The cross: barely above black.
const CROSS: u32 = 0x18181c;
/// How far the cross stops short of the area's edges (and of the bar above), of its height.
const CLEAR: f32 = 0.05;

pub(super) fn draw(f: &mut Frame, body: Rect) {
    let (cx, cy) = body.center();
    let m = body.h * CLEAR;
    f.c.hline(body.x + m, body.x + body.w - m, cy, 2, rgb(CROSS));
    f.c.vline(cx, body.y + m, body.y + body.h - m, 2, rgb(CROSS));
}
