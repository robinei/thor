//! What is using the machine now: CPU time per process (`/proc/<pid>/stat`), GPU time per DRM client
//! (see [`crate::os::drm`]; only our user's processes are readable), and each process's resident memory.
//!
//! Processes are grouped by our units (an app instance, the game), or else by Android app for
//! Waydroid's container, or else by program name for our user's processes. Everything else is grouped
//! as the system. Groups doing work come first, busiest first, followed by idle groups that hold a lot
//! of memory.

use std::collections::HashMap;
use std::os::unix::fs::MetadataExt;
use std::thread;
use std::time::Instant;

use clamshell_state::Instance;
use clamshell_state::status::battery::{Group, Spender};

use crate::os::{cgroup, drm, sys};

/// The most groups sent, and the smallest share of the machine that still counts. An idle group still
/// counts if it has `MUCH_MEMORY` resident.
const MOST: usize = 12;
const LEAST: f32 = 0.002;
const MUCH_MEMORY: u64 = 100 << 20;

/// One process's counters at a sample.
struct Counters {
    group: Group,
    /// CPU time in clock ticks.
    cpu: u64,
    /// Resident memory in bytes.
    memory: u64,
    /// Whether it may be ended, as it isn't the session's own (see [`endable`]).
    endable: bool,
    /// GPU time in ns per DRM client id.
    gpu: HashMap<u64, u64>,
}

/// Turns two readings of every process into each group's share of the machine.
#[derive(Default)]
pub(super) struct Sampler {
    last: HashMap<u32, Counters>,
    at: Option<Instant>,
}

impl Sampler {
    pub(super) fn sample(&mut self) -> Vec<Spender> {
        let now = Instant::now();
        let current = read_processes();
        let mut spenders: Vec<Spender> = Vec::new();
        if let Some(at) = self.at {
            let seconds = now.duration_since(at).as_secs_f32().max(0.1);
            let ticks = rustix::param::clock_ticks_per_second() as f32;
            let cores = thread::available_parallelism().map_or(1, |n| n.get()) as f32;
            for (pid, now) in &current {
                // Processes that started since the last reading count from the next one.
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
        // The busy ones by their work, then the idle ones by their memory.
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
        let instance = cgroup::unit_of(pid).and_then(|unit| Instance::from_unit(&unit));
        let group = match (instance, android(&dir), mine) {
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

/// Whether a process may be ended from the battery tab. ClamShell itself and the session's own processes
/// may not. The session's own are those in:
/// - `session.slice`, which holds ClamShell, udiskie, PipeWire and the bus;
/// - a login session's scope;
/// - the user manager's own scope;
/// - ClamShell's service.
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

/// The group of a process in Waydroid's container, which is its LXC payload cgroup. An app's process is
/// grouped by the app id Waydroid gives its windows (`waydroid.<package>`); Android names an app's
/// processes after its package, as `<package>[:<name>]`. Other processes are grouped as Android's own.
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

/// A process's resident memory in bytes, from `/proc/<pid>/statm`'s second field, which is in pages.
fn resident(dir: &std::path::Path) -> u64 {
    let pages: u64 = sys::text(dir.join("statm")).and_then(|t| t.split_whitespace().nth(1)?.parse().ok()).unwrap_or(0);
    pages * rustix::param::page_size() as u64
}

/// The process name and its user plus system CPU time in clock ticks, from `/proc/<pid>/stat`.
fn parse_stat(text: &str) -> Option<(String, u64)> {
    let open = text.find('(')?;
    let close = text.rfind(')')?;
    let comm = text.get(open + 1..close)?.to_owned();
    // After the name come state, ppid and so on. utime is the 14th field of the line, the 12th after the
    // name.
    let mut fields = text.get(close + 2..)?.split_whitespace();
    let utime: u64 = fields.nth(11)?.parse().ok()?;
    let stime: u64 = fields.next()?.parse().ok()?;
    Some((comm, utime + stime))
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    use std::time::Duration;

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

    /// On any Linux, two samples a moment apart find something busy, at least this test.
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
