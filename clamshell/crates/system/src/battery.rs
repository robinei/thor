//! The battery tab's readings, while it is open: the charge over the last day from UPower (system bus; it
//! records the battery's level as it changes), and what is using the machine now: CPU time per process
//! (`/proc/<pid>/stat`) and GPU time per DRM client (see [`crate::os::drm`]; only our user's processes are
//! readable), and the memory each has resident, grouped by our scopes (an app instance, the game), else
//! (Waydroid's container) by Android app, else by program name for our user's processes, else as the system.
//! Sampled every two seconds, sent as [`Reading::Battery`]: those doing work, the busiest first, then those
//! idle that hold much memory.

use std::collections::HashMap;
use std::fmt;
use std::io;
use std::os::unix::fs::MetadataExt;
use std::sync::mpsc::Receiver;
use std::thread;
use std::time::{Duration, Instant};

use crate::os::dbus::{self, Bus, Kept, Path};
use crate::os::worker::{self, Next, Shown, Worker};
use crate::os::{cgroup, drm, sys};
use crate::{Closed, Sink};
use clamshell_core::status::battery::{Battery, Charge, Group, Spender};
use clamshell_core::{Event, Instance, Reading};

const SAMPLE_EVERY: Duration = Duration::from_secs(2);
const HISTORY_EVERY: Duration = Duration::from_secs(60);
/// The history's span and how many points to ask for.
const HISTORY_SPAN: u32 = 24 * 3600;
const HISTORY_POINTS: u32 = 240;
/// Spenders sent at most, and the least share of the machine that still counts; an idle one still
/// counts with this much memory resident.
const MOST: usize = 12;
const LEAST: f32 = 0.002;
const MUCH_MEMORY: u64 = 100 << 20;

/// UPower's device type for a battery (UpDeviceKind).
const UPOWER_BATTERY: u32 = 2;

/// A history point's state (UpDeviceState): unknown states are gaps (with a level of 0).
#[repr(u32)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ChargeState {
    Unknown = 0,
    Charging = 1,
}

const UPOWER: &str = "org.freedesktop.UPower";
const UPOWER_DEVICE: &str = "org.freedesktop.UPower.Device";

#[derive(Debug)]
enum Error {
    Bus(dbus::Error),
    NoBattery,
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::Bus(e) => write!(f, "{e}"),
            Error::NoBattery => write!(f, "UPower has no battery"),
        }
    }
}

impl From<dbus::Error> for Error {
    fn from(e: dbus::Error) -> Self {
        Error::Bus(e)
    }
}

/// The battery thread: it samples what spends the battery while the battery tab is shown.
pub fn spawn(sink: impl Sink) -> io::Result<Worker<Shown>> {
    Worker::spawn("battery", move |commands| {
        let _ = sample(&commands, &sink);
    })
}

/// Samples every two seconds while started (the history once a minute), until the commands or the UI
/// go.
fn sample(commands: &Receiver<Shown>, sink: &impl Sink) -> Result<(), Closed> {
    let mut sampling = false;
    let mut sampler = Sampler::default();
    let mut battery = Battery::default();
    let mut last_history: Option<Instant> = None;
    let mut upower = Kept::system();
    loop {
        match worker::next(commands, sampling.then_some(SAMPLE_EVERY)) {
            Next::Command(Shown(shown)) => {
                sampling = shown;
                sampler = Sampler::default();
                last_history = None;
                battery.spenders.clear();
            }
            Next::Elapsed => {}
            Next::Disconnected => return Ok(()),
        }
        if !sampling {
            continue;
        }
        if last_history.is_none_or(|t| t.elapsed() >= HISTORY_EVERY) {
            last_history = Some(Instant::now());
            match history(&mut upower) {
                Ok(h) => {
                    battery.history = h;
                    battery.has_history = true;
                }
                Err(e) => {
                    log::debug!("battery: no charge history: {e}");
                    battery.has_history = false;
                }
            }
        }
        battery.spenders = sampler.sample();
        sink.send(Event::Reading(Reading::Battery(battery.clone())))?;
    }
}

/// The battery's charge over the last day, oldest first, from UPower (its connection kept between
/// calls).
fn history(upower: &mut Kept) -> Result<Vec<Charge>, Error> {
    let charges = read_history(upower.get()?);
    if let Err(Error::Bus(e)) = &charges {
        upower.failed(e);
    }
    charges
}

