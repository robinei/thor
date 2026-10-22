//! Bluetooth as the Bluetooth tab shows it (BlueZ's view, read by the system side), and its rules: a tap
//! connects a paired device or pairs a new one (pair, trust, connect), a long press forgets a paired one.
//! What a device's row and the tab's header say is decided here ([`Bluetooth::device_state`],
//! [`Bluetooth::status`]); the UI words it.

use crate::State;
use crate::effect::Effect;
use crate::id::BtId;
use crate::kit::list::ItemKey;
use crate::kit::list::Rows;
use crate::radios::RadioStatus;

/// What kind of device it is (from BlueZ's icon name), for its icon.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Kind {
    Audio,
    Gamepad,
    Keyboard,
    Mouse,
    Phone,
    Computer,
    #[default]
    Other,
}

impl Kind {
    /// From BlueZ's `Icon` property (`audio-headset`, `input-gaming`, ...).
    pub fn from_icon(icon: &str) -> Kind {
        match icon {
            i if i.starts_with("audio") => Kind::Audio,
            "input-gaming" => Kind::Gamepad,
            "input-keyboard" => Kind::Keyboard,
            "input-mouse" | "input-tablet" => Kind::Mouse,
            "phone" => Kind::Phone,
            "computer" => Kind::Computer,
            _ => Kind::Other,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Device {
    /// BlueZ's address (`AA:BB:CC:DD:EE:FF`): what it is asked about.
    pub address: String,
    pub name: String,
    pub kind: Kind,
    pub paired: bool,
    pub connected: bool,
}

/// What was being done when something went wrong.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    Connect,
    Disconnect,
    Forget,
    /// Switching the radio.
    Power,
    /// Looking for devices.
    Watch,
}

/// Why it went wrong.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Reason {
    /// BlueZ isn't there.
    Unavailable,
    /// BlueZ's answer (its error's message or name).
    Bus(String),
}

/// What went wrong, until the next attempt.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Problem {
    pub action: Action,
    pub reason: Reason,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Bluetooth {
    /// There is an adapter (and BlueZ runs).
    pub present: bool,
    /// The radio is on.
    pub on: bool,
    /// Connected first, then paired, then the rest by name.
    pub devices: Vec<Device>,
    /// The device being connected or paired, by address.
    pub busy: Option<String>,
    /// Why the last attempt failed, until the next one.
    pub error: Option<Problem>,
}

/// What a device's row says about it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DeviceState {
    Connected,
    Connecting,
    Paired,
    Other,
}

impl Bluetooth {
    /// How many devices are connected (for the bar).
    pub fn connected(&self) -> usize {
        self.devices.iter().filter(|d| d.connected).count()
    }

    pub fn device_state(&self, d: &Device) -> DeviceState {
        if d.connected {
            DeviceState::Connected
        } else if self.busy.as_deref() == Some(d.address.as_str()) {
            DeviceState::Connecting
        } else if d.paired {
            DeviceState::Paired
        } else {
            DeviceState::Other
        }
    }

    /// What Bluetooth is doing, for the tab's header: an error first, then no adapter, off, connecting
    /// (the device's name, or its address), connected (to one, or how many), or nothing.
    pub fn status(&self) -> RadioStatus<'_, Problem> {
        if let Some(e) = &self.error {
            return RadioStatus::Error(e);
        }
        if !self.present {
            return RadioStatus::NoDevice;
        }
        if !self.on {
            return RadioStatus::Off;
        }
        if let Some(address) = &self.busy {
            let name = self.devices.iter().find(|d| d.address == *address).map_or(address.as_str(), |d| d.name.as_str());
            return RadioStatus::Busy(name);
        }
        match self.connected() {
            0 => RadioStatus::Idle,
            1 => RadioStatus::Connected(self.devices.iter().find(|d| d.connected).map_or("", |d| d.name.as_str())),
            n => RadioStatus::Many(n),
        }
    }
}

/// What the Bluetooth side is asked to do.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Command {
    Power(bool),
    /// Connect a paired device, or pair, trust and connect a new one.
    Connect(String),
    Disconnect(String),
    /// Unpair (BlueZ removes the device).
    Forget(String),
}

impl Command {
    /// What it does, for saying what failed.
    pub fn action(&self) -> Action {
        match self {
            Command::Power(_) => Action::Power,
            Command::Connect(_) => Action::Connect,
            Command::Disconnect(_) => Action::Disconnect,
            Command::Forget(_) => Action::Forget,
        }
    }
}

