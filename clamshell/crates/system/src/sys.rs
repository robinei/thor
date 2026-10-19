//! Reading sysfs and procfs: a file's value, a class's devices, the processes, the battery that powers
//! the system. Everything here is optional: what a machine lacks reads as `None` or empty.

use std::fs;
use std::path::{Path, PathBuf};
use std::str::FromStr;

/// A file's text, if it can be read.
pub fn text(path: impl AsRef<Path>) -> Option<String> {
    fs::read_to_string(path).ok()
}

/// A file's value (surrounding whitespace ignored), if it can be read and parsed.
pub fn read<T: FromStr>(path: impl AsRef<Path>) -> Option<T> {
    text(path)?.trim().parse().ok()
}

/// The entries of a directory (a sysfs class: its devices), sorted; empty if it can't be read.
pub fn entries(dir: impl AsRef<Path>) -> Vec<PathBuf> {
    let mut out: Vec<PathBuf> = fs::read_dir(dir).map(|d| d.flatten().map(|e| e.path()).collect()).unwrap_or_default();
    out.sort();
    out
}

/// Every process: its pid and its /proc dir.
pub fn processes() -> impl Iterator<Item = (u32, PathBuf)> {
    fs::read_dir("/proc").into_iter().flatten().flatten().filter_map(|e| Some((e.file_name().to_str()?.parse().ok()?, e.path())))
}

/// A process, then its parent, and so on up to (not including) pid 1; as far as /proc says.
pub fn lineage(pid: u32) -> Vec<u32> {
    let mut out = Vec::new();
    let mut at = pid;
    // (A bound: /proc read as it changes can't loop forever.)
    while at > 1 && out.len() < 64 {
        out.push(at);
        let Some(stat) = text(format!("/proc/{at}/stat")) else {
            break;
        };
        // (Its fourth field, after the name in parentheses, which may hold anything.)
        let Some(parent) = stat.rsplit_once(')').and_then(|(_, rest)| rest.split_whitespace().nth(1)?.parse().ok()) else {
            break;
        };
        at = parent;
    }
    out
}

const POWER_SUPPLY: &str = "/sys/class/power_supply";

/// The battery that powers the system: the first power supply of type Battery that isn't a device's
/// (a mouse's or a headset's battery has the scope Device).
pub fn system_battery() -> Option<PathBuf> {
    system_battery_in(Path::new(POWER_SUPPLY))
}

fn system_battery_in(class: &Path) -> Option<PathBuf> {
    entries(class)
        .into_iter()
        .find(|dir| read::<String>(dir.join("type")).as_deref() == Some("Battery") && read::<String>(dir.join("scope")).as_deref() != Some("Device"))
}

/// Every power supply (for settings only some have, as the charge limit).
pub fn power_supplies() -> Vec<PathBuf> {
    entries(POWER_SUPPLY)
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    #[test]
    fn the_systems_battery_not_a_devices() {
        let class = std::env::temp_dir().join(format!("clamshell-sys-{}", std::process::id()));
        let _ = fs::remove_dir_all(&class);
        for (name, kind, scope) in [("AC", "Mains", None), ("hid-headset-battery", "Battery", Some("Device")), ("BAT0", "Battery", Some("System"))] {
            fs::create_dir_all(class.join(name)).unwrap();
            fs::write(class.join(name).join("type"), format!("{kind}\n")).unwrap();
            if let Some(scope) = scope {
                fs::write(class.join(name).join("scope"), scope).unwrap();
            }
        }
        assert_eq!(system_battery_in(&class), Some(class.join("BAT0")));
        assert_eq!(read::<u32>(class.join("BAT0/type")), None);
        assert_eq!(read::<String>(class.join("BAT0/type")).as_deref(), Some("Battery"));
        let _ = fs::remove_dir_all(&class);
    }

    #[test]
    fn this_process_is_listed() {
        assert!(processes().any(|(pid, _)| pid == std::process::id()));
    }
}
