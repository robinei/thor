//! Applying settings: the power profile and CPU/GPU caps (frequency caps), the big cores, the charge
//! limit, and the frame rate cap and HUD (MangoHud, through a per-game MangoHud config file that MangoHud
//! reloads when it changes).
//!
//! These write `scaling_max_freq` of each cpufreq policy, `max_freq` of the GPU's devfreq device, CPUs'
//! `online`, and the battery's `charge_control_end_threshold`, which needs write access to them (on the
//! device: a udev rule for the program's user).

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};

use clamshell_core::Power;
use clamshell_core::settings::{Setting, Values};

/// A profile's share of each frequency range.
fn fraction(power: Power) -> f64 {
    match power {
        Power::Battery => 0.55,
        Power::Balanced => 0.8,
        Power::Performance => 1.0,
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

fn read_u64(path: &Path) -> Option<u64> {
    fs::read_to_string(path).ok()?.trim().parse().ok()
}

fn read_list(path: &Path) -> Vec<u64> {
    fs::read_to_string(path).map(|s| s.split_whitespace().filter_map(|f| f.parse().ok()).collect()).unwrap_or_default()
}

static WARNED: AtomicBool = AtomicBool::new(false);

fn write(path: &Path, value: u64) {
    if let Err(e) = fs::write(path, value.to_string())
        && !WARNED.swap(true, Ordering::Relaxed)
    {
        log::warn!("power profiles need write access to cpufreq and devfreq ({}: {e})", path.display());
    }
}

/// The fraction of each range a cap in percent stands for (0: the profile's).
fn share(power: Power, percent: u16) -> f64 {
    if percent == 0 { fraction(power) } else { f64::from(percent.min(100)) / 100.0 }
}

/// The cpufreq policies: (dir, min and max frequency, its CPUs).
fn policies() -> Vec<(PathBuf, u64, u64, Vec<u32>)> {
    let mut out = Vec::new();
    for policy in fs::read_dir("/sys/devices/system/cpu/cpufreq").into_iter().flatten().flatten() {
        let dir = policy.path();
        if !dir.file_name().is_some_and(|n| n.to_string_lossy().starts_with("policy")) {
            continue;
        }
        let (Some(min), Some(max)) = (read_u64(&dir.join("cpuinfo_min_freq")), read_u64(&dir.join("cpuinfo_max_freq"))) else {
            continue;
        };
        let cpus = fs::read_to_string(dir.join("related_cpus"))
            .map(|s| s.split_whitespace().filter_map(|c| c.parse().ok()).collect())
            .unwrap_or_default();
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

/// Applies the settings in force to the hardware: frequency caps for the profile (or the CPU and GPU
/// caps where set), the big cores, the charge limit.
pub fn apply_hardware(v: &Values) {
    let power = Power::from_value(v.get(Setting::Power));
    let policies = policies();
    // Cores first, so the caps reach the policies that stay online.
    let clusters: Vec<(u64, Vec<u32>)> = policies.iter().map(|(_, _, max, cpus)| (*max, cpus.clone())).collect();
    let off = if v.get(Setting::BigCores) == 0 { big_cpus(&clusters) } else { vec![] };
    for (_, cpus) in &clusters {
        for &cpu in cpus.iter().filter(|&&c| c != 0) {
            let online = Path::new("/sys/devices/system/cpu").join(format!("cpu{cpu}/online"));
            let want = u64::from(!off.contains(&cpu));
            if read_u64(&online) != Some(want) {
                write(&online, want);
            }
        }
    }
    let f = share(power, v.get(Setting::CpuMax));
    for (dir, min, max, _) in &policies {
        let freq = pick(&read_list(&dir.join("scaling_available_frequencies")), *min, *max, f);
        write(&dir.join("scaling_max_freq"), freq);
    }
    let f = share(power, v.get(Setting::GpuMax));
    for dev in fs::read_dir("/sys/class/devfreq").into_iter().flatten().flatten() {
        if !dev.file_name().to_string_lossy().contains("gpu") {
            continue;
        }
        let dir = dev.path();
        let steps = read_list(&dir.join("available_frequencies"));
        let (Some(&min), Some(&max)) = (steps.iter().min(), steps.iter().max()) else {
            continue;
        };
        write(&dir.join("max_freq"), pick(&steps, min, max, f));
    }
    // Battery care: only where the battery has the setting, and only if it differs.
    for supply in fs::read_dir("/sys/class/power_supply").into_iter().flatten().flatten() {
        let file = supply.path().join("charge_control_end_threshold");
        let limit = u64::from(v.get(Setting::ChargeLimit));
        if file.exists() && read_u64(&file) != Some(limit) {
            write(&file, limit);
        }
    }
    log::info!("settings applied: power {}", power.label());
}

/// The MangoHud config file for a game (in `dir`), written with its frame rate cap (0: none) and HUD
/// (0: invisible, 1: the frame rate only, 2: the full overlay), logging for the FPS tile. Rewritten in
/// place, which MangoHud picks up live.
pub fn mangohud_config(dir: &Path, game: &str, fps_cap: u16, hud: u16) -> std::io::Result<PathBuf> {
    fs::create_dir_all(dir)?;
    let hash = game.bytes().fold(0xcbf2_9ce4_8422_2325u64, |h, b| (h ^ u64::from(b)).wrapping_mul(0x0100_0000_01b3));
    let path = dir.join(format!("game-{hash:016x}.conf"));
    let overlay = match hud {
        0 => "alpha=0\nbackground_alpha=0\n",
        1 => "fps_only\n",
        _ => "",
    };
    let text = format!(
        "{overlay}autostart_log=1\nlog_interval=500\noutput_folder={}\nfps_limit={fps_cap}\n",
        dir.display()
    );
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
