//! Wi-Fi through NetworkManager on the system bus. While the Wi-Fi tab is open, it reads the networks in
//! range, scanning every few seconds, and which are saved and which is connected. These are sent as
//! [`Reading::Wifi`] when they change. It also joins networks, either through a saved connection or a new
//! one with a password, forgets them, and turns the radio on and off.
//!
//! A new connection that fails to join is deleted again, so a mistyped password isn't kept. A join is
//! followed through the active connection NetworkManager makes for it rather than through the device,
//! because the device also goes through leaving the network it was on. Why a join failed is taken from
//! the device's failure, which leaving a network doesn't go through.

mod nm;

use std::collections::HashMap;
use std::fmt;
use std::io;
use std::sync::mpsc::Receiver;
use std::time::{Duration, Instant};

use clamshell_state::radios::wifi::{self, Command, Network, Problem, Wifi};
use clamshell_state::{Event, Reading};

use crate::os::dbus::{self, Bus, Path, Props, Value, value};
use crate::os::worker::{self, Changed, Ended, Next, Service, Worker, lost_on};
use crate::{Closed, Sink};
use nm::{AP_PRIVACY, KEY_MGMT_8021X, KEY_MGMT_PSK, KEY_MGMT_SAE, SERVICE};
use nm::{ActiveReason, ActiveState, DeviceReason, DeviceState, DeviceType};

/// How often the list is read while the tab is open, and how often a scan is asked for.
const READ_EVERY: Duration = Duration::from_secs(1);
const SCAN_EVERY: Duration = Duration::from_secs(10);
/// A join that hasn't finished by then has failed. NetworkManager gives up on getting an address after
/// 45 s and reports that itself first.
const JOIN_FOR: Duration = Duration::from_secs(60);
/// How long to wait before trying NetworkManager again.
const RETRY: Duration = Duration::from_secs(10);

/// What this thread is asked: to watch while the Wi-Fi tab is open, which scans now and keeps the list
/// fresh, and to carry out the state's commands.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Request {
    Watch(bool),
    Do(Command),
}

#[derive(Debug)]
enum Error {
    Bus(dbus::Error),
    NoDevice,
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::Bus(e) => write!(f, "{e}"),
            Error::NoDevice => write!(f, "no Wi-Fi device"),
        }
    }
}

impl From<dbus::Error> for Error {
    fn from(e: dbus::Error) -> Self {
        Error::Bus(e)
    }
}

lost_on!(Error: dbus::Error);

pub(crate) fn spawn(sink: impl Sink) -> io::Result<Worker<Request>> {
    let mut thread = WifiThread { sink, state: WifiState::default() };
    Worker::spawn(clamshell_state::Thread::Wifi.name(), move |commands| worker::supervise(&mut thread, commands, RETRY))
}

/// A connection being made.
struct Attempt {
    ssid: String,
    /// The connection we added for it, deleted again if the join fails.
    added: Option<Path<'static>>,
    /// The active connection NetworkManager made for it, and the device it is on.
    active: Path<'static>,
    device: Path<'static>,
    started: Instant,
    /// The device's reason for failing meanwhile, if it did, which tells why the join failed.
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
    object: Path<'static>,
    state: u32,
    reason: u32,
}

/// A connection to NetworkManager, subscribed to the state changes of every active connection and
/// device for as long as it lasts. A join's state changes pass too quickly to be polled.
struct Session {
    bus: Bus,
    /// The changes that came since the last look.
    active_changes: Vec<StateChange>,
    device_changes: Vec<StateChange>,
}

impl Session {
    fn connect() -> Result<Session, Error> {
        let mut bus = Bus::system()?;
        // Check that NetworkManager is there.
        bus.get::<bool>(SERVICE, nm::PATH, nm::MANAGER, "WirelessEnabled")?;
        for interface in [nm::ACTIVE, nm::DEVICE] {
            bus.add_match(&format!("type='signal',sender='{SERVICE}',interface='{interface}',member='StateChanged'"))?;
        }
        Ok(Session { bus, active_changes: Vec::new(), device_changes: Vec::new() })
    }

