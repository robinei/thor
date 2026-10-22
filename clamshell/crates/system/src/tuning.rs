//! Applying settings: the power profile and CPU/GPU caps (frequency caps), the big cores, the charge
//! limit, and the frame rate cap and HUD (MangoHud, through a per-game MangoHud config file that MangoHud
//! reloads when it changes).
//!
//! These write `scaling_max_freq` of each cpufreq policy, `max_freq` of the GPU's devfreq device, CPUs'
//! `online`, the battery's `charge_control_end_threshold` and `constant_charge_current` (see [`Hold`]), and
//! the fan daemon's mode file, which needs write access to them (on the device: the `power` group's, given by
//! thor-system's udev rule `73-thor-power.rules`, the mode file by clamshell's tmpfiles entry). The hardware
//! is set on a thread of its own ([`spawn`]): taking cores offline can take a while, and drawing mustn't wait
//! for it.
//!
//! A cluster whose cores are all offline can't be read (its cpufreq policy's files say EBUSY): the
//! thread remembers each cluster from when it could, and brings a CPU it knows nothing of online to read
//! it.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::time::Duration;

use clamshell_core::PowerProfile;
use clamshell_core::fan;
use clamshell_core::settings::{Setting, Values};

use crate::pad;
use crate::sys;
use crate::worker::{self, Next, Quiet, Worker};

/// A profile's share of each frequency range.
fn fraction(power: PowerProfile) -> f64 {
    match power {
        PowerProfile::Battery => 0.55,
        PowerProfile::Balanced => 0.8,
        PowerProfile::Performance => 1.0,
    }
}

/// The frequency to cap at: `fraction` of the way from `min` to `max`, rounded down to an available
/// step if there is a list (the lowest step at least).
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
    sys::text(path).map(|s| s.split_whitespace().filter_map(|f| f.parse().ok()).collect()).unwrap_or_default()
}

/// Writes a sysfs value. Failing is worth one warning (missing write access fails every write, at every
/// change of the settings).
fn write(path: &Path, value: u64, quiet: &mut Quiet) {
    if let Err(e) = fs::write(path, value.to_string()) {
        quiet.failed("the settings need write access to cpufreq, devfreq and the battery", &format_args!("{}: {e}", path.display()));
    }
}

/// The fraction of each range a cap in percent stands for (0: the profile's).
fn share(power: PowerProfile, percent: u16) -> f64 {
    if percent == 0 { fraction(power) } else { f64::from(percent.min(100)) / 100.0 }
}

/// While a charge limit or Bypass is on: the battery's level looked at, and the charge current asserted
/// again, this often (a charger plugged in may undo it).
const CHARGE_AGAIN: Duration = Duration::from_secs(30);
/// Held at the limit by the charge current, charging starts again this many percent below it (Kettle's).
const CHARGE_RESUME: u64 = 5;

/// How a charge limit is held (`CLAMSHELL_CHARGE_LIMIT`, read at start; Bypass is always the current at 0).
/// `Current`, the default: by the charge current, 0 from the limit until the level is [`CHARGE_RESUME`] below
/// it, so the charger powers the device and the battery is left alone (Kettle's kettle-powerd, measured on a
/// Thor: SM8550's battery firmware never answers the end threshold's request, but takes the current, Armada's
/// patch 0903). `threshold`: the battery firmware's own end threshold, for trying on firmware that takes it
/// (Armada's patch 0904 makes a refused write fail). `firmware`: the battery firmware's own switches, which
/// AYN's and Retroid's Android drivers write (PB-OS's kernel patch, linux-thor's; untried on a Thor): its
/// 80 % limit (`retroid_limit_capacity`) for 80, running from USB with the battery left alone
/// (`retroid_charge_now` 0, and the current at 0 too) for Bypass, the charge current for 90 (the firmware has
/// no other level); written again at every look, as the firmware may drop them when the charger is plugged in
/// again. Without the switches: as `Current`.
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

