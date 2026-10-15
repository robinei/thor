//! Wi-Fi through NetworkManager (D-Bus, system bus): while the Wi-Fi tab is open, the networks in
//! range (scanned every few seconds), which are saved and which is connected, sent as [`Reading::Wifi`]
//! when they change; joining (a saved connection, or a new one with a password), forgetting, the radio
//! on and off. A new connection that fails to join is deleted again, so a mistyped password isn't kept.
//! A join is followed through the active connection NetworkManager makes for it (not the device, which
//! also goes through leaving the network it was on); why one failed, from the device's failure (which
//! leaving a network doesn't go through).

mod nm;

use std::collections::HashMap;
use std::fmt;
use std::io;
use std::sync::mpsc::Receiver;
use std::time::{Duration, Instant};

use clamshell_core::wifi::{self, Command, Network, Problem, Wifi};
use clamshell_core::{Event, Reading};
use zbus::blocking::Connection;
use zbus::zvariant::{ObjectPath, OwnedObjectPath, Value};

use crate::dbus::{self, Props};
use crate::worker::{self, Changed, Ended, Next, Service, Worker, lost_on};
use crate::{Closed, Sink};
use nm::{AccessPointProxy, ActiveProxy, ActiveReason, ActiveState, ConnectionProxy, DeviceProxy, DeviceReason, DeviceState, DeviceType};
use nm::{AP_PRIVACY, KEY_MGMT_8021X, KEY_MGMT_PSK, KEY_MGMT_SAE, ManagerProxy, SettingsProxy, WirelessProxy};

/// How often the list is read while the mode is open, and how often a scan is asked for.
const READ_EVERY: Duration = Duration::from_secs(1);
const SCAN_EVERY: Duration = Duration::from_secs(10);
/// A join that hasn't finished by then has failed (NetworkManager gives up on an address after 45 s and
/// says so itself first).
const JOIN_FOR: Duration = Duration::from_secs(60);
/// How long to wait before trying NetworkManager again.
const RETRY: Duration = Duration::from_secs(10);

/// What this thread is asked: to watch while the Wi-Fi tab is open (scan now and keep the list fresh),
/// and the state's commands.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Request {
    Watch(bool),
    Do(Command),
}

#[derive(Debug)]
enum Error {
    Bus(zbus::Error),
    /// A thread forwarding signals couldn't be started.
    Thread(io::Error),
    NoDevice,
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::Bus(e) => write!(f, "{e}"),
            Error::Thread(e) => write!(f, "{e}"),
            Error::NoDevice => write!(f, "no Wi-Fi device"),
        }
    }
}

impl From<zbus::Error> for Error {
    fn from(e: zbus::Error) -> Self {
        Error::Bus(e)
    }
}

impl From<zbus::zvariant::Error> for Error {
    fn from(e: zbus::zvariant::Error) -> Self {
        Error::Bus(e.into())
    }
}

impl From<io::Error> for Error {
    fn from(e: io::Error) -> Self {
        Error::Thread(e)
    }
}

lost_on!(Error: zbus::Error, io::Error);

pub fn spawn(sink: impl Sink) -> io::Result<Worker<Request>> {
    Worker::spawn("wifi", move |commands| worker::supervise(&mut WifiThread { sink, state: WifiState::default() }, &commands, RETRY))
}

/// A connection being made.
struct Attempt {
    ssid: String,
    /// The connection we added for it (deleted again if the join fails).
    added: Option<OwnedObjectPath>,
    /// The active connection NetworkManager made for it, and the device it is on.
    active: OwnedObjectPath,
    device: OwnedObjectPath,
    started: Instant,
    /// The device's reason for failing meanwhile, if it did: why the join failed.
    device_failure: Option<u32>,
}

/// What this thread keeps between reads, and across reconnecting.
#[derive(Default)]
struct WifiState {
    watching: bool,
    attempt: Option<Attempt>,
    error: Option<Problem>,
    last_scan: Option<Instant>,
}

struct WifiThread<S> {
    sink: S,
    state: WifiState,
}

/// An object's StateChanged signal: its new state and the reason.
#[derive(Debug, Clone, PartialEq, Eq)]
struct StateChange {
    object: OwnedObjectPath,
    state: u32,
    reason: u32,
}

/// A connection to NetworkManager, with the state changes of every active connection and device,
/// subscribed to for as long as it lasts (a join's own pass too quickly to be polled).
struct Session {
    conn: Connection,
    nm: ManagerProxy<'static>,
    active_changes: Receiver<StateChange>,
    device_changes: Receiver<StateChange>,
}

