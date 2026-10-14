//! The Bluetooth mode: what Bluetooth is doing (connected, connecting, what went wrong) with the radio's
//! switch, then the devices, one row each: its kind's icon, its name, and whether it is connected, being
//! connected or paired. A tap connects (pairing a new device first) or disconnects, a long press forgets a
//! paired device. While open, new devices are looked for.

use std::fmt::Write;

use clamshell_core::bluetooth::Kind;
use clamshell_core::{Id, State};

use super::{ACCENT, DIM, Frame, GO, Look, SURFACE_HI, TEXT, TEXT_HI};
use crate::draw::{Keep, Rect, rgb};
use crate::icons::Icon;

/// Padding, the header's height and text size, a row's height, of the area's height.
const PAD: f32 = 0.03;
const HEADER_H: f32 = 0.1;
const TEXT_SIZE: f32 = 0.04;
const ROW_H: f32 = 0.105;
/// The switch's size, of the header's height.
const SWITCH_H: f32 = 0.42;
const SWITCH_W: f32 = 0.85;
/// Icons, of a row's height; the state column, of the area's width.
const ICON: f32 = 0.42;
const STATE_W: f32 = 0.22;

fn icon(kind: Kind) -> Icon {
    match kind {
        Kind::Audio => Icon::Headphones,
        Kind::Gamepad => Icon::Gamepad,
        Kind::Keyboard => Icon::Keyboard,
        Kind::Mouse => Icon::Mouse,
        Kind::Phone => Icon::Phone,
        Kind::Computer | Kind::Other => Icon::Bluetooth { connected: false },
    }
}

pub(super) fn draw(f: &mut Frame, body: Rect, buf: &mut String) {
    let s = f.s;
    let bt = &s.bluetooth;
    let pad = body.h * PAD;
    let area = Rect::new(body.x + pad, body.y, body.w - 2.0 * pad, body.h - pad);
    let (header, list) = area.split_top(body.h * HEADER_H);
    let size = body.h * TEXT_SIZE;

    let sw = Rect::new(header.x + header.w - header.h * 1.6, header.y, header.h * 1.6, header.h);
    let on = bt.powered;
    let look = Look { radius: 8.0, fill: None, inset: 2.0, ring_inset: 0.0 };
    f.button(Id::BluetoothPower, sw, look, |c| {
        let (h, w) = (sw.h * SWITCH_H, sw.h * SWITCH_W);
        let track = Rect::new(sw.x + (sw.w - w) / 2.0, sw.y + (sw.h - h) / 2.0, w, h);
        c.rounded(track, h / 2.0, rgb(if on { GO } else { SURFACE_HI }));
        let knob_x = if on { track.x + track.w - h / 2.0 } else { track.x + h / 2.0 };
        c.circle(knob_x, track.y + h / 2.0, h * 0.38, rgb(if on { TEXT_HI } else { DIM }));
    });
    let (color, text) = status_line(s, buf);
    f.c.text_fit((header.x, header.y + (header.h - size * 1.2) / 2.0, sw.x - header.x - pad), size, rgb(color), text, Keep::Start);

    let row_h = body.h * ROW_H;
    let hint_y = list.y + list.h - size * 1.2;
    let rows = ((hint_y - list.y) / row_h).floor().max(0.0) as usize;
    if bt.devices.iter().any(|d| d.paired) {
        f.c.text(list.x, hint_y, size * 0.8, rgb(DIM), "Long press a paired device to forget it");
    }
    if bt.powered && bt.devices.is_empty() {
        f.c.text(list.x, list.y + pad, size, rgb(DIM), "Looking for devices");
    }
    for (i, d) in bt.devices.iter().enumerate().take(rows) {
        let r = Rect::new(list.x - pad * 0.5, list.y + i as f32 * row_h, list.w + pad, row_h);
        let look = Look { radius: 8.0, fill: None, inset: 1.0, ring_inset: 0.0 };
        let busy = bt.busy.as_deref() == Some(d.address.as_str());
        f.button(Id::Device(i as u16), r, look, |c| {
            let side = r.h * ICON;
            let color = if d.connected { TEXT_HI } else { TEXT };
            c.icon(icon(d.kind), Rect::new(r.x + pad * 0.5, r.y + (r.h - side) / 2.0, side, side), rgb(color));
            let name_x = r.x + pad * 0.5 + side * 1.6;
            let state_x = r.x + r.w - r.w * STATE_W;
            let ty = r.y + (r.h - size * 1.2) / 2.0;
            c.text_fit((name_x, ty, state_x - name_x - pad), size, rgb(color), &d.name, Keep::Start);
            let (state, state_color) = match (d.connected, busy, d.paired) {
                (true, _, _) => ("Connected", GO),
                (_, true, _) => ("Connecting", ACCENT),
                (_, _, true) => ("Paired", DIM),
                _ => ("", DIM),
            };
            c.text(state_x + side, ty, size * 0.85, rgb(state_color), state);
        });
    }
}

/// The header's text: an error first, then connecting, connected, off, or no adapter.
fn status_line<'a>(s: &State, buf: &'a mut String) -> (u32, &'a str) {
    buf.clear();
    let bt = &s.bluetooth;
    let color = if let Some(e) = &bt.error {
        buf.push_str(e);
        ACCENT
    } else if !bt.adapter {
        buf.push_str("No Bluetooth adapter");
        DIM
    } else if !bt.powered {
        buf.push_str("Bluetooth is off");
        DIM
    } else if let Some(address) = &bt.busy {
        let name = bt.devices.iter().find(|d| d.address == *address).map_or(address.as_str(), |d| d.name.as_str());
        let _ = write!(buf, "Connecting {name}");
        TEXT
    } else {
        match bt.connected() {
            0 => buf.push_str("Nothing connected"),
            1 => {
                let one = bt.devices.iter().find(|d| d.connected).map_or("", |d| d.name.as_str());
                let _ = write!(buf, "Connected to {one}");
            }
            n => {
                let _ = write!(buf, "{n} devices connected");
            }
        }
        if bt.connected() == 0 { DIM } else { TEXT }
    };
    (color, buf)
}
