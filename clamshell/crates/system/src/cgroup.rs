//! Which systemd unit a process runs in, from its cgroup (`/proc/<pid>/cgroup`, the unified hierarchy):
//! the kernel's record of which launch it comes from, whatever it did since (detached, double-forked).

use crate::sys;

/// The unit a process runs in (`….scope`, `….service`), if it can be read.
pub fn unit_of(pid: u32) -> Option<String> {
    path_of(pid).as_deref().and_then(innermost_unit).map(str::to_owned)
}

/// A process's cgroup path in the unified hierarchy (`/user.slice/…/app.slice/foo.scope`).
pub fn path_of(pid: u32) -> Option<String> {
    parse_path(&sys::text(format!("/proc/{pid}/cgroup"))?).map(str::to_owned)
}

/// The unified hierarchy's path in a cgroup file (its `0::` line).
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
