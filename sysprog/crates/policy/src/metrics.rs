//! Reads the system's metrics for the sysinfo mode once a second: CPU load (total and per core) and
//! frequency, GPU load and frequency, temperatures, memory, battery power. Every source is optional:
//! what a machine lacks stays `None`. Sources: /proc/stat, /proc/meminfo, cpufreq, hwmon and thermal
//! zones, amdgpu (desktop) or devfreq (Adreno on the Thor), the battery's power supply.
//!
//! GPU load: amdgpu's busy file where there is one; otherwise (Adreno on the Thor) the kernel's per-client
//! GPU time in /proc/<pid>/fdinfo, summed over every DRM client, as MangoHud does for one process.

use std::collections::HashSet;
use std::fs;
use std::path::Path;
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use sysprog_core::Event;
use sysprog_core::metrics::{MAX_CORES, Metrics};

use crate::Sink;

const EVERY: Duration = Duration::from_secs(1);

pub fn spawn(sink: impl Sink) -> std::io::Result<JoinHandle<()>> {
    thread::Builder::new().name("metrics".into()).spawn(move || {
        let mut previous = read_cpu_times();
        let mut gpu_previous = gpu_time_ns().map(|t| (t, Instant::now()));
        loop {
            thread::sleep(EVERY);
            let now = read_cpu_times();
            let mut m = read(&previous, &now);
            // No busy file: GPU time used by all clients over the time elapsed.
            let gpu_now = gpu_time_ns().map(|t| (t, Instant::now()));
            if m.gpu.is_none()
                && let (Some((t0, i0)), Some((t1, i1))) = (gpu_previous, gpu_now)
            {
                let wall = i1.duration_since(i0).as_nanos() as f64;
                // Clients that exited take their time with them: no reading then.
                if t1 >= t0 && wall > 0.0 {
                    m.gpu = Some(((t1 - t0) as f64 / wall).clamp(0.0, 1.0) as f32);
                }
            }
            gpu_previous = gpu_now;
            if !sink.send(Event::Metrics(m)) {
                return;
            }
            previous = now;
        }
    })
}

fn read_file(path: impl AsRef<Path>) -> Option<String> {
    fs::read_to_string(path).ok()
}

fn read_number(path: impl AsRef<Path>) -> Option<f64> {
    read_file(path)?.trim().parse().ok()
}

/// (busy, total) jiffies for the whole CPU, then each core.
type CpuTimes = Vec<(u64, u64)>;

fn read_cpu_times() -> CpuTimes {
    read_file("/proc/stat").map(|s| parse_proc_stat(&s)).unwrap_or_default()
}

/// The `cpu` lines of /proc/stat: busy (all but idle and iowait) and total time each.
fn parse_proc_stat(text: &str) -> CpuTimes {
    text.lines()
        .filter(|l| l.starts_with("cpu"))
        .map(|l| {
            let fields: Vec<u64> = l.split_whitespace().skip(1).filter_map(|f| f.parse().ok()).collect();
            let total: u64 = fields.iter().sum();
            let idle = fields.get(3).copied().unwrap_or(0) + fields.get(4).copied().unwrap_or(0);
            (total.saturating_sub(idle), total)
        })
        .collect()
}

fn load((busy0, total0): (u64, u64), (busy1, total1): (u64, u64)) -> Option<f32> {
    let total = total1.checked_sub(total0).filter(|&t| t > 0)?;
    Some((busy1.saturating_sub(busy0) as f32 / total as f32).clamp(0.0, 1.0))
}

/// MemTotal, MemAvailable, SwapTotal, SwapFree from /proc/meminfo, in bytes.
fn parse_meminfo(text: &str) -> (u64, u64, u64, u64) {
    let field = |name: &str| {
        text.lines()
            .find(|l| l.starts_with(name))
            .and_then(|l| l.split_whitespace().nth(1))
            .and_then(|v| v.parse::<u64>().ok())
            .map_or(0, |kb| kb * 1024)
    };
    (field("MemTotal:"), field("MemAvailable:"), field("SwapTotal:"), field("SwapFree:"))
}

/// The current GPU clock from amdgpu's level list ("1: 1800Mhz *"), MHz.
fn parse_dpm_clock(text: &str) -> Option<u32> {
    let line = text.lines().find(|l| l.trim_end().ends_with('*'))?;
    let clock = line.split_whitespace().find(|w| w.to_ascii_lowercase().ends_with("mhz"))?;
    clock.get(..clock.len() - 3)?.parse().ok()
}

