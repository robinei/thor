//! Applying a game's settings: its power profile (CPU and GPU frequency caps) and its frame rate cap
//! (MangoHud's limiter, through a per-game MangoHud config file that MangoHud reloads when it changes).
//!
//! Frequency caps write `scaling_max_freq` of each cpufreq policy and `max_freq` of the GPU's devfreq
//! device, which needs write access to them (on the device: a udev rule for the program's user).

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};

use sysprog_core::Power;

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

/// Caps every CPU cluster and the GPU for `power` (Performance: their full range).
pub fn apply_power(power: Power) {
    let f = fraction(power);
    for policy in fs::read_dir("/sys/devices/system/cpu/cpufreq").into_iter().flatten().flatten() {
        let dir = policy.path();
        if !dir.file_name().is_some_and(|n| n.to_string_lossy().starts_with("policy")) {
            continue;
        }
        let (Some(min), Some(max)) = (read_u64(&dir.join("cpuinfo_min_freq")), read_u64(&dir.join("cpuinfo_max_freq"))) else {
            continue;
        };
        let freq = pick(&read_list(&dir.join("scaling_available_frequencies")), min, max, f);
        write(&dir.join("scaling_max_freq"), freq);
    }
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
    log::info!("power profile: {}", power.label());
}

/// The MangoHud config file for a game (in `dir`), written with its frame rate cap (0: none): the
/// overlay invisible, logging for the FPS tile. Rewritten in place, which MangoHud picks up live.
pub fn mangohud_config(dir: &Path, game: &str, fps_cap: u16) -> std::io::Result<PathBuf> {
    fs::create_dir_all(dir)?;
    let hash = game.bytes().fold(0xcbf2_9ce4_8422_2325u64, |h, b| (h ^ u64::from(b)).wrapping_mul(0x0100_0000_01b3));
    let path = dir.join(format!("game-{hash:016x}.conf"));
    let text = format!(
        "alpha=0\nbackground_alpha=0\nautostart_log=1\nlog_interval=500\noutput_folder={}\nfps_limit={fps_cap}\n",
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
    fn a_games_config_file_carries_its_cap() {
        let dir = std::env::temp_dir().join(format!("sysprog-tuning-{}", std::process::id()));
        let a = mangohud_config(&dir, "steam:1", 40).unwrap();
        assert!(fs::read_to_string(&a).unwrap().contains("fps_limit=40\n"));
        let again = mangohud_config(&dir, "steam:1", 0).unwrap();
        assert_eq!(a, again, "the same file, rewritten");
        assert!(fs::read_to_string(&a).unwrap().contains("fps_limit=0\n"));
        assert_ne!(mangohud_config(&dir, "steam:2", 0).unwrap(), a);
        let _ = fs::remove_dir_all(&dir);
    }
}
