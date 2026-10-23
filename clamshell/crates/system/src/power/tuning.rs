//! Applying settings to the hardware: the power profile and the CPU and GPU frequency caps, the big
//! cores, and the charge limit. The frame rate cap and HUD go to a game's MangoHud config instead (see
//! [`crate::readings::mangohud`]), and [`crate::wiring::Tuning`] sends each setting where it goes.
//!
//! This writes each cpufreq policy's `scaling_max_freq`, the GPU devfreq device's `max_freq`, the CPUs'
//! `online`, the battery's `charge_control_end_threshold` and `constant_charge_current` (see [`Hold`]),
//! and the fan daemon's mode file. That needs write access to them. On the device, the `power` group has
//! it through thor-system's udev rule `73-thor-power.rules`, and the mode file through clamshell's
//! tmpfiles entry. The hardware is set on a thread of its own ([`spawn`]), because taking cores offline
//! can take a while and drawing mustn't wait for it.
//!
//! A cluster whose cores are all offline can't be read, since its cpufreq policy's files return EBUSY.
//! The thread remembers each cluster from when it could read it, and brings a CPU it knows nothing
//! about online to read it.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::time::Duration;

use clamshell_state::settings::fan;
use clamshell_state::settings::{Cores, PowerProfile};
use clamshell_state::settings::{Setting, Values};

use crate::os::sys;
use crate::os::worker::{self, Next, Quiet, Worker};

/// A profile's share of each frequency range.
fn fraction(power: PowerProfile) -> f64 {
    match power {
        PowerProfile::Battery => 0.55,
        PowerProfile::Balanced => 0.8,
        PowerProfile::Performance => 1.0,
    }
}

/// The frequency to cap at: `fraction` of the way from `min` to `max`. If there is a list of available
/// steps, it is rounded down to one, but never below the lowest.
pub fn pick(available: &[u64], min: u64, max: u64, fraction: f64) -> u64 {
    let target = min + ((max.saturating_sub(min)) as f64 * fraction) as u64;
    let mut steps: Vec<u64> = available.to_vec();
    steps.sort_unstable();
    match steps.iter().rev().find(|&&f| f <= target) {
        Some(&f) => f,
        None => steps.first().copied().unwrap_or(target),
    }
}

fn read_list(path: &Path) -> Vec<u64> {
    sys::text(path)
        .map(|s| s.split_whitespace().filter_map(|f| f.parse().inspect_err(|e| log::warn!("tuning: {}: {f:?}: {e}", path.display())).ok()).collect())
        .unwrap_or_default()
}

/// Writes a sysfs value. A failure is warned about once, since missing write access fails every write at
/// every change of the settings.
fn write(path: &Path, value: u64, quiet: &mut Quiet) {
    if let Err(e) = fs::write(path, value.to_string()) {
        quiet.failed("the settings need write access to cpufreq, devfreq and the battery", &format_args!("{}: {e}", path.display()));
    }
}

/// The fraction of each range that a cap in percent stands for. 0 means the profile's fraction.
fn share(power: PowerProfile, percent: u16) -> f64 {
    if percent == 0 { fraction(power) } else { f64::from(percent.min(100)) / 100.0 }
}

/// While a charge limit or Bypass is on, how often the battery's level is checked and the charge current
/// set again, since plugging in a charger may undo it.
const CHARGE_AGAIN: Duration = Duration::from_secs(30);
/// When the charge current holds the battery at the limit, charging starts again this many percent below
/// it, as in Kettle.
const CHARGE_RESUME: u64 = 5;

/// How a charge limit is held, from `CLAMSHELL_CHARGE_LIMIT`, read at start. Bypass always sets the
/// current to 0.
///
/// - `Current`, the default, holds the limit with the charge current. The current is 0 from the limit
///   until the level is [`CHARGE_RESUME`] below it, so the charger powers the device and the battery is
///   left alone. This is Kettle's kettle-powerd, measured on a Thor: SM8550's battery firmware never
///   answers the end threshold's request but does take the current (Armada's patch 0903).
/// - `threshold` uses the battery firmware's own end threshold, for trying on firmware that takes it.
///   Armada's patch 0904 makes a refused write fail.
/// - `firmware` uses the battery firmware's own switches, which AYN's and Retroid's Android drivers write
///   (PB-OS's kernel patch, in linux-thor; untried on a Thor). For 80 it sets the firmware's 80 % limit
///   (`retroid_limit_capacity`). For Bypass it runs from USB with the battery left alone
///   (`retroid_charge_now` 0, with the current at 0 too). For 90 it uses the charge current, since the
///   firmware has no other level. The switches are written again at every check, as the firmware may
///   drop them when the charger is plugged in again. Without the switches it works like `Current`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Hold {
    Current,
    Threshold,
    Firmware,
}

