//! Our launches as systemd sees them: the user manager's scopes named `clamshell-run-*` (D-Bus,
//! org.freedesktop.systemd1), listed at start and again whenever it signals a
//! unit of ours coming, starting or going. This is the truth for what runs: a scope lives as long as any
//! of its processes, whatever its windows do, and survives a restart of ClamShell.
//!
//! But not one of sway: a scope whose processes were started under a sway that is gone (its socket no
//! longer there) can never show a window again. It is stopped (as Quit would: asked to end first) and
//! not listed.

use std::convert::Infallible;
use std::fmt;
use std::io;
use std::sync::mpsc::Receiver;
use std::thread;
use std::time::{Duration, Instant};

use clamshell_core::{Event, Instance};
use zbus::blocking::Connection;
use zbus::zvariant::OwnedObjectPath;

use crate::worker::{self, Changed, Ended, Service, lost_on};
use crate::{Closed, Sink, dbus};

const PATTERNS: [&str; 1] = ["clamshell-run-*.scope"];

/// How long to wait before trying the user manager again.
const RETRY: Duration = Duration::from_secs(5);

/// ListUnitsByPatterns' rows: name, description, load state, active state, sub state, following,
/// object path, job id, job type, job path.
type UnitRow = (String, String, String, String, String, String, OwnedObjectPath, u32, String, OwnedObjectPath);