impl Session {
    fn connect() -> Result<Session, Error> {
        let conn = Connection::system()?;
        let nm: ManagerProxy = dbus::default_proxy(&conn)?;
        // NetworkManager is there.
        nm.wireless_enabled()?;
        // Active connections signal (state, reason); devices (new state, old state, reason).
        let active_changes = state_changes(&conn, nm::ACTIVE, "wifi-active", |body| body.deserialize::<(u32, u32)>().ok())?;
        let device_changes = state_changes(&conn, nm::DEVICE, "wifi-device", |body| {
            body.deserialize::<(u32, u32, u32)>().ok().map(|(new, _old, reason)| (new, reason))
        })?;
        Ok(Session { conn, nm, active_changes, device_changes })
    }

    fn proxy<P>(&self, path: &str) -> zbus::Result<P>
    where
        P: zbus::blocking::proxy::ProxyImpl<'static> + From<zbus::Proxy<'static>>,
    {
        dbus::proxy(&self.conn, path)
    }

    /// The first Wi-Fi device NetworkManager manages (one it leaves alone, an access point's radio,
    /// isn't joined from), else the first Wi-Fi device, if any.
    fn wifi_device(&self) -> zbus::Result<Option<OwnedObjectPath>> {
        let mut unmanaged = None;
        for path in self.nm.get_devices()? {
            let device = self.proxy::<DeviceProxy>(path.as_str())?;
            if device.device_type()? != DeviceType::Wifi as u32 {
                continue;
            }
            if device.state()? != DeviceState::Unmanaged as u32 {
                return Ok(Some(path));
            }
            unmanaged.get_or_insert(path);
        }
        Ok(unmanaged)
    }

    fn access_points(&self, device: &OwnedObjectPath) -> zbus::Result<Vec<Ap>> {
        let paths = self.proxy::<WirelessProxy>(device.as_str())?.get_all_access_points()?;
        let mut aps = Vec::with_capacity(paths.len());
        for path in paths {
            let ap: AccessPointProxy = self.proxy(path.as_str())?;
            // An access point can vanish between the list and its properties.
            let Ok(ssid) = ap.ssid() else {
                continue;
            };
            aps.push(Ap {
                ssid: String::from_utf8_lossy(&ssid).into_owned(),
                strength: ap.strength().unwrap_or(0),
                flags: ap.flags().unwrap_or(0),
                security: ap.wpa_flags().unwrap_or(0) | ap.rsn_flags().unwrap_or(0),
                path,
            });
        }
        Ok(aps)
    }

    /// Saved Wi-Fi connections: (path, SSID).
    fn saved(&self) -> zbus::Result<Vec<(OwnedObjectPath, String)>> {
        let mut out = Vec::new();
        for path in dbus::default_proxy::<SettingsProxy>(&self.conn)?.list_connections()? {
            let settings = self.proxy::<ConnectionProxy>(path.as_str())?.get_settings()?;
            if let Some(ssid) = settings.get("802-11-wireless").and_then(|w| Props(w).bytes("ssid")) {
                out.push((path, String::from_utf8_lossy(&ssid).into_owned()));
            }
        }
        Ok(out)
    }

    fn delete(&self, connection: &OwnedObjectPath) -> zbus::Result<()> {
        self.proxy::<ConnectionProxy>(connection.as_str())?.delete()
    }
}

/// The StateChanged signals of every object with `interface`; `state` reads (state, reason) from a
/// signal's body.
fn state_changes(
    conn: &Connection,
    interface: &'static str,
    name: &str,
    state: fn(&zbus::message::Body) -> Option<(u32, u32)>,
) -> Result<Receiver<StateChange>, Error> {
    let rule = zbus::MatchRule::builder().msg_type(zbus::message::Type::Signal).interface(interface)?.member("StateChanged")?.build();
    let signals = zbus::blocking::MessageIterator::for_match_rule(rule, conn, None)?;
    Ok(dbus::forward_signals(name, signals, move |message| {
        let message = message.ok()?;
        let object = OwnedObjectPath::from(message.header().path()?.to_owned());
        let (state, reason) = state(&message.body())?;
        Some(StateChange { object, state, reason })
    })?)
}

