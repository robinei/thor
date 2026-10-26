//! Our launches as systemd sees them: the user manager's scopes named `clamshell-run-*`, read over D-Bus
//! (org.freedesktop.systemd1). They are listed at start, and again whenever the manager signals that a
//! unit of ours is coming, starting or going.
//!
//! This is the truth for what runs. A scope lives as long as any of its processes, whatever its windows
//! do, and it survives a restart of ClamShell. When the session ends, they all stop, because their slices are
//! stopped after it: by thor-session on the device, and by the nested session's unit on the desktop.

use std::convert::Infallible;
use std::fmt;
use std::io;
use std::sync::mpsc::Receiver;
use std::time::Duration;

use clamshell_state::{Event, Instance};

use crate::os::dbus::{self, Bus, Message, Path, value};
use crate::os::worker::{self, Changed, Ended, Service, lost_on};
use crate::{Closed, Sink};

const PATTERNS: [&str; 2] = ["clamshell-run-*.service", "clamshell-run-*.scope"];

/// How long to wait before trying the user manager again.
const RETRY: Duration = Duration::from_secs(5);

pub(super) const SYSTEMD: &str = "org.freedesktop.systemd1";
const MANAGER_PATH: &str = "/org/freedesktop/systemd1";
const MANAGER: &str = "org.freedesktop.systemd1.Manager";

/// A row of ListUnitsByPatterns: name, description, load state, active state, sub state, following,
/// object path, job id, job type, job path.
type UnitRow = (String, String, String, String, String, String, Path<'static>, u32, String, Path<'static>);

/// Calls a method of the user manager.
pub(super) fn call<R: ::dbus::arg::ReadAll>(bus: &mut Bus, method: &str, args: impl ::dbus::arg::AppendAll) -> dbus::Result<R> {
    bus.call(SYSTEMD, MANAGER_PATH, MANAGER, method, args)
}

/// The units matching `patterns`, in any state.
fn units(bus: &mut Bus, patterns: &[&str]) -> dbus::Result<Vec<UnitRow>> {
    let empty: Vec<&str> = Vec::new();
    let (rows,): (Vec<UnitRow>,) = call(bus, "ListUnitsByPatterns", (empty, patterns.to_vec()))?;
    Ok(rows)
}

#[derive(Debug)]
pub enum Error {
    Bus(dbus::Error),
    /// The manager's signals ended because the bus connection closed.
    SignalsEnded,
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::Bus(e) => write!(f, "{e}"),
            Error::SignalsEnded => write!(f, "the bus closed"),
        }
    }
}

impl From<dbus::Error> for Error {
    fn from(e: dbus::Error) -> Self {
        Error::Bus(e)
    }
}

lost_on!(Error: dbus::Error);

/// The session bus connection shared by every one-off unit operation. Several threads use it, one at a
/// time. It connects again after the bus fails.
static BUS: std::sync::Mutex<dbus::Kept> = std::sync::Mutex::new(dbus::Kept::session());

/// Runs `call` on the user manager over the shared connection.
pub(super) fn with_manager<T>(call: impl FnOnce(&mut Bus) -> Result<T, Error>) -> Result<T, Error> {
    let mut kept = crate::os::lock(&BUS);
    let result = call(kept.get()?);
    if let Err(Error::Bus(e)) = &result {
        kept.failed(e);
    }
    result
}

/// Our running scopes, which are dock app instances and games, sorted by name.
fn list(bus: &mut Bus) -> dbus::Result<Vec<Instance>> {
    let rows = units(bus, &PATTERNS)?;
    let mut names: Vec<String> = rows.into_iter().filter(|row| !matches!(row.3.as_str(), "inactive" | "failed")).map(|row| row.0).collect();
    names.sort();
    Ok(names.iter().filter_map(|n| Instance::from_unit(n)).collect())
}

/// Sends [`Event::Units`] at start and after every change of our scopes, and connects again if the bus
/// goes. At start, after the first list, it sends [`Event::Frozen`] with our slices that are found
/// frozen. An earlier ClamShell froze them, when suspending or pausing a hidden game, and they are held
/// as suspended.
pub fn spawn(sink: impl Sink) -> io::Result<()> {
    worker::detached("units", move || worker::supervise_alone(&mut Units { sink, frozen: None, looked: false }, RETRY))
}

