//! The touchpad tab: the area below the bar is a touchpad (the input side forwards its touches to a
//! virtual touchpad; see the policy crate). Black, with a faint cross through the middle to show where it
//! is; why it doesn't work, when it doesn't (no touchscreen on a desktop, the grab refused).

use clamshell_core::TouchpadProblem;

use super::Frame;
use super::format::Fmt;
use super::style::{Text, Tone};
use crate::draw::{Align, Rect, Rgb};

/// The cross: barely above black.
const CROSS: Rgb = Rgb(0x18181c);
/// How far the cross stops short of the area's edges (and of the bar above), of its height.
const CLEAR: f32 = 0.05;
/// The message's line: its top and height, of the area's height.
const MESSAGE_Y: f32 = 0.15;
const MESSAGE_H: f32 = 0.1;

pub(super) fn draw(f: &mut Frame, body: Rect) {
    let (cx, cy) = body.center();
    let m = body.h * CLEAR;
    f.c.hline(body.x + m, body.x + body.w - m, cy, 2, CROSS);
    f.c.vline(cx, body.y + m, body.y + body.h - m, 2, CROSS);
    if let Some(problem) = &f.s.sys().touchpad_problem {
        let r = Rect::new(body.x, body.y + body.h * MESSAGE_Y, body.w, body.h * MESSAGE_H);
        let text = match problem {
            TouchpadProblem::NoTouchscreen => Fmt::<128>::of(format_args!("No touchpad: no touchscreen")),
            TouchpadProblem::Failed(e) => Fmt::of(format_args!("No touchpad: {e}")),
        };
        f.c.text_in(r, Text::Body.size(body.h), Tone::Dim, &text, Align::Center);
    }
}
