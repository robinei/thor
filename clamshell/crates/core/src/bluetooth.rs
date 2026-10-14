//! Bluetooth as the Bluetooth mode shows it (BlueZ's view, read by the policy side), and its rules: a tap
//! connects a paired device or pairs a new one (pair, trust, connect), a long press forgets a paired one.

use crate::{Effect, State};

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

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Bluetooth {
    /// There is an adapter (and BlueZ runs).
    pub adapter: bool,
    pub powered: bool,
    /// Connected first, then paired, then the rest by name.
    pub devices: Vec<Device>,
    /// The device being connected or paired, by address.
    pub busy: Option<String>,
    /// Why the last attempt failed, until the next one.
    pub error: Option<String>,
}

impl Bluetooth {
    /// How many devices are connected (for the bar).
    pub fn connected(&self) -> usize {
        self.devices.iter().filter(|d| d.connected).count()
    }
}

/// What the Bluetooth side is asked to do.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Command {
    /// The mode opened (look for devices) or closed.
    Watch(bool),
    Power(bool),
    /// Connect a paired device, or pair, trust and connect a new one.
    Connect(String),
    Disconnect(String),
    /// Unpair (BlueZ removes the device).
    Forget(String),
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

impl State {
    /// A tap on the device at `index` (connect, or disconnect a connected one), or a long press (forget
    /// a paired one).
    pub(crate) fn tap_device(&mut self, index: usize, long: bool) -> Vec<Effect> {
        let Some(d) = self.bluetooth.devices.get(index) else {
            return vec![];
        };
        let address = d.address.clone();
        let command = match (long, d.paired, d.connected) {
            (true, true, _) => Command::Forget(address),
            (true, false, _) => return vec![],
            (false, _, true) => Command::Disconnect(address),
            (false, _, false) => {
                self.bluetooth.busy = Some(address.clone());
                self.bluetooth.error = None;
                Command::Connect(address)
            }
        };
        vec![Effect::Bluetooth(command)]
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tests::state;
    use crate::{Event, Id, Mode};

    fn dev(name: &str, paired: bool, connected: bool) -> Device {
        Device { address: format!("00:{name}"), name: name.into(), kind: Kind::Other, paired, connected }
    }

    #[test]
    fn connected_and_paired_first() {
        let names: Vec<String> = sort(vec![dev("zz", false, false), dev("", false, false), dev("pad", true, false), dev("buds", true, true)])
            .into_iter()
            .map(|d| d.name)
            .collect();
        assert_eq!(names, ["buds", "pad", "zz", ""]);
        assert_eq!(Kind::from_icon("audio-headphones"), Kind::Audio);
        assert_eq!(Kind::from_icon("input-gaming"), Kind::Gamepad);
    }

    #[test]
    fn taps_connect_or_disconnect_and_a_long_press_forgets() {
        let mut s = state();
        s.apply(Event::Activate(Id::Mode(Mode::Bluetooth)));
        let devices = vec![dev("buds", true, true), dev("pad", true, false), dev("new", false, false)];
        s.apply(Event::Bluetooth(Bluetooth { adapter: true, powered: true, devices, ..Bluetooth::default() }));
        let bt = |c: Command| vec![Effect::Bluetooth(c)];
        assert_eq!(s.apply(Event::Activate(Id::Device(0))).1, bt(Command::Disconnect("00:buds".into())));
        assert_eq!(s.apply(Event::Activate(Id::Device(1))).1, bt(Command::Connect("00:pad".into())));
        assert_eq!(s.bluetooth.busy.as_deref(), Some("00:pad"));
        assert_eq!(s.apply(Event::Activate(Id::Device(2))).1, bt(Command::Connect("00:new".into())), "pairs it");
        assert_eq!(s.apply(Event::LongPress(Id::Device(1))).1, bt(Command::Forget("00:pad".into())));
        assert_eq!(s.apply(Event::LongPress(Id::Device(2))).1, vec![], "nothing to forget");
        assert_eq!(s.apply(Event::Activate(Id::BluetoothPower)).1, bt(Command::Power(false)));
    }
}
