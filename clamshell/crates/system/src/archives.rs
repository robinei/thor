//! Archives browsed as folders (see clamshell-core's `files`): each mounted read-only with ratarmount
//! (FUSE: zip, 7z, rar, tar and its compressed kinds) when its folder is opened, so its entries are
//! files like any others (opened, copied out); unmounted once nothing uses it (the folder open isn't in
//! it, no running launch opened a file in it). The unmount is lazy: a program that still has a file of
//! it open keeps reading it.
//!
//! ratarmount runs in a scope of its own (it outlives a ClamShell restarted); its indexes are kept in
//! the cache, not beside the archives. Mounts a ClamShell before this one left are unmounted at start.

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
        if let Some(root) = &root {
            for dir in mounted_under(root) {
                unmount(&dir);
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
        let mut command = Command::new("systemd-run");
        command.args(["--user", "--scope", "--quiet", "--collect", "--unit"]).arg(format!("clamshell-archive-{:016x}-{}", hash(archive), unique()));
        command.arg("ratarmount");
        if let Some(index) = &self.index {
            let _ = fs::create_dir_all(index);
            command.arg("--index-folders").arg(index);
        }
        // (It mounts, then goes into the background: done when it returns.)
        let out = command.arg(archive).arg(&dir).stdin(Stdio::null()).output().map_err(|e| format!("Opening {name}: ratarmount: {e}"))?;
        if !out.status.success() || !mounted_under(root).contains(&dir) {
            let _ = fs::remove_dir(&dir);
            // (ratarmount's words, a boxed paragraph, for the log; the notice says what it means.)
            log::warn!("archives: {}: {}", archive.display(), String::from_utf8_lossy(&out.stderr).split_whitespace().collect::<Vec<_>>().join(" "));
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
                unmount(dir);
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

/// `dir` unmounted (lazily: open files stay readable) and removed.
fn unmount(dir: &Path) {
    let done = Command::new("fusermount3").arg("-uz").arg(dir).stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null()).status();
    if !done.is_ok_and(|s| s.success()) {
        log::warn!("archives: unmounting {} failed", dir.display());
    }
    let _ = fs::remove_dir(dir);
}

/// A number not used before for a scope's name (a mount's last scope may outlive its unmount a while).
fn unique() -> u128 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_nanos())
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
