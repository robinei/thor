//! The battery tab's readings, while it is open: the charge over the last day from UPower (system bus;
//! it records the battery's level as it changes), and what is using the machine now: CPU time per process
//! (`/proc/<pid>/stat`) and GPU time per DRM client (`/proc/<pid>/fdinfo`, `drm-engine-*`; only our
//! user's processes are readable), grouped by our scopes (an app instance, the game), else by program
//! name for our user's processes, else as the system. Sampled every two seconds, sent as
//! [`Event::Battery`].

use std::collections::HashMap;
use std::io;
use std::os::unix::fs::MetadataExt;
use std::sync::mpsc::{Receiver, RecvTimeoutError};
use std::thread;
use std::time::{Duration, Instant};

use clamshell_core::Event;
use clamshell_core::battery::{Charge, Group, Battery, Spender};
use zbus::blocking::{Connection, Proxy};
use zbus::zvariant::OwnedObjectPath;

use crate::Sink;

const SAMPLE_EVERY: Duration = Duration::from_secs(2);
const HISTORY_EVERY: Duration = Duration::from_secs(60);
/// The history's span and how many points to ask for.
const HISTORY_SPAN: u32 = 24 * 3600;
const HISTORY_POINTS: u32 = 240;
/// Spenders sent at most, and the least share of the machine that still counts.
const MOST: usize = 12;
const LEAST: f32 = 0.002;
/// UPower's device type for a battery, and its states.
const UPOWER_BATTERY: u32 = 2;
const UPOWER_CHARGING: u32 = 1;
const UPOWER_UNKNOWN: u32 = 0;

/// Runs while the battery mode is open (`true` starts, `false` stops).
pub fn spawn(sink: impl Sink, commands: Receiver<bool>) -> io::Result<()> {
    thread::Builder::new().name("power".into()).spawn(move || {
        let mut watching = false;
        let mut sampler = Sampler::default();
        let mut power = Battery::default();
        let mut last_history: Option<Instant> = None;
        loop {
            let command = if watching { commands.recv_timeout(SAMPLE_EVERY) } else { commands.recv().map_err(|_| RecvTimeoutError::Disconnected) };
            match command {
                Ok(on) => {
                    watching = on;
                    sampler = Sampler::default();
                    last_history = None;
                    power.spenders.clear();
                }
                Err(RecvTimeoutError::Timeout) => {}
                Err(RecvTimeoutError::Disconnected) => return,
            }
            if !watching {
                continue;
            }
            if last_history.is_none_or(|t| t.elapsed() >= HISTORY_EVERY) {
                last_history = Some(Instant::now());
                match history() {
                    Ok(h) => {
                        power.history = h;
                        power.has_history = true;
                    }
                    Err(e) => {
                        log::debug!("power: no charge history: {e}");
                        power.has_history = false;
                    }
                }
            }
            power.spenders = sampler.sample();
            if !sink.send(Event::Battery(power.clone())) {
                return;
            }
        }
    })?;
    Ok(())
}

/// The battery's charge over the last day, oldest first, from UPower.
fn history() -> zbus::Result<Vec<Charge>> {
    let conn = Connection::system()?;
    let upower = Proxy::new(&conn, "org.freedesktop.UPower", "/org/freedesktop/UPower", "org.freedesktop.UPower")?;
    let devices: Vec<OwnedObjectPath> = upower.call("EnumerateDevices", &())?;
    for path in devices {
        let device = Proxy::new(&conn, "org.freedesktop.UPower", path.as_str().to_owned(), "org.freedesktop.UPower.Device")?;
        if device.get_property::<u32>("Type")? != UPOWER_BATTERY || !device.get_property::<bool>("PowerSupply")? {
            continue;
        }
        let points: Vec<(u32, f64, u32)> = device.call("GetHistory", &("charge", HISTORY_SPAN, HISTORY_POINTS))?;
        return Ok(charges(points));
    }
    Err(zbus::Error::Failure("UPower has no battery".into()))
}

/// UPower's history points (newest first; unknown states are gaps with a level of 0) as charges.
fn charges(points: Vec<(u32, f64, u32)>) -> Vec<Charge> {
    let mut out: Vec<Charge> = points
        .into_iter()
        .filter(|&(_, _, state)| state != UPOWER_UNKNOWN)
        .map(|(at, level, state)| Charge { at: u64::from(at), level: level.round().clamp(0.0, 100.0) as u8, charging: state == UPOWER_CHARGING })
        .collect();
    out.sort_by_key(|c| c.at);
    out
}

