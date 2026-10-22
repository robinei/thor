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

/// When process `pid` started (clock ticks after boot: `/proc/<pid>/stat`'s 22nd field); none if it is
/// gone. With its id, it tells the process from a later one given the same id.
fn start_time(pid: u32) -> Option<u64> {
    stat_field(&text(format!("/proc/{pid}/stat"))?, 22)
}

/// Process `pid`'s parent (`/proc/<pid>/stat`'s 4th field).
pub fn parent(pid: u32) -> Option<u32> {
    parent_in(&text(format!("/proc/{pid}/stat"))?)
}

/// The parent in a `/proc/<pid>/stat` file's text.
pub fn parent_in(stat: &str) -> Option<u32> {
    stat_field(stat, 4)
}

/// Field `n` (from 1, as proc(5) numbers them) of a `/proc/<pid>/stat` file, from the 3rd on. The 2nd, the
/// name in parentheses, may hold anything, spaces and parentheses too, so fields are counted after its
/// last `)`.
fn stat_field<T: std::str::FromStr>(stat: &str, n: usize) -> Option<T> {
    stat.rsplit_once(')')?.1.split_whitespace().nth(n.checked_sub(3)?)?.parse().ok()
}

/// The windows' and tray icons' processes as last seen (their start times): what ClamShell acts on by a
/// process id (adopting it, ending it) is checked to be the same process still.
static SEEN: std::sync::LazyLock<std::sync::Mutex<std::collections::HashMap<u32, u64>>> = std::sync::LazyLock::new(Default::default);

/// Process `pid` seen (a window's, a tray icon's): its start time, remembered.
pub fn saw(pid: u32) -> Option<u64> {
    let start = start_time(pid)?;
    let mut seen = crate::lock(&SEEN);
    // (Bounded: those of windows long gone forgotten now and then.)
    if seen.len() > 1024 {
        seen.clear();
    }
    seen.insert(pid, start);
    Some(start)
}

/// Whether process `pid` is the one last seen with that id (one never seen: as far as is known, yes).
pub fn same_process(pid: u32) -> bool {
    let known = crate::lock(&SEEN).get(&pid).copied();
    known.is_none_or(|start| start_time(pid) == Some(start))
}

/// A process, then its parent, and so on up to (not including) pid 1; as far as /proc says.
pub fn lineage(pid: u32) -> Vec<u32> {
    let mut out = Vec::new();
    let mut at = pid;
    // (A bound: /proc read as it changes can't loop forever.)
    while at > 1 && out.len() < 64 {
        out.push(at);
        let Some(parent) = parent(at) else {
            break;
        };
        at = parent;
    }
    out
}

/// `d` as poll(2)'s timeout.
pub fn timespec(d: std::time::Duration) -> rustix::event::Timespec {
    rustix::event::Timespec { tv_sec: i64::try_from(d.as_secs()).unwrap_or(i64::MAX), tv_nsec: i64::from(d.subsec_nanos()) }
}

/// A line of the mount table.
pub struct MountLine {
    pub device: String,
    pub dir: PathBuf,
    pub fs: String,
}

/// The mount table (/proc/self/mounts), in the order mounted, its names unescaped (`\040`: a space).
pub fn mount_table() -> Vec<MountLine> {
    let text = text("/proc/self/mounts").unwrap_or_default();
    text.lines()
        .filter_map(|line| {
            let mut fields = line.split(' ');
            Some(MountLine { device: unescape(fields.next()?), dir: PathBuf::from(unescape(fields.next()?)), fs: fields.next()?.to_owned() })
        })
        .collect()
}

/// `\040` (mounts) and `\x20` (udev's names) as the characters they stand for.
pub fn unescape(text: &str) -> String {
    let bytes = text.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while let Some(&b) = bytes.get(i) {
        let octal = bytes.get(i + 1..i + 4).filter(|d| d.iter().all(|c| (b'0'..=b'7').contains(c)));
        let hex = bytes
            .get(i + 1..i + 4)
            .filter(|d| d.first() == Some(&b'x'))
            .and_then(|d| std::str::from_utf8(d.get(1..)?).ok())
            .and_then(|h| u8::from_str_radix(h, 16).ok());
        match (b, octal, hex) {
            (b'\\', Some(d), _) => {
                out.push(d.iter().fold(0u8, |n, c| n.wrapping_mul(8).wrapping_add(c - b'0')));
                i += 4;
            }
            (b'\\', None, Some(h)) => {
                out.push(h);
                i += 4;
            }
            _ => {
                out.push(b);
                i += 1;
            }
        }
    }
    String::from_utf8_lossy(&out).into_owned()
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
    fn escaped_names() {
        assert_eq!(unescape(r"/run/media/u/My\040Card"), "/run/media/u/My Card");
        assert_eq!(unescape(r"My\x20Card"), "My Card");
        assert_eq!(unescape(r"a\b\0"), r"a\b\0", "not an escape");
    }

    #[test]
    fn stat_fields_count_after_the_name() {
        // A name with spaces and parentheses, as a process may give itself.
        let stat = "4242 (a (b) c) S 17 4242 4242 0 -1 4194560 120 0 0 0 1 0 0 0 20 0 1 0 98765 0 0";
        assert_eq!(parent_in(stat), Some(17));
        assert_eq!(stat_field::<u64>(stat, 22), Some(98765), "the start time");
        assert_eq!(stat_field::<u64>(stat, 2), None, "the name isn't a field to read");
    }

    #[test]
    fn the_systems_battery_not_a_devices() {
        let class = clamshell_core::testdir::TempDir::new("sys");
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
    }

    #[test]
    fn this_process_is_listed() {
        assert!(processes().any(|(pid, _)| pid == std::process::id()));
    }
}

#[cfg(test)]
mod process_tests {
    use super::*;

    #[test]
    fn a_process_is_known_by_its_id_and_start() {
        let me = std::process::id();
        assert!(start_time(me).is_some());
        assert_eq!(saw(me), start_time(me));
        assert!(same_process(me));
        // Another start under the same id: not the same.
        crate::lock(&SEEN).insert(me, 1);
        assert!(!same_process(me));
        assert!(same_process(u32::MAX - 1), "never seen: as far as is known");
    }
}
