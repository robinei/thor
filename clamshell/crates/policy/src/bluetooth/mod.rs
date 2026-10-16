//! Bluetooth through BlueZ (D-Bus, system bus): the adapter and its devices, sent as
//! [`Reading::Bluetooth`] when they change (every few seconds; every second while the Bluetooth tab is
//! open, which also looks for new devices). Connecting a new device pairs, trusts and connects it; for
//! the pairing a "no input, no output" agent is registered (headphones and controllers pair without a
//! code), and unregistered after, so a desktop's own agent stays the default otherwise. Connecting (and
//! pairing, which can take long) runs on a worker of its own, so the list, the radio's switch and leaving
//! the tab don't wait for it; every call has a time limit.

mod bluez;

use std::fmt;
use std::io;
use std::sync::mpsc::Receiver;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::Duration;

use clamshell_core::bluetooth::{self, Action, Bluetooth, Command, Device, Kind, Problem, Reason};
use clamshell_core::{Event, Reading};
use zbus::blocking::Connection;
use zbus::zvariant::{ObjectPath, OwnedObjectPath};

use crate::dbus::{self, Props};
use crate::worker::{self, Changed, Ended, Next, Service, Worker, lost_on};
use crate::{Closed, Sink};
use bluez::{ADAPTER, AGENT_PATH, AdapterProxy, Agent, AgentManagerProxy, DEVICE, DeviceProxy, ManagedObjects, ObjectsProxy, device_path};

const READ_WATCHING: Duration = Duration::from_secs(1);
const READ_IDLE: Duration = Duration::from_secs(5);
/// How long a call may take: pairing waits for the device (BlueZ gives up within about half a minute).
const CALL_LIMIT: Duration = Duration::from_secs(45);
/// How long to wait before trying BlueZ again.
const RETRY: Duration = Duration::from_secs(10);

/// What this thread is asked: to watch while the Bluetooth tab is open (look for devices), and the
/// state's commands.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Request {
    Watch(bool),
    Do(Command),
}

impl Request {
    fn action(&self) -> Action {
        match self {
            Request::Watch(_) => Action::Watch,
            Request::Do(command) => command.action(),
        }
    }
}

#[derive(Debug)]
enum Error {
    Bus(zbus::Error),
    /// The connecting worker couldn't be started, or has ended.
    Worker(io::Error),
    NoAdapter,
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::Bus(e) => write!(f, "{e}"),
            Error::Worker(e) => write!(f, "the connecting worker: {e}"),
            Error::NoAdapter => write!(f, "no adapter"),
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
        Error::Worker(e)
    }
}

lost_on!(Error: zbus::Error, io::Error);

/// What went wrong doing `action`, for the screen.
fn failure(action: Action, e: &Error) -> Problem {
    // BlueZ's errors: "org.bluez.Error.AuthenticationFailed: ..." and the like.
    let reason = match e {
        Error::Bus(zbus::Error::MethodError(name, detail, _)) => {
            detail.clone().unwrap_or_else(|| name.as_str().trim_start_matches("org.bluez.Error.").to_owned())
        }
        other => other.to_string(),
    };
    Problem { action, reason: Reason::Bus(reason) }
}

/// What this thread keeps between reads (shared with the connecting worker), and across reconnecting.
#[derive(Default)]
struct BtState {
    watching: bool,
    busy: Option<String>,
    error: Option<Problem>,
}

#[derive(Clone, Default)]
struct Shared(Arc<Mutex<BtState>>);

impl Shared {
    fn lock(&self) -> MutexGuard<'_, BtState> {
        self.0.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

pub fn spawn(sink: impl Sink) -> io::Result<Worker<Request>> {
    Worker::spawn("bluetooth", move |commands| worker::supervise(&mut BtThread { sink, shared: Shared::default() }, &commands, RETRY))
}

struct BtThread<S> {
    sink: S,
    shared: Shared,
}

/// A connection to BlueZ, and the worker that connects devices through it.
struct Session {
    conn: Connection,
    objects: ObjectsProxy<'static>,
    connecting: Worker<String>,
}

impl Session {
    fn adapter_path(&self) -> Result<OwnedObjectPath, Error> {
        adapter_path(&self.objects)
    }

    fn adapter(&self) -> Result<AdapterProxy<'static>, Error> {
        Ok(dbus::proxy(&self.conn, self.adapter_path()?.as_str())?)
    }

