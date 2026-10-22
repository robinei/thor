//! cgroups in the unified hierarchy. This finds which systemd unit a process runs in, and how much CPU
//! and GPU time a cgroup's processes have used.
//!
//! The unit comes from the process's cgroup in `/proc/<pid>/cgroup`. That is the kernel's record of which
//! launch the process comes from, even if it has since detached or double-forked.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use crate::os::{drm, sys};

/// The unit a process runs in (`….scope`, `….service`), if it can be read.
pub fn unit_of(pid: u32) -> Option<String> {
    path_of(pid).as_deref().and_then(innermost_unit).map(str::to_owned)
}

/// A process's cgroup path in the unified hierarchy (`/user.slice/…/app.slice/foo.scope`).
pub fn path_of(pid: u32) -> Option<String> {
    parse_path(&sys::text(format!("/proc/{pid}/cgroup"))?).map(str::to_owned)
}

/// Where cgroup `path` (`/user.slice/…`) is in the cgroup filesystem.
pub fn dir(path: &str) -> PathBuf {
    Path::new("/sys/fs/cgroup").join(path.trim_start_matches('/'))
}

/// A cgroup's CPU time so far in microseconds, including its subgroups.
pub fn cpu_usec(dir: &Path) -> Option<u64> {
    let stat = sys::text(dir.join("cpu.stat"))?;
    stat.lines().find_map(|l| l.strip_prefix("usage_usec ")).and_then(|v| v.trim().parse().ok())
}

/// The GPU time so far of a cgroup's processes, including its subgroups, in nanoseconds. Each DRM client
/// is counted once.
pub fn gpu_ns(dir: &Path) -> u64 {
    let mut clients = HashMap::new();
    let mut dirs = vec![dir.to_path_buf()];
    while let Some(d) = dirs.pop() {
        for pid in sys::text(d.join("cgroup.procs")).unwrap_or_default().lines() {
            clients.extend(drm::clients(&Path::new("/proc").join(pid)));
        }
        match std::fs::read_dir(&d) {
            Ok(entries) => {
                let entries = entries.filter_map(|e| e.inspect_err(|e| log::trace!("reading {}: {e}", d.display())).ok());
                dirs.extend(entries.filter(|e| e.file_type().is_ok_and(|t| t.is_dir())).map(|e| e.path()));
            }
            Err(e) => log::trace!("reading {}: {e}", d.display()),
        }
    }
    clients.values().sum()
}

/// The unified hierarchy's path in a cgroup file, from its `0::` line.
fn parse_path(cgroup: &str) -> Option<&str> {
    cgroup.lines().find_map(|l| l.strip_prefix("0::"))
}

/// The innermost unit in a cgroup path.
fn innermost_unit(path: &str) -> Option<&str> {
    path.rsplit('/').find(|part| part.ends_with(".scope") || part.ends_with(".service"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_innermost_unit() {
        let unit = |cgroup| parse_path(cgroup).and_then(innermost_unit);
        let cgroup = "0::/user.slice/user-1000.slice/user@1000.service/app.slice/clamshell-run-foot_1-17-0.scope\n";
        assert_eq!(unit(cgroup), Some("clamshell-run-foot_1-17-0.scope"));
        assert_eq!(unit("0::/system.slice/foo.service/sub\n"), Some("foo.service"), "a sub-cgroup of a unit");
        assert_eq!(unit("0::/\n"), None);
        assert_eq!(unit("1:name=systemd:/x.scope\n"), None, "cgroup v1 only");
    }
}

#[cfg(test)]
mod live {
    /// Under a desktop session every process is in some scope or service.
    #[test]
    #[ignore = "needs a desktop session"]
    fn this_process_has_a_unit() {
        let unit = super::unit_of(std::process::id());
        assert!(unit.is_some_and(|u| u.ends_with(".scope") || u.ends_with(".service")));
    }
}
