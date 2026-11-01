//! Archives browsed as folders (see clamshell-state's `files`). When its folder is opened, an archive is
//! mounted read-only with ratarmount, a FUSE filesystem for zip, 7z, rar, tar and compressed tars. Its
//! entries are then files like any others, which can be opened or copied out. It is unmounted once
//! nothing uses it: the open folder isn't in it, and no copied entry or running launch's file is in it.
//!
//! Each mount is a service of its own, `clamshell-archive-<hash>.service`, running ratarmount in the
//! foreground in the session's `background-graphical.slice`. Unmounting means stopping the service. After it
//! stops or crashes the mount is detached lazily, so none is left behind, and the session ending stops
//! them all. The indexes are kept in the cache, not beside the archives. Mounts left by an earlier
//! ClamShell are kept, since a launch that outlived it may read from them (see [`Mounts::new`]).

use std::ffi::{OsStr, OsString};
use std::fs;
use std::path::{Path, PathBuf};

use crate::launch::job::{self, Job};
use crate::os::command;

/// The file name endings browsed as archives, in lowercase.
const ENDINGS: [&str; 13] = [".zip", ".7z", ".rar", ".tar", ".tgz", ".txz", ".tbz2", ".tar.gz", ".tar.xz", ".tar.zst", ".tar.bz2", ".tar.lz4", ".tzst"];

/// Whether file name `name` is an archive.
pub(crate) fn is_archive(name: &str) -> bool {
    let lower = name.to_lowercase();
    ENDINGS.iter().any(|e| lower.ends_with(e) && lower.len() > e.len())
}

/// Why an archive isn't mounted.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum NotMounted {
    /// It can't be, for the reason given, as said to the user.
    Failed(String),
    /// Something else was asked for while it was being mounted, so the mount was given up.
    Interrupted,
}

/// The mounted archives, and where they are mounted.
pub(crate) struct Mounts {
    /// Where they are mounted, each in a dir of its own, and where their indexes are kept.
    root: Option<PathBuf>,
    index: Option<PathBuf>,
    mounted: Vec<Mount>,
}

/// An archive mounted, known by its dir (a hash of the archive's path, as is its service's name).
#[derive(Debug, PartialEq, Eq)]
struct Mount {
    dir: PathBuf,
    /// Mounted by an earlier ClamShell, and not yet named by the files in use (see [`Mounts::new`]).
    adopted: bool,
}

impl Mounts {
    /// Mounts go under `runtime`'s `archives` (ClamShell's runtime folder) and indexes in `cache`. A mount
    /// an earlier ClamShell left running is kept: a launch that outlived it may be reading from it, and
    /// which file a launch from before opened isn't known again. It is unmounted once the files in use
    /// have named it and no longer do, else when the session ends. Services left starting or stopped and
    /// dirs left unmounted are cleaned up.
    pub(crate) fn new(runtime: Option<&Path>, cache: Option<PathBuf>) -> Mounts {
        let root = runtime.map(|d| d.join("archives"));
        let mut mounted = Vec::new();
        let mut stale = Vec::new();
        if let Some(root) = &root {
            let live = mounted_under(root);
            (mounted, stale) = adopt(root, &live, &active);
            for m in &mounted {
                log::info!("archives: {} kept, mounted before", m.dir.display());
            }
        }
        let ours = |unit: &str| mounted.iter().any(|m| unit_of_dir(&m.dir).as_deref() == Some(unit));
        if let Err(e) = crate::launch::units::stop_matching("clamshell-archive-*.service", &ours) {
            log::warn!("archives: stopping those left before: {e}");
        }
        for (dir, is_mount) in stale {
            match is_mount {
                true => detach(&dir),
                false => {
                    if let Err(err) = fs::remove_dir(&dir) {
                        log::warn!("archives: removing {}: {err}", dir.display());
                    }
                }
            }
        }
        Mounts { root, index: cache.map(|c| c.join("archive-index")), mounted }
    }

