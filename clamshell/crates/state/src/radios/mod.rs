//! The radios as their tabs show them. Wi-Fi and Bluetooth are each kept as their system thread last
//! read them, untouched, along with what the tab did itself and no reading has answered yet: an attempt to
//! join or connect ([`Attempt`]), and a network found here to need a login. What the tab shows is worked
//! out from these alone ([`Radios::show_wifi`]), whenever one changes. Their rules are in their modules.

pub mod bluetooth;
pub mod wifi;

use bluetooth::Bluetooth;
use wifi::Wifi;

#[derive(Debug, Clone, Default)]
pub struct Radios {
    /// As the system threads last read them.
    wifi_read: Wifi,
    bluetooth_read: Bluetooth,
    /// The attempts made in the tabs that no reading has answered yet.
    wifi_attempt: Option<Attempt<wifi::Problem>>,
    bluetooth_attempt: Option<Attempt<bluetooth::Problem>>,
    /// A network found here to need a login (an enterprise one not saved), by SSID, until the system's own
    /// error, a connection to it or another attempt answers it.
    login: Option<String>,
    /// What the tabs show: worked out from the above (see [`Radios::show_wifi`]).
    wifi: Wifi,
    bluetooth: Bluetooth,
}

/// An attempt made in a tab, to join a network or connect a device. A reading sent before the command
/// doesn't know of it, so the tab shows it busy until a reading answers it: one that shows the attempt
/// (busy with it), its result (connected), or an error other than the one from before (a reading from
/// before the command still carries that). An attempt that failed again the same way gives no such
/// reading, so one is given up [`ATTEMPT_WAIT`] after it was first seen, as one is whose thread was
/// started again (it lost the attempt).
#[derive(Debug, Clone)]
struct Attempt<P> {
    /// The network's SSID, or the device's address.
    target: String,
    /// When it was first seen by [`Radios::follow`] (monotonic seconds).
    since: Option<u64>,
    /// The error the reading had when it began.
    before: Option<P>,
}

impl<P: PartialEq> Attempt<P> {
    /// Whether a reading answers it: busy with it, `connected` to it, or with an error not from before.
    fn answered(&self, busy: Option<&str>, connected: bool, error: Option<&P>) -> bool {
        busy == Some(self.target.as_str()) || connected || error.is_some_and(|e| self.before.as_ref() != Some(e))
    }
}

/// How long an attempt's busy mark waits for a reading to answer it, in seconds.
pub(crate) const ATTEMPT_WAIT: u64 = 30;

/// Gives up `attempt` once [`ATTEMPT_WAIT`] has passed by `now` since it was first seen. Returns whether it
/// did.
fn wait_out<P>(attempt: &mut Option<Attempt<P>>, now: u64) -> bool {
    let Some(a) = attempt else { return false };
    let since = *a.since.get_or_insert(now);
    let over = now >= since + ATTEMPT_WAIT;
    if over {
        *attempt = None;
    }
    over
}

impl Radios {
    pub fn wifi(&self) -> &Wifi {
        &self.wifi
    }

    pub fn bluetooth(&self) -> &Bluetooth {
        &self.bluetooth
    }

    /// What the Wi-Fi tab shows: the reading, busy with the attempt while there is one, and the login
    /// needed while it stands. Worked out in place, so that it allocates nothing once its room is there.
    fn show_wifi(&mut self) {
        self.wifi.clone_from(&self.wifi_read);
        if let Some(a) = &self.wifi_attempt {
            self.wifi.busy = Some(a.target.clone());
            self.wifi.error = None;
        } else if let Some(ssid) = &self.login {
            self.wifi.error = Some(wifi::Problem::NeedsLogin { ssid: ssid.clone() });
        }
    }

    /// What the Bluetooth tab shows: the reading, busy with the attempt while there is one.
    fn show_bluetooth(&mut self) {
        self.bluetooth.clone_from(&self.bluetooth_read);
        if let Some(a) = &self.bluetooth_attempt {
            self.bluetooth.busy = Some(a.target.clone());
            self.bluetooth.error = None;
        }
    }

    /// A reading of Wi-Fi. It answers the attempt or the login needed if it can (see [`Attempt`]).
    pub(crate) fn read_wifi(&mut self, wifi: Wifi) {
        let connected = |ssid: &str| wifi.networks.iter().any(|n| n.connected && n.ssid == ssid);
        let mut answered = false;
        if self.wifi_attempt.as_ref().is_some_and(|a| a.answered(wifi.busy.as_deref(), connected(&a.target), wifi.error.as_ref())) {
            self.wifi_attempt = None;
            answered = true;
        }
        if self.login.as_deref().is_some_and(|ssid| wifi.error.is_some() || connected(ssid)) {
            self.login = None;
            answered = true;
        }
        // (The same reading again, answering nothing, shows nothing new.)
        if answered || wifi != self.wifi_read {
            self.wifi_read = wifi;
            self.show_wifi();
        }
    }

    /// A reading of Bluetooth. It answers the attempt if it can (see [`Attempt`]).
    pub(crate) fn read_bluetooth(&mut self, bluetooth: Bluetooth) {
        let connected = |a: &str| bluetooth.devices.iter().any(|d| d.connected && d.address == a);
        let mut answered = false;
        if self.bluetooth_attempt.as_ref().is_some_and(|a| a.answered(bluetooth.busy.as_deref(), connected(&a.target), bluetooth.error.as_ref())) {
            self.bluetooth_attempt = None;
            answered = true;
        }
        if answered || bluetooth != self.bluetooth_read {
            self.bluetooth_read = bluetooth;
            self.show_bluetooth();
        }
    }

    /// Gives up attempts no reading answered within [`ATTEMPT_WAIT`] (see [`Attempt`]).
    pub(crate) fn follow(&mut self, now: u64) {
        if wait_out(&mut self.wifi_attempt, now) {
            self.show_wifi();
        }
        if wait_out(&mut self.bluetooth_attempt, now) {
            self.show_bluetooth();
        }
    }

    /// The Wi-Fi (`wifi`) or a Bluetooth thread was started again after a panic, and lost the attempt it
    /// was making.
    pub(crate) fn restarted(&mut self, wifi: bool) {
        if wifi {
            self.wifi_attempt = None;
            self.show_wifi();
        } else {
            self.bluetooth_attempt = None;
            self.show_bluetooth();
        }
    }

    /// An attempt to join network `ssid` begins.
    fn attempt_wifi(&mut self, ssid: String) {
        self.wifi_attempt = Some(Attempt { target: ssid, since: None, before: self.wifi_read.error.clone() });
        self.login = None;
        self.show_wifi();
    }

    /// An attempt to connect device `address` begins.
    fn attempt_bluetooth(&mut self, address: String) {
        self.bluetooth_attempt = Some(Attempt { target: address, since: None, before: self.bluetooth_read.error.clone() });
        self.show_bluetooth();
    }

    /// Network `ssid` was found here to need a login.
    fn needs_login(&mut self, ssid: String) {
        self.login = Some(ssid);
        self.show_wifi();
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
