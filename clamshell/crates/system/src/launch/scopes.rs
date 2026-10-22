//! Our launches as systemd sees them: the user manager's scopes named `clamshell-run-*` (D-Bus,
//! org.freedesktop.systemd1), listed at start and again whenever it signals a unit of ours coming, starting
//! or going. This is the truth for what runs: a scope lives as long as any of its processes, whatever its
//! windows do, and survives a restart of ClamShell. A session's sway ending stops them all (the device's
//! uwsm, the desktop's nested sway's unit: their slices are stopped after it).

use std::convert::Infallible;
use std::fmt;
use std::io;
use std::sync::mpsc::Receiver;
use std::thread;
use std::time::{Duration, Instant};

use clamshell_core::{Event, Instance};

use crate::os::dbus::{self, Bus, Message, Path, value};
use crate::os::worker::{self, Changed, Ended, Service, lost_on};
use crate::{Closed, Sink};

const PATTERNS: [&str; 1] = ["clamshell-run-*.scope"];

/// How long to wait before trying the user manager again.
const RETRY: Duration = Duration::from_secs(5);

pub(super) const SYSTEMD: &str = "org.freedesktop.systemd1";
const MANAGER_PATH: &str = "/org/freedesktop/systemd1";
const MANAGER: &str = "org.freedesktop.systemd1.Manager";

/// ListUnitsByPatterns' rows: name, description, load state, active state, sub state, following,
/// object path, job id, job type, job path.
type UnitRow = (String, String, String, String, String, String, Path<'static>, u32, String, Path<'static>);

/// A method of the user manager's.
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
    /// The manager's signals ended: the bus connection closed.
    SignalsEnded,
    /// A scope we waited for didn't appear in time.
    DidNotStart(String),
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::Bus(e) => write!(f, "{e}"),
            Error::SignalsEnded => write!(f, "the bus closed"),
            Error::DidNotStart(unit) => write!(f, "{unit} didn't start"),
        }
    }
}

impl From<dbus::Error> for Error {
    fn from(e: dbus::Error) -> Self {
        Error::Bus(e)
    }
}

lost_on!(Error: dbus::Error);

/// The session bus, connected once for every one-off unit operation (made from several threads, one at
/// a time); connected again after the bus failed.
static BUS: std::sync::Mutex<dbus::Kept> = std::sync::Mutex::new(dbus::Kept::session());

/// `call` on the user manager, over the shared connection.
pub(super) fn with_manager<T>(call: impl FnOnce(&mut Bus) -> Result<T, Error>) -> Result<T, Error> {
    let mut kept = crate::os::lock(&BUS);
    let result = call(kept.get()?);
    if let Err(Error::Bus(e)) = &result {
        kept.failed(e);
    }
    result
}

/// Our scopes that run (dock app instances and games), in the order of their names.
fn list(bus: &mut Bus) -> dbus::Result<Vec<Instance>> {
    let rows = units(bus, &PATTERNS)?;
    let mut names: Vec<String> = rows.into_iter().filter(|row| !matches!(row.3.as_str(), "inactive" | "failed")).map(|row| row.0).collect();
    names.sort();
    Ok(names.iter().filter_map(|n| Instance::from_unit(n)).collect())
}

/// Sends [`Event::Units`] at start and after every change of our scopes; connects again if the bus goes.
/// At start, after the first, [`Event::Frozen`]: our slices found frozen (by a ClamShell before this
/// one: suspended, or paused hidden), held as suspended.
pub fn spawn(sink: impl Sink) -> io::Result<()> {
    worker::detached("units", move || worker::supervise_alone(&mut Units { sink, frozen: None, looked: false }, RETRY))
}

struct Units<S> {
    sink: S,
    /// Our slices found frozen, to be told after the first list.
    frozen: Option<Vec<std::sync::Arc<str>>>,
    /// They have been looked for (once, at start).
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
            let slices = units(&mut bus, &[&format!("{}*.slice", clamshell_core::session::instance::SLICE_PREFIX)])?;
            let frozen = slices
                .into_iter()
                .filter(|row| bus.get::<String>(SYSTEMD, &row.6, "org.freedesktop.systemd1.Unit", "FreezerState").is_ok_and(|s| s == "frozen"));
            self.frozen = Some(
                frozen.filter_map(|row| Some(row.0.strip_prefix(clamshell_core::session::instance::SLICE_PREFIX)?.strip_suffix(".slice")?.into())).collect(),
            );
        }
        Ok(bus)
    }

    /// Lists the scopes, then again at each signal about one of ours, until the bus closes.
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

/// The unit a signal of the manager's is about, for the signals that change what runs: a unit loaded
/// (UnitNew) or gone (UnitRemoved), and a job done (JobRemoved: a scope's start job ends once it runs;
/// at UnitNew it is still inactive, and listing it then can miss it).
fn unit_of_signal(message: &Message) -> Option<String> {
    match &*message.member()? {
        "UnitNew" | "UnitRemoved" => message.read2::<String, Path>().ok().map(|(unit, _)| unit),
        "JobRemoved" => message.read4::<u32, Path, String, String>().ok().map(|(_, _, unit, _)| unit),
        _ => None,
    }
}

