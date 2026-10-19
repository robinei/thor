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
/// Our slices' names before their keys.
const RUN_PREFIX: &str = "clamshell-run-";

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
    fn kill_unit(&self, name: &str, whom: &str, signal: i32) -> zbus::Result<()>;
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

#[zbus::proxy(interface = "org.freedesktop.systemd1.Unit", default_service = "org.freedesktop.systemd1", gen_async = false)]
trait Unit {
    /// `running`, `frozen`, `freezing` or `thawing`.
    #[zbus(property)]
    fn freezer_state(&self) -> zbus::Result<String>;
    /// The slice it is in.
    #[zbus(property)]
    fn slice(&self) -> zbus::Result<String>;
}

#[zbus::proxy(interface = "org.freedesktop.systemd1.Slice", default_service = "org.freedesktop.systemd1", gen_async = false)]
trait Slice {
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

/// The session bus, connected once for every one-off unit operation (each connection has threads of its
/// own); connected again after a call failed.
static BUS: std::sync::Mutex<Option<Connection>> = std::sync::Mutex::new(None);

/// `call` on the user manager, over the shared connection.
fn with_manager<T>(call: impl FnOnce(&ManagerProxy) -> Result<T, Error>) -> Result<T, Error> {
    let conn = {
        let mut bus = BUS.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        match bus.as_ref() {
            Some(c) => c.clone(),
            None => bus.insert(Connection::session()?).clone(),
        }
    };
    let result = manager(&conn).map_err(Error::from).and_then(|m| call(&m));
    if result.is_err() {
        *BUS.lock().unwrap_or_else(std::sync::PoisonError::into_inner) = None;
    }
    result
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
            // (Thawed first, if it was suspended: frozen, it couldn't end.)
            let unit = UnitProxy::builder(manager.inner().connection()).path(row.6.clone()).and_then(|b| b.build());
            if let Ok(slice) = unit.and_then(|u| u.slice()) {
                thaw_now(manager, &slice);
            }
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
    type Session = ManagerProxy<'static>;
    type Error = Error;
    const NAME: &'static str = "units";

    fn connect(&mut self) -> Result<ManagerProxy<'static>, Error> {
        let manager = manager(&Connection::session()?)?;
        manager.subscribe()?;
        if !std::mem::replace(&mut self.looked, true) {
            let slices = manager.list_units_by_patterns(&[], &["clamshell-run-*.slice"])?;
            let frozen = slices.into_iter().filter(|row| {
                let unit = UnitProxy::builder(manager.inner().connection()).path(row.6.clone()).and_then(|b| b.build());
                unit.and_then(|u| u.freezer_state()).is_ok_and(|s| s == "frozen")
            });
            self.frozen = Some(frozen.filter_map(|row| Some(row.0.strip_prefix(RUN_PREFIX)?.strip_suffix(".slice")?.into())).collect());
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
        if let Some(frozen) = self.frozen.take().filter(|f| !f.is_empty()) {
            log::info!("found frozen (held as suspended): {frozen:?}");
            self.sink.send(Event::Frozen(frozen))?;
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
    with_manager(|manager| {
        if loaded(manager, unit) {
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
    with_manager(|m| {
        match force {
            true => m.kill_unit(unit, "all", SIGKILL)?,
            false => drop(m.stop_unit(unit, "replace")?),
        }
        Ok(())
    })
}

/// Thaws a unit, if it is there and frozen (otherwise systemd does nothing).
fn thaw_now(manager: &ManagerProxy, unit: &str) {
    if loaded(manager, unit)
        && let Err(e) = manager.thaw_unit(unit)
    {
        log::debug!("thawing {unit}: {e}");
    }
}

/// Freezes a unit (a slice: every scope in it), or thaws it: the cgroup freezer, which stops its
/// processes where they are, without their knowing. Done by the freezer thread, one unit after another,
/// each unit's latest request only (thaws first). A freeze waits while the slice works without drawing
/// ([`working_without_drawing`]: loading, compiling shaders, which the user switched away to wait for);
/// a later request for the unit (shown again: thawed) ends the wait.
pub fn freeze(unit: String, frozen: bool) {
    static FREEZER: std::sync::OnceLock<Option<std::sync::mpsc::Sender<(String, bool)>>> = std::sync::OnceLock::new();
    let sender = FREEZER.get_or_init(|| {
        let (send, receive) = std::sync::mpsc::channel();
        worker::detached("freezer", move || freezer(&receive)).map_err(|e| log::warn!("freezer: {e}")).ok().map(|()| send)
    });
    if sender.as_ref().is_none_or(|s| s.send((unit, frozen)).is_err()) {
        log::warn!("freezer: gone");
    }
}

/// What the freezer does to units: systemd's, or a test's.
trait Freezer {
    fn thaw(&mut self, unit: &str);
    fn freeze(&mut self, unit: &str);
    /// Whether the unit works without drawing (looked at for a while: see [`working_without_drawing`]).
    fn busy(&mut self, unit: &str) -> bool;
}

/// The user manager's units.
struct Systemd;

impl Freezer for Systemd {
    fn thaw(&mut self, unit: &str) {
        match with_manager(|m| Ok(m.thaw_unit(unit)?)) {
            Ok(()) => log::info!("thawed {unit}"),
            Err(e) => log::warn!("thawing {unit}: {e}"),
        }
    }

    fn freeze(&mut self, unit: &str) {
        match with_manager(|m| Ok(m.freeze_unit(unit)?)) {
            Ok(()) => log::info!("froze {unit}"),
            Err(e) => log::warn!("freezing {unit}: {e}"),
        }
    }

    fn busy(&mut self, unit: &str) -> bool {
        with_manager(|m| Ok(working_without_drawing(m, unit))).unwrap_or(false)
    }
}

/// The freezer thread: carried out by systemd (see [`follow_requests`]).
fn freezer(requests: &std::sync::mpsc::Receiver<(String, bool)>) {
    follow_requests(requests, &mut Systemd);
}

/// The requests waiting, each unit's latest, carried out by `units` until none is left: thaws at once; a
/// freeze once its unit isn't busy, unless asked otherwise meanwhile (a busy one looked at again after
/// the others). Returns when the requests' sender is gone.
fn follow_requests(requests: &std::sync::mpsc::Receiver<(String, bool)>, units: &mut impl Freezer) {
    let mut waiting: Vec<(String, bool)> = Vec::new();
    // (Those a freeze waits for, said once each.)
    let mut said: Vec<String> = Vec::new();
    let take = |waiting: &mut Vec<(String, bool)>, (unit, frozen): (String, bool)| {
        waiting.retain(|(u, _)| *u != unit);
        waiting.push((unit, frozen));
    };
    loop {
        if waiting.is_empty() {
            match requests.recv() {
                Ok(r) => take(&mut waiting, r),
                Err(_) => return,
            }
        }
        while let Ok(r) = requests.try_recv() {
            take(&mut waiting, r);
        }
        for (unit, _) in waiting.iter().filter(|(_, frozen)| !frozen) {
            units.thaw(unit);
            said.retain(|u| u != unit);
        }
        waiting.retain(|(_, frozen)| *frozen);
        let Some((unit, _)) = waiting.first().cloned() else {
            continue;
        };
        let busy = units.busy(&unit);
        while let Ok(r) = requests.try_recv() {
            take(&mut waiting, r);
        }
        if !waiting.iter().any(|(u, frozen)| *u == unit && *frozen) {
            log::info!("{unit}: not frozen (asked otherwise since)");
            continue;
        }
        if busy {
            if !said.contains(&unit) {
                log::info!("{unit} works without drawing (loading?): frozen once it doesn't");
                said.push(unit.clone());
            }
            waiting.rotate_left(1);
            continue;
        }
        waiting.retain(|(u, _)| *u != unit);
        said.retain(|u| *u != unit);
        units.freeze(&unit);
    }
}

#[cfg(test)]
mod freezer_tests {
    use super::*;
    use std::sync::mpsc::{Sender, channel};

    /// Units in a test: what was done to them, in order; busy ones stay busy for as many looks as given,
    /// and a look may let more requests in (as the 2 s look would).
    #[derive(Default)]
    struct Fake {
        done: Vec<String>,
        busy: Vec<(String, u32)>,
        during_look: Vec<(String, bool)>,
        send: Option<Sender<(String, bool)>>,
    }

    impl Freezer for Fake {
        fn thaw(&mut self, unit: &str) {
            self.done.push(format!("thaw {unit}"));
        }
        fn freeze(&mut self, unit: &str) {
            self.done.push(format!("freeze {unit}"));
        }
        fn busy(&mut self, unit: &str) -> bool {
            // (Sent once, then the sender dropped: the loop ends when the requests are done.)
            if let Some(send) = self.send.take() {
                for r in self.during_look.drain(..) {
                    let _ = send.send(r);
                }
            }
            match self.busy.iter_mut().find(|(u, _)| u == unit) {
                Some((_, n)) if *n > 0 => {
                    *n -= 1;
                    true
                }
                _ => false,
            }
        }
    }

    fn run(requests: &[(&str, bool)], mut fake: Fake) -> Vec<String> {
        let (send, receive) = channel();
        for (u, f) in requests {
            let _ = send.send(((*u).to_owned(), *f));
        }
        // (Kept by the fake only to send more during a look; else dropped: the loop ends when the requests
        // are done.)
        if fake.during_look.is_empty() {
            drop(send);
        } else {
            fake.send = Some(send);
        }
        follow_requests(&receive, &mut fake);
        fake.done
    }

    #[test]
    fn each_units_latest_request_thaws_first() {
        // a: freeze then thaw: only the thaw; b: frozen.
        let done = run(&[("a", true), ("b", true), ("a", false)], Fake::default());
        assert_eq!(done, ["thaw a", "freeze b"]);
    }

    #[test]
    fn a_busy_unit_waits_and_a_thaw_meanwhile_cancels_its_freeze() {
        // a busy for two looks, b not: b frozen first, then a.
        let fake = Fake { busy: vec![("a".into(), 2)], ..Fake::default() };
        assert_eq!(run(&[("a", true), ("b", true)], fake), ["freeze b", "freeze a"]);
        // a busy, and shown during the look (thawed): never frozen.
        let fake = Fake { busy: vec![("a".into(), 1)], during_look: vec![("a".into(), false)], ..Fake::default() };
        assert_eq!(run(&[("a", true)], fake), ["thaw a"]);
    }
}

/// Above this CPU (cores) with the GPU below [`DRAWING_GPU`] (of its time), a slice works without
/// drawing.
const BUSY_CPU: f64 = 0.5;
const DRAWING_GPU: f64 = 0.05;
/// How long a slice is watched for that.
const WATCH: Duration = Duration::from_secs(2);

/// Whether slice `unit` works without drawing, over [`WATCH`]: much CPU, the GPU (its processes' DRM
/// clients) nearly idle; a game hidden but drawing on uses the GPU. Unreadable: not.
fn working_without_drawing(manager: &ManagerProxy, unit: &str) -> bool {
    let group = manager
        .get_unit(unit)
        .ok()
        .and_then(|path| SliceProxy::builder(manager.inner().connection()).path(path).ok()?.build().ok()?.control_group().ok())
        .filter(|g| !g.is_empty());
    let Some(group) = group else {
        thread::sleep(WATCH);
        return false;
    };
    let dir = std::path::PathBuf::from("/sys/fs/cgroup").join(group.trim_start_matches('/'));
    let sample = || (cpu_usec(&dir), gpu_ns(&dir), Instant::now());
    let (cpu0, gpu0, t0) = sample();
    thread::sleep(WATCH);
    let (cpu1, gpu1, t1) = sample();
    let (Some(cpu0), Some(cpu1)) = (cpu0, cpu1) else {
        return false;
    };
    let wall = t1.duration_since(t0).as_secs_f64();
    let cpu = cpu1.saturating_sub(cpu0) as f64 / 1e6 / wall;
    let gpu = gpu1.saturating_sub(gpu0) as f64 / 1e9 / wall;
    cpu > BUSY_CPU && gpu < DRAWING_GPU
}

/// A cgroup's CPU time so far (microseconds).
fn cpu_usec(dir: &std::path::Path) -> Option<u64> {
    let stat = crate::sys::text(dir.join("cpu.stat"))?;
    stat.lines().find_map(|l| l.strip_prefix("usage_usec ")).and_then(|v| v.trim().parse().ok())
}

/// The GPU time so far of a cgroup's processes (its subgroups' too), each DRM client once.
fn gpu_ns(dir: &std::path::Path) -> u64 {
    let mut clients = std::collections::HashMap::new();
    let mut dirs = vec![dir.to_path_buf()];
    while let Some(d) = dirs.pop() {
        for pid in crate::sys::text(d.join("cgroup.procs")).unwrap_or_default().lines() {
            clients.extend(crate::drm::clients(&std::path::Path::new("/proc").join(pid)));
        }
        if let Ok(entries) = std::fs::read_dir(&d) {
            dirs.extend(entries.flatten().filter(|e| e.file_type().is_ok_and(|t| t.is_dir())).map(|e| e.path()));
        }
    }
    clients.values().sum()
}

/// Waits for a scope to appear, within `appear`.
pub fn appear(unit: &str, appear: Duration) -> Result<(), Error> {
    let asked = Instant::now();
    while !with_manager(|m| Ok(loaded(m, unit)))? {
        if asked.elapsed() > appear {
            return Err(Error::DidNotStart(unit.to_owned()));
        }
        thread::sleep(Duration::from_millis(200));
    }
    Ok(())
}

/// Waits for a scope to end. Checks once a second: for when a launch ended, a second is exact enough.
pub fn wait(unit: &str) -> Result<(), Error> {
    while with_manager(|m| Ok(loaded(m, unit)))? {
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
        let unit = instance.unit().unwrap();
        appear(&unit, Duration::from_secs(1)).unwrap();
        wait(&unit).unwrap();
        child.wait().unwrap();
        assert!(!list(&manager).unwrap().contains(&instance));
        assert!(appear("clamshell-run-never-0-0.scope", Duration::from_millis(300)).is_err());
    }

    /// A slice working without drawing (a busy loop, no GPU) isn't frozen while it does; once idle, it is
    /// (run by hand: `--ignored`).
    #[test]
    #[ignore]
    #[allow(clippy::unwrap_used)]
    fn a_freeze_waits_while_the_slice_works_without_drawing() {
        let id = std::process::id();
        let slice = format!("clamshell-run-busytest_{id}.slice");
        let state = || {
            let out = Command::new("systemctl").args(["--user", "show", "-p", "FreezerState", "--value", &slice]).output().unwrap();
            String::from_utf8(out.stdout).unwrap().trim().to_owned()
        };
        let busy = format!("busytest-{id}-busy");
        let mut looping = Command::new("systemd-run")
            .args(["--user", "--quiet", "--collect", "--scope", &format!("--slice={slice}"), &format!("--unit={busy}"), "sh", "-c", "while :; do :; done"])
            .spawn()
            .unwrap();
        thread::sleep(Duration::from_secs(1));
        freeze(slice.clone(), true);
        thread::sleep(Duration::from_secs(6));
        assert_eq!(state(), "running", "busy without drawing: not yet");
        // Idle (the loop gone, a sleep left): frozen.
        let _ = Command::new("systemctl").args(["--user", "stop", &format!("{busy}.scope")]).status();
        let _ = looping.wait();
        let mut sleeping = Command::new("systemd-run")
            .args(["--user", "--quiet", "--collect", "--scope", &format!("--slice={slice}"), &format!("--unit=busytest-{id}-idle"), "sleep", "30"])
            .spawn()
            .unwrap();
        thread::sleep(Duration::from_secs(5));
        assert_eq!(state(), "frozen");
        freeze(slice.clone(), false);
        thread::sleep(Duration::from_secs(1));
        assert_eq!(state(), "running");
        let _ = Command::new("systemctl").args(["--user", "stop", &slice]).status();
        let _ = sleeping.wait();
    }
}
