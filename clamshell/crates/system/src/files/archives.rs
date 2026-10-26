//! Archives browsed as folders (see clamshell-state's `files`). When its folder is opened, an archive is
//! mounted read-only with ratarmount, a FUSE filesystem for zip, 7z, rar, tar and compressed tars. Its
//! entries are then files like any others, which can be opened or copied out. It is unmounted once
//! nothing uses it: the open folder isn't in it, and no copied entry or running launch's file is in it.
//!
//! Each mount is a service of its own, `clamshell-archive-<hash>.service`, running ratarmount in the
//! foreground in the session's `background-graphical.slice`. Unmounting means stopping the service. After it
//! stops or crashes the mount is detached lazily, so none is left behind, and the session ending stops
//! them all. The indexes are kept in the cache, not beside the archives. Services left by an earlier
//! ClamShell are stopped at start.

use std::ffi::{OsStr, OsString};
use std::fs;
use std::path::{Path, PathBuf};

use crate::os::command;

/// The file name endings browsed as archives, in lowercase.
const ENDINGS: [&str; 13] = [".zip", ".7z", ".rar", ".tar", ".tgz", ".txz", ".tbz2", ".tar.gz", ".tar.xz", ".tar.zst", ".tar.bz2", ".tar.lz4", ".tzst"];

/// Whether file name `name` is an archive.
pub fn is_archive(name: &str) -> bool {
    let lower = name.to_lowercase();
    ENDINGS.iter().any(|e| lower.ends_with(e) && lower.len() > e.len())
}

/// The mounted archives, and where they are mounted.
pub struct Mounts {
    /// Where they are mounted, each in a dir of its own, and where their indexes are kept.
    root: Option<PathBuf>,
    index: Option<PathBuf>,
    mounted: Vec<(PathBuf, PathBuf)>,
}

impl Mounts {
    /// Mounts go under `$XDG_RUNTIME_DIR/clamshell/archives` and indexes in `cache`. Mounts left there
    /// by an earlier ClamShell are unmounted.
    pub fn new(cache: Option<PathBuf>) -> Mounts {
        let root = std::env::var_os("XDG_RUNTIME_DIR").map(|d| PathBuf::from(d).join("clamshell/archives"));
        // Clean up after an earlier ClamShell: stop its services, detach any mount left, remove the dirs.
        if let Err(e) = crate::launch::units::stop_matching("clamshell-archive-*.service") {
            log::warn!("archives: stopping those left before: {e}");
        }
        if let Some(root) = &root {
            for dir in mounted_under(root) {
                detach(&dir);
            }
            let entries = fs::read_dir(root).inspect_err(|e| {
                if e.kind() == std::io::ErrorKind::NotFound {
                    log::debug!("archives: {}: {e}", root.display());
                } else {
                    log::warn!("archives: {}: {e}", root.display());
                }
            });
            for e in entries.into_iter().flatten().flatten() {
                if let Err(err) = fs::remove_dir(e.path()) {
                    log::warn!("archives: removing {}: {err}", e.path().display());
                }
            }
        }
        Mounts { root, index: cache.map(|c| c.join("archive-index")), mounted: Vec::new() }
    }

