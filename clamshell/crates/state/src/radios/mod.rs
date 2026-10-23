//! The radios as their tabs show them. Wi-Fi and Bluetooth are each kept as their system thread last
//! read them, along with what the tab's header says about each. Their rules are in their modules.
//!
//! A reading replaces the radio whole, except for what the tab set itself and the reading doesn't answer
//! yet (see [`Local`]): the busy mark of an attempt, which a reading sent before the command doesn't
//! know of, and an error found here, such as a network that needs a login.

pub mod bluetooth;
pub mod wifi;

use bluetooth::Bluetooth;
use wifi::Wifi;

#[derive(Debug, Clone, Default)]
pub struct Radios {
    wifi: Wifi,
    bluetooth: Bluetooth,
    wifi_local: Local<wifi::Problem>,
    bluetooth_local: Local<bluetooth::Problem>,
}

/// What a radio's tab set itself, kept over readings until one answers it. An attempt's busy mark is
/// answered by a reading that shows the attempt (busy with it), its result (connected), or an error. As
/// a reading sent before the command still carries the error from before the attempt, that error
/// doesn't count. An error found here is answered by the system's own error or by a connection.
#[derive(Debug, Clone)]
struct Local<P> {
    /// An attempt's busy mark is the radio's `busy`, and no reading has answered it.
    attempt: bool,
    /// The error shown when the attempt began.
    before: Option<P>,
    /// The radio's `error` was found here, and no reading has answered it.
    error: bool,
}

impl<P> Default for Local<P> {
    fn default() -> Local<P> {
        Local { attempt: false, before: None, error: false }
    }
}

impl<P> Local<P> {
    /// The attempt was answered.
    fn answered(&mut self) {
        (self.attempt, self.before) = (false, None);
    }

    /// An attempt begins, with `error` shown until now.
    fn attempt(&mut self, error: Option<P>) {
        *self = Local { attempt: true, before: error, error: false };
    }
}

impl Radios {
    pub fn wifi(&self) -> &Wifi {
        &self.wifi
    }

    pub fn bluetooth(&self) -> &Bluetooth {
        &self.bluetooth
    }

    /// A reading of Wi-Fi. What the tab set itself stays until the reading answers it (see [`Local`]).
    pub(crate) fn read_wifi(&mut self, mut wifi: Wifi) {
        let local = &mut self.wifi_local;
        let connected = |ssid: &str| wifi.networks.iter().any(|n| n.connected && n.ssid == ssid);
        if local.attempt {
            let ssid = self.wifi.busy.as_deref();
            let new_error = wifi.error.is_some() && wifi.error != local.before;
            if wifi.busy.as_deref() == ssid || ssid.is_some_and(connected) || new_error {
                local.answered();
            } else {
                wifi.busy = self.wifi.busy.take();
                wifi.error = None;
            }
        }
        if local.error {
            let login = match &self.wifi.error {
                Some(wifi::Problem::NeedsLogin { ssid }) => connected(ssid),
                _ => false,
            };
            if wifi.error.is_some() || login {
                local.error = false;
            } else {
                wifi.error = self.wifi.error.take();
            }
        }
        self.wifi = wifi;
    }

    /// A reading of Bluetooth. An attempt's busy mark stays until the reading answers it (see [`Local`]).
    pub(crate) fn read_bluetooth(&mut self, mut bluetooth: Bluetooth) {
        let local = &mut self.bluetooth_local;
        if local.attempt {
            let address = self.bluetooth.busy.as_deref();
            let connected = |a: &str| bluetooth.devices.iter().any(|d| d.connected && d.address == a);
            let new_error = bluetooth.error.is_some() && bluetooth.error != local.before;
            if bluetooth.busy.as_deref() == address || address.is_some_and(connected) || new_error {
                local.answered();
            } else {
                bluetooth.busy = self.bluetooth.busy.take();
                bluetooth.error = None;
            }
        }
        self.bluetooth = bluetooth;
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