    /// Starts or stops looking for devices (already doing so is fine).
    fn discover(&self, on: bool) {
        if let Ok(adapter) = self.adapter() {
            let _ = if on { adapter.start_discovery() } else { adapter.stop_discovery() };
        }
    }
}

/// The adapter's object path.
fn adapter_path(objects: &ObjectsProxy) -> Result<OwnedObjectPath, Error> {
    let all = objects.get_managed_objects()?;
    first_adapter(&all).map(|(path, _)| path.clone()).ok_or(Error::NoAdapter)
}

/// The adapter used: with several (a USB dongle besides the built-in one), always the same, the one
/// with the lowest path (hci0 before hci1); BlueZ lists them in no order.
fn first_adapter(objects: &ManagedObjects) -> Option<(&OwnedObjectPath, Props<'_>)> {
    objects.iter().filter_map(|(path, ifaces)| ifaces.get(ADAPTER).map(|props| (path, Props(props)))).min_by(|a, b| a.0.as_str().cmp(b.0.as_str()))
}

/// Connects (pairing and trusting a new device first) the devices it is sent, one at a time, showing
/// which in `busy` meanwhile and what went wrong in `error`.
fn connecting_worker(conn: Connection, shared: Shared) -> io::Result<Worker<String>> {
    Worker::spawn("bluetooth-connect", move |addresses: Receiver<String>| {
        for address in addresses {
            shared.lock().busy = Some(address.clone());
            if let Err(e) = connect(&conn, &address) {
                log::warn!("bluetooth: connecting {address}: {e}");
                shared.lock().error = Some(failure(Action::Connect, &e));
            }
            shared.lock().busy = None;
        }
    })
}

fn connect(conn: &Connection, address: &str) -> Result<(), Error> {
    let adapter = adapter_path(&dbus::default_proxy(conn)?)?;
    let device: DeviceProxy = dbus::proxy(conn, &device_path(&adapter, address))?;
    if !device.paired()? {
        pair(conn, &device)?;
        device.set_trusted(true)?;
    }
    device.connect()?;
    log::info!("bluetooth: connected {address}");
    Ok(())
}

/// Pairs with our agent registered for the time it takes.
fn pair(conn: &Connection, device: &DeviceProxy) -> zbus::Result<()> {
    conn.object_server().at(AGENT_PATH, Agent)?;
    let agents: AgentManagerProxy = dbus::default_proxy(conn)?;
    let path = ObjectPath::try_from(AGENT_PATH)?;
    agents.register_agent(&path, "NoInputNoOutput")?;
    let paired = device.pair();
    let _ = agents.unregister_agent(&path);
    let _ = conn.object_server().remove::<Agent, _>(AGENT_PATH);
    paired
}

/// The first adapter and its devices; devices without a name are left out unless paired.
fn read(objects: &ManagedObjects, state: &BtState) -> Bluetooth {
    let adapter = first_adapter(objects);
    let Some((adapter_path, adapter)) = adapter else {
        return Bluetooth { error: state.error.clone(), ..Bluetooth::default() };
    };
    let devices = objects
        .iter()
        .filter(|(path, _)| path.as_str().starts_with(adapter_path.as_str()))
        .filter_map(|(_, ifaces)| ifaces.get(DEVICE).map(Props))
        .filter_map(|props| {
            let name = props.str("Name");
            let paired = props.flag("Paired");
            if name.is_none() && !paired {
                return None;
            }
            Some(Device {
                address: props.str("Address")?,
                name: name.or_else(|| props.str("Alias")).unwrap_or_default(),
                kind: Kind::from_icon(&props.str("Icon").unwrap_or_default()),
                paired,
                connected: props.flag("Connected"),
            })
        })
        .collect();
    Bluetooth { adapter: true, powered: adapter.flag("Powered"), devices: bluetooth::sort(devices), busy: state.busy.clone(), error: state.error.clone() }
}

impl<S: Sink> BtThread<S> {
    fn send(&self, bt: Bluetooth) -> Result<(), Closed> {
        self.sink.send(Event::Reading(Reading::Bluetooth(bt)))
    }