impl Hold {
    fn from_env() -> Hold {
        match std::env::var("CLAMSHELL_CHARGE_LIMIT").as_deref() {
            Ok("threshold") => Hold::Threshold,
            Ok("firmware") => Hold::Firmware,
            _ => Hold::Current,
        }
    }
}

/// Whether charging is held, with the current at 0. `limit` is the charge limit held by the current: 0 is
/// Bypass, always held, and 100 is never held. `level` is the battery's level, and `was` whether it was
/// held.
pub fn held(limit: u16, level: Option<u64>, was: bool) -> bool {
    let limit = u64::from(limit);
    match (limit, level) {
        (0, _) => true,
        (100.., _) => false,
        (_, Some(level)) if level >= limit => true,
        (_, Some(level)) if level + CHARGE_RESUME <= limit => false,
        _ => was,
    }
}

/// A cpufreq policy: its dir, min and max frequency, and its CPUs.
type Policy = (PathBuf, u64, u64, Vec<u32>);

/// The cpufreq policies that can be read, which are those with a CPU online.
fn policies() -> Vec<Policy> {
    let mut out = Vec::new();
    for dir in sys::entries("/sys/devices/system/cpu/cpufreq") {
        if !dir.file_name().is_some_and(|n| n.to_string_lossy().starts_with("policy")) {
            continue;
        }
        let (Some(min), Some(max)) = (sys::read(dir.join("cpuinfo_min_freq")), sys::read(dir.join("cpuinfo_max_freq"))) else {
            continue;
        };
        let cpus = sys::text(dir.join("related_cpus"))
            .map(|s| {
                s.split_whitespace().filter_map(|c| c.parse().inspect_err(|e| log::warn!("tuning: {}/related_cpus: {c:?}: {e}", dir.display())).ok()).collect()
            })
            .unwrap_or_default();
        out.push((dir, min, max, cpus));
    }
    out
}

/// The CPUs to take offline for "little cores only": all but the slowest cluster's, when there are
/// clusters of different speeds. CPU 0 is never taken offline.
fn big_cpus(clusters: &[(u64, Vec<u32>)]) -> Vec<u32> {
    let Some(slowest) = clusters.iter().map(|(max, _)| *max).min() else {
        return vec![];
    };
    clusters.iter().filter(|(max, _)| *max > slowest).flat_map(|(_, cpus)| cpus.iter().copied()).filter(|&c| c != 0).collect()
}

/// Starts the hardware thread. It applies the settings it is sent, only the latest when several wait.
/// With a charge limit or Bypass on, it sees to the battery again every [`CHARGE_AGAIN`].
pub fn spawn() -> io::Result<Worker<Values>> {
    Worker::spawn("tuning", |commands| {
        let (mut quiet, mut known, mut last) = (Quiet::default(), Vec::new(), None::<Values>);
        let mut charge = Charge { hold: Hold::from_env(), held: false };
        log::info!(
            "charge limit held by {}",
            match charge.hold {
                Hold::Current => "the charge current",
                Hold::Threshold => "the battery's threshold",
                Hold::Firmware => "the battery firmware's switches",
            }
        );
        loop {
            let limit = last.map(|v| v.get(Setting::ChargeLimit));
            let every = limit.is_some_and(|l| l != 100).then_some(CHARGE_AGAIN);
            match worker::next(&commands, every) {
                Next::Command(first) => {
                    let Some(values) = worker::coalesce(first, &commands, |_| ()).pop() else { continue };
                    apply_hardware(&values, &mut known, &mut charge, &mut quiet);
                    last = Some(values);
                }
                Next::Elapsed => charge.battery(limit.unwrap_or(100), &mut quiet),
                // When ClamShell ends, hand the battery back to the firmware's own charging, so that a
                // session that doesn't come back doesn't leave it held.
                Next::Disconnected => return charge.battery(100, &mut quiet),
            }
        }
    })
}