#[zbus::proxy(
    interface = "org.freedesktop.systemd1.Manager",
    default_service = "org.freedesktop.systemd1",
    default_path = "/org/freedesktop/systemd1",
    gen_async = false
)]
trait Manager {
    /// Without it, the manager doesn't signal unit changes.
    fn subscribe(&self) -> zbus::Result<()>;
    fn list_units_by_patterns(&self, states: &[&str], patterns: &[&str]) -> zbus::Result<Vec<UnitRow>>;
    fn get_unit(&self, name: &str) -> zbus::Result<OwnedObjectPath>;
    fn stop_unit(&self, name: &str, mode: &str) -> zbus::Result<OwnedObjectPath>;
    fn freeze_unit(&self, name: &str) -> zbus::Result<()>;
    fn thaw_unit(&self, name: &str) -> zbus::Result<()>;
    fn attach_processes_to_unit(&self, name: &str, subcgroup: &str, pids: &[u32]) -> zbus::Result<()>;
    fn start_transient_unit(
        &self,
        name: &str,
        mode: &str,
        properties: &[(&str, zbus::zvariant::Value<'_>)],
        aux: &[(&str, Vec<(&str, zbus::zvariant::Value<'_>)>)],
    ) -> zbus::Result<OwnedObjectPath>;
}

#[zbus::proxy(interface = "org.freedesktop.systemd1.Scope", default_service = "org.freedesktop.systemd1", gen_async = false)]
trait Scope {
    /// Its cgroup, under /sys/fs/cgroup.
    #[zbus(property)]
    fn control_group(&self) -> zbus::Result<String>;
}

#[derive(Debug)]
pub enum Error {
    Bus(zbus::Error),
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

impl From<zbus::Error> for Error {
    fn from(e: zbus::Error) -> Self {
        Error::Bus(e)
    }
}

lost_on!(Error: zbus::Error);

fn manager(conn: &Connection) -> zbus::Result<ManagerProxy<'static>> {
    dbus::default_proxy(conn)
}

/// Our scopes that run (dock app instances and games), in the order of their names; those orphaned by a
/// sway gone stopped instead (see the module's doc).
fn list(manager: &ManagerProxy) -> zbus::Result<Vec<Instance>> {
    let rows = manager.list_units_by_patterns(&[], &PATTERNS)?;
    let mut names = Vec::new();
    for row in rows.into_iter().filter(|row| !matches!(row.3.as_str(), "inactive" | "failed")) {
        let group = ScopeProxy::builder(manager.inner().connection()).path(row.6.clone())?.build()?.control_group().unwrap_or_default();
        if !group.is_empty() && orphaned(std::path::Path::new("/sys/fs/cgroup"), &group, std::path::Path::new("/proc")) {
            log::warn!("{}: its sway is gone (it can show no window): stopped", row.0);
            if let Err(e) = manager.stop_unit(&row.0, "replace") {
                log::warn!("stopping {}: {e}", row.0);
            }
            continue;
        }
        names.push(row.0);
    }
    names.sort();
    Ok(names.iter().filter_map(|n| Instance::from_unit(n)).collect())
}

/// Whether the scope in cgroup `group` (under `cgroups`) was started under a sway that is gone: its first
/// process's SWAYSOCK (read under `proc`) is set and no longer there. Without one (not started under
/// sway), or unreadable: not.
fn orphaned(cgroups: &std::path::Path, group: &str, proc: &std::path::Path) -> bool {
    let procs = std::fs::read_to_string(cgroups.join(group.trim_start_matches('/')).join("cgroup.procs")).unwrap_or_default();
    let Some(pid) = procs.lines().next().filter(|p| !p.is_empty()) else {
        return false;
    };
    let environ = std::fs::read(proc.join(pid).join("environ")).unwrap_or_default();
    let sock = environ.split(|b| *b == 0).find_map(|v| v.strip_prefix(b"SWAYSOCK=")).map(|v| std::path::PathBuf::from(String::from_utf8_lossy(v).into_owned()));
    sock.is_some_and(|s| !s.exists())
}

/// Sends [`Event::Units`] at start and after every change of our scopes; connects again if the bus goes.
/// At start, our slices are thawed: one a ClamShell before this one froze would stay frozen, this one
/// not knowing it paused it (one hidden is frozen again after the grace).
pub fn spawn(sink: impl Sink) -> io::Result<()> {
    worker::detached("units", move || worker::supervise_alone(&mut Units { sink, thawed: false }, RETRY))
}

struct Units<S> {
    sink: S,
    /// Our slices have been thawed (once, at start).
    thawed: bool,
}

impl<S: Sink> Service for Units<S> {
    type Command = Infallible;
    type Session = ManagerProxy<'static>;
    type Error = Error;
    const NAME: &'static str = "units";

    fn connect(&mut self) -> Result<ManagerProxy<'static>, Error> {
        let manager = manager(&Connection::session()?)?;
        manager.subscribe()?;
        if !std::mem::replace(&mut self.thawed, true) {
            let slices = manager.list_units_by_patterns(&[], &["clamshell-run-*.slice"])?;
            for (name, ..) in slices {
                thaw_now(&manager, &name);
            }
        }
        Ok(manager)
    }

    /// Lists the scopes, then again at each signal about one of ours, until the bus closes.
    fn run_while_up(&mut self, manager: ManagerProxy<'static>, _: &Receiver<Infallible>) -> Result<(), Ended<Error>> {
        let signals = manager.inner().receive_all_signals()?;
        let mut last = Changed::default();
        if let Some(units) = last.offer(list(&manager)?) {
            self.sink.send(Event::Units(units))?;
        }
        for message in signals {
            let Some(name) = unit_of_signal(&message) else {
                continue;
            };
            if !Instance::is_ours(&name) {
                continue;
            }
            if let Some(units) = last.offer(list(&manager)?) {
                self.sink.send(Event::Units(units))?;
            }
        }
        Err(Ended::Lost(Error::SignalsEnded))
    }

    fn command_while_down(&mut self, command: Infallible) -> Result<(), Closed> {
        match command {}
    }
}

/// The unit a signal of the manager's is about, for the signals that change what runs: a unit loaded
/// (UnitNew) or gone (UnitRemoved), and a job done (JobRemoved: a scope's start job ends once it runs;
/// at UnitNew it is still inactive, and listing it then can miss it).
fn unit_of_signal(message: &zbus::Message) -> Option<String> {
    let header = message.header();
    let body = message.body();
    match header.member()?.as_str() {
        "UnitNew" | "UnitRemoved" => body.deserialize::<(String, OwnedObjectPath)>().ok().map(|(unit, _)| unit),
        "JobRemoved" => body.deserialize::<(u32, OwnedObjectPath, String, String)>().ok().map(|(_, _, unit, _)| unit),
        _ => None,
    }
}

/// Whether the user manager has this unit loaded (`name` with its suffix).
fn loaded(manager: &ManagerProxy, name: &str) -> bool {
    manager.get_unit(name).is_ok()
}

/// Moves running processes into scope `unit`, a new one (in `slice`, if given) unless it is there, as a
/// launch's own: for a program another program started (a Steam game, under Steam's launcher), so that
/// it is counted, frozen and ended as the launch, apart from its starter. What they start later is in it
/// too.
pub fn adopt(unit: &str, slice: Option<&str>, pids: &[u32]) -> Result<(), Error> {
    use zbus::zvariant::Value;
    let manager = manager(&Connection::session()?)?;
    if loaded(&manager, unit) {
        manager.attach_processes_to_unit(unit, "", pids)?;
        return Ok(());
    }
    // (Delegated: a user manager moves processes into a unit later only if it is.)
    let mut properties = vec![("PIDs", Value::from(pids.to_vec())), ("CollectMode", Value::from("inactive-or-failed")), ("Delegate", Value::from(true))];
    if let Some(slice) = slice {
        properties.push(("Slice", Value::from(slice)));
    }
    manager.start_transient_unit(unit, "fail", &properties, &[])?;
    Ok(())
}

/// Thaws a launch's slice before it starts (one left frozen: systemd starts no scope in it).
pub fn thaw_slice(slice: &str) {
    match Connection::session().map_err(Error::from).and_then(|c| Ok(manager(&c)?)) {
        Ok(manager) => thaw_now(&manager, slice),
        Err(e) => log::debug!("thawing {slice}: {e}"),
    }
}

/// Thaws a unit, if it is there and frozen (otherwise systemd does nothing).
fn thaw_now(manager: &ManagerProxy, unit: &str) {
    if loaded(manager, unit)
        && let Err(e) = manager.thaw_unit(unit)
    {
        log::debug!("thawing {unit}: {e}");
    }
}

/// Freezes a unit (a slice: every scope in it), or thaws it, on a thread of its own: the cgroup freezer,
/// which stops its processes where they are, without their knowing.
pub fn freeze(unit: String, frozen: bool) {
    let done = worker::detached("freeze", move || {
        let result = Connection::session()
            .map_err(Error::from)
            .and_then(|c| Ok(manager(&c)?))
            .and_then(|m| if frozen { m.freeze_unit(&unit) } else { m.thaw_unit(&unit) }.map_err(Error::from));
        match result {
            Ok(()) => log::info!("{} {unit}", if frozen { "froze" } else { "thawed" }),
            Err(e) => log::warn!("{} {unit}: {e}", if frozen { "freezing" } else { "thawing" }),
        }
    });
    if let Err(e) = done {
        log::warn!("freeze: {e}");
    }
}

/// Waits for a scope to appear (within `appear`) and then to end. Checks once a second: for a game's
/// play time, a second is exact enough.
pub fn wait(unit: &str, appear: Duration) -> Result<(), Error> {
    let manager = manager(&Connection::session()?)?;
    let asked = Instant::now();
    while !loaded(&manager, unit) {
        if asked.elapsed() > appear {
            return Err(Error::DidNotStart(unit.to_owned()));
        }
        thread::sleep(Duration::from_millis(200));
    }
    while loaded(&manager, unit) {
        thread::sleep(Duration::from_secs(1));
    }
    Ok(())
}

#[cfg(test)]
mod live {
    use super::*;
    use std::process::Command;

    #[test]
    #[allow(clippy::unwrap_used)]
    fn a_scope_of_a_sway_gone_is_orphaned() {
        let dir = std::env::temp_dir().join(format!("clamshell-orphans-{}", std::process::id()));
        let (group, live) = (dir.join("cg/run.scope"), dir.join("sway.sock"));
        std::fs::create_dir_all(&group).unwrap();
        std::fs::write(&live, "").unwrap();
        let scope = |pid: &str, environ: &str| {
            std::fs::write(group.join("cgroup.procs"), format!("{pid}\n")).unwrap();
            std::fs::create_dir_all(dir.join("proc").join(pid)).unwrap();
            std::fs::write(dir.join("proc").join(pid).join("environ"), environ.replace('|', "\0")).unwrap();
            orphaned(&dir.join("cg"), "/run.scope", &dir.join("proc"))
        };
        assert!(scope("7", &format!("HOME=/h|SWAYSOCK={}|", dir.join("gone.sock").display())), "its sway gone");
        assert!(!scope("8", &format!("SWAYSOCK={}|", live.display())), "its sway there");
        assert!(!scope("9", "HOME=/h|"), "not under sway");
        std::fs::remove_dir_all(&dir).unwrap();
    }

    /// With a user session (a desktop): lists our scopes; nothing is changed (run by hand: `--ignored
    /// --nocapture`).
    #[test]
    #[ignore]
    #[allow(clippy::unwrap_used)]
    fn lists_our_scopes() {
        let manager = manager(&Connection::session().unwrap()).unwrap();
        println!("{:?}", list(&manager).unwrap());
    }

    /// With a user session (a desktop): a scope we start shows in the list, and is gone once it ends.
    /// Starts a one-second `sleep` in a scope of its own (run by hand: `--ignored`).
    #[test]
    #[ignore]
    #[allow(clippy::unwrap_used)]
    fn a_scope_comes_and_goes() {
        let manager = manager(&Connection::session().unwrap()).unwrap();
        let instance = Instance::Run { key: "unittest".into(), secs: u64::from(std::process::id()), n: 0 };
        let name = instance.name();
        let mut child =
            Command::new("systemd-run").args(["--user", "--scope", "--quiet", "--collect", &format!("--unit={name}"), "--", "sleep", "1"]).spawn().unwrap();
        let start = Instant::now();
        while !list(&manager).unwrap().contains(&instance) {
            assert!(start.elapsed() < Duration::from_secs(5), "listed");
            thread::sleep(Duration::from_millis(50));
        }
        wait(&instance.unit().unwrap(), Duration::from_secs(1)).unwrap();
        child.wait().unwrap();
        assert!(!list(&manager).unwrap().contains(&instance));
        assert!(wait("clamshell-run-never-0-0.scope", Duration::from_millis(300)).is_err());
    }
}
