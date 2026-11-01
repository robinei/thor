//! The Bluetooth tab. The header shows what Bluetooth is doing (connected, connecting, the code a
//! keyboard being paired wants typed on it, or what went wrong) next to the radio's switch. Below it is one row per device, with an icon for its kind, its name,
//! and whether it is connected, connecting or paired. A tap connects, pairing a new device first, or
//! disconnects. A long press forgets a paired device. While the tab is open, it scans for new devices.

use std::fmt::{self, Display, Formatter};

use clamshell_state::radios::bluetooth::{Action, Bluetooth, DeviceState, PairingCode, Problem, Reason};
use clamshell_state::{BtId, Id, ListId, RadioStatus};

use super::icons;
use super::radio;
use clamshell_ui::toolkit::Frame;
use clamshell_ui::toolkit::format::Fmt;
use clamshell_ui::toolkit::style::Tone;
use clamshell_ui::toolkit::widgets::{self, RowFace};

pub(super) fn draw(f: &mut Frame) {
    let bt = f.s.radios().bluetooth();
    let status = Fmt::<256>::of(format_args!("{}", Status(bt)));
    let mut list = radio::header(f, Id::Bluetooth(BtId::Power), bt.on, (status_tone(bt), &status));
    if bt.devices.iter().any(|d| d.paired) {
        list.hint(f, "Long press a paired device to forget it");
    }
    if bt.on && bt.devices.is_empty() {
        list.empty(f, "Looking for devices");
    }
    let rows = list.scrolled(f, ListId::Bluetooth, bt.devices.len());
    for i in rows.shown() {
        let Some(d) = bt.devices.get(i) else {
            continue;
        };
        let (state, state_tone) = match bt.device_state(d) {
            DeviceState::Connected => ("Connected", Tone::Go),
            DeviceState::Connecting => ("Connecting", Tone::Accent),
            DeviceState::Paired => ("Paired", Tone::Dim),
            DeviceState::Other => ("", Tone::Dim),
        };
        let face = RowFace { icon: icons::device(d.kind), name: &d.name, tone: Tone::high_if(d.connected), lock: false, state, state_tone };
        widgets::status_row(f, ListId::Bluetooth.row(i as u16), rows.row(i), list.pad, list.size, face);
    }
    list.end(f, rows);
}

/// The header's tone: an error stands out; off, missing or idle is dim.
fn status_tone(bt: &Bluetooth) -> Tone {
    match bt.status() {
        RadioStatus::Error(_) => Tone::Accent,
        RadioStatus::Busy(_) | RadioStatus::Connected(_) | RadioStatus::Many(_) => Tone::Normal,
        RadioStatus::NoDevice | RadioStatus::Off | RadioStatus::Idle => Tone::Dim,
    }
}

/// The header's text: an error, no adapter, off, connecting, connected (to one device or several), or
/// nothing connected.
struct Status<'a>(&'a Bluetooth);

impl Display for Status<'_> {
    fn fmt(&self, f: &mut Formatter) -> fmt::Result {
        match self.0.status() {
            RadioStatus::Error(problem) => write!(f, "{}", ProblemText(problem)),
            RadioStatus::NoDevice => f.write_str("No Bluetooth adapter"),
            RadioStatus::Off => f.write_str("Bluetooth is off"),
            RadioStatus::Busy(name) => match &self.0.code {
                // A keyboard pairing by a code typed on it.
                Some(PairingCode::Type(code)) => write!(f, "Type {code} on {name}, then Enter"),
                // A device with a screen of its own, pairing by numeric comparison.
                Some(PairingCode::Compare(code)) => write!(f, "{name} should show {code}"),
                None => write!(f, "Connecting {name}"),
            },
            RadioStatus::Connected(name) => write!(f, "Connected to {name}"),
            RadioStatus::Many(n) => write!(f, "{n} devices connected"),
            RadioStatus::Idle => f.write_str("Nothing connected"),
        }
    }
}

/// What went wrong, in words: what was being done and why it failed.
struct ProblemText<'a>(&'a Problem);

impl Display for ProblemText<'_> {
    fn fmt(&self, f: &mut Formatter) -> fmt::Result {
        let action = match self.0.action {
            Action::Connect => "Couldn't connect",
            Action::Disconnect => "Couldn't disconnect",
            Action::Forget => "Couldn't forget it",
            Action::Power => "Couldn't switch Bluetooth",
            Action::Watch => "Couldn't look for devices",
        };
        let reason = match &self.0.reason {
            Reason::Unavailable => "Bluetooth isn't available",
            Reason::Bus(reason) => reason,
        };
        write!(f, "{action}: {reason}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// While a device pairs, the status line says what to do with its code: a keyboard's to type, a code
    /// to compare with the one a device with a screen shows.
    #[test]
    fn a_pairing_code_is_said_by_its_kind() {
        let said = |code| Status(&Bluetooth { present: true, on: true, busy: Some("Phone".into()), code, ..Bluetooth::default() }).to_string();
        assert_eq!(said(Some(PairingCode::Type("042137".into()))), "Type 042137 on Phone, then Enter");
        assert_eq!(said(Some(PairingCode::Compare("042137".into()))), "Phone should show 042137");
        assert_eq!(said(None), "Connecting Phone");
    }
}
