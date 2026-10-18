//! Wi-Fi as the Wi-Fi tab shows it (NetworkManager's view, read by the system side), and its rules: a
//! tap joins a network (asking for its password first if it has one and isn't saved), a long press
//! forgets a saved one. What a network's row and the tab's header say is decided here
//! ([`Wifi::net_state`], [`Wifi::status`]); the UI words it.

use crate::State;
use crate::editor::{Editor, Purpose};
use crate::effect::Effect;
use crate::id::WifiId;
use crate::items::ItemKey;
use crate::radios::RadioStatus;
use crate::scroll::Rows;

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Network {
    pub ssid: String,
    /// Signal strength 0-100.
    pub strength: u8,
    /// Needs a password (WEP or WPA personal).
    pub secure: bool,
    /// Needs a login (WPA enterprise): not supported here.
    pub enterprise: bool,
    /// NetworkManager has a connection saved for it.
    pub known: bool,
    /// Connected now.
    pub active: bool,
}

/// Why joining failed, or Wi-Fi can't be used.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Problem {
    /// The password was refused (or is missing).
    WrongPassword { ssid: String },
    /// Joined, but the network gave no address.
    NoAddress { ssid: String },
    /// No answer before the time limit.
    NoAnswer { ssid: String },
    /// NetworkManager ended the attempt for another reason (its NMActiveConnectionStateReason).
    Refused { ssid: String, reason: u32 },
    /// The network needs a login (WPA enterprise), which isn't supported here.
    NeedsLogin { ssid: String },
    /// NetworkManager isn't there.
    Unavailable,
    /// NetworkManager's D-Bus call failed (its message).
    Bus(String),
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Wifi {
    /// There is a Wi-Fi device (and NetworkManager runs).
    pub device: bool,
    /// The radio is on.
    pub enabled: bool,
    /// One per SSID, in order (see [`sort`]).
    pub networks: Vec<Network>,
    /// The network being joined, by SSID.
    pub connecting: Option<String>,
    /// Why the last attempt failed, until the next one.
    pub error: Option<Problem>,
}

/// What a network's row says about it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NetState {
    Connected,
    Joining,
    Saved,
    /// It needs a login, which isn't supported.
    Login,
    Other,
}

impl Wifi {
    pub fn net_state(&self, n: &Network) -> NetState {
        if n.active {
            NetState::Connected
        } else if self.connecting.as_deref() == Some(n.ssid.as_str()) {
            NetState::Joining
        } else if n.known {
            NetState::Saved
        } else if n.enterprise {
            NetState::Login
        } else {
            NetState::Other
        }
    }

    /// What Wi-Fi is doing, for the tab's header: an error first, then no device, off, joining (the
    /// network), connected (to it), or nothing.
    pub fn status(&self) -> RadioStatus<'_, Problem> {
        if let Some(e) = &self.error {
            RadioStatus::Error(e)
        } else if !self.device {
            RadioStatus::NoDevice
        } else if !self.enabled {
            RadioStatus::Off
        } else if let Some(ssid) = &self.connecting {
            RadioStatus::Busy(ssid)
        } else if let Some(n) = self.networks.iter().find(|n| n.active) {
            RadioStatus::Connected(&n.ssid)
        } else {
            RadioStatus::Idle
        }
    }
}

/// How many bars (0-3) a signal of `strength` (0-100) shows.
pub fn signal_bars(strength: u8) -> u8 {
    match strength {
        0..=24 => 0,
        25..=49 => 1,
        50..=74 => 2,
        _ => 3,
    }
}

/// What the Wi-Fi side is asked to do.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Command {
    /// The radio on or off.
    Enable(bool),
    /// Join a network: its saved connection, or a new one (with this password, if it needs one).
    Connect { ssid: String, password: Option<String> },
    /// Drop a network's saved connections (which also disconnects it).
    Forget(String),
}

/// One entry per SSID (the strongest access point; known and active if any of them is), connected
/// first, then saved ones, then by strength. Hidden networks (empty SSID) are left out.
pub fn sort(networks: Vec<Network>) -> Vec<Network> {
    let mut out: Vec<Network> = Vec::with_capacity(networks.len());
    for n in networks.into_iter().filter(|n| !n.ssid.is_empty()) {
        match out.iter_mut().find(|o| o.ssid == n.ssid) {
            Some(o) => {
                o.known |= n.known;
                o.active |= n.active;
                if n.strength > o.strength {
                    o.strength = n.strength;
                }
            }
            None => out.push(n),
        }
    }
    out.sort_by(|a, b| b.active.cmp(&a.active).then(b.known.cmp(&a.known)).then(b.strength.cmp(&a.strength)).then(a.ssid.cmp(&b.ssid)));
    out
}

/// The networks as a list's rows (found again by their SSID).
impl Rows for Wifi {
    fn len(&self) -> usize {
        self.networks.len()
    }

    fn key(&self, i: usize) -> Option<ItemKey> {
        self.networks.get(i).map(|n| ItemKey::Name(n.ssid.clone()))
    }

    fn find(&self, key: &ItemKey) -> Option<usize> {
        self.networks.iter().position(|n| matches!(key, ItemKey::Name(k) if *k == n.ssid))
    }
}

