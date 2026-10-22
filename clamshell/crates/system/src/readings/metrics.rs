//! Reads the system's metrics for the sysinfo mode: CPU load (total and per core) and frequency, GPU
//! load and frequency, temperatures, memory and battery power. They are read once a second while they
//! can be seen, meaning the sysinfo or battery tab is open, ClamShell's view is on the bottom screen and
//! the screens are on. Otherwise they are read every five seconds, which keeps the graphs' history at a
//! coarser rate.
//!
//! Every source is optional, and what a machine lacks stays `None`. The sources are /proc/stat,
//! /proc/meminfo, cpufreq, hwmon and thermal zones, amdgpu on a desktop or devfreq for the Thor's Adreno,
//! and the battery's power supply.
//!
//! GPU load comes from amdgpu's busy file where there is one. Otherwise, as for the Thor's Adreno, it is
//! the GPU time of every DRM client over the interval (see [`crate::os::drm`]).

use std::io;
use std::sync::mpsc::Receiver;
use std::time::{Duration, Instant};

use clamshell_core::status::metrics::{MAX_CORES, Metrics};
use clamshell_core::{Event, Reading};

use crate::os::worker::{Next, Shown, Worker};
use crate::os::{drm, sys, worker};
use crate::{Closed, Sink};

/// How often the metrics are read while they can be seen, and otherwise. While hidden, the graphs keep
/// a coarser history at less cost.
const SHOWN_EVERY: Duration = Duration::from_secs(1);
const HIDDEN_EVERY: Duration = Duration::from_secs(5);

/// Starts the metrics thread. Its commands say whether the metrics can be seen; they are read at once
/// when they become visible.
pub fn spawn(sink: impl Sink) -> io::Result<Worker<Shown>> {
    Worker::spawn("metrics", move |commands| {
        let _ = follow(&sink, &commands);
    })
}

/// Reads and sends the metrics every second while seen, every five seconds otherwise, until the UI is
/// gone.
fn follow(sink: &impl Sink, commands: &Receiver<Shown>) -> Result<(), Closed> {
    let mut previous = read_cpu_times();
    let mut gpu_previous = drm::total_ns().map(|t| (t, Instant::now()));
    let mut shown = false;
    loop {
        match worker::next(commands, Some(if shown { SHOWN_EVERY } else { HIDDEN_EVERY })) {
            Next::Command(Shown(now_shown)) => shown = now_shown,
            Next::Elapsed => {}
            Next::Disconnected => return Ok(()),
        }
        let now = read_cpu_times();
        let mut m = read(&previous, &now);
        // Without a busy file, use the GPU time of all clients over the time elapsed.
        let gpu_now = drm::total_ns().map(|t| (t, Instant::now()));
        if m.gpu.is_none()
            && let (Some((t0, i0)), Some((t1, i1))) = (gpu_previous, gpu_now)
        {
            let wall = i1.duration_since(i0).as_nanos() as f64;
            // Clients that exited take their time with them, so there is no reading then.
            if t1 >= t0 && wall > 0.0 {
                m.gpu = Some(((t1 - t0) as f64 / wall).clamp(0.0, 1.0) as f32);
            }
        }
        gpu_previous = gpu_now;
        sink.send(Event::Reading(Reading::Metrics(m)))?;
        previous = now;
    }
}

/// (busy, total) jiffies for the whole CPU, then for each core.
type CpuTimes = Vec<(u64, u64)>;

fn read_cpu_times() -> CpuTimes {
    sys::text("/proc/stat").map(|s| parse_proc_stat(&s)).unwrap_or_default()
}

