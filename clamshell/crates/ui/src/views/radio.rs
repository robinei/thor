//! What the Wi-Fi and Bluetooth tabs share: a header with what the radio is doing on the left and its
//! switch on the right, then a scrolled list of rows (networks, devices) above a hint line.

use clamshell_core::Id;

use crate::draw::{Keep, Rect};
use crate::toolkit::style::{self, Text, Tone};
use crate::toolkit::widgets::ListArea;
use crate::toolkit::{Frame, widgets};

/// The header's height and a row's height, of the area's height.
const HEADER_H: f32 = 0.1;
const ROW_H: f32 = 0.105;
/// The switch's button, of the header's height.
const SWITCH_W: f32 = 1.6;

/// The header: the radio's `switch` (on or off) and what it is doing (`status`, in its
/// tone); returns where the rows go.
pub(super) fn header(f: &mut Frame, switch: Id, on: bool, (tone, status): (Tone, &str)) -> ListArea {
    let body = f.body();
    let pad = f.pad();
    let mut list = style::content(body);
    let mut header = list.take_top(f.body().h * HEADER_H);
    let size = f.text(Text::Body);
    let sw = header.take_right(header.h * SWITCH_W);
    widgets::switch(f, switch, sw, on);
    f.c.text_fit(Rect::new(header.x, header.y, header.w - pad, header.h), size, tone, status, Keep::Start);
    ListArea::new(f, list, f.body().h * ROW_H)
}