impl State {
    /// A tap on the Wi-Fi tab's switch.
    pub(crate) fn on_wifi(&mut self, id: WifiId) -> Vec<Effect> {
        match id {
            WifiId::Power => vec![Effect::Wifi(Command::Enable(!self.radios.wifi.enabled))],
        }
    }

    /// A tap on the network at `index`, or a long press.
    pub(crate) fn tap_network(&mut self, index: usize, long: bool) -> Vec<Effect> {
        let Some(n) = self.radios.wifi.networks.get(index) else {
            return vec![];
        };
        let ssid = n.ssid.clone();
        if long {
            return if n.known { vec![Effect::Wifi(Command::Forget(ssid))] } else { vec![] };
        }
        if n.active {
            return vec![];
        }
        if n.enterprise && !n.known {
            self.radios.wifi.error = Some(Problem::NeedsLogin { ssid });
            return vec![];
        }
        if n.secure && !n.known {
            self.bottom.open_editor(Editor { purpose: Purpose::Password { ssid }, text: String::new() });
            return vec![];
        }
        self.join(ssid, None)
    }

    pub(crate) fn join(&mut self, ssid: String, password: Option<String>) -> Vec<Effect> {
        self.radios.wifi.connecting = Some(ssid.clone());
        self.radios.wifi.error = None;
        vec![Effect::Wifi(Command::Connect { ssid, password })]
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    use crate::testkit::*;
    use crate::*;

    fn net(ssid: &str, strength: u8, secure: bool, known: bool, active: bool) -> Network {
        Network { ssid: ssid.into(), strength, secure, enterprise: false, known, active }
    }

    #[test]
    fn one_entry_per_network_connected_and_saved_first() {
        let sorted = sort(vec![
            net("cafe", 90, false, false, false),
            net("home", 40, true, true, false),
            net("home", 70, true, false, true),
            net("", 99, false, false, false),
            net("work", 50, true, true, false),
        ]);
        let names: Vec<(&str, u8, bool)> = sorted.iter().map(|n| (n.ssid.as_str(), n.strength, n.known)).collect();
        assert_eq!(names, [("home", 70, true), ("work", 50, true), ("cafe", 90, false)]);
    }

    #[test]
    fn joining_asks_for_a_password_only_when_needed() {
        let mut s = state();
        tap(&mut s, Id::Tab(Tab::Wifi));
        let networks = vec![net("home", 70, true, true, false), net("cafe", 90, false, false, false), net("new", 60, true, false, false)];
        read(&mut s, Reading::Wifi(Wifi { device: true, enabled: true, networks, ..Wifi::default() }));
        let connect = |ssid: &str, password: Option<&str>| vec![Effect::Wifi(Command::Connect { ssid: ssid.into(), password: password.map(Into::into) })];
        let network = |i| ListId::Wifi.row(i);
        assert_eq!(tap(&mut s, network(0)), connect("home", None), "saved");
        assert_eq!(s.radios().wifi.connecting.as_deref(), Some("home"));
        assert_eq!(s.radios().wifi.status(), RadioStatus::Busy("home"));
        assert_eq!(tap(&mut s, network(1)), connect("cafe", None), "open");
        assert_eq!(tap(&mut s, network(2)), vec![], "asks first");
        assert!(matches!(s.bottom().editor(), Some(Editor { purpose: Purpose::Password { ssid }, .. }) if ssid == "new"));
        for c in ['p', 'w'] {
            tap(&mut s, Id::Key(keyboard::find(keyboard::Key::Char(c)).unwrap()));
        }
        assert_eq!(tap(&mut s, Id::Editor(EditorId::Save)), connect("new", Some("pw")));
        assert_eq!(s.bottom().editor(), None);
        assert_eq!(long(&mut s, network(0)), vec![Effect::Wifi(Command::Forget("home".into()))]);
        assert_eq!(long(&mut s, network(1)), vec![], "nothing saved to forget");
    }

    #[test]
    fn a_network_that_needs_a_login_says_so() {
        let mut s = state();
        let login = Network { enterprise: true, ..net("work", 60, true, false, false) };
        read(&mut s, Reading::Wifi(Wifi { device: true, enabled: true, networks: vec![login.clone()], ..Wifi::default() }));
        assert_eq!(s.radios().wifi.net_state(&login), NetState::Login);
        assert_eq!(tap(&mut s, ListId::Wifi.row(0)), vec![]);
        assert_eq!(s.radios().wifi.status(), RadioStatus::Error(&Problem::NeedsLogin { ssid: "work".into() }));
    }

    #[test]
    fn the_wifi_tab_watches_while_open() {
        let mut s = state();
        tap(&mut s, Id::Tab(Tab::Keyboard));
        assert_eq!(tap(&mut s, Id::Tab(Tab::Wifi)), vec![Effect::Watch(Watch::Wifi, true)]);
        assert_eq!(tap(&mut s, Id::Tab(Tab::Wifi)), vec![], "a tab: a second tap stays");
        assert_eq!(tap(&mut s, Id::Tab(Tab::Touchpad)), vec![Effect::Watch(Watch::Wifi, false)]);
        assert_eq!(s.bottom().tab(), Tab::Touchpad);
    }

    #[test]
    fn signal_bars_step_by_quarters() {
        assert_eq!([0, 24, 25, 49, 50, 74, 75, 100].map(signal_bars), [0, 0, 1, 1, 2, 2, 3, 3]);
    }
}