/// The busy and total time of each `cpu` line in /proc/stat. Busy is everything but idle and iowait.
fn parse_proc_stat(text: &str) -> CpuTimes {
    text.lines()
        .filter(|l| l.starts_with("cpu"))
        .map(|l| {
            let fields: Vec<u64> =
                l.split_whitespace().skip(1).filter_map(|f| f.parse().inspect_err(|e| log::debug!("metrics: /proc/stat: {f:?}: {e}")).ok()).collect();
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
            .and_then(|v| v.parse::<u64>().inspect_err(|e| log::debug!("metrics: /proc/meminfo: {name} {v:?}: {e}")).ok())
            .map_or(0, |kb| kb * 1024)
    };
    (field("MemTotal:"), field("MemAvailable:"), field("SwapTotal:"), field("SwapFree:"))
}

/// The current GPU clock in MHz, from amdgpu's level list ("1: 1800Mhz *").
fn parse_dpm_clock(text: &str) -> Option<u32> {
    let line = text.lines().find(|l| l.trim_end().ends_with('*'))?;
    let clock = line.split_whitespace().find(|w| w.to_ascii_lowercase().ends_with("mhz"))?;
    clock.get(..clock.len() - 3)?.parse().inspect_err(|e| log::debug!("metrics: pp_dpm_sclk: {clock:?}: {e}")).ok()
}

fn read(previous: &CpuTimes, now: &CpuTimes) -> Metrics {
    // CPU load: the first line is the total, then one per core.
    let mut m = Metrics { cpu: previous.first().zip(now.first()).and_then(|(a, b)| load(*a, *b)), ..Metrics::default() };
    let cores = previous.iter().zip(now.iter()).skip(1).take(MAX_CORES);
    for (slot, (a, b)) in m.cores.iter_mut().zip(cores) {
        *slot = load(*a, *b).unwrap_or(0.0);
        m.core_count += 1;
    }
    m.cpu_mhz =
        (0..256).map_while(|i| sys::read::<f64>(format!("/sys/devices/system/cpu/cpu{i}/cpufreq/scaling_cur_freq"))).map(|khz| (khz / 1000.0) as u32).max();

    // GPU: amdgpu on a desktop, or else a devfreq device named like a GPU (Adreno's is 3d00000.gpu).
    for card in sys::entries("/sys/class/drm") {
        let device = card.join("device");
        if let Some(busy) = sys::read::<f64>(device.join("gpu_busy_percent")) {
            m.gpu = Some((busy / 100.0) as f32);
            m.gpu_mhz = sys::text(device.join("pp_dpm_sclk")).as_deref().and_then(parse_dpm_clock);
            break;
        }
    }
    if m.gpu_mhz.is_none() {
        m.gpu_mhz = sys::entries("/sys/class/devfreq")
            .into_iter()
            .find(|d| d.file_name().is_some_and(|n| n.to_string_lossy().contains("gpu")))
            .and_then(|d| sys::read::<f64>(d.join("cur_freq")))
            .map(|hz| (hz / 1e6) as u32);
    }

    // Temperatures: hwmon chips by name and thermal zones by type, taking the hottest of each kind.
    let hottest = |slot: &mut Option<f32>, millideg: f64| {
        let c = (millideg / 1000.0) as f32;
        if c > -40.0 && c < 150.0 && slot.is_none_or(|t| c > t) {
            *slot = Some(c);
        }
    };
    for chip in sys::entries("/sys/class/hwmon") {
        let name = sys::read::<String>(chip.join("name")).unwrap_or_default();
        if name == "pwmfan" {
            m.fan_rpm = sys::read(chip.join("fan1_input"));
            continue;
        }
        let kind = match name.as_str() {
            "k10temp" | "coretemp" | "zenpower" | "cpu_thermal" => &mut m.cpu_temp,
            "amdgpu" | "nouveau" | "gpu_thermal" => &mut m.gpu_temp,
            _ => continue,
        };
        // temp1 is the package (Tctl) or the GPU edge on these chips.
        if let Some(t) = sys::read::<f64>(chip.join("temp1_input")) {
            hottest(kind, t);
        }
    }
    for zone in sys::entries("/sys/class/thermal") {
        let kind = sys::text(zone.join("type")).unwrap_or_default().to_ascii_lowercase();
        let Some(t) = sys::read::<f64>(zone.join("temp")) else {
            continue;
        };
        if kind.contains("cpu") {
            hottest(&mut m.cpu_temp, t);
        } else if kind.contains("gpu") {
            hottest(&mut m.gpu_temp, t);
        }
    }

    // Memory.
    if let Some(text) = sys::text("/proc/meminfo") {
        let (total, available, swap_total, swap_free) = parse_meminfo(&text);
        m.mem_total = total;
        m.mem_used = total.saturating_sub(available);
        m.swap_used = swap_total.saturating_sub(swap_free);
    }

    // Battery power and time left.
    if let Some(dir) = sys::system_battery() {
        let number = |name: &str| sys::read::<f64>(dir.join(name));
        let watts = number("power_now").map(|uw| uw / 1e6).or_else(|| Some(number("current_now")?.abs() * number("voltage_now")? / 1e12));
        let discharging = sys::read::<String>(dir.join("status")).as_deref() == Some("Discharging");
        m.power_w = watts.map(|w| if discharging { w as f32 } else { -(w as f32) });
        if discharging && let Some(w) = watts.filter(|&w| w > 0.1) {
            let wh = number("energy_now").map(|uwh| uwh / 1e6).or_else(|| Some(number("charge_now")? * number("voltage_now")? / 1e12));
            m.minutes_left = wh.map(|wh| (wh / w * 60.0) as u32);
        }
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
    fn reads_this_machine_without_failing() {
        let a = read_cpu_times();
        std::thread::sleep(Duration::from_millis(50));
        let m = read(&a, &read_cpu_times());
        eprintln!("{m:?}");
        assert!(m.mem_total > 0);
    }
}