    /// Where `archive` is mounted, mounting it now if it isn't. Errs with what went wrong if it can't be,
    /// or once `interrupted` says something else was asked for meanwhile: mounting a big archive reads it
    /// through first, which can take a while.
    pub(crate) fn mount(&mut self, archive: &Path, interrupted: &mut dyn FnMut() -> bool) -> Result<PathBuf, NotMounted> {
        let name = archive.file_name().map_or_else(String::new, |n| n.to_string_lossy().into_owned());
        let root = self.root.clone().ok_or_else(|| NotMounted::Failed("no runtime dir".into()))?;
        let dir = dir_of(&root, archive);
        if self.reuse(&dir, &mounted_under(&root)) {
            return Ok(dir);
        }
        fs::create_dir_all(&dir).map_err(|e| NotMounted::Failed(format!("Opening {name}: {e}")))?;
        let unit = unit_of(archive);
        let mut argv: Vec<OsString> = ["ratarmount", "--foreground"].map(OsString::from).into();
        if let Some(index) = &self.index {
            if let Err(e) = fs::create_dir_all(index) {
                log::warn!("archives: {}: {e}", index.display());
            }
            argv.extend(["--index-folders".into(), index.into()]);
        }
        argv.extend([archive.into(), dir.clone().into()]);
        let job = Job {
            unit: unit.clone(),
            slice: Some("background-graphical.slice".into()),
            argv,
            env: vec![],
            workdir: None,
            // When the service stops or crashes, the mount is detached lazily.
            properties: vec![format!("ExecStopPost=/usr/bin/fusermount3 -uz {}", dir.display())],
        };
        // The same archive's service, stopped a moment ago, may still be stopping: its name is free once it
        // is gone.
        for _ in 0..50 {
            match crate::launch::units::is_loaded(&unit) {
                Ok(true) => std::thread::sleep(std::time::Duration::from_millis(100)),
                Ok(false) => break,
                Err(e) => {
                    log::warn!("archives: {unit}: {e}");
                    break;
                }
            }
        }
        let started = match job::start(&job) {
            Ok(()) => true,
            Err(job::Error::Io(e)) => return Err(NotMounted::Failed(format!("Opening {name}: {e}"))),
            Err(e) => {
                log::warn!("archives: {}: {e}", archive.display());
                false
            }
        };
        // It is mounted once its dir is a mount, and has failed once the service isn't running. There is
        // no time limit, since a big compressed tar is read through for its index first; but it is given
        // up when something else is asked for.
        let mounted = started
            && loop {
                if mounted_under(&root).contains(&dir) {
                    break true;
                }
                if !active(&unit) {
                    break false;
                }
                if interrupted() {
                    log::info!("archives: {}: no longer wanted while mounting: stopped", archive.display());
                    if let Err(e) = crate::launch::units::end(&unit, false) {
                        log::warn!("archives: {}: {e}", archive.display());
                    }
                    if let Err(e) = fs::remove_dir(&dir) {
                        log::debug!("archives: removing {}: {e}", dir.display());
                    }
                    return Err(NotMounted::Interrupted);
                }
                std::thread::sleep(std::time::Duration::from_millis(100));
            };
        if !mounted {
            if let Err(e) = fs::remove_dir(&dir) {
                log::warn!("archives: removing {}: {e}", dir.display());
            }
            log::warn!("archives: {}: not mounted (journalctl --user -u {unit})", archive.display());
            return Err(NotMounted::Failed(format!("{name} can't be opened (damaged, or it has a password)")));
        }
        log::info!("archives: {} mounted at {}", archive.display(), dir.display());
        self.mounted.push(Mount { dir: dir.clone(), adopted: false });
        Ok(dir)
    }