/// Whether the user manager has this unit loaded (`name` with its suffix).
fn loaded(bus: &mut Bus, name: &str) -> bool {
    get_unit(bus, name).is_ok()
}

/// A loaded unit's object path.
pub(super) fn get_unit(bus: &mut Bus, name: &str) -> dbus::Result<Path<'static>> {
    let (path,): (Path<'static>,) = call(bus, "GetUnit", (name,))?;
    Ok(path)
}

/// Moves running processes into scope `unit`, a new one (in `slice`, if given) unless it is there, as a
/// launch's own: for a program another program started (a Steam game, under Steam's launcher), so that
/// it is counted, frozen and ended as the launch, apart from its starter. What they start later is in it
/// too.
pub fn adopt(unit: &str, slice: Option<&str>, pids: &[u32]) -> Result<(), Error> {
    with_manager(|bus| {
        if loaded(bus, unit) {
            call::<()>(bus, "AttachProcessesToUnit", (unit, "", pids.to_vec()))?;
            return Ok(());
        }
        // (Delegated: a user manager moves processes into a unit later only if it is.)
        let mut properties = vec![("PIDs", value(pids.to_vec())), ("CollectMode", value("inactive-or-failed".to_owned())), ("Delegate", value(true))];
        if let Some(slice) = slice {
            properties.push(("Slice", value(slice.to_owned())));
        }
        let aux: Vec<(&str, Vec<(&str, dbus::Value)>)> = Vec::new();
        call::<(Path<'static>,)>(bus, "StartTransientUnit", (unit, "fail", properties, aux))?;
        Ok(())
    })
}

/// Thaws a launch's slice before it starts (one left frozen: systemd starts no scope in it).
pub fn thaw_slice(slice: &str) {
    if let Err(e) = with_manager(|m| {
        thaw_now(m, slice);
        Ok(())
    }) {
        log::debug!("thawing {slice}: {e}");
    }
}

/// Ends unit `unit` (an entry's slice: all its launches): stopped (systemd asks its processes to end,
/// then kills what is left after its timeout), or with `force` killed at once.
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

/// Thaws a unit, if it is there and frozen (otherwise systemd does nothing).
fn thaw_now(bus: &mut Bus, unit: &str) {
    if loaded(bus, unit)
        && let Err(e) = call::<()>(bus, "ThawUnit", (unit,))
    {
        log::debug!("thawing {unit}: {e}");
    }
}

/// Waits for a scope to appear, within `appear`.
pub fn appear(unit: &str, appear: Duration) -> Result<(), Error> {
    let asked = Instant::now();
    while !with_manager(|bus| Ok(loaded(bus, unit)))? {
        if asked.elapsed() > appear {
            return Err(Error::DidNotStart(unit.to_owned()));
        }
        thread::sleep(Duration::from_millis(200));
    }
    Ok(())
}

#[cfg(test)]
mod live {
    use super::*;
    use std::process::Command;

    /// With a user session (a desktop): lists our scopes; nothing is changed (run by hand: `--ignored
    /// --nocapture`).
    #[test]
    #[ignore = "needs a user session"]
    #[allow(clippy::unwrap_used)]
    fn lists_our_scopes() {
        let mut bus = Bus::session().unwrap();
        println!("{:?}", list(&mut bus).unwrap());
    }

    /// With a user session (a desktop): a scope we start shows in the list, and is gone once it ends.
    /// Starts a one-second `sleep` in a scope of its own (run by hand: `--ignored`).
    #[test]
    #[ignore = "needs a user session; starts a scope"]
    #[allow(clippy::unwrap_used)]
    fn a_scope_comes_and_goes() {
        let mut bus = Bus::session().unwrap();
        let instance = Instance::Run { key: "unittest".into(), secs: u64::from(std::process::id()), n: 0 };
        let name = instance.name();
        let mut child =
            Command::new("systemd-run").args(["--user", "--scope", "--quiet", "--collect", &format!("--unit={name}"), "--", "sleep", "1"]).spawn().unwrap();
        let start = Instant::now();
        while !list(&mut bus).unwrap().contains(&instance) {
            assert!(start.elapsed() < Duration::from_secs(5), "listed");
            thread::sleep(Duration::from_millis(50));
        }
        let unit = instance.unit().unwrap();
        appear(&unit, Duration::from_secs(1)).unwrap();
        child.wait().unwrap();
        // (Gone from the list once its process has ended.)
        let start = Instant::now();
        while list(&mut bus).unwrap().contains(&instance) {
            assert!(start.elapsed() < Duration::from_secs(5), "gone");
            thread::sleep(Duration::from_millis(50));
        }
        assert!(appear("clamshell-run-never-0-0.scope", Duration::from_millis(300)).is_err());
    }
}