/// Whether charging is held (the current at 0), for the charge limit `limit` held by the current (0:
/// Bypass, always; 100: never), the battery's level and whether it was held.
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

/// A cpufreq policy: its dir, min and max frequency, its CPUs.
type Policy = (PathBuf, u64, u64, Vec<u32>);

/// The cpufreq policies that can be read (those with a CPU online).
fn policies() -> Vec<Policy> {
    let mut out = Vec::new();
    for dir in sys::entries("/sys/devices/system/cpu/cpufreq") {
        if !dir.file_name().is_some_and(|n| n.to_string_lossy().starts_with("policy")) {
            continue;
        }
        let (Some(min), Some(max)) = (sys::read(dir.join("cpuinfo_min_freq")), sys::read(dir.join("cpuinfo_max_freq"))) else {
            continue;
        };
        let cpus = sys::text(dir.join("related_cpus")).map(|s| s.split_whitespace().filter_map(|c| c.parse().ok()).collect()).unwrap_or_default();
        out.push((dir, min, max, cpus));
    }
    out
}

/// The CPUs to take offline for "little cores only": all but the slowest cluster's, when there are
/// clusters of different speeds (never CPU 0).
pub fn big_cpus(clusters: &[(u64, Vec<u32>)]) -> Vec<u32> {
    let Some(slowest) = clusters.iter().map(|(max, _)| *max).min() else {
        return vec![];
    };
    clusters.iter().filter(|(max, _)| *max > slowest).flat_map(|(_, cpus)| cpus.iter().copied()).filter(|&c| c != 0).collect()
}

/// The hardware thread: applies the settings it is sent, only the latest when several wait; with a
/// charge limit or Bypass on, sees to the battery again every [`CHARGE_AGAIN`].
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
                // (ClamShell ending: the battery back to the firmware's own charging, so that a session
                // that doesn't come back doesn't leave it held.)
                Next::Disconnected => return charge.battery(100, &mut quiet),
            }
        }
    })
}

/// Where the settings in force go: the hardware thread, the pad's keymap, and a
/// game's MangoHud config (in `mangohud`).
#[derive(Clone)]
pub struct Tuning {
    pub hardware: Worker<Values>,
    pub pad: Worker<pad::Command>,
    pub mangohud: PathBuf,
}

impl Tuning {
    /// The keymap in force, for the pad.
    pub fn keymap(&self, keymap: &clamshell_core::keymap::Keymap) {
        self.pad.send(pad::Command::Keymap(Box::new(keymap.clone())));
    }

    /// The gyro calibrated from its next second.
    pub fn calibrate_gyro(&self) {
        self.pad.send(pad::Command::CalibrateGyro);
    }

    /// Applies the settings in force (`game`'s, or the global ones without a game): the hardware, and
    /// for a game its MangoHud config (written, and reloaded live if it runs), whose
    /// path is returned for its launch.
    pub fn apply(&self, v: &Values, game: Option<&str>) -> Option<PathBuf> {
        self.hardware.send(*v);
        self.pad.send(pad::Command::Rumble(v.get(Setting::Rumble)));
        let game = game?;
        mangohud_config(&self.mangohud, game, v.get(Setting::FpsCap), v.get(Setting::Hud)).map_err(|e| log::warn!("MangoHud config: {e}")).ok()
    }
}