    /// Whether the mount on `dir` is known and still mounted (in `live`), so it can be used again. One
    /// whose ratarmount ended since (crashed, or killed) has left an empty dir, and is forgotten.
    fn reuse(&mut self, dir: &Path, live: &[PathBuf]) -> bool {
        let Some(i) = self.mounted.iter().position(|m| m.dir == dir) else { return false };
        if !live.iter().any(|d| d == dir) {
            log::warn!("archives: {} was no longer mounted (its service ended): mounted again", dir.display());
            self.mounted.remove(i);
            return false;
        }
        if let Some(m) = self.mounted.get_mut(i) {
            m.adopted = false;
        }
        true
    }

    /// Unmounts the archives `in_use` doesn't need. An archive is needed if a path in `in_use` is the
    /// archive or is inside its mount. One mounted before ClamShell started is needed until `in_use` first
    /// names it.
    pub(crate) fn keep(&mut self, in_use: &[&Path]) {
        for dir in self.unused(in_use) {
            log::info!("archives: {} unmounted", dir.display());
            let stopped = unit_of_dir(&dir).map(|unit| crate::launch::units::end(&unit, false));
            if let Some(Err(e)) = &stopped {
                log::warn!("archives: {}: {e}", dir.display());
            }
            if !matches!(stopped, Some(Ok(()))) {
                detach(&dir);
            }
        }
    }

    /// Forgets the mounts `in_use` doesn't need (see [`Mounts::keep`]) and returns their dirs.
    fn unused(&mut self, in_use: &[&Path]) -> Vec<PathBuf> {
        let Some(root) = &self.root else { return Vec::new() };
        let mut gone = Vec::new();
        self.mounted.retain_mut(|m| {
            let used = in_use.iter().any(|p| p.starts_with(&m.dir) || dir_of(root, p) == m.dir);
            if used {
                m.adopted = false;
            }
            if !used && !m.adopted {
                gone.push(m.dir.clone());
                return false;
            }
            true
        });
        gone
    }
}

/// The dirs under `root` sorted: the mounts whose service is running (`active`) are kept, and every
/// other dir is stale, with whether it is a mount still (`live`).
fn adopt(root: &Path, live: &[PathBuf], active: &dyn Fn(&str) -> bool) -> (Vec<Mount>, Vec<(PathBuf, bool)>) {
    let entries = fs::read_dir(root).inspect_err(|e| {
        crate::os::log_unread("archives", root, e);
    });
    let mut kept = Vec::new();
    let mut stale = Vec::new();
    for dir in entries.into_iter().flatten().flatten().map(|e| e.path()) {
        let is_mount = live.contains(&dir);
        match is_mount && unit_of_dir(&dir).is_some_and(|u| active(&u)) {
            true => kept.push(Mount { dir, adopted: true }),
            false => stale.push((dir, is_mount)),
        }
    }
    (kept, stale)
}

/// The mounts directly under `root`.
fn mounted_under(root: &Path) -> Vec<PathBuf> {
    crate::os::sys::mount_table().into_iter().map(|m| m.dir).filter(|d| d.parent() == Some(root)).collect()
}

/// The dir under `root` archive `archive` is mounted on.
fn dir_of(root: &Path, archive: &Path) -> PathBuf {
    root.join(format!("{:016x}", hash(archive)))
}

/// The service of archive `archive`'s mount.
fn unit_of(archive: &Path) -> String {
    format!("clamshell-archive-{:016x}.service", hash(archive))
}

