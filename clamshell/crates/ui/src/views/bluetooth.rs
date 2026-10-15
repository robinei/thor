//! The Bluetooth tab: what Bluetooth is doing (connected, connecting, what went wrong) with the radio's
//! switch, then the devices, one row each: its kind's icon, its name, and whether it is connected, being
//! connected or paired. A tap connects (pairing a new device first) or disconnects, a long press forgets a
//! paired device. While open, new devices are looked for.

use std::fmt::Write;

use clamshell_core::bluetooth::Kind;
use clamshell_core::{Id, State};

use super::radio::{self, ICON, STATE_W};
use super::{ACCENT, DIM, Frame, GO, Look, TEXT, TEXT_HI};
use crate::draw::{Keep, Rect, rgb};
use crate::icons::Icon;

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
    let status = status_line(s, buf);
    let list = radio::header(f, body, Id::BluetoothPower, bt.powered, status);
    let (pad, size) = (list.pad, list.size);
    if bt.devices.iter().any(|d| d.paired) {
        list.hint(f, "Long press a paired device to forget it");
    }
    if bt.powered && bt.devices.is_empty() {
        list.empty(f, "Looking for devices");
    }
    for (i, d) in bt.devices.iter().enumerate().take(list.rows) {
        let r = list.row(i);
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
