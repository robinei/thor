//! What the Wi-Fi and Bluetooth tabs share: a header with what the radio is doing on the left and its
//! switch on the right, then a list of rows (networks, devices) above a hint line.

use clamshell_core::Id;

use super::style::{self, Text, Tone};
use super::{Frame, widgets};
use crate::draw::{Keep, LINE, Rect};

/// The header's height and a row's height, of the area's height.
const HEADER_H: f32 = 0.1;
const ROW_H: f32 = 0.105;
/// The switch's button, of the header's height.
const SWITCH_W: f32 = 1.6;
/// The hint's text, of the rows' text.
const HINT: f32 = 0.8;

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
        f.c.text(self.area.x, self.hint_y, self.size * HINT, Tone::Dim, text);
    }

    /// A dim line where the rows would be (nothing found yet).
    pub fn empty(&self, f: &mut Frame, text: &str) {
        f.c.text(self.area.x, self.area.y + self.pad, self.size, Tone::Dim, text);
    }
}

/// The header over `body`: the radio's `switch` (on or off) and what it is doing (`status`, in its
/// tone); returns where the rows go.
pub(super) fn header(f: &mut Frame, body: Rect, switch: Id, on: bool, (tone, status): (Tone, &str)) -> List {
    let pad = style::pad(body.h);
    let mut list = style::content(body);
    let mut header = list.take_top(body.h * HEADER_H);
    let size = Text::Body.size(body.h);
    let sw = header.take_right(header.h * SWITCH_W);
    widgets::switch(f, switch, sw, on);
    f.c.text_fit(Rect::new(header.x, header.y, header.w - pad, header.h), size, tone, status, Keep::Start);
    let row_h = body.h * ROW_H;
    let hint_y = list.y + list.h - size * LINE;
    let rows = ((hint_y - list.y) / row_h).floor().max(0.0) as usize;
    List { pad, size, area: list, row_h, rows, hint_y }
}
