//! Wi-Fi as the Wi-Fi tab shows it (NetworkManager's view, read by the policy side), and its rules: a
//! tap joins a network (asking for its password first if it has one and isn't saved), a long press
//! forgets a saved one.

use crate::editor::{Editor, Purpose};
use crate::{Effect, State};

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
    pub error: Option<String>,
}

/// What the Wi-Fi side is asked to do.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Command {
    /// The Wi-Fi tab opened (scan now and keep the list fresh) or closed.
    Watch(bool),
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

impl State {
    /// A tap on the network at `index`, or a long press.
    pub(crate) fn tap_network(&mut self, index: usize, long: bool) -> Vec<Effect> {
        let Some(n) = self.wifi.networks.get(index) else {
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
            self.wifi.error = Some(format!("{ssid} needs a login, which isn't supported here"));
            return vec![];
        }
        if n.secure && !n.known {
            self.editor = Some(Editor { purpose: Purpose::Password { ssid }, text: String::new() });
            return vec![];
        }
        self.join(ssid, None)
    }

    pub(crate) fn join(&mut self, ssid: String, password: Option<String>) -> Vec<Effect> {
        self.wifi.connecting = Some(ssid.clone());
        self.wifi.error = None;
        vec![Effect::Wifi(Command::Connect { ssid, password })]
    }

    /// The Wi-Fi side's latest view; what the program asked for is kept until it reports on it.
    pub(crate) fn set_wifi(&mut self, wifi: Wifi) {
        self.wifi = wifi;
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    use crate::tests::state;
    use crate::{Event, Id, Tab};

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
        s.apply(Event::Activate(Id::Tab(Tab::Wifi)));
        let networks = vec![net("home", 70, true, true, false), net("cafe", 90, false, false, false), net("new", 60, true, false, false)];
        s.apply(Event::Wifi(Wifi { device: true, enabled: true, networks, ..Wifi::default() }));
        let connect = |ssid: &str, password: Option<&str>| {
            vec![Effect::Wifi(Command::Connect { ssid: ssid.into(), password: password.map(Into::into) })]
        };
        assert_eq!(s.apply(Event::Activate(Id::Network(0))).1, connect("home", None), "saved");
        assert_eq!(s.wifi.connecting.as_deref(), Some("home"));
        assert_eq!(s.apply(Event::Activate(Id::Network(1))).1, connect("cafe", None), "open");
        assert_eq!(s.apply(Event::Activate(Id::Network(2))).1, vec![], "asks first");
        assert!(matches!(&s.editor, Some(Editor { purpose: Purpose::Password { ssid }, .. }) if ssid == "new"));
        for c in ['p', 'w'] {
            s.apply(Event::Activate(Id::Key(crate::keyboard::find(crate::keyboard::Key::Char(c)).unwrap())));
        }
        assert_eq!(s.apply(Event::Activate(Id::EditorSave)).1, connect("new", Some("pw")));
        assert_eq!(s.editor, None);
        assert_eq!(s.apply(Event::LongPress(Id::Network(0))).1, vec![Effect::Wifi(Command::Forget("home".into()))]);
        assert_eq!(s.apply(Event::LongPress(Id::Network(1))).1, vec![], "nothing saved to forget");
    }

    #[test]
    fn the_wifi_mode_watches_while_open() {
        let mut s = state();
        s.apply(Event::Activate(Id::Tab(Tab::Keyboard)));
        let (_, fx) = s.apply(Event::Activate(Id::Tab(Tab::Wifi)));
        assert_eq!((s.tab, fx), (Tab::Wifi, vec![Effect::Wifi(Command::Watch(true))]));
        let (_, fx) = s.apply(Event::Activate(Id::Tab(Tab::Wifi)));
        assert_eq!((s.tab, fx), (Tab::Wifi, vec![]), "a tab: a second tap stays");
        let (_, fx) = s.apply(Event::Activate(Id::Tab(Tab::Touchpad)));
        assert_eq!(fx, vec![Effect::Wifi(Command::Watch(false)), Effect::Touchpad(true)]);
    }
}