/// An access point as NetworkManager has it.
struct Ap {
    path: OwnedObjectPath,
    ssid: String,
    strength: u8,
    flags: u32,
    security: u32,
}

impl Ap {
    fn secure(&self) -> bool {
        self.flags & AP_PRIVACY != 0 || self.security != 0
    }

    fn enterprise(&self) -> bool {
        self.security & KEY_MGMT_8021X != 0
    }
}

impl<S: Sink> WifiThread<S> {
    fn send(&self, wifi: Wifi) -> Result<(), Closed> {
        self.sink.send(Event::Reading(Reading::Wifi(wifi)))
    }

    fn read(&mut self, s: &Session, device: Option<&OwnedObjectPath>) -> zbus::Result<Wifi> {
        let st = &mut self.state;
        let enabled = s.nm.wireless_enabled()?;
        let Some(device) = device else {
            return Ok(Wifi { device: false, enabled, error: st.error.clone(), ..Wifi::default() });
        };
        let state = s.proxy::<DeviceProxy>(device.as_str())?.state()?;
        let active_ap = s.proxy::<WirelessProxy>(device.as_str())?.active_access_point()?;
        let known = s.saved()?;
        let aps = s.access_points(device)?;
        let active_ssid = aps.iter().find(|ap| ap.path == active_ap && state == DeviceState::Activated as u32).map(|ap| ap.ssid.clone());
        self.follow_attempt(s, active_ssid.as_deref())?;
        let st = &self.state;
        let networks = aps
            .into_iter()
            .map(|ap| Network {
                known: known.iter().any(|(_, ssid)| *ssid == ap.ssid),
                active: active_ssid.as_deref() == Some(&ap.ssid),
                secure: ap.secure(),
                enterprise: ap.enterprise(),
                strength: ap.strength,
                ssid: ap.ssid,
            })
            .collect();
        Ok(Wifi {
            device: true,
            enabled,
            networks: wifi::sort(networks),
            connecting: st.attempt.as_ref().map(|a| a.ssid.clone()),
            error: st.error.clone(),
        })
    }

    /// Ends the attempt once the network is connected, or with an error once it has failed.
    fn follow_attempt(&mut self, s: &Session, active: Option<&str>) -> zbus::Result<()> {
        let st = &mut self.state;
        // The signals since the last look: the attempt's own, if there is one.
        let active_changes: Vec<_> = s.active_changes.try_iter().collect();
        let device_changes: Vec<_> = s.device_changes.try_iter().collect();
        let Some(attempt) = st.attempt.as_mut() else {
            return Ok(());
        };
        let mut joined = active == Some(attempt.ssid.as_str());
        let mut ended = None;
        for change in active_changes.into_iter().filter(|c| c.object == attempt.active) {
            joined |= change.state == ActiveState::Activated as u32;
            if change.state == ActiveState::Deactivated as u32 {
                ended = Some(change.reason);
            }
        }
        for change in device_changes.into_iter().filter(|c| c.object == attempt.device && c.state == DeviceState::Failed as u32) {
            attempt.device_failure = Some(change.reason);
        }
        if joined {
            log::info!("wifi: joined {}", attempt.ssid);
            st.attempt = None;
            return Ok(());
        }
        // Gone without a word (it is removed once deactivated): failed, the reason unknown.
        if ended.is_none() && s.proxy::<ActiveProxy>(attempt.active.as_str())?.state().is_err() {
            ended = Some(0);
        }
        let timed_out = attempt.started.elapsed() > JOIN_FOR;
        if ended.is_none() && !timed_out {
            return Ok(());
        }
        let problem = classify(ended.unwrap_or(0), attempt.device_failure, timed_out, attempt.ssid.clone());
        log::warn!("wifi: {problem:?}");
        if let Some(path) = &attempt.added {
            let _ = s.delete(path);
        }
        st.error = Some(problem);
        st.attempt = None;
        Ok(())
    }

