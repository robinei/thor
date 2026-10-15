//! The Wi-Fi tab: what Wi-Fi is doing (connected, joining, what went wrong) with the radio's switch,
//! then the networks in range, one row each: signal, name, a lock if it needs a password, and whether it
//! is connected, being joined or saved. A tap joins (asking for the password first when needed), a long
//! press forgets a saved network. The password is typed in the shared editor.

use std::fmt::Write;

use clamshell_core::editor::{Editor, Purpose};
use clamshell_core::wifi::{NetState, Problem, Wifi, signal_bars};
use clamshell_core::{Id, RadioStatus, WifiId};

use super::radio::{self, ICON, STATE_W};
use super::{ACCENT, DIM, Frame, GO, Look, TEXT, TEXT_HI};
use crate::draw::{Keep, Rect, rgb};
use crate::icons::Icon;

pub(super) fn draw(f: &mut Frame, body: Rect, buf: &mut String) {
    if let Some(Editor { purpose: Purpose::Password { ssid }, .. }) = f.s.bottom().editor() {
        buf.clear();
        let _ = write!(buf, "Password for {ssid}");
        return super::editor::draw(f, body, buf, "Join");
    }
    let wifi = &f.s.radios().wifi;
    let status = status_line(wifi, buf);
    let list = radio::header(f, body, Id::Wifi(WifiId::Power), wifi.enabled, status);
    let (pad, size) = (list.pad, list.size);
    // The networks, as many as fit above the hint.
    if wifi.networks.iter().any(|n| n.known) {
        list.hint(f, "Long press a saved network to forget it");
    }
    if wifi.enabled && wifi.networks.is_empty() {
        list.empty(f, "Looking for networks");
    }
    for (i, n) in wifi.networks.iter().enumerate().take(list.rows) {
        let r = list.row(i);
        let look = Look { radius: 8.0, fill: None, inset: 1.0, ring_inset: 0.0 };
        let net_state = wifi.net_state(n);
        f.button(Id::Wifi(WifiId::Network(i as u16)), r, look, |c| {
            let icon = r.h * ICON;
            let iy = r.y + (r.h - icon) / 2.0;
            let bars = Some(signal_bars(n.strength));
            c.icon(Icon::Wifi { bars }, Rect::new(r.x + pad * 0.5, iy, icon, icon), rgb(if n.active { TEXT_HI } else { TEXT }));
            let name_x = r.x + pad * 0.5 + icon * 1.6;
            let state_x = r.x + r.w - r.w * STATE_W;
            let ty = r.y + (r.h - size * 1.2) / 2.0;
            c.text_fit((name_x, ty, state_x - name_x - pad), size, rgb(if n.active { TEXT_HI } else { TEXT }), &n.ssid, Keep::Start);
            if n.secure {
                c.icon(Icon::Lock, Rect::new(state_x, r.y + (r.h - icon * 0.7) / 2.0, icon * 0.7, icon * 0.7), rgb(DIM));
            }
            let (state, color) = match net_state {
                NetState::Connected => ("Connected", GO),
                NetState::Joining => ("Joining", ACCENT),
                NetState::Saved => ("Saved", DIM),
                NetState::Login => ("Login", DIM),
                NetState::Other => ("", DIM),
            };
            c.text(state_x + icon, ty, size * 0.85, rgb(color), state);
        });
    }
}

/// The header's text: an error first, then no device, off, joining, connected, or not.
fn status_line<'a>(wifi: &Wifi, buf: &'a mut String) -> (u32, &'a str) {
    buf.clear();
    let color = match wifi.status() {
        RadioStatus::Error(problem) => {
            problem_text(problem, buf);
            ACCENT
        }
        RadioStatus::NoDevice => {
            buf.push_str("No Wi-Fi device");
            DIM
        }
        RadioStatus::Off => {
            buf.push_str("Wi-Fi is off");
            DIM
        }
        RadioStatus::Busy(ssid) => {
            let _ = write!(buf, "Joining {ssid}");
            TEXT
        }
        RadioStatus::Connected(ssid) => {
            let _ = write!(buf, "Connected to {ssid}");
            TEXT
        }
        RadioStatus::Many(_) | RadioStatus::Idle => {
            buf.push_str("Not connected");
            DIM
        }
    };
    (color, buf)
}

/// What went wrong, in words.
fn problem_text(problem: &Problem, buf: &mut String) {
    let _ = match problem {
        Problem::WrongPassword { ssid } => write!(buf, "Couldn't join {ssid}: wrong password?"),
        Problem::NoAddress { ssid } => write!(buf, "Joined {ssid}, but it gave no address"),
        Problem::NoAnswer { ssid } => write!(buf, "Couldn't join {ssid}: no answer"),
        Problem::Refused { ssid, reason } => write!(buf, "Couldn't join {ssid} (NetworkManager reason {reason})"),
        Problem::NeedsLogin { ssid } => write!(buf, "{ssid} needs a login, which isn't supported here"),
        Problem::Unavailable => write!(buf, "NetworkManager isn't available"),
        Problem::Bus(e) => write!(buf, "NetworkManager: {e}"),
    };
}
