//! Bluetooth through BlueZ (D-Bus, system bus): the adapter and its devices, sent as
//! [`Event::Bluetooth`] when they change (every few seconds; every second while the Bluetooth mode is
//! open, which also looks for new devices). Connecting a new device pairs, trusts and connects it; for
//! the pairing a "no input, no output" agent is registered (headphones and controllers pair without a
//! code), and unregistered after, so a desktop's own agent stays the default otherwise.

use std::collections::HashMap;
use std::io;
use std::sync::mpsc::{Receiver, RecvTimeoutError};
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

type Objects = HashMap<OwnedObjectPath, HashMap<String, HashMap<String, OwnedValue>>>;

/// What this thread keeps between reads.
#[derive(Default)]
struct Side {
    watching: bool,
    busy: Option<String>,
    error: Option<String>,
}

pub fn spawn(sink: impl Sink, commands: Receiver<Command>) -> io::Result<()> {
    thread::Builder::new().name("bluetooth".into()).spawn(move || {
        let mut side = Side::default();
        loop {
            match run(&sink, &commands, &mut side) {
                Ok(()) => return,
                Err(e) => {
                    log::debug!("bluetooth: {e}");
                    if !sink.send(Event::Bluetooth(Bluetooth::default())) {
                        return;
                    }
                }
            }
            match commands.recv_timeout(Duration::from_secs(10)) {
                Ok(Command::Watch(on)) => side.watching = on,
                Ok(_) | Err(RecvTimeoutError::Timeout) => {}
                Err(RecvTimeoutError::Disconnected) => return,
            }
        }
    })?;
    Ok(())
}

fn run(sink: &impl Sink, commands: &Receiver<Command>, side: &mut Side) -> zbus::Result<()> {
    let conn = Connection::system()?;
    let manager = Proxy::new(&conn, BLUEZ, "/", "org.freedesktop.DBus.ObjectManager")?;
    let mut last = None;
    // Discovery picks up where it was, after a reconnect.
    if side.watching {
        discover(&conn, &manager, true);
    }
    loop {
        let objects: Objects = manager.call("GetManagedObjects", &())?;
        let bt = read(&objects, side);
        if last.as_ref() != Some(&bt) {
            if !sink.send(Event::Bluetooth(bt.clone())) {
                return Ok(());
            }
            last = Some(bt);
        }
        let wait = if side.watching { READ_WATCHING } else { READ_IDLE };
        match commands.recv_timeout(wait) {
            Ok(command) => {
                if let Err(e) = carry_out(&conn, &manager, &command, side) {
                    log::warn!("bluetooth: {command:?}: {e}");
                    side.error = Some(failure(&command, &e));
                }
                side.busy = None;
            }
            Err(RecvTimeoutError::Timeout) => {}
            Err(RecvTimeoutError::Disconnected) => return Ok(()),
        }
    }
}

/// What went wrong, for the screen.
fn failure(command: &Command, e: &zbus::Error) -> String {
    let what = match command {
        Command::Connect(_) => "Couldn't connect",
        Command::Disconnect(_) => "Couldn't disconnect",
        Command::Forget(_) => "Couldn't forget it",
        Command::Power(_) => "Couldn't switch Bluetooth",
        Command::Watch(_) => "Couldn't look for devices",
    };
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

fn carry_out(conn: &Connection, manager: &Proxy, command: &Command, side: &mut Side) -> zbus::Result<()> {
    let adapter = adapter_path(manager)?;
    let adapter_proxy = Proxy::new(conn, BLUEZ, adapter.as_str().to_owned(), ADAPTER)?;
    let device = |address: &str| Proxy::new(conn, BLUEZ, device_path(&adapter, address), DEVICE);
    match command {
        Command::Watch(on) => {
            side.watching = *on;
            discover(conn, manager, *on);
        }
        Command::Power(on) => adapter_proxy.set_property("Powered", *on).map_err(zbus::Error::from)?,
        Command::Connect(address) => {
            side.error = None;
            side.busy = Some(address.clone());
            let d = device(address)?;
            if !d.get_property::<bool>("Paired")? {
                pair(conn, &d)?;
                d.set_property("Trusted", true).map_err(zbus::Error::from)?;
            }
            d.call::<_, _, ()>("Connect", &())?;
            log::info!("bluetooth: connected {address}");
        }
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
