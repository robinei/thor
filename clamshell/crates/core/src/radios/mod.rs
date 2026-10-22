//! The radios as their tabs show them. Wi-Fi and Bluetooth are each kept as their system thread last
//! read them, along with what the tab's header says about each. Their rules are in their modules.

pub mod bluetooth;
pub mod wifi;

use bluetooth::Bluetooth;
use wifi::Wifi;

use crate::kit::replace;

#[derive(Debug, Clone, Default)]
pub struct Radios {
    wifi: Wifi,
    bluetooth: Bluetooth,
}

impl Radios {
    pub fn wifi(&self) -> &Wifi {
        &self.wifi
    }

    pub fn bluetooth(&self) -> &Bluetooth {
        &self.bluetooth
    }

    /// A reading of Wi-Fi. Returns whether it changed.
    pub(crate) fn read_wifi(&mut self, wifi: Wifi) -> bool {
        replace(&mut self.wifi, wifi)
    }

    /// A reading of Bluetooth. Returns whether it changed.
    pub(crate) fn read_bluetooth(&mut self, bluetooth: Bluetooth) -> bool {
        replace(&mut self.bluetooth, bluetooth)
    }
}

/// What a radio is doing, for its tab's header. `P` is its kind of problem.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RadioStatus<'a, P> {
    /// The last attempt failed, until the next one, or the radio can't be used.
    Error(&'a P),
    NoDevice,
    Off,
    /// Joining a network or connecting a device, with its name.
    Busy(&'a str),
    /// Connected to one network or device, with its name.
    Connected(&'a str),
    /// Connected to this many devices.
    Many(usize),
    /// On, but connected to nothing.
    Idle,
}
