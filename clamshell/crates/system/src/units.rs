//! Our launches as systemd sees them: the user manager's scopes named `clamshell-run-*` (D-Bus,
//! org.freedesktop.systemd1), listed at start and again whenever it signals a
//! unit of ours coming, starting or going. This is the truth for what runs: a scope lives as long as any
//! of its processes, whatever its windows do, and survives a restart of ClamShell. A session's sway
//! ending stops them all (the device's uwsm, the desktop's nested sway's unit: their slices are stopped
//! after it).

use std::convert::Infallible;
use std::fmt;
use std::io;
use std::sync::mpsc::Receiver;
use std::thread;
use std::time::{Duration, Instant};

use clamshell_core::{Event, Instance};

use crate::dbus::{self, Bus, Message, Path, value};
use crate::worker::{self, Changed, Ended, Service, lost_on};
use crate::{Closed, Sink};

const PATTERNS: [&str; 1] = ["clamshell-run-*.scope"];

/// How long to wait before trying the user manager again.
const RETRY: Duration = Duration::from_secs(5);

const SYSTEMD: &str = "org.freedesktop.systemd1";
const MANAGER_PATH: &str = "/org/freedesktop/systemd1";
const MANAGER: &str = "org.freedesktop.systemd1.Manager";

/// ListUnitsByPatterns' rows: name, description, load state, active state, sub state, following,
/// object path, job id, job type, job path.
type UnitRow = (String, String, String, String, String, String, Path<'static>, u32, String, Path<'static>);

/// A method of the user manager's.
fn call<R: ::dbus::arg::ReadAll>(bus: &mut Bus, method: &str, args: impl ::dbus::arg::AppendAll) -> dbus::Result<R> {
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
/// a time); connected again after a call failed.
static BUS: std::sync::Mutex<Option<Bus>> = std::sync::Mutex::new(None);

/// `call` on the user manager, over the shared connection.
fn with_manager<T>(call: impl FnOnce(&mut Bus) -> Result<T, Error>) -> Result<T, Error> {
    let mut shared = BUS.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    let bus = match shared.take() {
        Some(bus) => bus,
        None => Bus::session()?,
    };
    let result = call(shared.insert(bus));
    if result.is_err() {
        *shared = None;
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
            let slices = units(&mut bus, &[&format!("{}*.slice", clamshell_core::instance::SLICE_PREFIX)])?;
            let frozen = slices
                .into_iter()
                .filter(|row| bus.get::<String>(SYSTEMD, &row.6, "org.freedesktop.systemd1.Unit", "FreezerState").is_ok_and(|s| s == "frozen"));
            self.frozen =
                Some(frozen.filter_map(|row| Some(row.0.strip_prefix(clamshell_core::instance::SLICE_PREFIX)?.strip_suffix(".slice")?.into())).collect());
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
fn get_unit(bus: &mut Bus, name: &str) -> dbus::Result<Path<'static>> {
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
        match with_manager(|bus| Ok(call::<()>(bus, "ThawUnit", (unit,))?)) {
            Ok(()) => log::info!("thawed {unit}"),
            Err(e) => log::warn!("thawing {unit}: {e}"),
        }
    }

    fn freeze(&mut self, unit: &str) {
        match with_manager(|bus| Ok(call::<()>(bus, "FreezeUnit", (unit,))?)) {
            Ok(()) => log::info!("froze {unit}"),
            Err(e) => log::warn!("freezing {unit}: {e}"),
        }
    }

    fn busy(&mut self, unit: &str) -> bool {
        let group = with_manager(|bus| Ok(control_group(bus, unit))).ok().flatten();
        working_without_drawing(group)
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

/// A slice's cgroup, under /sys/fs/cgroup.
fn control_group(bus: &mut Bus, unit: &str) -> Option<String> {
    let path = get_unit(bus, unit).ok()?;
    bus.get::<String>(SYSTEMD, &path, "org.freedesktop.systemd1.Slice", "ControlGroup").ok().filter(|g| !g.is_empty())
}

/// Whether the slice with cgroup `group` works without drawing, over [`WATCH`]: much CPU, the GPU (its
/// processes' DRM clients) nearly idle; a game hidden but drawing on uses the GPU. Unreadable: not.
fn working_without_drawing(group: Option<String>) -> bool {
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
    #[ignore]
    #[allow(clippy::unwrap_used)]
    fn lists_our_scopes() {
        let mut bus = Bus::session().unwrap();
        println!("{:?}", list(&mut bus).unwrap());
    }

    /// With a user session (a desktop): a scope we start shows in the list, and is gone once it ends.
    /// Starts a one-second `sleep` in a scope of its own (run by hand: `--ignored`).
    #[test]
    #[ignore]
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

    /// A slice working without drawing (a busy loop, no GPU) isn't frozen while it does; once idle, it is
    /// (run by hand: `--ignored`).
    #[test]
    #[ignore]
    #[allow(clippy::unwrap_used)]
    fn a_freeze_waits_while_the_slice_works_without_drawing() {
        let id = std::process::id();
        let slice = format!("{}busytest_{id}.slice", clamshell_core::instance::SLICE_PREFIX);
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
