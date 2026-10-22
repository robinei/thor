//! The radios as their tabs show them: Wi-Fi and Bluetooth, each as its system thread last read it, and
//! what the tab's header says about either.

pub mod bluetooth;
pub mod wifi;

use bluetooth::Bluetooth;
use wifi::Wifi;

#[derive(Debug, Clone, Default)]
pub struct Radios {
    pub wifi: Wifi,
    pub bluetooth: Bluetooth,
}

/// What a radio is doing, for its tab's header. `P` is its kind of problem.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RadioStatus<'a, P> {
    /// The last attempt failed (until the next one), or the radio can't be used.
    Error(&'a P),
    NoDevice,
    Off,
    /// Joining a network, connecting a device: its name.
    Busy(&'a str),
    /// Connected to one network or device: its name.
    Connected(&'a str),
    /// Connected to this many devices.
    Many(usize),
    /// On, connected to nothing.
    Idle,
}