/// One process's counters at a sample.
struct Counters {
    group: Group,
    /// CPU time in clock ticks.
    cpu: u64,
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
                // Processes that started since the last reading count from the next one.
                let Some(before) = self.last.get(pid) else {
                    continue;
                };
                let cpu = now.cpu.saturating_sub(before.cpu) as f32 / ticks / seconds / cores;
                let gpu_ns: u64 = now.gpu.iter().map(|(client, ns)| ns.saturating_sub(before.gpu.get(client).copied().unwrap_or(*ns))).sum();
                let gpu = gpu_ns as f32 / 1e9 / seconds;
                if cpu <= 0.0 && gpu <= 0.0 {
                    continue;
                }
                match spenders.iter_mut().find(|s| s.group == now.group) {
                    Some(s) => {
                        s.cpu += cpu;
                        s.gpu += gpu;
                    }
                    None => spenders.push(Spender { group: now.group.clone(), cpu, gpu }),
                }
            }
        }
        for s in &mut spenders {
            s.cpu = s.cpu.min(1.0);
            s.gpu = s.gpu.min(1.0);
        }
        spenders.retain(|s| s.load() >= LEAST);
        spenders.sort_by(|a, b| b.load().total_cmp(&a.load()));
        spenders.truncate(MOST);
        self.last = current;
        self.at = Some(now);
        spenders
    }
}

fn read_processes() -> HashMap<u32, Counters> {
    let me = rustix::process::getuid().as_raw();
    let mut out = HashMap::new();
    let Ok(dir) = std::fs::read_dir("/proc") else {
        return out;
    };
    for entry in dir.flatten() {
        let Some(pid) = entry.file_name().to_str().and_then(|n| n.parse::<u32>().ok()) else {
            continue;
        };
        let path = entry.path();
        let Some((comm, cpu)) = std::fs::read_to_string(path.join("stat")).ok().as_deref().and_then(parse_stat) else {
            continue;
        };
        let mine = entry.metadata().is_ok_and(|m| m.uid() == me);
        let scope = std::fs::read_to_string(path.join("cgroup")).ok().as_deref().and_then(our_scope);
        let group = match (scope, mine) {
            (Some(unit), _) => Group::Unit(unit),
            (None, true) => Group::Process(comm),
            (None, false) => Group::System,
        };
        let gpu = if mine { gpu_clients(&path) } else { HashMap::new() };
        out.insert(pid, Counters { group, cpu, gpu });
    }
    out
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

/// One of our scopes the process runs in (`clamshell-app-…` or `clamshell-game-…`), without `.scope`.
fn our_scope(cgroup: &str) -> Option<String> {
    cgroup
        .lines()
        .flat_map(|line| line.rsplit('/'))
        .find_map(|part| part.strip_suffix(".scope").filter(|u| u.starts_with("clamshell-app-") || u.starts_with("clamshell-game-")))
        .map(str::to_owned)
}

/// GPU time per DRM client of a process: the sum of its `drm-engine-*` times (ns), per `drm-client-id`
/// (a client can be open on several file descriptors).
fn gpu_clients(process: &std::path::Path) -> HashMap<u64, u64> {
    let mut clients = HashMap::new();
    let Ok(fds) = std::fs::read_dir(process.join("fdinfo")) else {
        return clients;
    };
    for fd in fds.flatten() {
        let Ok(text) = std::fs::read_to_string(fd.path()) else {
            continue;
        };
        if let Some((client, ns)) = parse_fdinfo(&text) {
            clients.insert(client, ns);
        }
    }
    clients
}

fn parse_fdinfo(text: &str) -> Option<(u64, u64)> {
    let mut client = None;
    let mut ns = 0u64;
    let mut any = false;
    for line in text.lines() {
        let Some((key, value)) = line.split_once(':') else {
            continue;
        };
        if key == "drm-client-id" {
            client = value.trim().parse().ok();
        } else if key.starts_with("drm-engine-") && !key.starts_with("drm-engine-capacity") {
            ns += value.trim().trim_end_matches(" ns").trim().parse::<u64>().unwrap_or(0);
            any = true;
        }
    }
    any.then_some(())?;
    Some((client?, ns))
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    #[test]
    fn process_counters_and_scopes() {
        let stat = "1234 (Web Content) S 1 2 3 4 5 6 7 8 9 10 250 50 0 0 20 0 1 0";
        assert_eq!(parse_stat(stat), Some(("Web Content".into(), 300)));
        let cgroup = "0::/user.slice/user-1000.slice/user@1000.service/app.slice/clamshell-app-terminal-17-0.scope\n";
        assert_eq!(our_scope(cgroup).as_deref(), Some("clamshell-app-terminal-17-0"));
        assert_eq!(our_scope("0::/user.slice/user-1000.slice/session-2.scope\n"), None);
        let fdinfo = "pos:\t0\nflags:\t02100002\ndrm-driver:\tmsm\ndrm-client-id:\t14\ndrm-engine-gpu:\t3751190288 ns\ndrm-engine-capacity-gpu:\t1\n";
        assert_eq!(parse_fdinfo(fdinfo), Some((14, 3_751_190_288)));
        assert_eq!(parse_fdinfo("pos:\t0\nflags:\t0\n"), None);
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