/// Applies the settings in force to the hardware: frequency caps for the profile, or the CPU and GPU caps
/// where set, the big cores and the charge limit. `known` holds the policies as last read (see the
/// module doc).
fn apply_hardware(v: &Values, known: &mut Vec<Policy>, charge: &mut Charge, quiet: &mut Quiet) {
    let power: PowerProfile = v.choice();
    // Set the cores first, so the caps reach the policies that stay online. Do it twice, because a CPU of
    // a cluster not known yet comes online to be read, and may then belong offline.
    for _ in 0..2 {
        remember(known, policies());
        set_cores(v.choice::<Cores>() == Cores::All, known, quiet);
    }
    let policies = policies();
    let f = share(power, v.get(Setting::CpuMax));
    for (dir, min, max, _) in &policies {
        let freq = pick(&read_list(&dir.join("scaling_available_frequencies")), *min, *max, f);
        write(&dir.join("scaling_max_freq"), freq, quiet);
    }
    let f = share(power, v.get(Setting::GpuMax));
    for dir in sys::entries("/sys/class/devfreq") {
        if !dir.file_name().is_some_and(|n| n.to_string_lossy().contains("gpu")) {
            continue;
        }
        let steps = read_list(&dir.join("available_frequencies"));
        let (Some(&min), Some(&max)) = (steps.iter().min(), steps.iter().max()) else {
            continue;
        };
        write(&dir.join("max_freq"), pick(&steps, min, max, f), quiet);
    }
    charge.battery(v.get(Setting::ChargeLimit), quiet);
    // The fan's mode, for the fan daemon, where it runs and the file exists. The whole value is written
    // in one write. The file can't be replaced, as /run isn't the session's, and the daemon treats an
    // empty read between the truncation and the write as no change.
    let mode = v.choice::<fan::Mode>().name();
    if sys::text(fan::MODE_FILE).is_some_and(|now| now.trim() != mode)
        && let Err(e) = fs::write(fan::MODE_FILE, mode)
    {
        quiet.failed("the fan's mode needs write access to its file", &format_args!("{}: {e}", fan::MODE_FILE));
    }
    log::info!("settings applied: power {}", power.label());
}

/// Each policy read replaces the one known from the same dir.
fn remember(known: &mut Vec<Policy>, read: Vec<Policy>) {
    for p in read {
        known.retain(|k| k.0 != p.0);
        known.push(p);
    }
}

/// Takes the big cores offline if `big_cores` is off, or brings them online, using the clusters in
/// `known`. Every other CPU is brought online, including any of a cluster not known yet, so it can be
/// read.
fn set_cores(big_cores: bool, known: &[Policy], quiet: &mut Quiet) {
    let clusters: Vec<(u64, Vec<u32>)> = known.iter().map(|(_, _, max, cpus)| (*max, cpus.clone())).collect();
    let off = if big_cores { vec![] } else { big_cpus(&clusters) };
    for dir in sys::entries("/sys/devices/system/cpu") {
        let Some(cpu) = dir.file_name().and_then(|n| n.to_str()?.strip_prefix("cpu")?.parse::<u32>().ok()) else {
            continue;
        };
        let online = dir.join("online");
        let want = u64::from(!off.contains(&cpu));
        if cpu != 0 && online.exists() && sys::read(&online) != Some(want) {
            write(&online, want, quiet);
        }
    }
}

/// The charge limit's state: how it is held, and whether charging is held now.
struct Charge {
    hold: Hold,
    held: bool,
}

