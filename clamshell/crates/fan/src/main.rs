//! clamshell-fan: the fan daemon (see [`clamshell_core::fan`]), a system service started early in boot
//! and restarted if it stops. Every tick: the fan (the hwmon chip named `pwmfan`, looked for until it is
//! there: pwm-fan is a module), the mode ([`MODE_FILE`]), the CPU and GPU thermal zones, and the speed
//! from those written when it changes. It takes the fan once found: `pwm1_enable` 3, which also cuts the
//! fan's supply when the speed is 0 (and during suspend).
//!
//!   `clamshell-fan [ROOT]`
//!   `clamshell-fan --safe [ROOT]`
//!
//! ROOT (default /): where /sys and /run are, for trying it on a copy. `--safe`: the fan to its safe speed
//! ([`fan::SAFE`]), then exit: as the service stops (its `ExecStopPost`), so that a stopped daemon
//! doesn't leave the fan off.

use std::fs;
use std::path::{Path, PathBuf};

use clamshell_core::fan::{self, Duty, MODE_FILE, Mode};

/// The fan's hwmon directory under `root`, if it is there.
fn find_fan(root: &Path) -> Option<PathBuf> {
    entries(&root.join("sys/class/hwmon")).into_iter().find(|chip| text(&chip.join("name")).as_deref() == Some("pwmfan") && chip.join("pwm1").exists())
}

