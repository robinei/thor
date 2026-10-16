//! The Bluetooth tab: what Bluetooth is doing (connected, connecting, what went wrong) with the radio's
//! switch, then the devices, one row each: its kind's icon, its name, and whether it is connected, being
//! connected or paired. A tap connects (pairing a new device first) or disconnects, a long press forgets a
//! paired device. While open, new devices are looked for.

use std::fmt::{self, Display, Formatter};

use clamshell_core::bluetooth::{Action, Bluetooth, DeviceState, Problem, Reason};
use clamshell_core::{BtId, Id, ListId, RadioStatus};

use super::format::Fmt;
use super::style::Tone;
use super::widgets::{self, RowFace};
use super::{Frame, radio};
use crate::icons::Icon;

pub(super) fn draw(f: &mut Frame) {
    let bt = &f.s.radios().bluetooth;
    let status = Fmt::<256>::of(format_args!("{}", Status(bt)));
    let list = radio::header(f, Id::Bluetooth(BtId::Power), bt.powered, (status_tone(bt), &status));
    if bt.devices.iter().any(|d| d.paired) {
        list.hint(f, "Long press a paired device to forget it");
    }
    if bt.powered && bt.devices.is_empty() {
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
        let face = RowFace { icon: Icon::for_device(d.kind), name: &d.name, tone: Tone::high_if(d.connected), lock: false, state, state_tone };
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

/// The header's text: an error first, then no adapter, off, connecting, connected, or nothing.
struct Status<'a>(&'a Bluetooth);

impl Display for Status<'_> {
    fn fmt(&self, f: &mut Formatter) -> fmt::Result {
        match self.0.status() {
            RadioStatus::Error(problem) => write!(f, "{}", ProblemText(problem)),
            RadioStatus::NoDevice => f.write_str("No Bluetooth adapter"),
            RadioStatus::Off => f.write_str("Bluetooth is off"),
            RadioStatus::Busy(name) => write!(f, "Connecting {name}"),
            RadioStatus::Connected(name) => write!(f, "Connected to {name}"),
            RadioStatus::Many(n) => write!(f, "{n} devices connected"),
            RadioStatus::Idle => f.write_str("Nothing connected"),
        }
    }
}

/// What went wrong, in words: what was being done, and why it failed.
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
