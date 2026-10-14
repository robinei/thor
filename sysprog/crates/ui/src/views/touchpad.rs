//! Touchpad mode: the area below the bar is a touchpad (the input side forwards its touches to a virtual
//! touchpad; see the policy crate). Black, with a faint cross through the middle to show where it is.

use super::Frame;
use crate::draw::{Rect, rgb};

/// The cross: barely above black.
const CROSS: u32 = 0x18181c;

pub(super) fn draw(f: &mut Frame, body: Rect) {
    let (cx, cy) = body.center();
    f.c.hline(body.x, body.x + body.w, cy, 2, rgb(CROSS));
    f.c.vline(cx, body.y, body.y + body.h, 2, rgb(CROSS));
}