    /// Carries out a request.
    fn apply(&mut self, s: &Session, request: Request) -> Result<(), Error> {
        let st = &mut self.state;
        let command = match request {
            Request::Watch(on) => {
                st.watching = on;
                if on {
                    st.last_scan = None;
                }
                return Ok(());
            }
            Request::Do(command) => command,
        };
        match command {
            Command::Enable(on) => s.nm.set_wireless_enabled(on)?,
            Command::Forget(ssid) => {
                for (path, _) in s.saved()?.into_iter().filter(|(_, s)| *s == ssid) {
                    s.delete(&path)?;
                }
                log::info!("wifi: forgot {ssid}");
            }
            Command::Connect { ssid, password } => {
                st.error = None;
                let device = s.wifi_device()?.ok_or(Error::NoDevice)?;
                // From here on, the state changes are this join's.
                s.active_changes.try_iter().for_each(drop);
                s.device_changes.try_iter().for_each(drop);
                let root = ObjectPath::try_from("/")?;
                let (added, active) = match (s.saved()?.into_iter().find(|(_, s)| *s == ssid), password) {
                    (Some((connection, _)), None) => (None, s.nm.activate_connection(&connection, &device, &root)?),
                    (_, password) => {
                        let aps = s.access_points(&device)?;
                        let ap = aps.iter().filter(|ap| ap.ssid == ssid).max_by_key(|ap| ap.strength);
                        let settings = new_connection(&ssid, password.as_deref(), ap);
                        let ap_path = ap.map_or_else(|| root.clone(), |ap| ap.path.as_ref().clone());
                        let (path, active) = s.nm.add_and_activate_connection(settings, &device, &ap_path)?;
                        (Some(path), active)
                    }
                };
                log::info!("wifi: joining {ssid}");
                st.attempt = Some(Attempt { ssid, added, active, device, started: Instant::now(), device_failure: None });
            }
        }
        Ok(())
    }
}

impl<S: Sink> Service for WifiThread<S> {
    type Command = Request;
    type Session = Session;
    type Error = Error;
    const NAME: &'static str = "wifi";

    fn connect(&mut self) -> Result<Session, Error> {
        Session::connect()
    }

    /// Reads every second while watching or joining (scanning every few seconds while watching), and
    /// carries out requests, until NetworkManager fails or goes.
    fn run_while_up(&mut self, s: Session, commands: &Receiver<Request>) -> Result<(), Ended<Error>> {
        let mut last = Changed::default();
        loop {
            let busy = self.state.watching || self.state.attempt.is_some();
            match worker::next(commands, busy.then_some(READ_EVERY)) {
                Next::Command(request) => {
                    if let Err(e) = self.apply(&s, request) {
                        log::warn!("wifi: {e}");
                        self.state.error = Some(Problem::Bus(e.to_string()));
                        self.state.attempt = None;
                    }
                }
                Next::Elapsed => {}
                Next::Disconnected => return Ok(()),
            }
            if !(self.state.watching || self.state.attempt.is_some()) {
                last.forget();
                continue;
            }
            let device = s.wifi_device()?;
            if let Some(device) = &device
                && self.state.watching
                && self.state.last_scan.is_none_or(|t| t.elapsed() >= SCAN_EVERY)
            {
                self.state.last_scan = Some(Instant::now());
                // Refused while one runs, or right after one: harmless.
                let _ = s.proxy::<WirelessProxy>(device.as_str())?.request_scan(HashMap::new());
            }
            let wifi = self.read(&s, device.as_ref())?;
            if let Some(wifi) = last.offer(wifi) {
                self.send(wifi)?;
            }
        }
    }

    /// Watching is kept; anything else can't be done and says so.
    fn command_while_down(&mut self, request: Request) -> Result<(), Closed> {
        match request {
            Request::Watch(on) => self.state.watching = on,
            Request::Do(_) => self.send(Wifi { error: Some(Problem::Unavailable), ..Wifi::default() })?,
        }
        Ok(())
    }

    fn unavailable(&mut self, e: &Error) -> Result<(), Closed> {
        self.send(Wifi { error: Some(Problem::Bus(e.to_string())), ..Wifi::default() })
    }
}

/// Why a join failed: from the active connection's reason for ending (0: none given), the device's
/// reason for failing (if it did), and whether the attempt ran out of time.
fn classify(reason: u32, device_reason: Option<u32>, timed_out: bool, ssid: String) -> Problem {
    if reason == ActiveReason::NoSecrets as u32 || device_reason == Some(DeviceReason::NoSecrets as u32) {
        Problem::WrongPassword { ssid }
    } else if reason == ActiveReason::NoAddress as u32 || device_reason == Some(DeviceReason::NoAddress as u32) {
        Problem::NoAddress { ssid }
    } else if timed_out {
        Problem::NoAnswer { ssid }
    } else {
        Problem::Refused { ssid, reason }
    }
}

