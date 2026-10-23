//! Wi-Fi as the Wi-Fi tab shows it, which is NetworkManager's view as read by the system side, and its
//! rules. A tap joins a network, first asking for its password if it has one and it isn't saved. A long
//! press forgets a saved network. What a network's row and the tab's header say is decided here
//! ([`Wifi::net_state`], [`Wifi::status`]), and the UI puts it into words.

use crate::bottom::editor::{Editor, Editors, Purpose};
use crate::effect::Effect;
use crate::kit::list::ItemKey;
use crate::kit::list::Rows;
use crate::kit::secret::Secret;
use crate::out::Out;
use crate::radios::{RadioStatus, Radios};

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Network {
    pub ssid: String,
    /// Signal strength 0-100.
    pub strength: u8,
    /// Needs a password (WEP or WPA personal).
    pub secure: bool,
    /// Needs a login (WPA enterprise), which isn't supported here.
    pub enterprise: bool,
    /// NetworkManager has a connection saved for it.
    pub saved: bool,
    /// Connected now.
    pub connected: bool,
}

/// Why joining failed, or Wi-Fi can't be used.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Problem {
    /// The password was refused or is missing.
    WrongPassword { ssid: String },
    /// Joined, but the network gave no address.
    NoAddress { ssid: String },
    /// No answer before the time limit.
    NoAnswer { ssid: String },
    /// NetworkManager ended the attempt for another reason, given as its NMActiveConnectionStateReason.
    Refused { ssid: String, reason: u32 },
    /// The network needs a login (WPA enterprise), which isn't supported here.
    NeedsLogin { ssid: String },
    /// NetworkManager isn't there.
    Unavailable,
    /// NetworkManager's D-Bus call failed, with its message.
    Bus(String),
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Wifi {
    /// There is a Wi-Fi device and NetworkManager is running.
    pub present: bool,
    /// The radio is on.
    pub on: bool,
    /// One per SSID, in order (see [`sort`]).
    pub networks: Vec<Network>,
    /// The network being joined, by SSID.
    pub busy: Option<String>,
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
        if n.connected {
            NetState::Connected
        } else if self.busy.as_deref() == Some(n.ssid.as_str()) {
            NetState::Joining
        } else if n.saved {
            NetState::Saved
        } else if n.enterprise {
            NetState::Login
        } else {
            NetState::Other
        }
    }

    /// What Wi-Fi is doing, for the tab's header. In order of precedence: an error, no device, off,
    /// joining a network, connected to one, or nothing.
    pub fn status(&self) -> RadioStatus<'_, Problem> {
        if let Some(e) = &self.error {
            RadioStatus::Error(e)
        } else if !self.present {
            RadioStatus::NoDevice
        } else if !self.on {
            RadioStatus::Off
        } else if let Some(ssid) = &self.busy {
            RadioStatus::Busy(ssid)
        } else if let Some(n) = self.networks.iter().find(|n| n.connected) {
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
    /// Turn the radio on or off.
    Enable(bool),
    /// Join a network, using its saved connection or a new one with this password if it needs one.
    Connect { ssid: String, password: Option<Secret> },
    /// Drop a network's saved connections, which also disconnects it.
    Forget(String),
}

/// Sorts the networks to one entry per SSID. Each entry has the strength of the strongest access point,
/// and counts as saved or connected if any of the SSID's access points is. The connected network comes first, then
/// saved ones, then the rest by strength. Hidden networks, with an empty SSID, are left out.
pub fn sort(networks: &mut Vec<Network>) {
    networks.retain(|n| !n.ssid.is_empty());
    // By SSID, keeping their order, so that each SSID's first access point takes in the others.
    networks.sort_by(|a, b| a.ssid.cmp(&b.ssid));
    networks.dedup_by(|n, first| {
        if n.ssid != first.ssid {
            return false;
        }
        first.saved |= n.saved;
        first.connected |= n.connected;
        first.strength = first.strength.max(n.strength);
        true
    });
    networks.sort_by(|a, b| b.connected.cmp(&a.connected).then(b.saved.cmp(&a.saved)).then(b.strength.cmp(&a.strength)).then(a.ssid.cmp(&b.ssid)));
}

/// The networks as a list's rows, found again by their SSID.
impl Rows for Wifi {
    fn len(&self) -> usize {
        self.networks.len()
    }

