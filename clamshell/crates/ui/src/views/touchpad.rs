//! Touchpad mode: the area below the bar is a touchpad (the input side forwards its touches to a virtual
//! touchpad; see the policy crate). Black, with a faint cross through the middle to show where it is;
//! why it doesn't work, when it doesn't (no touchscreen on a desktop, the grab refused).

use super::{DIM, Frame, label};
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
    if let Some(problem) = &f.s.touchpad_problem {
        let r = Rect::new(body.x, body.y + body.h * 0.15, body.w, body.h * 0.1);
        label(f.c, r, body.h * 0.04, DIM, problem);
    }
}
