//! Files written whole, so that a crash or power loss mid-write leaves the last complete version, and
//! JSON read and written that way.

use std::fs;
use std::io::{self, Write as _};
use std::path::{Path, PathBuf};

use serde::Serialize;
use serde::de::DeserializeOwned;

/// Writes `bytes` to `path` whole. They go to a file beside it, which is flushed to the disk and then
/// renamed over it. The folder is synced too, so the rename lasts. The file beside it has a name of this
/// write's own (`<name>.part-<process>-<count>`), so two writers of one path, threads or processes (the
/// FEX presets from ClamShell and `--launch-env`), each put a whole file in place; the last one stays.
pub(crate) fn write_file(path: &Path, bytes: &[u8]) -> io::Result<()> {
    use std::sync::atomic::{AtomicU64, Ordering};
    static WRITES: AtomicU64 = AtomicU64::new(0);
    let dir = path.parent().ok_or_else(|| io::Error::other("no folder"))?;
    fs::create_dir_all(dir)?;
    let mut part = path.as_os_str().to_owned();
    part.push(format!(".part-{}-{}", std::process::id(), WRITES.fetch_add(1, Ordering::Relaxed)));
    let part = PathBuf::from(part);
    let written = (|| {
        let mut file = fs::OpenOptions::new().write(true).create_new(true).open(&part)?;
        file.write_all(bytes)?;
        file.sync_all()?;
        fs::rename(&part, path)
    })();
    if written.is_err()
        && let Err(e) = fs::remove_file(&part)
        && e.kind() != io::ErrorKind::NotFound
    {
        log::warn!("{}: {e}", part.display());
    }
    written?;
    fs::File::open(dir)?.sync_all()
}

/// Whether `name`, from another program or a drive, is one file's name, to be joined to a folder: not
/// empty, `.` or `..`, and without a `/` or a NUL. A window's app id or a tray icon's name looked up as
/// an icon must be, or it could lead out of the icon folders to any file of the user's.
pub(crate) fn is_name(name: &str) -> bool {
    !name.is_empty() && name != "." && name != ".." && !name.contains(['/', '\0'])
}

/// Whether `path`, from another program or a drive, stays below the folder it is joined to: relative, and
/// without `..`.
pub(crate) fn stays_below(path: &Path) -> bool {
    use std::path::Component;
    path.components().next().is_some() && path.components().all(|c| matches!(c, Component::Normal(_) | Component::CurDir))
}

/// Writes `value` as pretty JSON, whole ([`write_file`]).
pub(crate) fn write_json(path: &Path, value: &impl Serialize) -> io::Result<()> {
    let mut text = serde_json::to_string_pretty(value).map_err(io::Error::other)?;
    text.push('\n');
    write_file(path, text.as_bytes())
}

/// Reads a file as `T`. Returns none if the file isn't there or doesn't parse, which is logged.
pub(crate) fn read_json<T: DeserializeOwned>(path: &Path) -> Option<T> {
    let text = fs::read_to_string(path)
        .map_err(|e| {
            if e.kind() != io::ErrorKind::NotFound {
                log::warn!("{}: {e}", path.display())
            }
        })
        .ok()?;
    serde_json::from_str(&text).map_err(|e| log::warn!("{}: {e}", path.display())).ok()
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::indexing_slicing)]
mod tests {
    use super::*;

    #[test]
    fn names_and_paths_from_outside_are_checked() {
        for name in ["org.gnome.Calculator", "steam", "a b", ".hidden"] {
            assert!(is_name(name), "{name}");
        }
        for name in ["", ".", "..", "/home/x/a.png", "../../x", "a/b", "a\0b"] {
            assert!(!is_name(name), "{name:?}");
        }
        for path in ["a", "a/b", "./a/b"] {
            assert!(stays_below(Path::new(path)), "{path}");
        }
        for path in ["", "/etc/x", "../x", "a/../../x"] {
            assert!(!stays_below(Path::new(path)), "{path}");
        }
    }

    /// Two writers of one file at once each put a whole file in place, and neither fails.
    #[test]
    fn writers_of_one_file_at_once_each_write_it_whole() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("list.json");
        let writers: Vec<_> = [b'a', b'b']
            .into_iter()
            .map(|c| {
                let path = path.clone();
                std::thread::spawn(move || {
                    let bytes = vec![c; 1 << 16];
                    (0..100).all(|_| write_file(&path, &bytes).is_ok())
                })
            })
            .collect();
        for w in writers {
            assert!(w.join().unwrap(), "no write failed");
        }
        let kept = fs::read(&path).unwrap();
        assert_eq!(kept.len(), 1 << 16);
        assert!(kept.iter().all(|&c| c == kept[0]), "one writer's whole");
        assert_eq!(fs::read_dir(dir.path()).unwrap().count(), 1, "no part left");
    }
}