/// One DRM client's id and GPU engine time (ns) from its fdinfo: `drm-engine-gpu` (msm) or
/// `drm-engine-gfx` (amdgpu).
fn parse_fdinfo(text: &str) -> Option<(u64, u64)> {
    let mut id = None;
    let mut ns = None;
    for line in text.lines() {
        let Some((key, value)) = line.split_once(':') else {
            continue;
        };
        let value = value.trim();
        match key {
            "drm-client-id" => id = value.parse().ok(),
            "drm-engine-gpu" | "drm-engine-gfx" => ns = value.trim_end_matches(" ns").parse().ok(),
            _ => {}
        }
    }
    Some((id?, ns?))
}

/// Total GPU time of all DRM clients this user can see, each counted once (a client may be open
/// through several file descriptors).
fn gpu_time_ns() -> Option<u64> {
    let mut seen = HashSet::new();
    let mut total = 0u64;
    let mut any = false;
    for proc_entry in fs::read_dir("/proc").ok()?.flatten() {
        let pid = proc_entry.path();
        let Ok(fds) = fs::read_dir(pid.join("fd")) else {
            continue;
        };
        for fd in fds.flatten() {
            let is_gpu = fs::read_link(fd.path()).is_ok_and(|t| t.starts_with("/dev/dri"));
            if !is_gpu {
                continue;
            }
            let info = pid.join("fdinfo").join(fd.file_name());
            if let Some((id, ns)) = read_file(info).as_deref().and_then(parse_fdinfo)
                && seen.insert(id)
            {
                total = total.saturating_add(ns);
                any = true;
            }
        }
    }
    any.then_some(total)
}

fn read(previous: &CpuTimes, now: &CpuTimes) -> Metrics {
    // CPU load: the first line is the total, then one per core.
    let mut m = Metrics { cpu: previous.first().zip(now.first()).and_then(|(a, b)| load(*a, *b)), ..Metrics::default() };
    let cores = previous.iter().zip(now.iter()).skip(1).take(MAX_CORES);
    for (slot, (a, b)) in m.cores.iter_mut().zip(cores) {
        *slot = load(*a, *b).unwrap_or(0.0);
        m.core_count += 1;
    }
    m.cpu_mhz = (0..256)
        .map_while(|i| read_number(format!("/sys/devices/system/cpu/cpu{i}/cpufreq/scaling_cur_freq")))
        .map(|khz| (khz / 1000.0) as u32)
        .max();

    // GPU: amdgpu (desktop), else a devfreq device named like a GPU (Adreno: 3d00000.gpu).
    for card in fs::read_dir("/sys/class/drm").into_iter().flatten().flatten() {
        let device = card.path().join("device");
        if let Some(busy) = read_number(device.join("gpu_busy_percent")) {
            m.gpu = Some((busy / 100.0) as f32);
            m.gpu_mhz = read_file(device.join("pp_dpm_sclk")).as_deref().and_then(parse_dpm_clock);
            break;
        }
    }
    if m.gpu_mhz.is_none() {
        m.gpu_mhz = fs::read_dir("/sys/class/devfreq")
            .into_iter()
            .flatten()
            .flatten()
            .find(|d| d.file_name().to_string_lossy().contains("gpu"))
            .and_then(|d| read_number(d.path().join("cur_freq")))
            .map(|hz| (hz / 1e6) as u32);
    }

    // Temperatures: hwmon chips by name, and thermal zones by type, hottest of each kind.
    let hottest = |slot: &mut Option<f32>, millideg: f64| {
        let c = (millideg / 1000.0) as f32;
        if c > -40.0 && c < 150.0 && slot.is_none_or(|t| c > t) {
            *slot = Some(c);
        }
    };
    for chip in fs::read_dir("/sys/class/hwmon").into_iter().flatten().flatten() {
        let name = read_file(chip.path().join("name")).unwrap_or_default();
        let kind = match name.trim() {
            "k10temp" | "coretemp" | "zenpower" | "cpu_thermal" => &mut m.cpu_temp,
            "amdgpu" | "nouveau" | "gpu_thermal" => &mut m.gpu_temp,
            _ => continue,
        };
        // temp1 is the package (Tctl) or the GPU edge on these chips.
        if let Some(t) = read_number(chip.path().join("temp1_input")) {
            hottest(kind, t);
        }
    }
    for zone in fs::read_dir("/sys/class/thermal").into_iter().flatten().flatten() {
        let kind = read_file(zone.path().join("type")).unwrap_or_default().to_ascii_lowercase();
        let Some(t) = read_number(zone.path().join("temp")) else {
            continue;
        };
        if kind.contains("cpu") {
            hottest(&mut m.cpu_temp, t);
        } else if kind.contains("gpu") {
            hottest(&mut m.gpu_temp, t);
        }
    }

    // Memory.
    if let Some(text) = read_file("/proc/meminfo") {
        let (total, available, swap_total, swap_free) = parse_meminfo(&text);
        m.mem_total = total;
        m.mem_used = total.saturating_sub(available);
        m.swap_used = swap_total.saturating_sub(swap_free);
    }

    // Battery power and time left (the first battery).
    for supply in fs::read_dir("/sys/class/power_supply").into_iter().flatten().flatten() {
        let dir = supply.path();
        if read_file(dir.join("type")).as_deref().map(str::trim) != Some("Battery") {
            continue;
        }
        let watts = read_number(dir.join("power_now")).map(|uw| uw / 1e6).or_else(|| {
            let (ua, uv) = (read_number(dir.join("current_now"))?, read_number(dir.join("voltage_now"))?);
            Some(ua.abs() * uv / 1e12)
        });
        let discharging = read_file(dir.join("status")).as_deref().map(str::trim) == Some("Discharging");
        m.power_w = watts.map(|w| if discharging { w as f32 } else { -(w as f32) });
        if discharging && let Some(w) = watts.filter(|&w| w > 0.1) {
            let wh = read_number(dir.join("energy_now")).map(|uwh| uwh / 1e6).or_else(|| {
                let (uah, uv) = (read_number(dir.join("charge_now"))?, read_number(dir.join("voltage_now"))?);
                Some(uah * uv / 1e12)
            });
            m.minutes_left = wh.map(|wh| (wh / w * 60.0) as u32);
        }
        break;
    }
    m
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::indexing_slicing)]
mod tests {
    use super::*;

