//! What the Wi-Fi and Bluetooth tabs share: a header with what the radio is doing on the left and its
//! switch on the right, then a list of rows (networks, devices) above a hint line.

use clamshell_core::Id;

use super::{DIM, Frame, GO, Look, SURFACE_HI, TEXT_HI};
use crate::draw::{Keep, Rect, rgb};

/// Padding, the header's height and text size, a row's height, of the area's height.
const PAD: f32 = 0.03;
const HEADER_H: f32 = 0.1;
const TEXT_SIZE: f32 = 0.04;
const ROW_H: f32 = 0.105;
/// The switch's size, of the header's height.
const SWITCH_H: f32 = 0.42;
const SWITCH_W: f32 = 0.85;
/// Icons, of a row's height; the right column (state), of the area's width.
pub(super) const ICON: f32 = 0.42;
pub(super) const STATE_W: f32 = 0.22;

/// Where the rows go.
pub(super) struct List {
    pub pad: f32,
    /// Text size.
    pub size: f32,
    pub area: Rect,
    pub row_h: f32,
    /// How many rows fit above the hint line, and where that line is.
    pub rows: usize,
    pub hint_y: f32,
}

impl List {
    /// Row `i`'s rect (a little wider than the area, for the pressed highlight).
    pub fn row(&self, i: usize) -> Rect {
        Rect::new(self.area.x - self.pad * 0.5, self.area.y + i as f32 * self.row_h, self.area.w + self.pad, self.row_h)
    }

    /// The hint at the bottom, in small dim text.
    pub fn hint(&self, f: &mut Frame, text: &str) {
        f.c.text(self.area.x, self.hint_y, self.size * 0.8, rgb(DIM), text);
    }

    /// A dim line where the rows would be (nothing found yet).
    pub fn empty(&self, f: &mut Frame, text: &str) {
        f.c.text(self.area.x, self.area.y + self.pad, self.size, rgb(DIM), text);
    }
}

/// The header over `body`: the radio's `switch` (on or off) and what it is doing (`status`, in its
/// colour); returns where the rows go.
pub(super) fn header(f: &mut Frame, body: Rect, switch: Id, on: bool, (color, status): (u32, &str)) -> List {
    let pad = body.h * PAD;
    let area = Rect::new(body.x + pad, body.y, body.w - 2.0 * pad, body.h - pad);
    let (header, list) = area.split_top(body.h * HEADER_H);
    let size = body.h * TEXT_SIZE;
    let sw = Rect::new(header.x + header.w - header.h * 1.6, header.y, header.h * 1.6, header.h);
    let look = Look { radius: 8.0, fill: None, inset: 2.0, ring_inset: 0.0 };
    f.button(switch, sw, look, |c| {
        let (h, w) = (sw.h * SWITCH_H, sw.h * SWITCH_W);
        let track = Rect::new(sw.x + (sw.w - w) / 2.0, sw.y + (sw.h - h) / 2.0, w, h);
        c.rounded(track, h / 2.0, rgb(if on { GO } else { SURFACE_HI }));
        let knob_x = if on { track.x + track.w - h / 2.0 } else { track.x + h / 2.0 };
        c.circle(knob_x, track.y + h / 2.0, h * 0.38, rgb(if on { TEXT_HI } else { DIM }));
    });
    f.c.text_fit((header.x, header.y + (header.h - size * 1.2) / 2.0, sw.x - header.x - pad), size, rgb(color), status, Keep::Start);
    let row_h = body.h * ROW_H;
    let hint_y = list.y + list.h - size * 1.2;
    let rows = ((hint_y - list.y) / row_h).floor().max(0.0) as usize;
    List { pad, size, area: list, row_h, rows, hint_y }
}