struct Units<S> {
    sink: S,
    /// Our slices found frozen, to be sent after the first list.
    frozen: Option<Vec<std::sync::Arc<str>>>,
    /// Whether frozen slices have been looked for. That happens once, at start.
    looked: bool,
}

impl<S: Sink> Service for Units<S> {
    type Command = Infallible;
    type Session = Bus;
    type Error = Error;
    const NAME: &'static str = "units";

    fn connect(&mut self) -> Result<Bus, Error> {
        let mut bus = Bus::session()?;
        bus.add_match(&format!("type='signal',sender='{SYSTEMD}',path='{MANAGER_PATH}',interface='{MANAGER}'"))?;
        call::<()>(&mut bus, "Subscribe", ())?;
        if !std::mem::replace(&mut self.looked, true) {
            let slices = units(&mut bus, &[&format!("{}*.slice", clamshell_state::session::instance::SLICE_PREFIX)])?;
            let frozen = slices.into_iter().filter(|row| {
                bus.get::<String>(SYSTEMD, &row.6, "org.freedesktop.systemd1.Unit", "FreezerState")
                    .inspect_err(|e| log::warn!("units: {}'s freezer state: {e}", row.0))
                    .is_ok_and(|s| s == "frozen")
            });
            self.frozen = Some(
                frozen.filter_map(|row| Some(row.0.strip_prefix(clamshell_state::session::instance::SLICE_PREFIX)?.strip_suffix(".slice")?.into())).collect(),
            );
        }
        Ok(bus)
    }

    /// Lists the scopes, and again at each signal about one of ours, until the bus closes.
    fn run_while_up(&mut self, mut bus: Bus, _: &Receiver<Infallible>) -> Result<(), Ended<Error>> {
        let mut last = Changed::default();
        if let Some(units) = last.offer(list(&mut bus)?) {
            self.sink.send(Event::Units(units))?;
        }
        if let Some(frozen) = self.frozen.take().filter(|f| !f.is_empty()) {
            log::info!("found frozen (held as suspended): {frozen:?}");
            self.sink.send(Event::Frozen(frozen))?;
        }
        loop {
            bus.wait(None).map_err(|_| Error::SignalsEnded)?;
            let signals = bus.signals().map_err(|_| Error::SignalsEnded)?;
            if signals.iter().filter_map(unit_of_signal).any(|name| Instance::is_ours(&name))
                && let Some(units) = last.offer(list(&mut bus)?)
            {
                self.sink.send(Event::Units(units))?;
            }
        }
    }

    fn command_while_down(&mut self, command: Infallible) -> Result<(), Closed> {
        match command {}
    }
}

/// The unit a manager signal is about, for the signals that change what runs: a unit loaded (UnitNew), a
/// unit gone (UnitRemoved) and a job done (JobRemoved). JobRemoved matters because a scope's start job
/// ends once the scope runs. At UnitNew the scope is still inactive, so a list made then can miss it.
fn unit_of_signal(message: &Message) -> Option<String> {
    match &*message.member()? {
        "UnitNew" | "UnitRemoved" => {
            message.read2::<String, Path>().inspect_err(|e| log::warn!("units: reading a unit signal: {e}")).ok().map(|(unit, _)| unit)
        }
        "JobRemoved" => {
            message.read4::<u32, Path, String, String>().inspect_err(|e| log::warn!("units: reading a job signal: {e}")).ok().map(|(_, _, unit, _)| unit)
        }
        _ => None,
    }
}

/// Whether the user manager has this unit loaded. `name` includes the suffix.
fn loaded(bus: &mut Bus, name: &str) -> bool {
    get_unit(bus, name).inspect_err(|e| log::debug!("{name} not loaded: {e}")).is_ok()
}

/// A loaded unit's object path.
pub(super) fn get_unit(bus: &mut Bus, name: &str) -> dbus::Result<Path<'static>> {
    let (path,): (Path<'static>,) = call(bus, "GetUnit", (name,))?;
    Ok(path)
}

