//! The Bluetooth tab: what Bluetooth is doing (connected, connecting, what went wrong) with the radio's
//! switch, then the devices, one row each: its kind's icon, its name, and whether it is connected, being
//! connected or paired. A tap connects (pairing a new device first) or disconnects, a long press forgets a
//! paired device. While open, new devices are looked for.

use std::fmt::Write;

use clamshell_core::bluetooth::{Action, Bluetooth, DeviceState, Kind, Problem, Reason};
use clamshell_core::{BtId, Id, RadioStatus};

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
    let bt = &f.s.radios().bluetooth;
    let status = status_line(bt, buf);
    let list = radio::header(f, body, Id::Bluetooth(BtId::Power), bt.powered, status);
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
        let device_state = bt.device_state(d);
        f.button(Id::Bluetooth(BtId::Device(i as u16)), r, look, |c| {
            let side = r.h * ICON;
            let color = if d.connected { TEXT_HI } else { TEXT };
            c.icon(icon(d.kind), Rect::new(r.x + pad * 0.5, r.y + (r.h - side) / 2.0, side, side), rgb(color));
            let name_x = r.x + pad * 0.5 + side * 1.6;
            let state_x = r.x + r.w - r.w * STATE_W;
            let ty = r.y + (r.h - size * 1.2) / 2.0;
            c.text_fit((name_x, ty, state_x - name_x - pad), size, rgb(color), &d.name, Keep::Start);
            let (state, state_color) = match device_state {
                DeviceState::Connected => ("Connected", GO),
                DeviceState::Connecting => ("Connecting", ACCENT),
                DeviceState::Paired => ("Paired", DIM),
                DeviceState::Other => ("", DIM),
            };
            c.text(state_x + side, ty, size * 0.85, rgb(state_color), state);
        });
    }
}

/// The header's text: an error first, then no adapter, off, connecting, connected, or nothing.
fn status_line<'a>(bt: &Bluetooth, buf: &'a mut String) -> (u32, &'a str) {
    buf.clear();
    let color = match bt.status() {
        RadioStatus::Error(problem) => {
            problem_text(problem, buf);
            ACCENT
        }
        RadioStatus::NoDevice => {
            buf.push_str("No Bluetooth adapter");
            DIM
        }
        RadioStatus::Off => {
            buf.push_str("Bluetooth is off");
            DIM
        }
        RadioStatus::Busy(name) => {
            let _ = write!(buf, "Connecting {name}");
            TEXT
        }
        RadioStatus::Connected(name) => {
            let _ = write!(buf, "Connected to {name}");
            TEXT
        }
        RadioStatus::Many(n) => {
            let _ = write!(buf, "{n} devices connected");
            TEXT
        }
        RadioStatus::Idle => {
            buf.push_str("Nothing connected");
            DIM
        }
    };
    (color, buf)
}

/// What went wrong, in words: what was being done, and why it failed.
fn problem_text(problem: &Problem, buf: &mut String) {
    buf.push_str(match problem.action {
        Action::Connect => "Couldn't connect",
        Action::Disconnect => "Couldn't disconnect",
        Action::Forget => "Couldn't forget it",
        Action::Power => "Couldn't switch Bluetooth",
        Action::Watch => "Couldn't look for devices",
    });
    buf.push_str(": ");
    buf.push_str(match &problem.reason {
        Reason::Unavailable => "Bluetooth isn't available",
        Reason::Bus(reason) => reason,
    });
}