    fn key(&self, i: usize) -> Option<ItemKey> {
        self.networks.get(i).map(|n| ItemKey::name(&n.ssid))
    }

    fn find(&self, key: &ItemKey) -> Option<usize> {
        self.networks.iter().position(|n| ItemKey::name(&n.ssid) == *key)
    }
}

impl Radios {
    /// The Wi-Fi tab's switch.
    pub(crate) fn power_wifi(&self, out: &mut Out) {
        out.effects.push(Effect::Wifi(Command::Enable(!self.wifi.on)));
    }

    /// A tap on the network at `index`, or a long press. A network with a password that isn't saved
    /// asks for it first.
    pub(crate) fn tap_network(&mut self, index: usize, long: bool, editors: &mut Editors, out: &mut Out) {
        let Some(n) = self.wifi.networks.get(index) else {
            return;
        };
        let ssid = n.ssid.clone();
        if long {
            if n.saved {
                out.effects.push(Effect::Wifi(Command::Forget(ssid)));
            }
            return;
        }
        if n.connected {
            return;
        }
        if n.enterprise && !n.saved {
            self.wifi.error = Some(Problem::NeedsLogin { ssid });
            return;
        }
        if n.secure && !n.saved {
            editors.open(Editor { purpose: Purpose::Password { ssid }, text: String::new() });
            return;
        }
        self.join(ssid, None, out);
    }

    pub(crate) fn join(&mut self, ssid: String, password: Option<String>, out: &mut Out) {
        self.wifi.busy = Some(ssid.clone());
        self.wifi.error = None;
        out.effects.push(Effect::Wifi(Command::Connect { ssid, password: password.map(Secret) }));
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    use crate::testkit::*;
    use crate::*;

    fn net(ssid: &str, strength: u8, secure: bool, saved: bool, connected: bool) -> Network {
        Network { ssid: ssid.into(), strength, secure, enterprise: false, saved, connected }
    }

    #[test]
    fn one_entry_per_network_connected_and_saved_first() {
        let mut sorted = vec![
            net("cafe", 90, false, false, false),
            net("home", 40, true, true, false),
            net("home", 70, true, false, true),
            net("", 99, false, false, false),
            net("work", 50, true, true, false),
        ];
        sort(&mut sorted);
        let names: Vec<(&str, u8, bool)> = sorted.iter().map(|n| (n.ssid.as_str(), n.strength, n.saved)).collect();
        assert_eq!(names, [("home", 70, true), ("work", 50, true), ("cafe", 90, false)]);
    }

    #[test]
    fn joining_asks_for_a_password_only_when_needed() {
        let mut s = state();
        tap(&mut s, Id::Tab(Tab::Wifi));
        let networks = vec![net("home", 70, true, true, false), net("cafe", 90, false, false, false), net("new", 60, true, false, false)];
        read(&mut s, Reading::Wifi(Wifi { present: true, on: true, networks, ..Wifi::default() }));
        let connect =
            |ssid: &str, password: Option<&str>| vec![Effect::Wifi(Command::Connect { ssid: ssid.into(), password: password.map(|p| Secret(p.into())) })];
        let network = |i| ListId::Wifi.row(i);
        assert_eq!(tap(&mut s, network(0)), connect("home", None), "saved");
        assert_eq!(s.radios().wifi().busy.as_deref(), Some("home"));
        assert_eq!(s.radios().wifi().status(), RadioStatus::Busy("home"));
        assert_eq!(tap(&mut s, network(1)), connect("cafe", None), "open");
        assert_eq!(tap(&mut s, network(2)), vec![], "asks first");
        assert!(matches!(s.bottom().editor(), Some(Editor { purpose: Purpose::Password { ssid }, .. }) if ssid == "new"));
        for c in ['p', 'w'] {
            tap(&mut s, Id::Key(crate::bottom::keyboard::find(crate::bottom::keyboard::Key::Char(c)).unwrap()));
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
        read(&mut s, Reading::Wifi(Wifi { present: true, on: true, networks: vec![login.clone()], ..Wifi::default() }));
        assert_eq!(s.radios().wifi().net_state(&login), NetState::Login);
        assert_eq!(tap(&mut s, ListId::Wifi.row(0)), vec![]);
        assert_eq!(s.radios().wifi().status(), RadioStatus::Error(&Problem::NeedsLogin { ssid: "work".into() }));
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
