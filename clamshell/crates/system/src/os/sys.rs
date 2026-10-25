//! Reading sysfs and procfs: a file's value, a class's devices, the processes, the battery that powers
//! the system. Everything here is optional. Whatever a machine lacks reads as `None` or empty.

use std::fs;
use std::path::{Path, PathBuf};
use std::str::FromStr;

/// A file's text, if it can be read.
pub fn text(path: impl AsRef<Path>) -> Option<String> {
    fs::read_to_string(path).ok()
}

/// A file's value, ignoring surrounding whitespace, if it can be read and parsed.
pub fn read<T: FromStr>(path: impl AsRef<Path>) -> Option<T> {
    text(path)?.trim().parse().ok()
}

/// The entries of a directory, such as the devices of a sysfs class, sorted. Empty if it can't be read.
pub fn entries(dir: impl AsRef<Path>) -> Vec<PathBuf> {
    let mut out: Vec<PathBuf> = fs::read_dir(dir).map(|d| d.flatten().map(|e| e.path()).collect()).unwrap_or_default();
    out.sort();
    out
}

/// Every process: its pid and its /proc dir.
pub fn processes() -> impl Iterator<Item = (u32, PathBuf)> {
    fs::read_dir("/proc").into_iter().flatten().flatten().filter_map(|e| Some((e.file_name().to_str()?.parse().ok()?, e.path())))
}

/// When process `pid` started, in clock ticks after boot, from the 22nd field of `/proc/<pid>/stat`.
/// None if the process is gone. Together with the id, it tells the process apart from a later one given
/// the same id.
fn start_time(pid: u32) -> Option<u64> {
    stat_field(&text(format!("/proc/{pid}/stat"))?, 22)
}

/// The parent of process `pid`, from the 4th field of `/proc/<pid>/stat`.
pub fn parent(pid: u32) -> Option<u32> {
    parent_in(&text(format!("/proc/{pid}/stat"))?)
}

/// The parent in a `/proc/<pid>/stat` file's text.
pub fn parent_in(stat: &str) -> Option<u32> {
    stat_field(stat, 4)
}

/// Field `n` of a `/proc/<pid>/stat` file, counted from 1 as proc(5) numbers them. Only the 3rd field on
/// can be read. The 2nd is the name in parentheses, which may hold anything including spaces and
/// parentheses, so fields are counted after the last `)`.
fn stat_field<T: std::str::FromStr>(stat: &str, n: usize) -> Option<T> {
    stat.rsplit_once(')')?.1.split_whitespace().nth(n.checked_sub(3)?)?.parse().ok()
}

/// The start times of the windows' and tray icons' processes as last seen. When ClamShell acts on a
/// process by its id, for example to adopt or end it, this is used to check it is still the same process.
static SEEN: std::sync::LazyLock<std::sync::Mutex<std::collections::HashMap<u32, u64>>> = std::sync::LazyLock::new(Default::default);

/// Remembers the start time of process `pid`, seen as a window's or a tray icon's, and returns it.
pub fn saw(pid: u32) -> Option<u64> {
    let start = start_time(pid)?;
    let mut seen = crate::os::lock(&SEEN);
    // Keep the map bounded by forgetting everything now and then, including windows long gone.
    if seen.len() > 1024 {
        seen.clear();
    }
    seen.insert(pid, start);
    Some(start)
}

/// Whether process `pid` is the one last seen with that id. A process never seen counts as the same.
pub fn same_process(pid: u32) -> bool {
    let known = crate::os::lock(&SEEN).get(&pid).copied();
    known.is_none_or(|start| start_time(pid) == Some(start))
}

/// The Steam game a process belongs to, by the `SteamAppId` Steam gives its games' environment.
pub fn steam_app_of(pid: u32) -> Option<u32> {
    steam_app_in(&fs::read(format!("/proc/{pid}/environ")).ok()?)
}

fn steam_app_in(environ: &[u8]) -> Option<u32> {
    environ.split(|b| *b == 0).find_map(|var| std::str::from_utf8(var.strip_prefix(b"SteamAppId=")?).ok()?.parse().ok()).filter(|id| *id != 0)
}

/// A process, then its parent, and so on up to but not including pid 1, as far as /proc says.
pub fn lineage(pid: u32) -> Vec<u32> {
    let mut out = Vec::new();
    let mut at = pid;
    // A bound, so reading /proc while it changes can't loop forever.
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

/// The mount table from /proc/self/mounts, in the order mounted, with its names unescaped (`\040` is a
/// space).
pub fn mount_table() -> Vec<MountLine> {
    let text = text("/proc/self/mounts").unwrap_or_default();
    text.lines()
        .filter_map(|line| {
            let mut fields = line.split(' ');
            Some(MountLine {
                device: super::text::unescape(fields.next()?),
                dir: PathBuf::from(super::text::unescape(fields.next()?)),
                fs: fields.next()?.to_owned(),
            })
        })
        .collect()
}

const POWER_SUPPLY: &str = "/sys/class/power_supply";

/// The battery that powers the system: the first power supply of type Battery that doesn't belong to a
/// device. A mouse's or a headset's battery has the scope Device.
pub fn system_battery() -> Option<PathBuf> {
    system_battery_in(Path::new(POWER_SUPPLY))
}

fn system_battery_in(class: &Path) -> Option<PathBuf> {
    entries(class)
        .into_iter()
        .find(|dir| read::<String>(dir.join("type")).as_deref() == Some("Battery") && read::<String>(dir.join("scope")).as_deref() != Some("Device"))
}

/// Every power supply, for settings only some of them have, such as the charge limit.
pub fn power_supplies() -> Vec<PathBuf> {
    entries(POWER_SUPPLY)
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    #[test]
    fn escaped_names() {}

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
        let tmp = tempfile::tempdir().unwrap();
        let class = tmp.path();
        for (name, kind, scope) in [("AC", "Mains", None), ("hid-headset-battery", "Battery", Some("Device")), ("BAT0", "Battery", Some("System"))] {
            fs::create_dir_all(class.join(name)).unwrap();
            fs::write(class.join(name).join("type"), format!("{kind}\n")).unwrap();
            if let Some(scope) = scope {
                fs::write(class.join(name).join("scope"), scope).unwrap();
            }
        }
        assert_eq!(system_battery_in(class), Some(class.join("BAT0")));
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
    fn a_steam_game_is_known_by_its_app_id() {
        assert_eq!(steam_app_in(b"HOME=/home/a\0SteamAppId=1478500\0SteamGameId=1478500\0"), Some(1478500));
        assert_eq!(steam_app_in(b"SteamAppId=0\0"), None, "Steam's own client");
        assert_eq!(steam_app_in(b"HOME=/home/a\0"), None);
    }

    #[test]
    fn a_process_is_known_by_its_id_and_start() {
        let me = std::process::id();
        assert!(start_time(me).is_some());
        assert_eq!(saw(me), start_time(me));
        assert!(same_process(me));
        // A different start time under the same id is a different process.
        crate::os::lock(&SEEN).insert(me, 1);
        assert!(!same_process(me));
        assert!(same_process(u32::MAX - 1), "never seen: as far as is known");
    }
}