fn read_history(bus: &mut Bus) -> Result<Vec<Charge>, Error> {
    let (devices,): (Vec<Path<'static>>,) = bus.call(UPOWER, "/org/freedesktop/UPower", UPOWER, "EnumerateDevices", ())?;
    for path in devices {
        if bus.get::<u32>(UPOWER, &path, UPOWER_DEVICE, "Type")? != UPOWER_BATTERY || !bus.get::<bool>(UPOWER, &path, UPOWER_DEVICE, "PowerSupply")? {
            continue;
        }
        // (time, value, state) points, newest first.
        let (points,): (Vec<(u32, f64, u32)>,) = bus.call(UPOWER, &path, UPOWER_DEVICE, "GetHistory", ("charge", HISTORY_SPAN, HISTORY_POINTS))?;
        return Ok(charges(points));
    }
    Err(Error::NoBattery)
}

/// UPower's history points (newest first; unknown states are gaps with a level of 0) as charges.
fn charges(points: Vec<(u32, f64, u32)>) -> Vec<Charge> {
    let mut out: Vec<Charge> = points
        .into_iter()
        .filter(|&(_, _, state)| state != ChargeState::Unknown as u32)
        .map(|(at, level, state)| Charge { at: u64::from(at), level: level.round().clamp(0.0, 100.0) as u8, charging: state == ChargeState::Charging as u32 })
        .collect();
    out.sort_by_key(|c| c.at);
    out
}

/// One process's counters at a sample.
struct Counters {
    group: Group,
    /// CPU time in clock ticks.
    cpu: u64,
    /// Memory resident (bytes).
    memory: u64,
    /// Not the session's own (see [`endable`]).
    endable: bool,
    /// GPU time in ns per DRM client id.
    gpu: HashMap<u64, u64>,
}

/// Turns two readings of every process into each group's share of the machine.
#[derive(Default)]
struct Sampler {
    last: HashMap<u32, Counters>,
    at: Option<Instant>,
}

impl Sampler {
    fn sample(&mut self) -> Vec<Spender> {
        let now = Instant::now();
        let current = read_processes();
        let mut spenders: Vec<Spender> = Vec::new();
        if let Some(at) = self.at {
            let seconds = now.duration_since(at).as_secs_f32().max(0.1);
            let ticks = rustix::param::clock_ticks_per_second() as f32;
            let cores = thread::available_parallelism().map_or(1, |n| n.get()) as f32;
            for (pid, now) in &current {
                // (Processes that started since the last reading do work from the next one.)
                let (cpu, gpu) = match self.last.get(pid) {
                    Some(before) => {
                        let gpu_ns: u64 = now.gpu.iter().map(|(client, ns)| ns.saturating_sub(before.gpu.get(client).copied().unwrap_or(*ns))).sum();
                        (now.cpu.saturating_sub(before.cpu) as f32 / ticks / seconds / cores, gpu_ns as f32 / 1e9 / seconds)
                    }
                    None => (0.0, 0.0),
                };
                match spenders.iter_mut().find(|s| s.group == now.group) {
                    Some(s) => {
                        s.cpu += cpu;
                        s.gpu += gpu;
                        s.memory += now.memory;
                        s.pids.push(*pid);
                        s.endable &= now.endable;
                    }
                    None => spenders.push(Spender { group: now.group.clone(), cpu, gpu, memory: now.memory, pids: vec![*pid], endable: now.endable }),
                }
            }
        }
        for s in &mut spenders {
            s.cpu = s.cpu.min(1.0);
            s.gpu = s.gpu.min(1.0);
        }
        spenders.retain(|s| s.load() >= LEAST || s.memory >= MUCH_MEMORY);
        // The busy by their work, then the idle by their memory.
        let busy = |s: &Spender| s.load() >= LEAST;
        spenders.sort_by(|a, b| busy(b).cmp(&busy(a)).then(b.load().total_cmp(&a.load())).then(b.memory.cmp(&a.memory)));
        spenders.truncate(MOST);
        self.last = current;
        self.at = Some(now);
        spenders
    }
}

fn read_processes() -> HashMap<u32, Counters> {
    let me = rustix::process::getuid().as_raw();
    let mut out = HashMap::new();
    for (pid, dir) in sys::processes() {
        let Some((comm, cpu)) = sys::text(dir.join("stat")).as_deref().and_then(parse_stat) else {
            continue;
        };
        let mine = std::fs::metadata(&dir).is_ok_and(|m| m.uid() == me);
        let scope = cgroup::unit_of(pid).and_then(|unit| Instance::from_unit(&unit));
        let group = match (scope, android(&dir), mine) {
            (Some(instance), ..) => Group::Instance(instance),
            (None, Some(group), _) => group,
            (None, None, true) => Group::Process(comm),
            (None, None, false) => Group::System,
        };
        let gpu = if mine { drm::clients(&dir) } else { HashMap::new() };
        let endable = !matches!(group, Group::System) && endable(pid, &dir);
        out.insert(pid, Counters { group, cpu, gpu, memory: resident(&dir), endable });
    }
    out
}

/// Whether a process may be ended from the battery tab: not ClamShell itself, nor the session's own
/// (`session.slice`: under uwsm, sway, ClamShell, mako, udiskie, and PipeWire and the bus; a login
/// session's scope, where sway runs without uwsm, in a desktop's session; the user manager's own scope;
/// ClamShell's service).
fn endable(pid: u32, dir: &std::path::Path) -> bool {
    if pid == std::process::id() {
        return false;
    }
    let cgroup = sys::text(dir.join("cgroup")).unwrap_or_default();
    let own = |path: &str| {
        let unit = path.rsplit('/').next().unwrap_or("");
        path.contains("/session.slice/") || (unit.starts_with("session-") && unit.ends_with(".scope")) || unit == "init.scope" || unit == "clamshell.service"
    };
    !cgroup.lines().any(|line| own(line.trim()))
}

/// A process's resident memory (bytes): `/proc/<pid>/statm`'s second field, in pages.
/// A process of Waydroid's container (its LXC payload cgroup): an app's (Android names an app's
/// processes after its package, `<package>[:<name>]`), by the app id Waydroid gives its windows
/// (`waydroid.<package>`), else Android's own.
fn android(dir: &std::path::Path) -> Option<Group> {
    if !sys::text(dir.join("cgroup"))?.contains("/lxc.payload.waydroid") {
        return None;
    }
    Some(android_group(&std::fs::read(dir.join("cmdline")).unwrap_or_default()))
}

fn android_group(cmdline: &[u8]) -> Group {
    let first = cmdline.split(|&b| b == 0).next().unwrap_or_default();
    let name = String::from_utf8_lossy(first);
    let package = name.split(':').next().unwrap_or_default();
    if package.contains('.') && !package.contains('/') { Group::App(format!("waydroid.{package}")) } else { Group::Process("Android".to_owned()) }
}

fn resident(dir: &std::path::Path) -> u64 {
    let pages: u64 = sys::text(dir.join("statm")).and_then(|t| t.split_whitespace().nth(1)?.parse().ok()).unwrap_or(0);
    pages * rustix::param::page_size() as u64
}

/// `/proc/<pid>/stat`: the process name and its user and system CPU time (clock ticks).
fn parse_stat(text: &str) -> Option<(String, u64)> {
    let open = text.find('(')?;
    let close = text.rfind(')')?;
    let comm = text.get(open + 1..close)?.to_owned();
    // After the name: state, ppid, ... utime is the 14th field of the line, the 12th after the name.
    let mut fields = text.get(close + 2..)?.split_whitespace();
    let utime: u64 = fields.nth(11)?.parse().ok()?;
    let stime: u64 = fields.next()?.parse().ok()?;
    Some((comm, utime + stime))
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    #[test]
    fn process_counters() {
        let stat = "1234 (Web Content) S 1 2 3 4 5 6 7 8 9 10 250 50 0 0 20 0 1 0";
        assert_eq!(parse_stat(stat), Some(("Web Content".into(), 300)));
    }

    #[test]
    fn androids_processes_by_app() {
        let app = |p: &str| Group::App(p.to_owned());
        assert_eq!(android_group(b"org.fdroid.fdroid\0"), app("waydroid.org.fdroid.fdroid"));
        assert_eq!(android_group(b"org.fdroid.fdroid:remote\0"), app("waydroid.org.fdroid.fdroid"), "its other processes");
        assert_eq!(android_group(b"system_server\0"), Group::Process("Android".into()));
        assert_eq!(android_group(b"/system/bin/surfaceflinger\0"), Group::Process("Android".into()));
    }

    #[test]
    fn upower_history_without_its_gaps_oldest_first() {
        let points = vec![(300, 51.0, 2), (200, 52.0, 1), (200, 0.0, 0), (100, 53.4, 2)];
        let levels: Vec<(u64, u8, bool)> = charges(points).iter().map(|c| (c.at, c.level, c.charging)).collect();
        assert_eq!(levels, [(100, 53, false), (200, 52, true), (300, 51, false)]);
    }

    /// On any Linux: two samples a moment apart find someone busy (this test, at least).
    #[test]
    fn sampling_finds_the_busy() {
        let mut sampler = Sampler::default();
        assert!(sampler.sample().is_empty(), "the first reading only primes");
        let end = Instant::now() + Duration::from_millis(300);
        let mut x = 0u64;
        while Instant::now() < end {
            x = x.wrapping_add(1);
        }
        assert!(x > 0);
        assert!(!sampler.sample().is_empty());
    }
}

#[cfg(test)]
mod live {
    use super::*;

    /// With UPower running (a desktop): reads the charge history, or says why there is none; nothing is
    /// changed (run by hand: `--ignored --nocapture`).
    #[test]
    #[ignore = "needs UPower"]
    fn reads_upower() {
        match history(&mut Kept::system()) {
            Ok(charges) => println!("{} points, the last {:?}", charges.len(), charges.last()),
            Err(e) => println!("no history: {e}"),
        }
    }
}
