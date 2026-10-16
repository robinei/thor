//! What the Wi-Fi and Bluetooth tabs share: a header with what the radio is doing on the left and its
//! switch on the right, then a scrolled list of rows (networks, devices) above a hint line.

use clamshell_core::{Id, ListId};

use super::scroll::Scrolled;
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
    /// Where the hint line is (the rows scroll above it).
    pub hint_y: f32,
}

impl List {
    /// The rows' list begun: `len` rows above the hint line, each a little wider than the area (for the
    /// pressed highlight).
    pub fn scrolled(&self, f: &mut Frame, list: ListId, len: usize) -> Scrolled {
        let view = Rect::new(self.area.x - self.pad * 0.5, self.area.y, self.area.w + self.pad, self.hint_y - self.area.y - self.pad * 0.3);
        Scrolled::begin(f, list, view, self.row_h, len)
    }

    /// Where the scroll bar goes: just right of the rows.
    pub fn bar_x(&self) -> f32 {
        self.area.x + self.area.w + self.pad * 0.6
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
pub(super) fn header(f: &mut Frame, switch: Id, on: bool, (tone, status): (Tone, &str)) -> List {
    let body = f.body();
    let pad = f.pad();
    let mut list = style::content(body);
    let mut header = list.take_top(f.body().h * HEADER_H);
    let size = f.text(Text::Body);
    let sw = header.take_right(header.h * SWITCH_W);
    widgets::switch(f, switch, sw, on);
    f.c.text_fit(Rect::new(header.x, header.y, header.w - pad, header.h), size, tone, status, Keep::Start);
    let row_h = f.body().h * ROW_H;
    let hint_y = list.y + list.h - size * LINE;
    List { pad, size, area: list, row_h, hint_y }
}