    /// Adds the state changes signalled since the last look to those kept.
    fn take_signals(&mut self) -> dbus::Result<()> {
        for message in self.bus.signals()? {
            let Some(object) = message.path().map(Path::into_static) else {
                continue;
            };
            match message.interface().as_deref() {
                // Active connections signal (state, reason), and devices (new state, old state, reason).
                Some(nm::ACTIVE) => match message.read2::<u32, u32>() {
                    Ok((state, reason)) => self.active_changes.push(StateChange { object, state, reason }),
                    Err(e) => log::warn!("wifi: reading {object}'s state change: {e}"),
                },
                Some(nm::DEVICE) => match message.read3::<u32, u32, u32>() {
                    Ok((state, _old, reason)) => self.device_changes.push(StateChange { object, state, reason }),
                    Err(e) => log::warn!("wifi: reading {object}'s state change: {e}"),
                },
                _ => {}
            }
        }
        Ok(())
    }

    fn get<T: ::dbus::arg::Arg + for<'z> ::dbus::arg::Get<'z>>(&mut self, path: &str, interface: &str, name: &str) -> dbus::Result<T> {
        self.bus.get(SERVICE, path, interface, name)
    }

    fn call<R: ::dbus::arg::ReadAll>(&mut self, path: &str, interface: &str, method: &str, args: impl ::dbus::arg::AppendAll) -> dbus::Result<R> {
        self.bus.call(SERVICE, path, interface, method, args)
    }

    fn wireless_enabled(&mut self) -> dbus::Result<bool> {
        self.get(nm::PATH, nm::MANAGER, "WirelessEnabled")
    }

    /// The first Wi-Fi device NetworkManager manages, or else the first Wi-Fi device, if any. A device it
    /// leaves alone, such as an access point's radio, isn't used to join.
    fn wifi_device(&mut self) -> dbus::Result<Option<Path<'static>>> {
        let (devices,): (Vec<Path<'static>>,) = self.call(nm::PATH, nm::MANAGER, "GetDevices", ())?;
        let mut unmanaged = None;
        for path in devices {
            if self.get::<u32>(&path, nm::DEVICE, "DeviceType")? != DeviceType::Wifi as u32 {
                continue;
            }
            if self.get::<u32>(&path, nm::DEVICE, "State")? != DeviceState::Unmanaged as u32 {
                return Ok(Some(path));
            }
            unmanaged.get_or_insert(path);
        }
        Ok(unmanaged)
    }