/// A new connection's settings: the SSID, and the password as the access point's security wants it
/// (WPA2/WPA3 personal, or WEP).
fn new_connection<'a>(ssid: &'a str, password: Option<&'a str>, ap: Option<&Ap>) -> HashMap<&'static str, HashMap<&'static str, Value<'a>>> {
    let mut settings = HashMap::new();
    settings.insert("connection", HashMap::from([("type", Value::from("802-11-wireless")), ("id", Value::from(ssid))]));
    settings.insert("802-11-wireless", HashMap::from([("ssid", Value::from(ssid.as_bytes().to_vec())), ("mode", Value::from("infrastructure"))]));
    if let Some(password) = password {
        let security = ap.map_or(KEY_MGMT_PSK, |ap| ap.security);
        let wep = ap.is_some_and(|ap| ap.flags & AP_PRIVACY != 0 && ap.security == 0);
        let section = if wep {
            HashMap::from([("key-mgmt", Value::from("none")), ("wep-key0", Value::from(password)), ("wep-key-type", Value::from(1u32))])
        } else {
            // WPA3-only networks take SAE; everything else (and mixed ones) WPA-PSK.
            let key_mgmt = if security & KEY_MGMT_PSK == 0 && security & KEY_MGMT_SAE != 0 { "sae" } else { "wpa-psk" };
            HashMap::from([("key-mgmt", Value::from(key_mgmt)), ("psk", Value::from(password))])
        };
        settings.insert("802-11-wireless-security", section);
    }
    settings
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::indexing_slicing)]
mod tests {
    use super::*;

    fn ap(flags: u32, security: u32) -> Ap {
        Ap { path: OwnedObjectPath::try_from("/ap/1").unwrap(), ssid: "x".into(), strength: 50, flags, security }
    }

    #[test]
    fn a_failed_join_says_why() {
        let ssid = || "home".to_owned();
        let (no_secrets, no_address) = (ActiveReason::NoSecrets as u32, ActiveReason::NoAddress as u32);
        assert_eq!(classify(no_secrets, None, false, ssid()), Problem::WrongPassword { ssid: ssid() });
        assert_eq!(classify(0, Some(DeviceReason::NoSecrets as u32), false, ssid()), Problem::WrongPassword { ssid: ssid() }, "the device's word");
        assert_eq!(classify(no_address, None, true, ssid()), Problem::NoAddress { ssid: ssid() });
        assert_eq!(classify(0, Some(DeviceReason::NoAddress as u32), false, ssid()), Problem::NoAddress { ssid: ssid() });
        assert_eq!(classify(0, None, true, ssid()), Problem::NoAnswer { ssid: ssid() });
        assert_eq!(classify(3, Some(1), false, ssid()), Problem::Refused { ssid: ssid(), reason: 3 });
    }

    #[test]
    fn the_password_goes_where_the_security_wants_it() {
        let key_mgmt = |ap: Ap| {
            let s = new_connection("x", Some("pw"), Some(&ap));
            s["802-11-wireless-security"]["key-mgmt"].to_string()
        };
        assert_eq!(key_mgmt(ap(AP_PRIVACY, KEY_MGMT_PSK)), "\"wpa-psk\"");
        assert_eq!(key_mgmt(ap(AP_PRIVACY, KEY_MGMT_PSK | KEY_MGMT_SAE)), "\"wpa-psk\"", "mixed");
        assert_eq!(key_mgmt(ap(AP_PRIVACY, KEY_MGMT_SAE)), "\"sae\"");
        assert_eq!(key_mgmt(ap(AP_PRIVACY, 0)), "\"none\"", "WEP");
        assert!(!new_connection("x", None, None).contains_key("802-11-wireless-security"), "open");
        assert!(ap(0, KEY_MGMT_8021X).enterprise());
    }
}

#[cfg(test)]
mod live {
    use super::*;

    /// With NetworkManager running (a desktop): its Wi-Fi view reads without error; nothing is changed
    /// (run by hand: `--ignored --nocapture`).
    #[test]
    #[ignore]
    #[allow(clippy::unwrap_used)]
    fn reads_networkmanager() {
        let s = Session::connect().unwrap();
        let device = s.wifi_device().unwrap();
        let (tx, _rx) = std::sync::mpsc::channel();
        let mut thread = WifiThread { sink: tx, state: WifiState::default() };
        let wifi = thread.read(&s, device.as_ref()).unwrap();
        println!("{wifi:#?}");
        assert_eq!(wifi.device, device.is_some());
    }
}
