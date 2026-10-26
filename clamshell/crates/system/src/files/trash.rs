//! freedesktop's Trash: the home Trash or a filesystem's own `.Trash-<uid>`, with a record of where each
//! thing was so it can be restored.

use std::fs;
use std::io::{self, Write};
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};

use super::ops::{free_name, name_of};
use crate::os::{sys, text};

/// The home Trash, in the data home (`$XDG_DATA_HOME`, else `~/.local/share`).
pub(super) fn home_trash(data_home: Option<&Path>) -> Option<PathBuf> {
    data_home.map(|d| d.join("Trash"))
}

/// The mount point of the filesystem `path` is on: the longest mount point it is under.
pub(super) fn mount_point(path: &Path) -> PathBuf {
    sys::mount_table().into_iter().map(|m| m.dir).filter(|d| path.starts_with(d)).max_by_key(|d| d.as_os_str().len()).unwrap_or_else(|| PathBuf::from("/"))
}

/// Moves `path` to the Trash. That is the home Trash if `path` is on the same filesystem, or else its
/// filesystem's `.Trash-<uid>`, where the path is recorded relative to the filesystem's top.
pub(super) fn trash(path: &Path, data_home: Option<&Path>) -> Result<(), String> {
    let path = path.canonicalize().map_err(|e| format!("{}: {e}", name_of(path)))?;
    let dev = |p: &Path| fs::metadata(p).map(|m| m.dev()).ok();
    let home_trash = home_trash(data_home).ok_or("no home dir")?;
    let on_home = dev(&path) == home_trash.ancestors().find_map(dev);
    let (trash, recorded) = if on_home {
        (home_trash, path.clone())
    } else {
        let top = mount_point(&path);
        let recorded = path.strip_prefix(&top).map(Path::to_path_buf).unwrap_or_else(|_| path.clone());
        (top.join(format!(".Trash-{}", rustix::process::getuid().as_raw())), recorded)
    };
    let (files, info) = (trash.join("files"), trash.join("info"));
    for dir in [&files, &info] {
        fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    }
    use std::os::unix::fs::PermissionsExt;
    if let Err(e) = fs::set_permissions(&trash, fs::Permissions::from_mode(0o700)) {
        log::warn!("trash: {}: {e}", trash.display());
    }
    // Find a name free in both dirs. The record is created first, exclusively, then the file is moved.
    let name = name_of(&path);
    let mut n = 1;
    loop {
        let candidate = if n == 1 { name.clone() } else { format!("{name}.{n}") };
        let record = info.join(format!("{candidate}.trashinfo"));
        let text = format!("[Trash Info]\nPath={}\nDeletionDate={}\n", text::percent_encode_path(&recorded.to_string_lossy()), deletion_date());
        match fs::OpenOptions::new().write(true).create_new(true).open(&record) {
            Ok(mut f) if files.join(&candidate).symlink_metadata().is_err() => {
                f.write_all(text.as_bytes()).map_err(|e| format!("{}: {e}", record.display()))?;
                if let Err(e) = fs::rename(&path, files.join(&candidate)) {
                    if let Err(e) = fs::remove_file(&record) {
                        log::warn!("trash: removing {}: {e}", record.display());
                    }
                    return Err(format!("Moving {name} to the Trash: {e}"));
                }
                return Ok(());
            }
            Ok(_) => {
                if let Err(e) = fs::remove_file(&record) {
                    log::warn!("trash: removing {}: {e}", record.display());
                }
            }
            Err(e) if e.kind() == io::ErrorKind::AlreadyExists => {}
            Err(e) => return Err(format!("{}: {e}", record.display())),
        }
        n += 1;
    }
}

/// The Trash's record of `path`, if it is at the top of a Trash's `files`.
pub(super) fn trash_info(path: &Path) -> Option<PathBuf> {
    let files = path.parent().filter(|p| p.file_name().is_some_and(|n| n == "files"))?;
    let info = files.parent()?.join("info").join(format!("{}.trashinfo", name_of(path)));
    info.exists().then_some(info)
}

/// Moves `path`, at the top of a Trash's `files`, back to where its record says it was, under a free name
/// if that is taken. Returns where it went.
pub(super) fn restore(path: &Path) -> Result<PathBuf, String> {
    let name = name_of(path);
    let info = trash_info(path).ok_or_else(|| format!("{name}: the Trash has no record of where it was"))?;
    let text = fs::read_to_string(&info).map_err(|e| format!("{}: {e}", info.display()))?;
    let recorded = text.lines().find_map(|l| l.strip_prefix("Path=")).map(text::percent_decode).ok_or_else(|| format!("{}: no Path", info.display()))?;
    let mut to = PathBuf::from(&recorded);
    // A filesystem's own Trash records paths relative to its top.
    if to.is_relative()
        && let Some(top) = info.parent().and_then(Path::parent).and_then(Path::parent)
    {
        to = top.join(to);
    }
    let parent = to.parent().ok_or_else(|| format!("{name}: no folder to go back to"))?.to_path_buf();
    fs::create_dir_all(&parent).map_err(|e| format!("{}: {e}", parent.display()))?;
    let to = free_name(&parent, &name_of(&to));
    fs::rename(path, &to).map_err(|e| format!("Restoring {name}: {e}"))?;
    if let Err(e) = fs::remove_file(&info) {
        log::warn!("trash: removing {}: {e}", info.display());
    }
    Ok(to)
}

/// The current local time as the Trash records it, like 2026-10-18T14:03:00.
pub(super) fn deletion_date() -> String {
    jiff::Zoned::now().strftime("%Y-%m-%dT%H:%M:%S").to_string()
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::indexing_slicing)]
mod tests {
    use super::*;

    #[test]
    fn the_trash_records_where_things_were_and_restores_them() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path();
        let home = dir.join("home");
        fs::create_dir_all(home.join("docs")).unwrap();
        fs::write(home.join("docs/a b.txt"), "x").unwrap();
        let data = home.join(".local/share");
        let trash_dir = home_trash(Some(&data)).unwrap();
        trash(&home.join("docs/a b.txt"), Some(&data)).unwrap();
        assert!(!home.join("docs/a b.txt").exists());
        let record = fs::read_to_string(trash_dir.join("info/a b.txt.trashinfo")).unwrap();
        assert!(record.contains("Path=") && record.contains("a%20b.txt"), "{record}");
        // The same name again gets a different name in the Trash.
        fs::write(home.join("docs/a b.txt"), "y").unwrap();
        trash(&home.join("docs/a b.txt"), Some(&data)).unwrap();
        assert_eq!(fs::read_dir(trash_dir.join("files")).unwrap().count(), 2);
        let to = restore(&trash_dir.join("files/a b.txt")).unwrap();
        assert_eq!(to, home.join("docs/a b.txt").canonicalize().unwrap());
        assert!(!trash_dir.join("info/a b.txt.trashinfo").exists());
    }
}