/// Applies the settings in force to the hardware: frequency caps for the profile (or the CPU and GPU
/// caps where set), the big cores, the charge limit.
/// `known`: the policies as last read (see the module doc).
fn apply_hardware(v: &Values, known: &mut Vec<Policy>, charge: &mut Charge, quiet: &mut Quiet) {
    let power = PowerProfile::from_value(v.get(Setting::Power));
    // Cores first, so the caps reach the policies that stay online; twice, as a CPU of a cluster not
    // known yet comes online to be read, and may then belong offline.
    for _ in 0..2 {
        remember(known, policies());
        set_cores(v.big_cores(), known, quiet);
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
    // The fan's mode, for the fan daemon (where it runs: the file is there). One write of the whole
    // value: the file can't be replaced (/run isn't the session's), and the daemon takes an empty read,
    // between the truncation and the write, for no change.
    let mode = fan::Mode::of(v.get(Setting::Fan)).name();
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

/// The big cores offline (`big_cores` off) or online, by the clusters `known`; every other CPU online
/// (one of a cluster not known, so it can be read).
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
    /// Battery care (`limit`: the Charge limit setting, 0 Bypass), by [`Hold`]: only where the battery
    /// has the files. The end threshold written only if it differs (100 unless the threshold holds the
    /// limit); the charge current 0 while held, written each time then, as whether the firmware keeps it
    /// (unplugged, replugged) isn't known, else its maximum where it differs.
    fn battery(&mut self, limit: u16, quiet: &mut Quiet) {
        let switches = oem_switches();
        let firmware = self.hold == Hold::Firmware && switches.is_some();
        // The end threshold, the limit held by the current, the switches' charge_now and limit.
        let (by_threshold, by_current, charge_now, limit80) = match (self.hold, limit) {
            // (Bypass by the switch and the current both: the current is the one measured on a Thor.)
            (Hold::Firmware, 0) if firmware => (100, 0, 0, 0),
            (Hold::Firmware, 80) if firmware => (100, 100, 1, 1),
            (_, 0) => (100, 0, 1, 0),
            (Hold::Threshold, l) => (l, 100, 1, 0),
            (_, l) => (100, l, 1, 0),
        };
        // (Their files show what was last written, -1 before: the firmware's defaults written once.)
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

/// The battery firmware's own charge switches' device (the battery manager's), where the kernel has
/// them (see [`Hold`]).
fn oem_switches() -> Option<PathBuf> {
    sys::entries("/sys/bus/auxiliary/devices").into_iter().find(|d| d.join("retroid_charge_now").exists())
}

/// The MangoHud config file for a game (in `dir`), written with its frame rate cap (0: none) and HUD
/// (0: invisible, 1: the frame rate only, 2: the full overlay), logging for the FPS tile. Rewritten in
/// place, which MangoHud picks up live.
pub fn mangohud_config(dir: &Path, game: &str, fps_cap: u16, hud: u16) -> io::Result<PathBuf> {
    fs::create_dir_all(dir)?;
    // (Named by the entry's key, as its scopes, workspace and art are.)
    let path = dir.join(format!("{}.conf", clamshell_library::key_of(game)));
    let overlay = match hud {
        0 => "alpha=0\nbackground_alpha=0\n",
        1 => "fps_only\n",
        _ => "",
    };
    let text = format!("{overlay}autostart_log=1\nlog_interval=500\noutput_folder={}\nfps_limit={fps_cap}\n", dir.display());
    fs::write(&path, text)?;
    Ok(path)
}

/// The environment that loads MangoHud with this config file.
pub fn mangohud_env(config: &Path) -> Vec<(String, String)> {
    vec![("MANGOHUD".into(), "1".into()), ("MANGOHUD_CONFIGFILE".into(), config.display().to_string())]
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

    #[test]
    fn a_games_config_file_carries_its_cap_and_hud() {
        let dir = std::env::temp_dir().join(format!("clamshell-tuning-{}", std::process::id()));
        let a = mangohud_config(&dir, "steam:1", 40, 0).unwrap();
        let text = fs::read_to_string(&a).unwrap();
        assert!(text.contains("fps_limit=40\n") && text.contains("alpha=0\n"));
        let again = mangohud_config(&dir, "steam:1", 0, 1).unwrap();
        assert_eq!(a, again, "the same file, rewritten");
        let text = fs::read_to_string(&a).unwrap();
        assert!(text.contains("fps_limit=0\n") && text.contains("fps_only\n") && !text.contains("alpha"));
        assert_ne!(mangohud_config(&dir, "steam:2", 0, 0).unwrap(), a);
        let _ = fs::remove_dir_all(&dir);
    }
}
