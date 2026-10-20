//! Applying settings: the power profile and CPU/GPU caps (frequency caps), the big cores, the charge
//! limit, and the frame rate cap and HUD (MangoHud, through a per-game MangoHud config file that MangoHud
//! reloads when it changes).
//!
//! These write `scaling_max_freq` of each cpufreq policy, `max_freq` of the GPU's devfreq device, CPUs'
//! `online`, the battery's `charge_control_end_threshold` and `constant_charge_current`, and the fan
//! daemon's mode file, which needs write access to them (on the device: the `power` group's, given by
//! tools/device/system.sh). The hardware is set on a thread of its own ([`spawn`]):
//! taking cores offline can take a while, and drawing mustn't wait for it.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use clamshell_core::PowerProfile;
use clamshell_core::fan;
use clamshell_core::settings::{Setting, Values};

use crate::pad;
use crate::sys;
use crate::worker::{self, Quiet, Worker};

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

/// The cpufreq policies: (dir, min and max frequency, its CPUs).
fn policies() -> Vec<(PathBuf, u64, u64, Vec<u32>)> {
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

/// The hardware thread: applies the settings it is sent, only the latest when several wait.
pub fn spawn() -> io::Result<Worker<Values>> {
    let mut quiet = Quiet::default();
    worker::spawn_coalescing("tuning", |_| (), move |values| apply_hardware(&values, &mut quiet))
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
fn apply_hardware(v: &Values, quiet: &mut Quiet) {
    let power = PowerProfile::from_value(v.get(Setting::Power));
    let policies = policies();
    // Cores first, so the caps reach the policies that stay online.
    let clusters: Vec<(u64, Vec<u32>)> = policies.iter().map(|(_, _, max, cpus)| (*max, cpus.clone())).collect();
    let off = if !v.big_cores() { big_cpus(&clusters) } else { vec![] };
    for (_, cpus) in &clusters {
        for &cpu in cpus.iter().filter(|&&c| c != 0) {
            let online = Path::new("/sys/devices/system/cpu").join(format!("cpu{cpu}/online"));
            let want = u64::from(!off.contains(&cpu));
            if sys::read(&online) != Some(want) {
                write(&online, want, quiet);
            }
        }
    }
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
    // Battery care: only where the battery has the setting, and only if it differs. Bypass: the charge
    // current limited to 0 (the Thor's battery firmware, through Armada's kernel patch 0903), so that the
    // charger powers the device and the battery is left alone; written each time, as whether the firmware
    // keeps it (unplugged, replugged) isn't known.
    let limit = v.get(Setting::ChargeLimit);
    for supply in sys::power_supplies() {
        let file = supply.join("charge_control_end_threshold");
        let end = u64::from(if limit == 0 { 100 } else { limit });
        if file.exists() && sys::read(&file) != Some(end) {
            write(&file, end, quiet);
        }
        let current = supply.join("constant_charge_current");
        if let Some(max) = sys::read::<u64>(supply.join("constant_charge_current_max")).filter(|_| current.exists()) {
            let want = if limit == 0 { 0 } else { max };
            if limit == 0 || sys::read(&current) != Some(want) {
                write(&current, want, quiet);
            }
        }
    }
    // The fan's mode, for the fan daemon (where it runs: the file is there).
    let mode = fan::Mode::of(v.get(Setting::Fan)).name();
    if sys::text(fan::MODE_FILE).is_some_and(|now| now.trim() != mode)
        && let Err(e) = fs::write(fan::MODE_FILE, mode)
    {
        quiet.failed("the fan's mode needs write access to its file", &format_args!("{}: {e}", fan::MODE_FILE));
    }
    log::info!("settings applied: power {}", power.label());
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