impl Charge {
    /// Battery care by [`Hold`], where the battery has the files. `limit` is the Charge limit setting,
    /// with 0 for Bypass.
    ///
    /// The end threshold is written only if it differs; it is 100 unless the threshold holds the limit.
    /// While charging is held, the charge current is set to 0 every time, since it isn't known whether
    /// the firmware keeps it across unplugging and replugging. Otherwise it is set to its maximum where
    /// it differs.
    fn battery(&mut self, limit: u16, quiet: &mut Quiet) {
        let switches = oem_switches();
        let firmware = self.hold == Hold::Firmware && switches.is_some();
        // The end threshold, the limit held by the current, and the switches' charge_now and limit.
        let (by_threshold, by_current, charge_now, limit80) = match (self.hold, limit) {
            // Bypass by both the switch and the current, since the current is the one measured on a Thor.
            (Hold::Firmware, 0) if firmware => (100, 0, 0, 0),
            (Hold::Firmware, 80) if firmware => (100, 100, 1, 1),
            (_, 0) => (100, 0, 1, 0),
            (Hold::Threshold, l) => (l, 100, 1, 0),
            (_, l) => (100, l, 1, 0),
        };
        // The switch files show what was last written, or -1 before that, so the firmware's defaults are
        // written once.
        if let Some(dir) = switches {
            for (name, want) in [("retroid_charge_now", charge_now), ("retroid_limit_capacity", limit80)] {
                let file = dir.join(name);
                if firmware || sys::read::<u64>(&file) != Some(want) {
                    write(&file, want, quiet);
                }
            }
        }
        for supply in sys::power_supplies() {
            let file = supply.join("charge_control_end_threshold");
            let end = u64::from(by_threshold);
            if file.exists() && sys::read(&file) != Some(end) {
                write(&file, end, quiet);
            }
            let current = supply.join("constant_charge_current");
            if let Some(max) = sys::read::<u64>(supply.join("constant_charge_current_max")).filter(|_| current.exists()) {
                let held = held(by_current, sys::read(supply.join("capacity")), self.held);
                if held != self.held {
                    log::info!("charging {} ({} %)", if held { "held" } else { "on" }, sys::text(supply.join("capacity")).unwrap_or_default().trim());
                }
                self.held = held;
                let want = if held { 0 } else { max };
                if held || sys::read(&current) != Some(want) {
                    write(&current, want, quiet);
                }
            }
        }
    }
}

/// The device with the battery firmware's own charge switches, the battery manager, where the kernel has
/// them (see [`Hold`]).
fn oem_switches() -> Option<PathBuf> {
    sys::entries("/sys/bus/auxiliary/devices").into_iter().find(|d| d.join("retroid_charge_now").exists())
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    #[test]
    fn picks_a_step_at_or_below_the_target() {
        let steps = [300_000, 600_000, 1_000_000, 1_800_000, 2_400_000];
        assert_eq!(pick(&steps, 300_000, 2_400_000, 1.0), 2_400_000);
        assert_eq!(pick(&steps, 300_000, 2_400_000, 0.8), 1_800_000);
        assert_eq!(pick(&steps, 300_000, 2_400_000, 0.0), 300_000);
        assert_eq!(pick(&[], 1000, 2000, 0.5), 1500, "no list: the target itself");
    }

    #[test]
    fn a_charge_limit_holds_at_the_limit_and_charges_again_five_below() {
        assert!(held(0, Some(40), false), "Bypass: always");
        assert!(!held(100, Some(100), true), "full: never");
        assert!(!held(80, Some(79), false));
        assert!(held(80, Some(80), false), "at the limit");
        assert!(held(80, Some(76), true), "still held above 75");
        assert!(!held(80, Some(75), true), "five below: charging again");
        assert!(!held(80, Some(78), false), "charging on up to the limit");
        assert!(held(80, None, true) && !held(80, None, false), "no level: as it was");
    }

    #[test]
    fn only_the_slowest_cluster_stays_online() {
        let thor = [(2_016_000, vec![0, 1, 2]), (2_803_000, vec![3, 4, 5, 6]), (3_187_000, vec![7])];
        assert_eq!(big_cpus(&thor), vec![3, 4, 5, 6, 7]);
        assert_eq!(big_cpus(&[(4_000_000, vec![0, 1]), (4_000_000, vec![2, 3])]), Vec::<u32>::new(), "all alike");
        assert_eq!(big_cpus(&[(1, vec![1]), (2, vec![0, 2])]), vec![2], "never CPU 0");
        assert_eq!(big_cpus(&[]), Vec::<u32>::new());
    }
}
