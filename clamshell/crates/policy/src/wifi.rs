//! Wi-Fi through NetworkManager (D-Bus, system bus): while the Wi-Fi mode is open, the networks in
//! range (scanned every few seconds), which are saved and which is connected, sent as [`Event::Wifi`]
//! when they change; joining (a saved connection, or a new one with a password), forgetting, the radio
//! on and off. A new connection that fails to join is deleted again, so a mistyped password isn't kept.

use std::collections::HashMap;
use std::io;
use std::sync::mpsc::{self, Receiver, RecvTimeoutError};
use std::thread;
use std::time::{Duration, Instant};

use clamshell_core::Event;
use clamshell_core::wifi::{self, Command, Network, Wifi};
use zbus::blocking::{Connection, Proxy};
use zbus::zvariant::{ObjectPath, OwnedObjectPath, OwnedValue, Value};

use crate::Sink;

const NM: &str = "org.freedesktop.NetworkManager";
const NM_PATH: &str = "/org/freedesktop/NetworkManager";
const SETTINGS_PATH: &str = "/org/freedesktop/NetworkManager/Settings";
const DEVICE: &str = "org.freedesktop.NetworkManager.Device";
const WIRELESS: &str = "org.freedesktop.NetworkManager.Device.Wireless";
const ACCESS_POINT: &str = "org.freedesktop.NetworkManager.AccessPoint";
const SETTINGS: &str = "org.freedesktop.NetworkManager.Settings";
const CONNECTION: &str = "org.freedesktop.NetworkManager.Settings.Connection";

/// NM_DEVICE_TYPE_WIFI.
const TYPE_WIFI: u32 = 2;
/// NMDeviceState: disconnected, activated, failed; preparing to connecting are between.
const STATE_DISCONNECTED: u32 = 30;
const STATE_ACTIVATED: u32 = 100;
const STATE_FAILED: u32 = 120;
/// NMDeviceStateReason: no address from the network; the secrets (the password) missing or refused.
const REASON_NO_ADDRESS: u32 = 5;
const REASON_NO_SECRETS: u32 = 7;
/// NM80211ApFlags privacy; NM80211ApSecurityFlags key management PSK, 802.1X, SAE.
const AP_PRIVACY: u32 = 0x1;
const KEY_MGMT_PSK: u32 = 0x100;
const KEY_MGMT_8021X: u32 = 0x200;
const KEY_MGMT_SAE: u32 = 0x400;

/// How often the list is read while the mode is open, and how often a scan is asked for.
const READ_EVERY: Duration = Duration::from_secs(1);
const SCAN_EVERY: Duration = Duration::from_secs(10);
/// A join that hasn't finished by then has failed (NetworkManager gives up on an address after 45 s and
/// says so itself first).
const JOIN_FOR: Duration = Duration::from_secs(60);

/// A connection being made.
struct Attempt {
    ssid: String,
    /// The connection we added for it (deleted again if the join fails).
    added: Option<OwnedObjectPath>,
    started: Instant,
    /// The device left the disconnected state since (so going back to it means failure).
    under_way: bool,
    /// Why the device failed, from its state changes (failing passes too quickly to be polled).
    failures: Receiver<u32>,
}

/// What this thread keeps between reads.
#[derive(Default)]
struct Side {
    watching: bool,
    attempt: Option<Attempt>,
    error: Option<String>,
    last_scan: Option<Instant>,
}

pub fn spawn(sink: impl Sink, commands: Receiver<Command>) -> io::Result<()> {
    thread::Builder::new().name("wifi".into()).spawn(move || {
        let mut side = Side::default();
        loop {
            match run(&sink, &commands, &mut side) {
                Ok(()) => return,
                Err(e) => {
                    log::warn!("wifi: {e}");
                    if !sink.send(Event::Wifi(Wifi { error: Some(format!("NetworkManager: {e}")), ..Wifi::default() })) {
                        return;
                    }
                }
            }
            // Try again on the next command, or in a while.
            match commands.recv_timeout(Duration::from_secs(10)) {
                Ok(Command::Watch(on)) => side.watching = on,
                Ok(_) | Err(RecvTimeoutError::Timeout) => {}
                Err(RecvTimeoutError::Disconnected) => return,
            }
        }
    })?;
    Ok(())
}