    #[test]
    fn cpu_load_from_two_readings() {
        let a = parse_proc_stat("cpu  100 0 100 700 100 0 0 0 0 0\ncpu0 50 0 50 350 50 0 0 0 0 0\n");
        let b = parse_proc_stat("cpu  200 0 200 900 100 0 0 0 0 0\ncpu0 150 0 50 350 50 0 0 0 0 0\n");
        assert_eq!(a.len(), 2);
        // Total: busy +200 of +400.
        assert_eq!(load(a[0], b[0]), Some(0.5));
        // Core 0: busy +100 of +100.
        assert_eq!(load(a[1], b[1]), Some(1.0));
    }

    #[test]
    fn meminfo_in_bytes() {
        let text = "MemTotal:       1000 kB\nMemFree: 1 kB\nMemAvailable:    400 kB\nSwapTotal: 100 kB\nSwapFree: 60 kB\n";
        assert_eq!(parse_meminfo(text), (1_024_000, 409_600, 102_400, 61_440));
    }

    #[test]
    fn amdgpu_current_clock() {
        assert_eq!(parse_dpm_clock("0: 500Mhz\n1: 1800Mhz *\n2: 2400Mhz\n"), Some(1800));
        assert_eq!(parse_dpm_clock("S: 19Mhz *\n"), Some(19));
        assert_eq!(parse_dpm_clock("0: 500Mhz\n"), None);
    }

    #[test]
    fn fdinfo_engine_time() {
        let msm = "pos:\t0\ndrm-driver:\tmsm\ndrm-client-id:\t7\ndrm-engine-gpu:\t123456 ns\ndrm-cycles-gpu:\t99\n";
        assert_eq!(parse_fdinfo(msm), Some((7, 123_456)));
        let amd = "drm-driver:\tamdgpu\ndrm-client-id:\t16\ndrm-engine-gfx:\t1300507034074 ns\n";
        assert_eq!(parse_fdinfo(amd), Some((16, 1_300_507_034_074)));
        assert_eq!(parse_fdinfo("drm-driver:\tamdgpu\ndrm-client-id:\t15\n"), None);
    }

    /// Compares the fdinfo load with amdgpu's own busy figure (where there is one).
    #[test]
    fn fdinfo_load_on_this_machine() {
        let (Some(t0), i0) = (gpu_time_ns(), Instant::now()) else {
            return;
        };
        std::thread::sleep(Duration::from_millis(1000));
        let (t1, i1) = (gpu_time_ns().unwrap(), Instant::now());
        let load = (t1.saturating_sub(t0)) as f64 / i1.duration_since(i0).as_nanos() as f64;
        let busy = fs::read_dir("/sys/class/drm").into_iter().flatten().flatten().find_map(|c| read_number(c.path().join("device/gpu_busy_percent")));
        eprintln!("fdinfo load {:.1}%, busy file {busy:?}%", load * 100.0);
    }

    #[test]
    fn reads_this_machine_without_failing() {
        let a = read_cpu_times();
        std::thread::sleep(Duration::from_millis(50));
        let m = read(&a, &read_cpu_times());
        eprintln!("{m:?}");
        assert!(m.mem_total > 0);
    }
}