/// The service of the mount on `dir`, named as its dir is.
fn unit_of_dir(dir: &Path) -> Option<String> {
    Some(format!("clamshell-archive-{}.service", dir.file_name()?.to_str()?))
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
    fn mounts_left_running_are_kept_and_the_rest_cleaned_up() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        let [running, crashed, empty] = [Path::new("/a.zip"), Path::new("/b.zip"), Path::new("/c.zip")].map(|a| dir_of(root, a));
        for dir in [&running, &crashed, &empty] {
            fs::create_dir(dir).unwrap();
        }
        let live = [running.clone(), crashed.clone()];
        let unit = unit_of(Path::new("/a.zip"));
        let (kept, stale) = adopt(root, &live, &|u| u == unit);
        assert_eq!(kept, [Mount { dir: running, adopted: true }]);
        let mut stale = stale;
        stale.sort();
        let mut expected = vec![(crashed, true), (empty, false)];
        expected.sort();
        assert_eq!(stale, expected, "the crashed one is detached, the empty dir removed");
    }

    #[test]
    fn a_mount_kept_from_before_is_unmounted_only_once_the_files_in_use_have_named_it() {
        let root = PathBuf::from("/run/archives");
        let game = dir_of(&root, Path::new("/roms/game.7z"));
        let other = dir_of(&root, Path::new("/roms/other.zip"));
        let mut mounts =
            Mounts { root: Some(root), index: None, mounted: vec![Mount { dir: game.clone(), adopted: true }, Mount { dir: other.clone(), adopted: false }] };
        assert_eq!(mounts.unused(&[]), [other], "the files in use at start don't know what a launch from before opened");
        let disc = game.join("disc1.cue");
        assert!(mounts.unused(&[disc.as_path()]).is_empty());
        assert_eq!(mounts.unused(&[]), [game], "named, then no longer");
        assert!(mounts.mounted.is_empty());
    }

    #[test]
    fn an_archive_in_use_by_its_own_path_stays_mounted() {
        let root = PathBuf::from("/run/archives");
        let archive = Path::new("/roms/game.7z");
        let mut mounts = Mounts { root: Some(root.clone()), index: None, mounted: vec![Mount { dir: dir_of(&root, archive), adopted: true }] };
        assert!(mounts.unused(&[archive]).is_empty());
        assert_eq!(mounts.mounted, [Mount { dir: dir_of(&root, archive), adopted: false }]);
    }

    #[test]
    fn a_mount_whose_ratarmount_ended_is_mounted_again() {
        let root = PathBuf::from("/run/archives");
        let dir = dir_of(&root, Path::new("/roms/game.7z"));
        let mut mounts = Mounts { root: Some(root), index: None, mounted: vec![Mount { dir: dir.clone(), adopted: false }] };
        assert!(mounts.reuse(&dir, std::slice::from_ref(&dir)), "still mounted");
        assert!(!mounts.reuse(&dir, &[]), "its dir no longer a mount");
        assert!(mounts.mounted.is_empty(), "forgotten, so it is mounted again");
    }

    #[test]
    #[ignore = "needs a user manager, ratarmount and bsdtar"]
    fn live_mount_and_unmount() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path();
        fs::write(dir.join("inside.txt"), "hello").unwrap();
        let zip = dir.join("a.zip");
        assert!(std::process::Command::new("bsdtar").arg("-C").arg(dir).args(["-a", "-cf"]).arg(&zip).arg("inside.txt").status().unwrap().success());
        let mut mounts = Mounts::new(Some(dir), None);
        assert_eq!(mounts.mount(&zip, &mut || true), Err(NotMounted::Interrupted), "something else asked for at once");
        let at = mounts.mount(&zip, &mut || false).unwrap();
        assert_eq!(fs::read_to_string(at.join("inside.txt")).unwrap(), "hello");
        // A ClamShell started again keeps it, until the files in use have named it and no longer do.
        let mut mounts = Mounts::new(Some(dir), None);
        mounts.keep(&[]);
        assert_eq!(fs::read_to_string(at.join("inside.txt")).unwrap(), "hello", "kept from before");
        assert_eq!(mounts.mount(&zip, &mut || false).unwrap(), at);
        mounts.keep(&[]);
        let unit = unit_of(&zip);
        let gone = (0..50).any(|_| {
            std::thread::sleep(std::time::Duration::from_millis(100));
            !active(&unit)
        });
        assert!(gone);
    }
}