/// The CPU and GPU thermal zones' readings (°C).
fn readings(root: &Path) -> Vec<f32> {
    entries(&root.join("sys/class/thermal"))
        .into_iter()
        .filter(|zone| text(&zone.join("type")).is_some_and(|t| t.starts_with("cpu") || t.starts_with("gpu")))
        .filter_map(|zone| text(&zone.join("temp"))?.parse::<i32>().ok())
        .map(|millideg| millideg as f32 / 1000.0)
        .collect()
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

/// What the daemon keeps from tick to tick.
#[derive(Debug, Default)]
struct Daemon {
    /// The fan's hwmon directory, once found.
    fan: Option<PathBuf>,
    /// `pwm1_enable` set (tried again each tick until it is).
    taken: bool,
    /// The mode in force: the mode file's, kept while the file can't be read or is half written.
    mode: Option<Mode>,
    /// The mode last logged.
    said: Option<Mode>,
    /// The temperature, smoothed.
    temp: Option<f32>,
}

/// One tick: the speed set (or kept). What happened worth a line in the log, if anything.
fn tick(root: &Path, d: &mut Daemon) -> Option<String> {
    let mut found = false;
    if d.fan.as_ref().is_none_or(|dir| !dir.join("pwm1").exists()) {
        d.fan = find_fan(root);
        d.taken = false;
        found = d.fan.is_some();
    }
    let dir = d.fan.clone()?;
    let mut problem = None;
    if !d.taken {
        match fs::write(dir.join("pwm1_enable"), "3") {
            Ok(()) => d.taken = true,
            Err(e) => problem = Some(format!("{}: pwm1_enable: {e}", dir.display())),
        }
    }
    // (ClamShell writes the file in place: empty or partial for a moment.)
    if let Some(mode) = text(&root.join(MODE_FILE.trim_start_matches('/'))).as_deref().and_then(Mode::parse) {
        d.mode = Some(mode);
    }
    let mode = d.mode.unwrap_or(Mode::Auto);
    let changed = d.said != Some(mode);
    d.said = Some(mode);
    let now: Duty = text(&dir.join("pwm1")).and_then(|t| t.parse().ok()).unwrap_or(0);
    d.temp = fan::smooth(d.temp, fan::temperature(readings(root)));
    let duty = fan::next(mode, d.temp, now);
    if duty != now
        && let Err(e) = fs::write(dir.join("pwm1"), duty.to_string())
    {
        return Some(format!("{}: pwm1: {e}", dir.display()));
    }
    match d.temp {
        _ if problem.is_some() => problem,
        None if mode != Mode::Max => Some("no temperature can be read: the fan at its safe speed".to_owned()),
        _ if found => Some(format!("the fan: {}, mode {}", dir.display(), mode.name())),
        _ if changed => Some(format!("mode {}", mode.name())),
        _ => None,
    }
}

/// The fan to its safe speed (the service stopping).
fn safe(root: &Path) -> Result<(), String> {
    let dir = find_fan(root).ok_or("no fan")?;
    fs::write(dir.join("pwm1"), fan::SAFE.to_string()).map_err(|e| format!("{}: pwm1: {e}", dir.display()))
}

fn main() {
    let mut args = std::env::args().skip(1).peekable();
    let to_safe = args.next_if(|a| a == "--safe").is_some();
    let root = PathBuf::from(args.next().unwrap_or_else(|| "/".to_owned()));
    if to_safe {
        if let Err(e) = safe(&root) {
            eprintln!("{e}");
            std::process::exit(1);
        }
        return;
    }
    let mut daemon = Daemon::default();
    let mut said = None;
    loop {
        // (Each thing said once until something else is.)
        let line = tick(&root, &mut daemon);
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
    fn machine(name: &str) -> clamshell_core::testdir::TempDir {
        let root = clamshell_core::testdir::TempDir::new(&format!("fan-{name}"));
        for (dir, kind, temp) in [("thermal_zone0", "cpuss0", 72_000), ("thermal_zone1", "gpuss-0", 61_000), ("thermal_zone2", "pm8550_tz", 99_000)] {
            let zone = root.join("sys/class/thermal").join(dir);
            fs::create_dir_all(&zone).unwrap();
            fs::write(zone.join("type"), format!("{kind}\n")).unwrap();
            fs::write(zone.join("temp"), format!("{temp}\n")).unwrap();
        }
        fs::create_dir_all(root.join("sys/class/hwmon/hwmon0")).unwrap();
        fs::write(root.join("sys/class/hwmon/hwmon0/name"), "pm8550_tz\n").unwrap();
        fs::create_dir_all(root.join("run")).unwrap();
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
    fn the_cpu_and_gpu_sensors_only() {
        let root = machine("readings");
        assert_eq!(readings(&root), vec![72.0, 61.0], "the PMIC's zone isn't the CPU's or the GPU's");
    }

    #[test]
    fn waits_for_the_fan_then_takes_it_and_sets_it() {
        let root = machine("ticks");
        let mut d = Daemon::default();
        assert_eq!(tick(&root, &mut d), None, "no fan yet");
        let dir = add_fan(&root);
        assert!(tick(&root, &mut d).is_some_and(|l| l.starts_with("the fan") && l.ends_with("mode auto")));
        assert_eq!(text(&dir.join("pwm1_enable")).as_deref(), Some("3"));
        assert_eq!(text(&dir.join("pwm1")).as_deref(), Some(&*fan::next(Mode::Auto, Some(66.5), 70).to_string()));

        fs::write(root.join("run/clamshell-fan"), "max").unwrap();
        assert_eq!(tick(&root, &mut d).as_deref(), Some("mode max"));
        assert_eq!(tick(&root, &mut d), None, "nothing new");
        // (Half written: the mode kept.)
        fs::write(root.join("run/clamshell-fan"), "").unwrap();
        assert_eq!(tick(&root, &mut d), None);
        assert_eq!(d.mode, Some(Mode::Max));
        for _ in 0..8 {
            tick(&root, &mut d);
        }
        assert_eq!(text(&dir.join("pwm1")).as_deref(), Some("255"), "full, a step at a time");
    }

    #[test]
    fn taking_the_fan_is_tried_again() {
        let root = machine("retake");
        let dir = add_fan(&root);
        fs::remove_file(dir.join("pwm1_enable")).unwrap();
        fs::create_dir(dir.join("pwm1_enable")).unwrap();
        let mut d = Daemon::default();
        assert!(tick(&root, &mut d).is_some_and(|l| l.contains("pwm1_enable")));
        assert!(!d.taken);
        fs::remove_dir(dir.join("pwm1_enable")).unwrap();
        tick(&root, &mut d);
        assert!(d.taken);
        assert_eq!(text(&dir.join("pwm1_enable")).as_deref(), Some("3"));
    }

    #[test]
    fn stopping_leaves_it_at_the_safe_speed() {
        let root = machine("safe");
        assert!(safe(&root).is_err(), "no fan");
        let dir = add_fan(&root);
        safe(&root).unwrap();
        assert_eq!(text(&dir.join("pwm1")).as_deref(), Some("128"));
    }
}