fn proxy(conn: &Connection, path: &str, interface: &'static str) -> zbus::Result<Proxy<'static>> {
    zbus::blocking::proxy::Builder::<Proxy>::new(conn)
        .destination(NM)?
        .path(path.to_owned())?
        .interface(interface)?
        .cache_properties(zbus::proxy::CacheProperties::No)
        .build()
}

fn run(sink: &impl Sink, commands: &Receiver<Command>, side: &mut Side) -> zbus::Result<()> {
    let conn = Connection::system()?;
    let nm = proxy(&conn, NM_PATH, NM)?;
    let mut last = None;
    loop {
        let busy = side.watching || side.attempt.is_some();
        let command = if busy { commands.recv_timeout(READ_EVERY) } else { commands.recv().map_err(|_| RecvTimeoutError::Disconnected) };
        match command {
            Ok(command) => {
                if let Err(e) = carry_out(&conn, &nm, command, side) {
                    log::warn!("wifi: {e}");
                    side.error = Some(e.to_string());
                    side.attempt = None;
                }
            }
            Err(RecvTimeoutError::Timeout) => {}
            Err(RecvTimeoutError::Disconnected) => return Ok(()),
        }
        if !(side.watching || side.attempt.is_some()) {
            last = None;
            continue;
        }
        let device = wifi_device(&conn, &nm)?;
        if let Some(device) = &device
            && side.watching
            && side.last_scan.is_none_or(|t| t.elapsed() >= SCAN_EVERY)
        {
            side.last_scan = Some(Instant::now());
            // Refused while one runs, or right after one: harmless.
            let options: HashMap<&str, Value> = HashMap::new();
            let _ = proxy(&conn, device.as_str(), WIRELESS)?.call::<_, _, ()>("RequestScan", &(options,));
        }
        let wifi = read(&conn, &nm, device.as_ref(), side)?;
        if last.as_ref() != Some(&wifi) {
            if !sink.send(Event::Wifi(wifi.clone())) {
                return Ok(());
            }
            last = Some(wifi);
        }
    }
}

/// The first Wi-Fi device, if any.
fn wifi_device(conn: &Connection, nm: &Proxy) -> zbus::Result<Option<OwnedObjectPath>> {
    let devices: Vec<OwnedObjectPath> = nm.call("GetDevices", &())?;
    for path in devices {
        if proxy(conn, path.as_str(), DEVICE)?.get_property::<u32>("DeviceType")? == TYPE_WIFI {
            return Ok(Some(path));
        }
    }
    Ok(None)
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

fn access_points(conn: &Connection, device: &OwnedObjectPath) -> zbus::Result<Vec<Ap>> {
    let paths: Vec<OwnedObjectPath> = proxy(conn, device.as_str(), WIRELESS)?.call("GetAllAccessPoints", &())?;
    let mut aps = Vec::with_capacity(paths.len());
    for path in paths {
        let ap = proxy(conn, path.as_str(), ACCESS_POINT)?;
        // An access point can vanish between the list and its properties.
        let Ok(ssid) = ap.get_property::<Vec<u8>>("Ssid") else {
            continue;
        };
        aps.push(Ap {
            ssid: String::from_utf8_lossy(&ssid).into_owned(),
            strength: ap.get_property("Strength").unwrap_or(0),
            flags: ap.get_property("Flags").unwrap_or(0),
            security: ap.get_property::<u32>("WpaFlags").unwrap_or(0) | ap.get_property::<u32>("RsnFlags").unwrap_or(0),
            path,
        });
    }
    Ok(aps)
}

/// Saved Wi-Fi connections: (path, SSID).
fn saved(conn: &Connection) -> zbus::Result<Vec<(OwnedObjectPath, String)>> {
    let paths: Vec<OwnedObjectPath> = proxy(conn, SETTINGS_PATH, SETTINGS)?.call("ListConnections", &())?;
    let mut out = Vec::new();
    for path in paths {
        let settings: HashMap<String, HashMap<String, OwnedValue>> = proxy(conn, path.as_str(), CONNECTION)?.call("GetSettings", &())?;
        let ssid = settings.get("802-11-wireless").and_then(|w| w.get("ssid")).and_then(|v| Vec::<u8>::try_from(v.try_clone().ok()?).ok());
        if let Some(ssid) = ssid {
            out.push((path, String::from_utf8_lossy(&ssid).into_owned()));
        }
    }
    Ok(out)
}

fn read(conn: &Connection, nm: &Proxy, device: Option<&OwnedObjectPath>, side: &mut Side) -> zbus::Result<Wifi> {
    let enabled = nm.get_property::<bool>("WirelessEnabled")?;
    let Some(device) = device else {
        return Ok(Wifi { device: false, enabled, error: side.error.clone(), ..Wifi::default() });
    };
    let dev = proxy(conn, device.as_str(), DEVICE)?;
    let state: u32 = dev.get_property("State")?;
    let active_ap: OwnedObjectPath = proxy(conn, device.as_str(), WIRELESS)?.get_property("ActiveAccessPoint")?;
    let known = saved(conn)?;
    let aps = access_points(conn, device)?;
    let active_ssid = aps.iter().find(|ap| ap.path == active_ap && state == STATE_ACTIVATED).map(|ap| ap.ssid.clone());
    follow_attempt(conn, &dev, state, active_ssid.as_deref(), side)?;
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
        connecting: side.attempt.as_ref().map(|a| a.ssid.clone()),
        error: side.error.clone(),
    })
}