    /// Carries out a request. Every request but connecting (the worker's, which finds the adapter
    /// itself) needs the adapter, and fails without one.
    fn apply(&self, s: &Session, request: &Request) -> Result<(), Error> {
        match request {
            Request::Do(Command::Connect(address)) => {
                s.connecting.try_send(address.clone()).map_err(|_| io::Error::other("it has ended"))?;
            }
            Request::Watch(on) => {
                s.adapter_path()?;
                self.shared.lock().watching = *on;
                s.discover(*on);
            }
            Request::Do(Command::Power(on)) => s.adapter()?.set_powered(*on)?,
            Request::Do(Command::Disconnect(address)) => {
                dbus::proxy::<DeviceProxy>(&s.conn, &device_path(&s.adapter_path()?, address))?.disconnect()?;
            }
            Request::Do(Command::Forget(address)) => {
                let device = ObjectPath::try_from(device_path(&s.adapter_path()?, address))?;
                s.adapter()?.remove_device(&device)?;
                log::info!("bluetooth: forgot {address}");
            }
        }
        Ok(())
    }
}

impl<S: Sink> Service for BtThread<S> {
    type Command = Request;
    type Session = Session;
    type Error = Error;
    const NAME: &'static str = "bluetooth";

    fn connect(&mut self) -> Result<Session, Error> {
        let conn = dbus::system(Some(CALL_LIMIT))?;
        let objects: ObjectsProxy = dbus::default_proxy(&conn)?;
        // BlueZ is there.
        objects.get_managed_objects()?;
        let connecting = connecting_worker(conn.clone(), self.shared.clone())?;
        Ok(Session { conn, objects, connecting })
    }

    /// Reads every few seconds (every second while watching) and carries out requests, until BlueZ fails
    /// or goes.
    fn run_while_up(&mut self, s: Session, commands: &Receiver<Request>) -> Result<(), Ended<Error>> {
        let mut last = Changed::default();
        // Discovery picks up where it was, after a reconnect.
        if self.shared.lock().watching {
            s.discover(true);
        }
        loop {
            let bt = read(&s.objects.get_managed_objects()?, &self.shared.lock());
            if let Some(bt) = last.offer(bt) {
                self.send(bt)?;
            }
            let every = if self.shared.lock().watching { READ_WATCHING } else { READ_IDLE };
            match worker::next(commands, Some(every)) {
                Next::Command(request) => {
                    // A new attempt: what went wrong before is old news.
                    self.shared.lock().error = None;
                    match self.apply(&s, &request) {
                        Ok(()) => {}
                        // Without its worker, connecting can't go on.
                        Err(e @ Error::Worker(_)) => return Err(Ended::Lost(e)),
                        Err(e) => {
                            log::warn!("bluetooth: {request:?}: {e}");
                            self.shared.lock().error = Some(failure(request.action(), &e));
                        }
                    }
                }
                Next::Elapsed => {}
                Next::Disconnected => return Ok(()),
            }
        }
    }

    /// Watching is kept; anything else can't be done and says so.
    fn command_while_down(&mut self, request: Request) -> Result<(), Closed> {
        if let Request::Watch(on) = request {
            self.shared.lock().watching = on;
            return Ok(());
        }
        let error = Problem { action: request.action(), reason: Reason::Unavailable };
        self.shared.lock().error = Some(error.clone());
        self.send(Bluetooth { error: Some(error), ..Bluetooth::default() })
    }

    fn unavailable(&mut self, _: &Error) -> Result<(), Closed> {
        let error = self.shared.lock().error.clone();
        self.send(Bluetooth { error, ..Bluetooth::default() })
    }

    /// Without BlueZ on the bus (no Bluetooth on this machine) there is nothing to warn about.
    fn expected(e: &Error) -> bool {
        matches!(e, Error::Bus(e) if dbus::is_absent(e))
    }
}

#[cfg(test)]
mod live {
    use super::*;

    /// With BlueZ running (a desktop): its view reads without error; nothing is changed (run by hand:
    /// `--ignored --nocapture`).
    #[test]
    #[ignore]
    #[allow(clippy::unwrap_used)]
    fn reads_bluez() {
        let conn = dbus::system(None).unwrap();
        let objects = dbus::default_proxy::<ObjectsProxy>(&conn).unwrap().get_managed_objects().unwrap();
        let bt = read(&objects, &BtState::default());
        println!("{bt:#?}");
        assert_eq!(bt.adapter, objects.values().any(|i| i.contains_key(ADAPTER)));
    }
}
