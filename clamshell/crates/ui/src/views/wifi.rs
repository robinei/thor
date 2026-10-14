//! The Wi-Fi mode: what Wi-Fi is doing (connected, joining, what went wrong) with the radio's switch,
//! then the networks in range, one row each: signal, name, a lock if it needs a password, and whether it
//! is connected, being joined or saved. A tap joins (asking for the password first when needed), a long
//! press forgets a saved network. The password is typed in the shared editor.

use std::fmt::Write;

use clamshell_core::editor::{Editor, Purpose};
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
/// Icons, of a row's height; the right column (lock and state), of the area's width.
const ICON: f32 = 0.42;
const STATE_W: f32 = 0.22;

pub(super) fn draw(f: &mut Frame, body: Rect, buf: &mut String) {
    if let Some(Editor { purpose: Purpose::Password { ssid }, .. }) = &f.s.editor {
        buf.clear();
        let _ = write!(buf, "Password for {ssid}");
        return super::editor::draw(f, body, buf, "Join");
    }
    let s = f.s;
    let pad = body.h * PAD;
    let area = Rect::new(body.x + pad, body.y, body.w - 2.0 * pad, body.h - pad);
    let (header, list) = area.split_top(body.h * HEADER_H);
    let size = body.h * TEXT_SIZE;

    // The header: the radio's switch on the right, what is going on to its left.
    let sw = Rect::new(header.x + header.w - header.h * 1.6, header.y, header.h * 1.6, header.h);
    let on = s.wifi.enabled;
    let look = Look { radius: 8.0, fill: None, inset: 2.0, ring_inset: 0.0 };
    f.button(Id::WifiPower, sw, look, |c| {
        let (h, w) = (sw.h * SWITCH_H, sw.h * SWITCH_H / SWITCH_H * SWITCH_W);
        let track = Rect::new(sw.x + (sw.w - w) / 2.0, sw.y + (sw.h - h) / 2.0, w, h);
        c.rounded(track, h / 2.0, rgb(if on { GO } else { SURFACE_HI }));
        let knob_x = if on { track.x + track.w - h / 2.0 } else { track.x + h / 2.0 };
        c.circle(knob_x, track.y + h / 2.0, h * 0.38, rgb(if on { TEXT_HI } else { DIM }));
    });
    let (color, text) = status_line(s, buf);
    f.c.text_fit((header.x, header.y + (header.h - size * 1.2) / 2.0, sw.x - header.x - pad), size, rgb(color), text, Keep::Start);

    // The networks, as many as fit above the hint.
    let row_h = body.h * ROW_H;
    let hint_y = list.y + list.h - size * 1.2;
    let rows = ((hint_y - list.y) / row_h).floor().max(0.0) as usize;
    if s.wifi.networks.iter().any(|n| n.known) {
        f.c.text(list.x, hint_y, size * 0.8, rgb(DIM), "Long press a saved network to forget it");
    }
    if s.wifi.enabled && s.wifi.networks.is_empty() {
        f.c.text(list.x, list.y + pad, size, rgb(DIM), "Looking for networks");
    }
    for (i, n) in s.wifi.networks.iter().enumerate().take(rows) {
        let r = Rect::new(list.x - pad * 0.5, list.y + i as f32 * row_h, list.w + pad, row_h);
        let look = Look { radius: 8.0, fill: None, inset: 1.0, ring_inset: 0.0 };
        let joining = s.wifi.connecting.as_deref() == Some(n.ssid.as_str());
        f.button(Id::Network(i as u16), r, look, |c| {
            let icon = r.h * ICON;
            let iy = r.y + (r.h - icon) / 2.0;
            let bars = Some(match n.strength {
                0..=29 => 0,
                30..=54 => 1,
                55..=74 => 2,
                _ => 3,
            });
            c.icon(Icon::Wifi { bars }, Rect::new(r.x + pad * 0.5, iy, icon, icon), rgb(if n.active { TEXT_HI } else { TEXT }));
            let name_x = r.x + pad * 0.5 + icon * 1.6;
            let state_x = r.x + r.w - r.w * STATE_W;
            let ty = r.y + (r.h - size * 1.2) / 2.0;
            c.text_fit((name_x, ty, state_x - name_x - pad), size, rgb(if n.active { TEXT_HI } else { TEXT }), &n.ssid, Keep::Start);
            if n.secure {
                c.icon(Icon::Lock, Rect::new(state_x, r.y + (r.h - icon * 0.7) / 2.0, icon * 0.7, icon * 0.7), rgb(DIM));
            }
            let (state, color) = match (n.active, joining, n.known, n.enterprise) {
                (true, _, _, _) => ("Connected", GO),
                (_, true, _, _) => ("Joining", ACCENT),
                (_, _, true, _) => ("Saved", DIM),
                (_, _, _, true) => ("Login", DIM),
                _ => ("", DIM),
            };
            c.text(state_x + icon, ty, size * 0.85, rgb(color), state);
        });
    }
}

/// The header's text: an error first, then joining, connected, off, or no device.
fn status_line<'a>(s: &State, buf: &'a mut String) -> (u32, &'a str) {
    buf.clear();
    let w = &s.wifi;
    let color = if let Some(e) = &w.error {
        buf.push_str(e);
        ACCENT
    } else if !w.device {
        buf.push_str("No Wi-Fi device");
        DIM
    } else if !w.enabled {
        buf.push_str("Wi-Fi is off");
        DIM
    } else if let Some(ssid) = &w.connecting {
        let _ = write!(buf, "Joining {ssid}");
        TEXT
    } else if let Some(n) = w.networks.iter().find(|n| n.active) {
        let _ = write!(buf, "Connected to {}", n.ssid);
        TEXT
    } else {
        buf.push_str("Not connected");
        DIM
    };
    (color, buf)
}