/// Moves running processes into the scope `unit`, creating it (in `slice`, if given) if it doesn't
/// exist. This is for a program started by another program, such as a Steam game started by Steam's
/// launcher: in a scope of its own it is counted, frozen and ended as the launch, separately from its
/// starter. Anything it starts later is in the scope too.
pub fn adopt(unit: &str, slice: Option<&str>, pids: &[u32]) -> Result<(), Error> {
    with_manager(|bus| {
        if loaded(bus, unit) {
            call::<()>(bus, "AttachProcessesToUnit", (unit, "", pids.to_vec()))?;
            return Ok(());
        }
        // Delegated, because a user manager moves processes into a unit later only if it is.
        let mut properties = vec![("PIDs", value(pids.to_vec())), ("CollectMode", value("inactive-or-failed".to_owned())), ("Delegate", value(true))];
        if let Some(slice) = slice {
            properties.push(("Slice", value(slice.to_owned())));
        }
        let aux: Vec<(&str, Vec<(&str, dbus::Value)>)> = Vec::new();
        call::<(Path<'static>,)>(bus, "StartTransientUnit", (unit, "fail", properties, aux))?;
        Ok(())
    })
}

/// Thaws a launch's slice before it starts, because systemd starts no scope in a slice left frozen.
pub fn thaw_slice(slice: &str) {
    if let Err(e) = with_manager(|m| {
        thaw_now(m, slice);
        Ok(())
    }) {
        log::debug!("thawing {slice}: {e}");
    }
}

/// Ends `unit`, such as an entry's slice with all its launches. Normally it is stopped: systemd asks its
/// processes to end, then kills what is left after its timeout. With `force` they are killed at once.
pub fn end(unit: &str, force: bool) -> Result<(), Error> {
    const SIGKILL: i32 = 9;
    with_manager(|bus| {
        match force {
            true => call::<()>(bus, "KillUnit", (unit, "all", SIGKILL))?,
            false => drop(call::<(Path<'static>,)>(bus, "StopUnit", (unit, "replace"))?),
        }
        Ok(())
    })
}

/// Stops every running unit whose name matches `pattern`, a glob as `ListUnitsByPatterns` takes it.
pub fn stop_matching(pattern: &str) -> Result<(), Error> {
    with_manager(|bus| {
        for row in units(bus, &[pattern])? {
            if !matches!(row.3.as_str(), "inactive" | "failed") {
                call::<(Path<'static>,)>(bus, "StopUnit", (row.0.as_str(), "replace"))?;
            }
        }
        Ok(())
    })
}

/// Whether unit `unit` is running or on its way there.
pub fn is_active(unit: &str) -> Result<bool, Error> {
    with_manager(|bus| Ok(units(bus, &[unit])?.iter().any(|row| matches!(row.3.as_str(), "active" | "activating" | "reloading"))))
}

/// Thaws a unit if it is loaded. systemd does nothing if it isn't frozen.
fn thaw_now(bus: &mut Bus, unit: &str) {
    if loaded(bus, unit)
        && let Err(e) = call::<()>(bus, "ThawUnit", (unit,))
    {
        log::debug!("thawing {unit}: {e}");
    }
}

#[cfg(test)]
mod live {
    use super::*;
    use std::process::Command;
    use std::thread;
    use std::time::Instant;

    /// Needs a user session, so a desktop. Lists our scopes without changing anything. Run it by hand
    /// with `--ignored --nocapture`.
    #[test]
    #[ignore = "needs a user session"]
    #[allow(clippy::unwrap_used)]
    fn lists_our_scopes() {
        let mut bus = Bus::session().unwrap();
        println!("{:?}", list(&mut bus).unwrap());
    }

    /// Needs a user session, so a desktop. A launch's unit we start shows in the list and is gone once it
    /// ends. Starts a one-second `sleep` as a service of its own. Run it by hand with `--ignored`.
    #[test]
    #[ignore = "needs a user session; starts a service"]
    #[allow(clippy::unwrap_used)]
    fn a_launch_comes_and_goes() {
        let mut bus = Bus::session().unwrap();
        let instance = Instance { key: "unittest".into(), secs: u64::from(std::process::id()), n: 0 };
        let name = instance.name();
        let mut child = Command::new("systemd-run").args(["--user", "--quiet", "--collect", &format!("--unit={name}"), "--", "sleep", "1"]).spawn().unwrap();
        let start = Instant::now();
        while !list(&mut bus).unwrap().contains(&instance) {
            assert!(start.elapsed() < Duration::from_secs(5), "listed");
            thread::sleep(Duration::from_millis(50));
        }
        child.wait().unwrap();
        // It is gone from the list once its process has ended.
        let start = Instant::now();
        while list(&mut bus).unwrap().contains(&instance) {
            assert!(start.elapsed() < Duration::from_secs(5), "gone");
            thread::sleep(Duration::from_millis(50));
        }
    }
}