    fn access_points(&mut self, device: &Path<'static>) -> dbus::Result<Vec<Ap>> {
        let (paths,): (Vec<Path<'static>>,) = self.call(device, nm::WIRELESS, "GetAllAccessPoints", ())?;
        let mut aps = Vec::with_capacity(paths.len());
        for path in paths {
            // An access point can vanish between the list and its properties.
            let props = match self.bus.get_all(SERVICE, &path, nm::ACCESS_POINT) {
                Ok(props) => props,
                Err(e) => {
                    log::debug!("wifi: access point {path}: {e}");
                    continue;
                }
            };
            let p = Props(&props);
            let Some(ssid) = p.bytes("Ssid") else {
                continue;
            };
            let number = |key: &str| props.get(key).and_then(|v| v.0.as_u64()).unwrap_or(0);
            aps.push(Ap {
                ssid: String::from_utf8_lossy(&ssid).into_owned(),
                strength: u8::try_from(number("Strength")).unwrap_or(0),
                flags: u32::try_from(number("Flags")).unwrap_or(0),
                security: u32::try_from(number("WpaFlags") | number("RsnFlags")).unwrap_or(0),
                path,
            });
        }
        Ok(aps)
    }

    /// Saved Wi-Fi connections: (path, SSID).
    fn saved(&mut self) -> dbus::Result<Vec<(Path<'static>, String)>> {
        let (connections,): (Vec<Path<'static>>,) = self.call(nm::SETTINGS_PATH, nm::SETTINGS, "ListConnections", ())?;
        let mut out = Vec::new();
        for path in connections {
            let (settings,): (HashMap<String, dbus::PropMap>,) = self.call(&path, nm::CONNECTION, "GetSettings", ())?;
            if let Some(ssid) = settings.get("802-11-wireless").and_then(|w| Props(w).bytes("ssid")) {
                out.push((path, String::from_utf8_lossy(&ssid).into_owned()));
            }
        }
        Ok(out)
    }

    fn delete(&mut self, connection: &Path<'static>) -> dbus::Result<()> {
        self.call(connection, nm::CONNECTION, "Delete", ())
    }
}

/// An access point as NetworkManager has it.
struct Ap {
    path: Path<'static>,
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

    fn read(&mut self, s: &mut Session, device: Option<&Path<'static>>) -> dbus::Result<Wifi> {
        let st = &mut self.state;
        let enabled = s.wireless_enabled()?;
        let Some(device) = device else {
            return Ok(Wifi { present: false, on: enabled, error: st.error.clone(), ..Wifi::default() });
        };
        let state = s.get::<u32>(device, nm::DEVICE, "State")?;
        let active_ap = s.get::<Path<'static>>(device, nm::WIRELESS, "ActiveAccessPoint")?;
        let known = s.saved()?;
        let aps = s.access_points(device)?;
        let active_ssid = aps.iter().find(|ap| ap.path == active_ap && state == DeviceState::Activated as u32).map(|ap| ap.ssid.clone());
        self.follow_attempt(s, active_ssid.as_deref())?;
        let st = &self.state;
        let mut networks = aps
            .into_iter()
            .map(|ap| Network {
                saved: known.iter().any(|(_, ssid)| *ssid == ap.ssid),
                connected: active_ssid.as_deref() == Some(&ap.ssid),
                secure: ap.secure(),
                enterprise: ap.enterprise(),
                strength: ap.strength,
                ssid: ap.ssid,
            })
            .collect();
        wifi::sort(&mut networks);
        Ok(Wifi { present: true, on: enabled, networks, busy: st.attempt.as_ref().map(|a| a.ssid.clone()), error: st.error.clone() })
    }

    /// Ends the attempt once the network is connected, or with an error once it has failed.
    fn follow_attempt(&mut self, s: &mut Session, active: Option<&str>) -> dbus::Result<()> {
        let st = &mut self.state;
        // Take the signals since the last look, which belong to the attempt if there is one.
        s.take_signals()?;
        let active_changes = std::mem::take(&mut s.active_changes);
        let device_changes = std::mem::take(&mut s.device_changes);
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
        // If the active connection is gone without a signal, since it is removed once deactivated, the
        // join failed for an unknown reason.
        if ended.is_none() && s.get::<u32>(&attempt.active, nm::ACTIVE, "State").inspect_err(|e| log::debug!("wifi: {}: {e}", attempt.active)).is_err() {
            ended = Some(0);
        }
        let timed_out = attempt.started.elapsed() > JOIN_FOR;
        if ended.is_none() && !timed_out {
            return Ok(());
        }
        let problem = classify(ended.unwrap_or(0), attempt.device_failure, timed_out, attempt.ssid.clone());
        log::warn!("wifi: {problem:?}");
        if let Some(path) = &attempt.added
            && let Err(e) = s.delete(path)
        {
            log::warn!("wifi: removing the connection added for {}: {e}", attempt.ssid);
        }
        st.error = Some(problem);
        st.attempt = None;
        Ok(())
    }

    /// Carries out a request.
    fn apply(&mut self, s: &mut Session, request: Request) -> Result<(), Error> {
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
            Command::Enable(on) => s.bus.set(SERVICE, nm::PATH, nm::MANAGER, "WirelessEnabled", on)?,
            Command::Forget(ssid) => {
                for (path, _) in s.saved()?.into_iter().filter(|(_, s)| *s == ssid) {
                    s.delete(&path)?;
                }
                log::info!("wifi: forgot {ssid}");
            }
            Command::Connect { ssid, password } => {
                st.error = None;
                let device = s.wifi_device()?.ok_or(Error::NoDevice)?;
                // From here on, the state changes belong to this join.
                s.take_signals()?;
                s.active_changes.clear();
                s.device_changes.clear();
                let root = Path::from("/");
                let (added, active) = match (s.saved()?.into_iter().find(|(_, s)| *s == ssid), password) {
                    (Some((connection, _)), None) => {
                        let (active,): (Path<'static>,) = s.call(nm::PATH, nm::MANAGER, "ActivateConnection", (connection, device.clone(), root))?;
                        (None, active)
                    }
                    (_, password) => {
                        let aps = s.access_points(&device)?;
                        let ap = aps.iter().filter(|ap| ap.ssid == ssid).max_by_key(|ap| ap.strength);
                        let settings = new_connection(&ssid, password.as_ref().map(|p| p.0.as_str()), ap);
                        let ap_path = ap.map_or(root, |ap| ap.path.clone());
                        let (path, active): (Path<'static>, Path<'static>) =
                            s.call(nm::PATH, nm::MANAGER, "AddAndActivateConnection", (settings, device.clone(), ap_path))?;
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

    /// Reads every second while watching or joining, scanning every few seconds while watching, and
    /// carries out requests until NetworkManager fails or goes away.
    fn run_while_up(&mut self, mut s: Session, commands: &Receiver<Request>) -> Result<(), Ended<Error>> {
        let mut last = Changed::default();
        loop {
            let busy = self.state.watching || self.state.attempt.is_some();
            match worker::next(commands, busy.then_some(READ_EVERY)) {
                Next::Command(request) => {
                    if let Err(e) = self.apply(&mut s, request) {
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
                // A scan is refused while one runs or right after one, which is harmless.
                if let Err(e) = s.call::<()>(device, nm::WIRELESS, "RequestScan", (dbus::PropMap::new(),)) {
                    log::debug!("wifi: scan: {e}");
                }
            }
            let wifi = self.read(&mut s, device.as_ref())?;
            if let Some(wifi) = last.offer(wifi) {
                self.send(wifi)?;
            }
        }
    }

    /// Watching is remembered. Anything else can't be done, and an error says so.
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

/// Why a join failed, from the active connection's reason for ending (0 if none was given), the
/// device's reason for failing if it did, and whether the attempt ran out of time.
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

/// A new connection's settings: the SSID, and the password in the form the access point's security
/// wants (WPA2/WPA3 personal, or WEP).
fn new_connection(ssid: &str, password: Option<&str>, ap: Option<&Ap>) -> HashMap<&'static str, HashMap<&'static str, Value>> {
    let text = |s: &str| value(s.to_owned());
    let mut settings = HashMap::new();
    settings.insert("connection", HashMap::from([("type", text("802-11-wireless")), ("id", text(ssid))]));
    settings.insert("802-11-wireless", HashMap::from([("ssid", value(ssid.as_bytes().to_vec())), ("mode", text("infrastructure"))]));
    if let Some(password) = password {
        let security = ap.map_or(KEY_MGMT_PSK, |ap| ap.security);
        let wep = ap.is_some_and(|ap| ap.flags & AP_PRIVACY != 0 && ap.security == 0);
        let section = if wep {
            HashMap::from([("key-mgmt", text("none")), ("wep-key0", text(password)), ("wep-key-type", value(1u32))])
        } else {
            // WPA3-only networks take SAE. Everything else, including mixed networks, takes WPA-PSK.
            let key_mgmt = if security & KEY_MGMT_PSK == 0 && security & KEY_MGMT_SAE != 0 { "sae" } else { "wpa-psk" };
            HashMap::from([("key-mgmt", text(key_mgmt)), ("psk", text(password))])
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
        Ap { path: Path::from("/ap/1"), ssid: "x".into(), strength: 50, flags, security }
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
            s["802-11-wireless-security"]["key-mgmt"].0.as_str().unwrap_or_default().to_owned()
        };
        assert_eq!(key_mgmt(ap(AP_PRIVACY, KEY_MGMT_PSK)), "wpa-psk");
        assert_eq!(key_mgmt(ap(AP_PRIVACY, KEY_MGMT_PSK | KEY_MGMT_SAE)), "wpa-psk", "mixed");
        assert_eq!(key_mgmt(ap(AP_PRIVACY, KEY_MGMT_SAE)), "sae");
        assert_eq!(key_mgmt(ap(AP_PRIVACY, 0)), "none", "WEP");
        assert!(!new_connection("x", None, None).contains_key("802-11-wireless-security"), "open");
        assert!(ap(0, KEY_MGMT_8021X).enterprise());
    }
}

#[cfg(test)]
mod live {
    use super::*;

    /// With NetworkManager running, as on a desktop, its Wi-Fi view reads without error. Nothing is
    /// changed. Run it by hand with `--ignored --nocapture`.
    #[test]
    #[ignore = "needs NetworkManager"]
    #[allow(clippy::unwrap_used)]
    fn reads_networkmanager() {
        let mut s = Session::connect().unwrap();
        let device = s.wifi_device().unwrap();
        let (tx, _rx) = std::sync::mpsc::channel();
        let mut thread = WifiThread { sink: tx, state: WifiState::default() };
        let wifi = thread.read(&mut s, device.as_ref()).unwrap();
        println!("{wifi:#?}");
        assert_eq!(wifi.present, device.is_some());
    }
}