    /// Where `archive` is mounted, mounting it now if it isn't. Errs with what went wrong if it can't be.
    pub fn mount(&mut self, archive: &Path) -> Result<PathBuf, String> {
        if let Some((_, dir)) = self.mounted.iter().find(|(a, _)| a == archive) {
            return Ok(dir.clone());
        }
        let name = archive.file_name().map_or_else(String::new, |n| n.to_string_lossy().into_owned());
        let root = self.root.as_ref().ok_or("no runtime dir")?;
        let dir = root.join(format!("{:016x}", hash(archive)));
        fs::create_dir_all(&dir).map_err(|e| format!("Opening {name}: {e}"))?;
        let unit = unit_of(archive);
        let mut args: Vec<OsString> = ["--user", "--quiet", "--collect", "--slice=background-graphical.slice"].map(OsString::from).into();
        args.push(format!("--unit={unit}").into());
        // When the service stops or crashes, the mount is detached lazily.
        args.push(format!("--property=ExecStopPost=/usr/bin/fusermount3 -uz {}", dir.display()).into());
        args.extend(["ratarmount", "--foreground"].map(OsString::from));
        if let Some(index) = &self.index {
            if let Err(e) = fs::create_dir_all(index) {
                log::warn!("archives: {}: {e}", index.display());
            }
            args.extend(["--index-folders".into(), index.into()]);
        }
        args.extend([archive.into(), dir.clone().into()]);
        let started = command::run("systemd-run", &args, None, command::LIMIT).map_err(|e| format!("Opening {name}: {e}"))?.status;
        // It is mounted once its dir is a mount, and has failed once the service isn't running. There is
        // no time limit, since a big compressed tar is read through for its index first.
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
            if let Err(e) = fs::remove_dir(&dir) {
                log::warn!("archives: removing {}: {e}", dir.display());
            }
            log::warn!("archives: {}: not mounted (journalctl --user -u {unit})", archive.display());
            return Err(format!("{name} can't be opened (damaged, or it has a password)"));
        }
        log::info!("archives: {} mounted at {}", archive.display(), dir.display());
        self.mounted.push((archive.to_path_buf(), dir.clone()));
        Ok(dir)
    }

    /// Unmounts the archives `in_use` doesn't need. An archive is needed if a path in `in_use` is the
    /// archive or is inside its mount.
    pub fn keep(&mut self, in_use: &[&Path]) {
        self.mounted.retain(|(archive, dir)| {
            let used = in_use.iter().any(|p| *p == archive || p.starts_with(dir));
            if !used {
                log::info!("archives: {} unmounted", archive.display());
                if let Err(e) = crate::launch::units::end(&unit_of(archive), false) {
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
    crate::os::sys::mount_table().into_iter().map(|m| m.dir).filter(|d| d.parent() == Some(root)).collect()
}

/// The service of archive `archive`'s mount.
fn unit_of(archive: &Path) -> String {
    format!("clamshell-archive-{:016x}.service", hash(archive))
}

/// Whether service `unit` is running or starting. If the user manager can't be asked, it counts as
/// stopped, so a mount isn't waited for forever.
fn active(unit: &str) -> bool {
    crate::launch::units::is_active(unit).unwrap_or_else(|e| {
        log::warn!("archives: {unit}: {e}");
        false
    })
}

/// Detaches `dir` lazily, so open files stay readable, and removes it.
fn detach(dir: &Path) {
    let done = command::run("fusermount3", &[OsStr::new("-uz"), dir.as_os_str()], None, command::LIMIT);
    if !done.is_ok_and(|ran| ran.status.success()) {
        log::warn!("archives: unmounting {} failed", dir.display());
    }
    if let Err(e) = fs::remove_dir(dir) {
        log::warn!("archives: removing {}: {e}", dir.display());
    }
}

/// FNV-1a of a path, used to name its mount's dir.
fn hash(path: &Path) -> u64 {
    use std::os::unix::ffi::OsStrExt;
    clamshell_library::hash::fnv1a64(path.as_os_str().as_bytes())
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
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

    #[test]
    #[ignore = "needs a user manager, ratarmount and bsdtar"]
    fn live_mount_and_unmount() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path();
        fs::write(dir.join("inside.txt"), "hello").unwrap();
        let zip = dir.join("a.zip");
        assert!(std::process::Command::new("bsdtar").arg("-C").arg(dir).args(["-a", "-cf"]).arg(&zip).arg("inside.txt").status().unwrap().success());
        let mut mounts = Mounts::new(None);
        let at = mounts.mount(&zip).unwrap();
        assert_eq!(fs::read_to_string(at.join("inside.txt")).unwrap(), "hello");
        mounts.keep(&[]);
        let unit = unit_of(&zip);
        let gone = (0..50).any(|_| {
            std::thread::sleep(std::time::Duration::from_millis(100));
            !active(&unit)
        });
        assert!(gone);
    }
}
