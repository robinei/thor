//! Archives browsed as folders (see clamshell-core's `files`): each mounted read-only with ratarmount
//! (FUSE: zip, 7z, rar, tar and its compressed kinds) when its folder is opened, so its entries are
//! files like any others (opened, copied out); unmounted once nothing uses it (the folder open isn't in
//! it, no copied entry and no running launch's file is in it).
//!
//! Each mount is a service of its own (`clamshell-archive-<hash>.service`: ratarmount in the
//! foreground, in uwsm's `background-graphical.slice`): unmounting is stopping it, and after it stops
//! (or crashes) the mount is detached lazily, so none is left behind; the session ending stops them all.
//! Its indexes are kept in the cache, not beside the archives. Those a ClamShell before this one left
//! are stopped at start.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

/// The names browsed as archives (their endings, lowercase).
const ENDINGS: [&str; 13] = [".zip", ".7z", ".rar", ".tar", ".tgz", ".txz", ".tbz2", ".tar.gz", ".tar.xz", ".tar.zst", ".tar.bz2", ".tar.lz4", ".tzst"];

/// Whether file name `name` is an archive's.
pub fn is_archive(name: &str) -> bool {
    let lower = name.to_lowercase();
    ENDINGS.iter().any(|e| lower.ends_with(e) && lower.len() > e.len())
}

/// The archives mounted, and where.
pub struct Mounts {
    /// Where they are mounted (each in a dir of its own), and their indexes kept.
    root: Option<PathBuf>,
    index: Option<PathBuf>,
    mounted: Vec<(PathBuf, PathBuf)>,
}

impl Mounts {
    /// Mounts under `$XDG_RUNTIME_DIR/clamshell/archives`, indexes in `cache`; those left there by a
    /// ClamShell before unmounted.
    pub fn new(cache: Option<PathBuf>) -> Mounts {
        let root = std::env::var_os("XDG_RUNTIME_DIR").map(|d| PathBuf::from(d).join("clamshell/archives"));
        // (Left by a ClamShell before: their services stopped, any mount left detached, the dirs gone.)
        let _ = Command::new("systemctl").args(["--user", "stop", "clamshell-archive-*.service"]).stdin(Stdio::null()).stderr(Stdio::null()).status();
        if let Some(root) = &root {
            for dir in mounted_under(root) {
                detach(&dir);
            }
            for e in fs::read_dir(root).into_iter().flatten().flatten() {
                let _ = fs::remove_dir(e.path());
            }
        }
        Mounts { root, index: cache.map(|c| c.join("archive-index")), mounted: Vec::new() }
    }

    /// Where `archive` is mounted, mounted now if it isn't; what went wrong if it can't be.
    pub fn mount(&mut self, archive: &Path) -> Result<PathBuf, String> {
        if let Some((_, dir)) = self.mounted.iter().find(|(a, _)| a == archive) {
            return Ok(dir.clone());
        }
        let name = archive.file_name().map_or_else(String::new, |n| n.to_string_lossy().into_owned());
        let root = self.root.as_ref().ok_or("no runtime dir")?;
        let dir = root.join(format!("{:016x}", hash(archive)));
        fs::create_dir_all(&dir).map_err(|e| format!("Opening {name}: {e}"))?;
        let unit = unit_of(archive);
        let mut command = Command::new("systemd-run");
        command.args(["--user", "--quiet", "--collect", "--slice=background-graphical.slice"]).arg(format!("--unit={unit}"));
        // (Stopped or crashed: the mount detached, lazily.)
        command.arg(format!("--property=ExecStopPost=/usr/bin/fusermount3 -uz {}", dir.display()));
        command.args(["ratarmount", "--foreground"]);
        if let Some(index) = &self.index {
            let _ = fs::create_dir_all(index);
            command.arg("--index-folders").arg(index);
        }
        let started = command.arg(archive).arg(&dir).stdin(Stdio::null()).status().map_err(|e| format!("Opening {name}: systemd-run: {e}"))?;
        // Mounted once its dir is a mount; failed once the service isn't running (a big compressed tar
        // is read through for its index first: no time limit).
        let mounted = started.success()
            && loop {
                if mounted_under(root).contains(&dir) {
                    break true;
                }
                if !active(&unit) {
                    break false;
                }
                std::thread::sleep(std::time::Duration::from_millis(100));
            };
        if !mounted {
            let _ = fs::remove_dir(&dir);
            log::warn!("archives: {}: not mounted (journalctl --user -u {unit})", archive.display());
            return Err(format!("{name} can't be opened (damaged, or it has a password)"));
        }
        log::info!("archives: {} mounted at {}", archive.display(), dir.display());
        self.mounted.push((archive.to_path_buf(), dir.clone()));
        Ok(dir)
    }

    /// Only the archives `in_use` needs mounted (one of them the archive, or in its mount).
    pub fn keep(&mut self, in_use: &[&Path]) {
        self.mounted.retain(|(archive, dir)| {
            let used = in_use.iter().any(|p| *p == archive || p.starts_with(dir));
            if !used {
                log::info!("archives: {} unmounted", archive.display());
                if let Err(e) = crate::units::end(&unit_of(archive), false) {
                    log::warn!("archives: {}: {e}", archive.display());
                    detach(dir);
                }
            }
            used
        });
    }
}

/// The mounts directly under `root`.
fn mounted_under(root: &Path) -> Vec<PathBuf> {
    let text = fs::read_to_string("/proc/self/mounts").unwrap_or_default();
    text.lines().filter_map(|l| l.split(' ').nth(1)).map(PathBuf::from).filter(|d| d.parent() == Some(root)).collect()
}

/// The service of archive `archive`'s mount.
fn unit_of(archive: &Path) -> String {
    format!("clamshell-archive-{:016x}.service", hash(archive))
}

/// Whether service `unit` runs (or is starting).
fn active(unit: &str) -> bool {
    let out = Command::new("systemctl").args(["--user", "is-active", unit]).stdin(Stdio::null()).output();
    out.is_ok_and(|o| matches!(String::from_utf8_lossy(&o.stdout).trim(), "active" | "activating" | "reloading"))
}

/// `dir` detached (lazily: open files stay readable) and removed.
fn detach(dir: &Path) {
    let done = Command::new("fusermount3").arg("-uz").arg(dir).stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null()).status();
    if !done.is_ok_and(|s| s.success()) {
        log::warn!("archives: unmounting {} failed", dir.display());
    }
    let _ = fs::remove_dir(dir);
}

/// FNV-1a of a path: its mount's dir name.
fn hash(path: &Path) -> u64 {
    use std::os::unix::ffi::OsStrExt;
    path.as_os_str().as_bytes().iter().fold(0xcbf2_9ce4_8422_2325, |h, b| (h ^ u64::from(*b)).wrapping_mul(0x0100_0000_01b3))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn archives_are_known_by_their_names() {
        for name in ["a.zip", "B.7Z", "c.tar.gz", "d.tgz", "e.rar", "f.tar.zst"] {
            assert!(is_archive(name), "{name}");
        }
        for name in ["zip", ".zip", "a.gz", "a.iso", "a.zipx"] {
            assert!(!is_archive(name), "{name}");
        }
    }
}
