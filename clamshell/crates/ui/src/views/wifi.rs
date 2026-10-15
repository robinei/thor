//! The Wi-Fi tab: what Wi-Fi is doing (connected, joining, what went wrong) with the radio's switch,
//! then the networks in range, one row each: signal, name, a lock if it needs a password, and whether it
//! is connected, being joined or saved. A tap joins (asking for the password first when needed), a long
//! press forgets a saved network. The password is typed in the shared editor.

use std::fmt::Write;

use clamshell_core::editor::{Editor, Purpose};
use clamshell_core::{Id, State};

use super::radio::{self, ICON, STATE_W};
use super::{ACCENT, DIM, Frame, GO, Look, TEXT, TEXT_HI};
use crate::draw::{Keep, Rect, rgb};
use crate::icons::Icon;

pub(super) fn draw(f: &mut Frame, body: Rect, buf: &mut String) {
    if let Some(Editor { purpose: Purpose::Password { ssid }, .. }) = &f.s.editor {
        buf.clear();
        let _ = write!(buf, "Password for {ssid}");
        return super::editor::draw(f, body, buf, "Join");
    }
    let s = f.s;
    let status = status_line(s, buf);
    let list = radio::header(f, body, Id::WifiPower, s.wifi.enabled, status);
    let (pad, size) = (list.pad, list.size);
    // The networks, as many as fit above the hint.
    if s.wifi.networks.iter().any(|n| n.known) {
        list.hint(f, "Long press a saved network to forget it");
    }
    if s.wifi.enabled && s.wifi.networks.is_empty() {
        list.empty(f, "Looking for networks");
    }
    for (i, n) in s.wifi.networks.iter().enumerate().take(list.rows) {
        let r = list.row(i);
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
