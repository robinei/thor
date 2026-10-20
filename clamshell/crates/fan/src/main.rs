//! clamshell-fan: the fan daemon (see [`clamshell_core::fan`]), a system service started early in boot
//! and restarted if it stops. Every tick: the fan (the hwmon chip named `pwmfan`, looked for until it is
//! there: pwm-fan is a module), the mode ([`MODE_FILE`]), the hottest CPU or GPU thermal zone, and the
//! speed from those written when it changes. It takes the fan once found: `pwm1_enable` 3, which also
//! cuts the fan's supply when the speed is 0 (and during suspend).
//!
//!   clamshell-fan [ROOT]
//!
//! ROOT (default /): where /sys and /run are, for trying it on a copy.

use std::fs;
use std::path::{Path, PathBuf};

use clamshell_core::fan::{self, Duty, MODE_FILE, Mode};

/// The fan's hwmon directory under `root`, if it is there.
fn find_fan(root: &Path) -> Option<PathBuf> {
    entries(&root.join("sys/class/hwmon")).into_iter().find(|chip| text(&chip.join("name")).as_deref() == Some("pwmfan") && chip.join("pwm1").exists())
}

/// The hottest CPU or GPU thermal zone (°C), if any can be read.
fn hottest(root: &Path) -> Option<f32> {
    entries(&root.join("sys/class/thermal"))
        .into_iter()
        .filter(|zone| text(&zone.join("type")).is_some_and(|t| t.starts_with("cpu") || t.starts_with("gpu")))
        .filter_map(|zone| text(&zone.join("temp"))?.parse::<i32>().ok())
        .map(|millideg| millideg as f32 / 1000.0)
        // (A sensor reading nonsense isn't believed.)
        .filter(|&c| c > -40.0 && c < 150.0)
        .reduce(f32::max)
}

/// A file's text, trimmed.
fn text(path: &Path) -> Option<String> {
    fs::read_to_string(path).ok().map(|s| s.trim().to_owned())
}

fn entries(dir: &Path) -> Vec<PathBuf> {
    let mut out: Vec<PathBuf> = fs::read_dir(dir).map(|d| d.flatten().map(|e| e.path()).collect()).unwrap_or_default();
    out.sort();
    out
}

/// One tick: the speed set (or kept); the fan found, `fan` keeps it. What happened worth a line in the
/// log, if anything.
fn tick(root: &Path, fan: &mut Option<PathBuf>, mode_was: &mut Option<Mode>) -> Option<String> {
    let mut found = false;
    if fan.as_ref().is_none_or(|dir| !dir.join("pwm1").exists()) {
        *fan = find_fan(root);
        let dir = fan.as_ref()?;
        if let Err(e) = fs::write(dir.join("pwm1_enable"), "3") {
            return Some(format!("{}: pwm1_enable: {e}", dir.display()));
        }
        found = true;
    }
    let dir = fan.as_ref()?;
    let mode = text(&root.join(MODE_FILE.trim_start_matches('/'))).as_deref().and_then(Mode::parse).unwrap_or(Mode::Auto);
    let changed = *mode_was != Some(mode);
    *mode_was = Some(mode);
    let now: Duty = text(&dir.join("pwm1")).and_then(|t| t.parse().ok()).unwrap_or(0);
    let temp = hottest(root);
    let duty = fan::next(mode, temp, now);
    if duty != now
        && let Err(e) = fs::write(dir.join("pwm1"), duty.to_string())
    {
        return Some(format!("{}: pwm1: {e}", dir.display()));
    }
    match temp {
        None if now != fan::SAFE => Some("no temperature can be read: the fan at its safe speed".to_owned()),
        _ if found => Some(format!("the fan: {}, mode {}", dir.display(), mode.name())),
        _ if changed => Some(format!("mode {}", mode.name())),
        _ => None,
    }
}

fn main() {
    let root = PathBuf::from(std::env::args().nth(1).unwrap_or_else(|| "/".to_owned()));
    let (mut fan, mut mode) = (None, None);
    let mut said = None;
    loop {
        // (Each thing said once until something else is.)
        let line = tick(&root, &mut fan, &mut mode);
        if line.is_some() && line != said {
            if let Some(line) = &line {
                eprintln!("{line}");
            }
            said = line;
        }
        std::thread::sleep(fan::TICK);
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    /// A machine's /sys and /run in a temporary directory.
    fn machine(name: &str) -> PathBuf {
        let root = std::env::temp_dir().join(format!("clamshell-fan-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        for (dir, kind, temp) in [("thermal_zone0", "cpuss0", 72_000), ("thermal_zone1", "gpuss-0", 61_000), ("thermal_zone2", "pm8550_tz", 99_000)] {
            let zone = root.join("sys/class/thermal").join(dir);
            fs::create_dir_all(&zone).unwrap();
            fs::write(zone.join("type"), format!("{kind}\n")).unwrap();
            fs::write(zone.join("temp"), format!("{temp}\n")).unwrap();
        }
        fs::create_dir_all(root.join("sys/class/hwmon/hwmon0")).unwrap();
        fs::write(root.join("sys/class/hwmon/hwmon0/name"), "pm8550_tz\n").unwrap();
        root
    }

    fn add_fan(root: &Path) -> PathBuf {
        let dir = root.join("sys/class/hwmon/hwmon1");
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("name"), "pwmfan\n").unwrap();
        fs::write(dir.join("pwm1"), "70\n").unwrap();
        fs::write(dir.join("pwm1_enable"), "1\n").unwrap();
        dir
    }

    #[test]
    fn the_hottest_cpu_or_gpu_sensor() {
        let root = machine("hottest");
        assert_eq!(hottest(&root), Some(72.0), "the PMIC's zone isn't the CPU's or the GPU's");
    }

    #[test]
    fn waits_for_the_fan_then_takes_it_and_sets_it() {
        let root = machine("ticks");
        let (mut fan, mut mode) = (None, None);
        assert_eq!(tick(&root, &mut fan, &mut mode), None, "no fan yet");
        let dir = add_fan(&root);
        assert!(tick(&root, &mut fan, &mut mode).is_some_and(|l| l.starts_with("the fan") && l.ends_with("mode auto")));
        assert_eq!(text(&dir.join("pwm1_enable")).as_deref(), Some("3"));
        assert_eq!(text(&dir.join("pwm1")).as_deref(), Some(&*fan::next(Mode::Auto, Some(72.0), 70).to_string()));

        fs::create_dir_all(root.join("run")).unwrap();
        fs::write(root.join("run/clamshell-fan"), "max").unwrap();
        assert_eq!(tick(&root, &mut fan, &mut mode).as_deref(), Some("mode max"));
        assert_eq!(text(&dir.join("pwm1")).as_deref(), Some("255"));
        assert_eq!(tick(&root, &mut fan, &mut mode), None, "nothing new");
    }
}
