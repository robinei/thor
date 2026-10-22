//! GPU time from the kernel's per-client DRM statistics, read the way MangoHud reads it for one process.
//! The statistics are in `/proc/<pid>/fdinfo/<fd>` for a /dev/dri file descriptor; the format is in
//! Documentation/gpu/drm-usage-stats.rst.
//!
//! GPU time means the render engine's time. That is `drm-engine-gpu` on msm (the Thor's Adreno, which
//! reports only that engine) or `drm-engine-gfx` on amdgpu. Other engines such as compute, video and copy
//! aren't counted. They run alongside the render engine, so adding them up could pass 100%, and the
//! render engine alone matches what amdgpu's busy figure means. The sysinfo tab's GPU load and the
//! battery tab's GPU shares follow the same rule.

use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::Path;

use crate::os::sys;

/// A DRM client's id and its render engine time in nanoseconds, from an fdinfo file. Returns `None` if
/// the file isn't a DRM client's or has no render engine.
fn parse_fdinfo(text: &str) -> Option<(u64, u64)> {
    let mut id = None;
    let mut ns = None;
    for line in text.lines() {
        let Some((key, value)) = line.split_once(':') else {
            continue;
        };
        let value = value.trim();
        match key {
            "drm-client-id" => id = value.parse().ok(),
            "drm-engine-gpu" | "drm-engine-gfx" => ns = value.trim_end_matches(" ns").trim().parse().ok(),
            _ => {}
        }
    }
    Some((id?, ns?))
}

/// The DRM clients a process has open, with their GPU time. `process` is its /proc dir. A client open
/// through several file descriptors counts once. Only our user's processes can be read.
pub fn clients(process: &Path) -> HashMap<u64, u64> {
    let mut clients = HashMap::new();
    let dir = process.join("fd");
    let fds = match fs::read_dir(&dir) {
        Ok(fds) => fds,
        Err(e) => {
            log::trace!("reading {}: {e}", dir.display());
            return clients;
        }
    };
    for fd in fds.filter_map(|fd| fd.inspect_err(|e| log::trace!("reading {}: {e}", dir.display())).ok()) {
        if !fs::read_link(fd.path()).is_ok_and(|t| t.starts_with("/dev/dri")) {
            continue;
        }
        if let Some((id, ns)) = sys::text(process.join("fdinfo").join(fd.file_name())).as_deref().and_then(parse_fdinfo) {
            clients.insert(id, ns);
        }
    }
    clients
}

/// The GPU time of every DRM client this user can see, each counted once. `None` if there are none.
pub fn total_ns() -> Option<u64> {
    let mut seen = HashSet::new();
    let mut total = None::<u64>;
    for (_, dir) in sys::processes() {
        for (id, ns) in clients(&dir) {
            if seen.insert(id) {
                total = Some(total.unwrap_or(0).saturating_add(ns));
            }
        }
    }
    total
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_render_engines_time() {
        let msm = "pos:\t0\ndrm-driver:\tmsm\ndrm-client-id:\t7\ndrm-engine-gpu:\t123456 ns\ndrm-cycles-gpu:\t99\ndrm-engine-capacity-gpu:\t1\n";
        assert_eq!(parse_fdinfo(msm), Some((7, 123_456)));
        let amd = "drm-driver:\tamdgpu\ndrm-client-id:\t16\ndrm-engine-gfx:\t1300507034074 ns\ndrm-engine-dec:\t500 ns\n";
        assert_eq!(parse_fdinfo(amd), Some((16, 1_300_507_034_074)), "video decoding isn't counted");
        assert_eq!(parse_fdinfo("drm-driver:\tamdgpu\ndrm-client-id:\t15\n"), None);
        assert_eq!(parse_fdinfo("pos:\t0\nflags:\t0\n"), None);
    }
}

#[cfg(test)]
mod live {
    use super::*;
    use std::time::{Duration, Instant};

    /// Compares the fdinfo load with amdgpu's own busy figure, where there is one. Run it by hand with
    /// `--ignored --nocapture`.
    #[test]
    #[ignore = "reads this machine's GPU"]
    fn fdinfo_load_on_this_machine() {
        let (Some(t0), i0) = (total_ns(), Instant::now()) else {
            eprintln!("no DRM clients");
            return;
        };
        std::thread::sleep(Duration::from_millis(1000));
        let (t1, i1) = (total_ns().unwrap_or(t0), Instant::now());
        let load = (t1.saturating_sub(t0)) as f64 / i1.duration_since(i0).as_nanos() as f64;
        let busy = sys::entries("/sys/class/drm").iter().find_map(|c| sys::read::<f64>(c.join("device/gpu_busy_percent")));
        eprintln!("fdinfo load {:.1}%, busy file {busy:?}%", load * 100.0);
    }
}
