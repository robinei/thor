//! Bluetooth through BlueZ (D-Bus, system bus): the adapter and its devices, sent as
//! [`Event::Bluetooth`] when they change (every few seconds; every second while the Bluetooth tab is
//! open, which also looks for new devices). Connecting a new device pairs, trusts and connects it; for
//! the pairing a "no input, no output" agent is registered (headphones and controllers pair without a
//! code), and unregistered after, so a desktop's own agent stays the default otherwise. Connecting (and
//! pairing, which can take long) runs on a worker of its own, so the list, the radio's switch and leaving
//! the tab don't wait for it; every call has a time limit.

use std::collections::HashMap;
use std::io;
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

use clamshell_core::Event;
use clamshell_core::bluetooth::{self, Bluetooth, Command, Device, Kind};
use zbus::blocking::{Connection, Proxy};
use zbus::zvariant::{OwnedObjectPath, OwnedValue};

use crate::Sink;

const BLUEZ: &str = "org.bluez";
const ADAPTER: &str = "org.bluez.Adapter1";
const DEVICE: &str = "org.bluez.Device1";
const AGENT_PATH: &str = "/org/clamshell/agent";

const READ_WATCHING: Duration = Duration::from_secs(1);
const READ_IDLE: Duration = Duration::from_secs(5);
/// How long a call may take: pairing waits for the device (BlueZ gives up within about half a minute).
const CALL_LIMIT: Duration = Duration::from_secs(45);

type Objects = HashMap<OwnedObjectPath, HashMap<String, HashMap<String, OwnedValue>>>;

/// What this side keeps between reads (shared with the connecting worker).
#[derive(Default)]
struct Side {
    watching: bool,
    busy: Option<String>,
    error: Option<String>,
}

type Shared = Arc<Mutex<Side>>;

fn side(shared: &Shared) -> std::sync::MutexGuard<'_, Side> {
    shared.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
}

pub fn spawn(sink: impl Sink, commands: Receiver<Command>) -> io::Result<()> {
    thread::Builder::new().name("bluetooth".into()).spawn(move || {
        let shared = Shared::default();
        loop {
            match run(&sink, &commands, &shared) {
                Ok(()) => return,
                Err(e) => {
                    log::debug!("bluetooth: {e}");
                    let error = side(&shared).error.clone();
                    if !sink.send(Event::Bluetooth(Bluetooth { error, ..Bluetooth::default() })) {
                        return;
                    }
                }
            }
            // BlueZ isn't there (or went away): try again in a while; what is asked meanwhile can't be done.
            match commands.recv_timeout(Duration::from_secs(10)) {
                Ok(Command::Watch(on)) => side(&shared).watching = on,
                Ok(command) => {
                    let error = format!("{}: Bluetooth isn't available", what_failed(&command));
                    side(&shared).error = Some(error.clone());
                    if !sink.send(Event::Bluetooth(Bluetooth { error: Some(error), ..Bluetooth::default() })) {
                        return;
                    }
                }
                Err(RecvTimeoutError::Timeout) => {}
                Err(RecvTimeoutError::Disconnected) => return,
            }
        }
    })?;
    Ok(())
}

fn connect_bus() -> zbus::Result<Connection> {
    zbus::blocking::connection::Builder::system()?.method_timeout(CALL_LIMIT).build()
}

fn run(sink: &impl Sink, commands: &Receiver<Command>, shared: &Shared) -> zbus::Result<()> {
    let conn = connect_bus()?;
    let manager = Proxy::new(&conn, BLUEZ, "/", "org.freedesktop.DBus.ObjectManager")?;
    let connecting = connecting_worker(conn.clone(), shared.clone())?;
    let mut last = None;
    // Discovery picks up where it was, after a reconnect.
    if side(shared).watching {
        discover(&conn, &manager, true);
    }
    loop {
        let objects: Objects = manager.call("GetManagedObjects", &())?;
        let bt = read(&objects, &side(shared));
        if last.as_ref() != Some(&bt) {
            if !sink.send(Event::Bluetooth(bt.clone())) {
                return Ok(());
            }
            last = Some(bt);
        }
        let wait = if side(shared).watching { READ_WATCHING } else { READ_IDLE };
        match commands.recv_timeout(wait) {
            Ok(command) => {
                // A new attempt: what went wrong before is old news.
                side(shared).error = None;
                if let Command::Connect(address) = command {
                    if connecting.send(address).is_err() {
                        return Err(zbus::Error::Failure("the connecting worker is gone".into()));
                    }
                    continue;
                }
                if let Err(e) = carry_out(&conn, &manager, &command, shared) {
                    log::warn!("bluetooth: {command:?}: {e}");
                    side(shared).error = Some(failure(&command, &e));
                }
            }
            Err(RecvTimeoutError::Timeout) => {}
            Err(RecvTimeoutError::Disconnected) => return Ok(()),
        }
    }
}

