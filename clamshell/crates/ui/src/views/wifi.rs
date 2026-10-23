//! The Wi-Fi tab. The header shows what Wi-Fi is doing (connected, joining, or what went wrong) next to
//! the radio's switch. Below it is one row per network in range, with its signal, its name, a lock if it
//! needs a password, and whether it is connected, joining or saved. A tap joins, asking for the password
//! first when needed. A long press forgets a saved network. The password is typed in the shared editor.

use std::fmt::{self, Display, Formatter};

use clamshell_state::bottom::editor::{Editor, Purpose};
use clamshell_state::radios::wifi::{NetState, Problem, Wifi};
use clamshell_state::{Id, ListId, RadioStatus, WifiId};

use super::radio;
use crate::gfx::icons::Icon;
use crate::toolkit::Frame;
use crate::toolkit::format::Fmt;
use crate::toolkit::style::Tone;
use crate::toolkit::widgets::{self, RowFace};

pub(super) fn draw(f: &mut Frame) {
    if let Some(Editor { purpose: Purpose::Password { ssid }, .. }) = f.s.bottom().editor() {
        let title = Fmt::<128>::of(format_args!("Password for {ssid}"));
        return super::editor::draw(f, &title, "Join");
    }
    let wifi = f.s.radios().wifi();
    let status = Fmt::<256>::of(format_args!("{}", Status(wifi)));
    let list = radio::header(f, Id::Wifi(WifiId::Power), wifi.on, (status_tone(wifi), &status));
    // The networks, as many as fit above the hint.
    if wifi.networks.iter().any(|n| n.saved) {
        list.hint(f, "Long press a saved network to forget it");
    }
    if wifi.on && wifi.networks.is_empty() {
        list.empty(f, "Looking for networks");
    }
    let rows = list.scrolled(f, ListId::Wifi, wifi.networks.len());
    for i in rows.shown() {
        let Some(n) = wifi.networks.get(i) else {
            continue;
        };
        let (state, state_tone) = match wifi.net_state(n) {
            NetState::Connected => ("Connected", Tone::Go),
            NetState::Joining => ("Joining", Tone::Accent),
            NetState::Saved => ("Saved", Tone::Dim),
            NetState::Login => ("Login", Tone::Dim),
            NetState::Other => ("", Tone::Dim),
        };
        let face = RowFace { icon: Icon::wifi(Some(n.strength)), name: &n.ssid, tone: Tone::high_if(n.connected), lock: n.secure, state, state_tone };
        widgets::status_row(f, ListId::Wifi.row(i as u16), rows.row(i), list.pad, list.size, face);
    }
    list.end(f, rows);
}

/// The header's tone: an error stands out; off, missing or idle is dim.
fn status_tone(wifi: &Wifi) -> Tone {
    match wifi.status() {
        RadioStatus::Error(_) => Tone::Accent,
        RadioStatus::Busy(_) | RadioStatus::Connected(_) => Tone::Normal,
        RadioStatus::NoDevice | RadioStatus::Off | RadioStatus::Many(_) | RadioStatus::Idle => Tone::Dim,
    }
}

/// The header's text: an error, no device, off, joining, connected, or not connected.
struct Status<'a>(&'a Wifi);

impl Display for Status<'_> {
    fn fmt(&self, f: &mut Formatter) -> fmt::Result {
        match self.0.status() {
            RadioStatus::Error(problem) => write!(f, "{}", ProblemText(problem)),
            RadioStatus::NoDevice => f.write_str("No Wi-Fi device"),
            RadioStatus::Off => f.write_str("Wi-Fi is off"),
            RadioStatus::Busy(ssid) => write!(f, "Joining {ssid}"),
            RadioStatus::Connected(ssid) => write!(f, "Connected to {ssid}"),
            RadioStatus::Many(_) | RadioStatus::Idle => f.write_str("Not connected"),
        }
    }
}

/// What went wrong, in words.
struct ProblemText<'a>(&'a Problem);

impl Display for ProblemText<'_> {
    fn fmt(&self, f: &mut Formatter) -> fmt::Result {
        match self.0 {
            Problem::WrongPassword { ssid } => write!(f, "Couldn't join {ssid}: wrong password?"),
            Problem::NoAddress { ssid } => write!(f, "Joined {ssid}, but it gave no address"),
            Problem::NoAnswer { ssid } => write!(f, "Couldn't join {ssid}: no answer"),
            Problem::Refused { ssid, reason } => write!(f, "Couldn't join {ssid} (NetworkManager reason {reason})"),
            Problem::NeedsLogin { ssid } => write!(f, "{ssid} needs a login, which isn't supported here"),
            Problem::Unavailable => f.write_str("NetworkManager isn't available"),
            Problem::Bus(e) => write!(f, "NetworkManager: {e}"),
        }
    }
}
