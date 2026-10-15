//! Which systemd unit a process runs in, from its cgroup (`/proc/<pid>/cgroup`, the unified hierarchy):
//! the kernel's record of which launch it comes from, whatever it did since (detached, double-forked).

use crate::sys;

/// The unit a process runs in (`….scope`, `….service`), if it can be read.
pub fn unit_of(pid: u32) -> Option<String> {
    parse_unit(&sys::text(format!("/proc/{pid}/cgroup"))?).map(str::to_owned)
}

/// The innermost unit in a cgroup file's path (`0::/user.slice/…/app.slice/foo.scope`).
pub fn parse_unit(cgroup: &str) -> Option<&str> {
    let path = cgroup.lines().find_map(|l| l.strip_prefix("0::"))?;
    path.rsplit('/').find(|part| part.ends_with(".scope") || part.ends_with(".service"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_innermost_unit() {
        let cgroup = "0::/user.slice/user-1000.slice/user@1000.service/app.slice/clamshell-app-terminal-17-0.scope\n";
        assert_eq!(parse_unit(cgroup), Some("clamshell-app-terminal-17-0.scope"));
        assert_eq!(parse_unit("0::/system.slice/foo.service/sub\n"), Some("foo.service"), "a sub-cgroup of a unit");
        assert_eq!(parse_unit("0::/\n"), None);
        assert_eq!(parse_unit("1:name=systemd:/x.scope\n"), None, "cgroup v1 only");
    }
}

#[cfg(test)]
mod live {
    /// Under a desktop session every process is in some scope or service.
    #[test]
    #[ignore]
    fn this_process_has_a_unit() {
        let unit = super::unit_of(std::process::id());
        assert!(unit.is_some_and(|u| u.ends_with(".scope") || u.ends_with(".service")));
    }
}