/// Connects (pairing and trusting a new device first) the devices it is sent, one at a time, showing
/// which in `busy` meanwhile and what went wrong in `error`.
fn connecting_worker(conn: Connection, shared: Shared) -> zbus::Result<Sender<String>> {
    let (tx, rx) = mpsc::channel::<String>();
    thread::Builder::new()
        .name("bluetooth-connect".into())
        .spawn(move || {
            for address in rx {
                side(&shared).busy = Some(address.clone());
                if let Err(e) = connect(&conn, &address) {
                    log::warn!("bluetooth: connecting {address}: {e}");
                    side(&shared).error = Some(failure(&Command::Connect(address.clone()), &e));
                }
                side(&shared).busy = None;
            }
        })
        .map_err(|e| zbus::Error::Failure(e.to_string()))?;
    Ok(tx)
}

fn connect(conn: &Connection, address: &str) -> zbus::Result<()> {
    let manager = Proxy::new(conn, BLUEZ, "/", "org.freedesktop.DBus.ObjectManager")?;
    let adapter = adapter_path(&manager)?;
    let d = Proxy::new(conn, BLUEZ, device_path(&adapter, address), DEVICE)?;
    if !d.get_property::<bool>("Paired")? {
        pair(conn, &d)?;
        d.set_property("Trusted", true).map_err(zbus::Error::from)?;
    }
    d.call::<_, _, ()>("Connect", &())?;
    log::info!("bluetooth: connected {address}");
    Ok(())
}

fn what_failed(command: &Command) -> &'static str {
    match command {
        Command::Connect(_) => "Couldn't connect",
        Command::Disconnect(_) => "Couldn't disconnect",
        Command::Forget(_) => "Couldn't forget it",
        Command::Power(_) => "Couldn't switch Bluetooth",
        Command::Watch(_) => "Couldn't look for devices",
    }
}

/// What went wrong, for the screen.
fn failure(command: &Command, e: &zbus::Error) -> String {
    let what = what_failed(command);
    // BlueZ's errors: "org.bluez.Error.AuthenticationFailed: ..." and the like.
    let reason = match e {
        zbus::Error::MethodError(name, detail, _) => detail.clone().unwrap_or_else(|| name.as_str().trim_start_matches("org.bluez.Error.").to_owned()),
        other => other.to_string(),
    };
    format!("{what}: {reason}")
}

fn string(props: &HashMap<String, OwnedValue>, key: &str) -> Option<String> {
    props.get(key).and_then(|v| String::try_from(v.try_clone().ok()?).ok())
}

fn flag(props: &HashMap<String, OwnedValue>, key: &str) -> bool {
    props.get(key).and_then(|v| bool::try_from(v).ok()).unwrap_or(false)
}

/// The first adapter and its devices; devices without a name are left out unless paired.
fn read(objects: &Objects, side: &Side) -> Bluetooth {
    let adapter = objects.iter().find_map(|(path, ifaces)| ifaces.get(ADAPTER).map(|props| (path, props)));
    let Some((adapter_path, adapter)) = adapter else {
        return Bluetooth { error: side.error.clone(), ..Bluetooth::default() };
    };
    let devices = objects
        .iter()
        .filter(|(path, _)| path.as_str().starts_with(adapter_path.as_str()))
        .filter_map(|(_, ifaces)| ifaces.get(DEVICE))
        .filter_map(|props| {
            let name = string(props, "Name");
            let paired = flag(props, "Paired");
            if name.is_none() && !paired {
                return None;
            }
            Some(Device {
                address: string(props, "Address")?,
                name: name.or_else(|| string(props, "Alias")).unwrap_or_default(),
                kind: Kind::from_icon(&string(props, "Icon").unwrap_or_default()),
                paired,
                connected: flag(props, "Connected"),
            })
        })
        .collect();
    Bluetooth {
        adapter: true,
        powered: flag(adapter, "Powered"),
        devices: bluetooth::sort(devices),
        busy: side.busy.clone(),
        error: side.error.clone(),
    }
}

/// The adapter's object path.
fn adapter_path(manager: &Proxy) -> zbus::Result<OwnedObjectPath> {
    let objects: Objects = manager.call("GetManagedObjects", &())?;
    objects.into_iter().find(|(_, i)| i.contains_key(ADAPTER)).map(|(p, _)| p).ok_or_else(|| zbus::Error::Failure("no adapter".into()))
}