/// Connected first, then paired, then by name (unnamed ones last).
pub fn sort(mut devices: Vec<Device>) -> Vec<Device> {
    devices.sort_by(|a, b| {
        b.connected
            .cmp(&a.connected)
            .then(b.paired.cmp(&a.paired))
            .then(a.name.is_empty().cmp(&b.name.is_empty()))
            .then(a.name.to_lowercase().cmp(&b.name.to_lowercase()))
    });
    devices
}

/// The devices as a list's rows (found again by their address).
impl Rows for Bluetooth {
    fn len(&self) -> usize {
        self.devices.len()
    }

    fn key(&self, i: usize) -> Option<ItemKey> {
        self.devices.get(i).map(|d| ItemKey::Name(d.address.clone()))
    }

    fn find(&self, key: &ItemKey) -> Option<usize> {
        self.devices.iter().position(|d| matches!(key, ItemKey::Name(k) if *k == d.address))
    }
}

impl State {
    /// A tap on the Bluetooth tab's switch.
    pub(crate) fn on_bluetooth(&mut self, id: BtId) -> Vec<Effect> {
        match id {
            BtId::Power => vec![Effect::Bluetooth(Command::Power(!self.radios.bluetooth.on))],
        }
    }

    /// A tap on the device at `index` (connect, or disconnect a connected one), or a long press (forget
    /// a paired one).
    pub(crate) fn tap_device(&mut self, index: usize, long: bool) -> Vec<Effect> {
        let bt = &mut self.radios.bluetooth;
        let Some(d) = bt.devices.get(index) else {
            return vec![];
        };
        let address = d.address.clone();
        let command = match (long, d.paired, d.connected) {
            (true, true, _) => Command::Forget(address),
            (true, false, _) => return vec![],
            (false, _, true) => Command::Disconnect(address),
            (false, _, false) => {
                bt.busy = Some(address.clone());
                bt.error = None;
                Command::Connect(address)
            }
        };
        vec![Effect::Bluetooth(command)]
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testkit::*;
    use crate::*;

    fn dev(name: &str, paired: bool, connected: bool) -> Device {
        Device { address: format!("00:{name}"), name: name.into(), kind: Kind::Other, paired, connected }
    }

    #[test]
    fn connected_and_paired_first() {
        let names: Vec<String> =
            sort(vec![dev("zz", false, false), dev("", false, false), dev("pad", true, false), dev("buds", true, true)]).into_iter().map(|d| d.name).collect();
        assert_eq!(names, ["buds", "pad", "zz", ""]);
        assert_eq!(Kind::from_icon("audio-headphones"), Kind::Audio);
        assert_eq!(Kind::from_icon("input-gaming"), Kind::Gamepad);
    }

    #[test]
    fn taps_connect_or_disconnect_and_a_long_press_forgets() {
        let mut s = state();
        tap(&mut s, Id::Tab(Tab::Bluetooth));
        let devices = vec![dev("buds", true, true), dev("pad", true, false), dev("new", false, false)];
        read(&mut s, Reading::Bluetooth(Bluetooth { present: true, on: true, devices, ..Bluetooth::default() }));
        assert_eq!(s.radios().bluetooth.status(), RadioStatus::Connected("buds"));
        let bt = |c: Command| vec![Effect::Bluetooth(c)];
        let device = |i| ListId::Bluetooth.row(i);
        assert_eq!(tap(&mut s, device(0)), bt(Command::Disconnect("00:buds".into())));
        assert_eq!(tap(&mut s, device(1)), bt(Command::Connect("00:pad".into())));
        assert_eq!(s.radios().bluetooth.busy.as_deref(), Some("00:pad"));
        assert_eq!(s.radios().bluetooth.status(), RadioStatus::Busy("pad"));
        assert_eq!(tap(&mut s, device(2)), bt(Command::Connect("00:new".into())), "pairs it");
        assert_eq!(long(&mut s, device(1)), bt(Command::Forget("00:pad".into())));
        assert_eq!(long(&mut s, device(2)), vec![], "nothing to forget");
        assert_eq!(tap(&mut s, Id::Bluetooth(BtId::Power)), bt(Command::Power(false)));
    }
}