/// Ends the attempt once the network is connected, or with an error once it has failed.
fn follow_attempt(conn: &Connection, dev: &Proxy, state: u32, active: Option<&str>, side: &mut Side) -> zbus::Result<()> {
    let Some(attempt) = side.attempt.as_mut() else {
        return Ok(());
    };
    if active == Some(attempt.ssid.as_str()) {
        log::info!("wifi: joined {}", attempt.ssid);
        side.attempt = None;
        return Ok(());
    }
    if state != STATE_DISCONNECTED {
        attempt.under_way = true;
    }
    let failed = state == STATE_FAILED || (attempt.under_way && state == STATE_DISCONNECTED);
    let timed_out = attempt.started.elapsed() > JOIN_FOR;
    let failed_now = attempt.failures.try_recv().ok();
    let failed = failed || failed_now.is_some();
    if !(failed || timed_out) {
        return Ok(());
    }
    let reason = failed_now.unwrap_or_else(|| dev.get_property::<(u32, u32)>("StateReason").map_or(0, |r| r.1));
    let message = if reason == REASON_NO_SECRETS {
        format!("Couldn't join {}: wrong password?", attempt.ssid)
    } else if reason == REASON_NO_ADDRESS {
        format!("Joined {}, but it gave no address", attempt.ssid)
    } else if timed_out {
        format!("Couldn't join {}: no answer", attempt.ssid)
    } else {
        format!("Couldn't join {} (NetworkManager reason {reason})", attempt.ssid)
    };
    log::warn!("wifi: {message}");
    if let Some(path) = &attempt.added {
        let _ = proxy(conn, path.as_str(), CONNECTION)?.call::<_, _, ()>("Delete", &());
    }
    side.error = Some(message);
    side.attempt = None;
    Ok(())
}

fn carry_out(conn: &Connection, nm: &Proxy, command: Command, side: &mut Side) -> zbus::Result<()> {
    match command {
        Command::Watch(on) => {
            side.watching = on;
            if on {
                side.last_scan = None;
            }
        }
        Command::Enable(on) => nm.set_property("WirelessEnabled", on).map_err(zbus::Error::from)?,
        Command::Forget(ssid) => {
            for (path, _) in saved(conn)?.into_iter().filter(|(_, s)| *s == ssid) {
                proxy(conn, path.as_str(), CONNECTION)?.call::<_, _, ()>("Delete", &())?;
            }
            log::info!("wifi: forgot {ssid}");
        }
        Command::Connect { ssid, password } => {
            side.error = None;
            let Some(device) = wifi_device(conn, nm)? else {
                return Err(zbus::Error::Failure("no Wi-Fi device".into()));
            };
            let root = ObjectPath::try_from("/")?;
            let failures = watch_failures(conn, &device)?;
            let added = match (saved(conn)?.into_iter().find(|(_, s)| *s == ssid), password) {
                (Some((connection, _)), None) => {
                    let _: OwnedObjectPath = nm.call("ActivateConnection", &(connection, &device, &root))?;
                    None
                }
                (_, password) => {
                    let aps = access_points(conn, &device)?;
                    let ap = aps.iter().filter(|ap| ap.ssid == ssid).max_by_key(|ap| ap.strength);
                    let settings = new_connection(&ssid, password.as_deref(), ap);
                    let ap_path = ap.map_or_else(|| root.clone(), |ap| ap.path.as_ref().clone());
                    let (path, _active): (OwnedObjectPath, OwnedObjectPath) = nm.call("AddAndActivateConnection", &(settings, &device, ap_path))?;
                    Some(path)
                }
            };
            log::info!("wifi: joining {ssid}");
            side.attempt = Some(Attempt { ssid, added, started: Instant::now(), under_way: false, failures });
        }
    }
    Ok(())
}

/// The reasons the device fails for, from its StateChanged signals, until it is connected, has failed
/// or is back to disconnected (subscribed before the join starts, so none is missed).
fn watch_failures(conn: &Connection, device: &OwnedObjectPath) -> zbus::Result<Receiver<u32>> {
    let signals = proxy(conn, device.as_str(), DEVICE)?.receive_signal("StateChanged")?;
    let (tx, rx) = mpsc::channel();
    thread::Builder::new()
        .name("wifi-join".into())
        .spawn(move || {
            for message in signals {
                let Ok((new, _old, reason)) = message.body().deserialize::<(u32, u32, u32)>() else {
                    continue;
                };
                if new == STATE_FAILED {
                    let _ = tx.send(reason);
                }
                // Joined, failed, or given up (back to disconnected): done.
                if matches!(new, STATE_FAILED | STATE_ACTIVATED | STATE_DISCONNECTED) {
                    return;
                }
            }
        })
        .map_err(|e| zbus::Error::Failure(e.to_string()))?;
    Ok(rx)
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

    /// With NetworkManager running (a desktop): its Wi-Fi view reads without error (nothing is changed).
    #[test]
    fn reads_networkmanager() {
        let Ok(conn) = Connection::system() else {
            return;
        };
        let Ok(nm) = proxy(&conn, NM_PATH, NM) else {
            return;
        };
        if nm.get_property::<bool>("WirelessEnabled").is_err() {
            return;
        }
        let device = wifi_device(&conn, &nm).unwrap();
        let wifi = read(&conn, &nm, device.as_ref(), &mut Side::default()).unwrap();
        assert_eq!(wifi.device, device.is_some());
    }
}