/// A device's object path: the adapter's, then `dev_` and its address with underscores.
fn device_path(adapter: &OwnedObjectPath, address: &str) -> String {
    format!("{}/dev_{}", adapter.as_str(), address.replace(':', "_"))
}

fn discover(conn: &Connection, manager: &Proxy, on: bool) {
    let Ok(adapter) = adapter_path(manager) else {
        return;
    };
    let Ok(proxy) = Proxy::new(conn, BLUEZ, adapter.as_str().to_owned(), ADAPTER) else {
        return;
    };
    // Already (not) discovering: fine.
    let _ = proxy.call::<_, _, ()>(if on { "StartDiscovery" } else { "StopDiscovery" }, &());
}

fn carry_out(conn: &Connection, manager: &Proxy, command: &Command, shared: &Shared) -> zbus::Result<()> {
    let adapter = adapter_path(manager)?;
    let adapter_proxy = Proxy::new(conn, BLUEZ, adapter.as_str().to_owned(), ADAPTER)?;
    let device = |address: &str| Proxy::new(conn, BLUEZ, device_path(&adapter, address), DEVICE);
    match command {
        Command::Watch(on) => {
            side(shared).watching = *on;
            discover(conn, manager, *on);
        }
        Command::Power(on) => adapter_proxy.set_property("Powered", *on).map_err(zbus::Error::from)?,
        // (The connecting worker's.)
        Command::Connect(_) => {}
        Command::Disconnect(address) => device(address)?.call::<_, _, ()>("Disconnect", &())?,
        Command::Forget(address) => {
            let path = zbus::zvariant::ObjectPath::try_from(device_path(&adapter, address))?;
            adapter_proxy.call::<_, _, ()>("RemoveDevice", &(path,))?;
            log::info!("bluetooth: forgot {address}");
        }
    }
    Ok(())
}

/// Pairs with our agent registered for the time it takes.
fn pair(conn: &Connection, device: &Proxy) -> zbus::Result<()> {
    conn.object_server().at(AGENT_PATH, Agent)?;
    let agents = Proxy::new(conn, BLUEZ, "/org/bluez", "org.bluez.AgentManager1")?;
    let path = zbus::zvariant::ObjectPath::try_from(AGENT_PATH)?;
    agents.call::<_, _, ()>("RegisterAgent", &(&path, "NoInputNoOutput"))?;
    let paired = device.call::<_, _, ()>("Pair", &());
    let _ = agents.call::<_, _, ()>("UnregisterAgent", &(&path,));
    let _ = conn.object_server().remove::<Agent, _>(AGENT_PATH);
    paired
}

/// The pairing agent: no screen or keyboard to show or type a code with; what BlueZ asks to confirm is
/// confirmed (the user asked to pair).
struct Agent;

#[zbus::interface(name = "org.bluez.Agent1")]
impl Agent {
    fn release(&self) {}

    fn request_confirmation(&self, _device: OwnedObjectPath, _passkey: u32) {}

    fn request_authorization(&self, _device: OwnedObjectPath) {}

    fn authorize_service(&self, _device: OwnedObjectPath, _uuid: String) {}

    fn request_pin_code(&self, _device: OwnedObjectPath) -> zbus::fdo::Result<String> {
        Err(zbus::fdo::Error::Failed("no way to enter a PIN".into()))
    }

    fn request_passkey(&self, _device: OwnedObjectPath) -> zbus::fdo::Result<u32> {
        Err(zbus::fdo::Error::Failed("no way to enter a passkey".into()))
    }

    fn display_pin_code(&self, _device: OwnedObjectPath, _pincode: String) {}

    fn display_passkey(&self, _device: OwnedObjectPath, _passkey: u32, _entered: u16) {}

    fn cancel(&self) {}
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    #[test]
    fn device_paths_are_the_adapters_and_the_address() {
        let adapter = OwnedObjectPath::try_from("/org/bluez/hci0").unwrap();
        assert_eq!(device_path(&adapter, "AA:BB:CC:00:11:22"), "/org/bluez/hci0/dev_AA_BB_CC_00_11_22");
    }

    /// With BlueZ running (a desktop): its view reads without error (nothing is changed).
    #[test]
    fn reads_bluez() {
        let Ok(conn) = Connection::system() else {
            return;
        };
        let Ok(manager) = Proxy::new(&conn, BLUEZ, "/", "org.freedesktop.DBus.ObjectManager") else {
            return;
        };
        let Ok(objects) = manager.call::<_, _, Objects>("GetManagedObjects", &()) else {
            return;
        };
        let bt = read(&objects, &Side::default());
        assert_eq!(bt.adapter, objects.values().any(|i| i.contains_key(ADAPTER)));
    }
}
