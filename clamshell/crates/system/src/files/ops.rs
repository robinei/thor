//! The files thread: the operations one after another (rename, new folder, paste, the trash, delete,
//! eject, extract), with their progress.

use std::fs;
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use clamshell_core::Event;
use clamshell_core::shelf::files::{FileOp, FilesEvent};

use super::trash::{restore, trash, trash_info};
use crate::Sink;
use crate::os::command;
use crate::os::worker::Worker;

/// How long ejecting a card may take.
const EJECT_LIMIT: Duration = Duration::from_secs(60);

/// A paste's progress, at most this often.
const PROGRESS: Duration = Duration::from_millis(250);

/// The files thread: each operation done, then what was done (or what went wrong) sent.
pub fn spawn_ops(home: Option<PathBuf>, sink: impl Sink) -> io::Result<Worker<FileOp>> {
    Worker::spawn("files", move |ops| {
        for op in ops {
            log::info!("files: {op:?}");
            let result = run(&op, home.as_deref(), &sink);
            if let Err(e) = &result {
                log::warn!("files: {e}");
            }
            if sink.send(Event::Files(FilesEvent::Done(result))).is_err() {
                return;
            }
        }
    })
}

pub(super) fn name_of(path: &Path) -> String {
    path.file_name().map_or_else(|| path.display().to_string(), |n| n.to_string_lossy().into_owned())
}

fn run(op: &FileOp, home: Option<&Path>, sink: &impl Sink) -> Result<String, String> {
    match op {
        FileOp::Rename { path, name } => {
            let to = path.with_file_name(name);
            if to.symlink_metadata().is_ok() {
                return Err(format!("{name} is there already"));
            }
            fs::rename(path, &to).map_err(|e| failed("Renaming", path, &e))?;
            Ok(format!("Renamed to {name}"))
        }
        FileOp::NewFolder { dir, name } => {
            fs::create_dir(dir.join(name)).map_err(|e| failed("Making", &dir.join(name), &e))?;
            Ok(format!("Made {name}"))
        }
        FileOp::Paste { path, into, cut } => paste(path, into, *cut, sink),
        FileOp::Extract { archive, name } => extract(archive, name, sink),
        FileOp::Trash(path) => {
            trash(path, home)?;
            Ok(format!("Moved {} to the Trash", name_of(path)))
        }
        FileOp::Restore(path) => {
            let to = restore(path)?;
            Ok(format!("Restored {}", to.display()))
        }
        FileOp::Delete(path) => {
            remove(path).map_err(|e| failed("Deleting", path, &e))?;
            // (In the Trash: its record too.)
            if let Some(info) = trash_info(path) {
                let _ = fs::remove_file(info);
            }
            Ok(format!("Deleted {}", name_of(path)))
        }
        FileOp::EmptyTrash(files) => {
            for entry in fs::read_dir(files).map_err(|e| failed("Reading", files, &e))?.flatten() {
                remove(&entry.path()).map_err(|e| failed("Deleting", &entry.path(), &e))?;
            }
            if let Some(info) = files.parent().map(|t| t.join("info"))
                && let Ok(entries) = fs::read_dir(info)
            {
                for e in entries.flatten() {
                    let _ = fs::remove_file(e.path());
                }
            }
            Ok("Emptied the Trash".to_owned())
        }
        FileOp::Eject(device) => {
            // (Unmounting writes out what is pending, which on a slow card can take a while.)
            let out = command::run("udisksctl", &["unmount", "-b", device], None, EJECT_LIMIT)?;
            if !out.status.success() {
                return Err(format!("Ejecting {device}: {}", out.said()));
            }
            // (An SD card reader may not power off: unmounted is enough.)
            let _ = command::run("udisksctl", &["power-off", "-b", device], None, command::LIMIT);
            Ok(format!("{device} can be taken out"))
        }
    }
}

/// What went wrong doing `what` to `path`.
pub(super) fn failed(what: &str, path: &Path, e: &io::Error) -> String {
    format!("{what} {}: {e}", name_of(path))
}

/// A file, or a folder with everything in it, deleted.
fn remove(path: &Path) -> io::Result<()> {
    match path.symlink_metadata()?.is_dir() {
        true => fs::remove_dir_all(path),
        false => fs::remove_file(path),
    }
}

/// A name in `dir` free for `name`: itself, else `stem (2).ext`, `stem (3).ext` ...
pub(super) fn free_name(dir: &Path, name: &str) -> PathBuf {
    let (stem, ext) = match name.rsplit_once('.') {
        Some((stem, ext)) if !stem.is_empty() => (stem, format!(".{ext}")),
        _ => (name, String::new()),
    };
    let mut path = dir.join(name);
    let mut n = 2;
    while path.symlink_metadata().is_ok() {
        path = dir.join(format!("{stem} ({n}){ext}"));
        n += 1;
    }
    path
}

/// `path` copied (or moved, `cut`) into `into`, under a free name; a copy's progress sent.
fn paste(path: &Path, into: &Path, cut: bool, sink: &impl Sink) -> Result<String, String> {
    let name = name_of(path);
    if into.starts_with(path) {
        return Err(format!("{name} can't go into itself"));
    }
    if cut && path.parent() == Some(into) {
        return Ok(format!("{name} is there already"));
    }
    let to = free_name(into, &name);
    if cut && fs::rename(path, &to).is_ok() {
        return Ok(format!("Moved {name}"));
    }
    let mut progress =
        Progress { what: format!("{} {name}", if cut { "Moving" } else { "Copying" }), total: size(path).max(1), done: 0, last: Instant::now(), sink };
    if let Err(e) = copy(path, &to, &mut progress) {
        let _ = remove(&to);
        return Err(format!("Copying {name}: {e}"));
    }
    if cut {
        remove(path).map_err(|e| format!("Moving {name}: copied, but the original stays: {e}"))?;
        return Ok(format!("Moved {name}"));
    }
    Ok(format!("Copied {name}"))
}

/// Archive `archive`'s entries extracted (bsdtar) into a new folder `name` beside it (" (2)" if taken);
/// its progress, how far bsdtar has read the archive, sent.
fn extract(archive: &Path, name: &str, sink: &impl Sink) -> Result<String, String> {
    let what = name_of(archive);
    let into = free_name(archive.parent().unwrap_or(Path::new("/")), name);
    fs::create_dir(&into).map_err(|e| failed("Making", &into, &e))?;
    let child = std::process::Command::new("bsdtar")
        .arg("-xf")
        .arg(archive)
        .arg("-C")
        .arg(&into)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::piped())
        .spawn();
    let mut child = child.map_err(|e| {
        let _ = fs::remove_dir(&into);
        format!("Extracting {what}: bsdtar: {e}")
    })?;
    let total = archive.metadata().map_or(1, |m| m.len().max(1));
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break Ok(status),
            Ok(None) => {}
            Err(e) => break Err(e),
        }
        if let Some(read) = read_so_far(child.id(), archive) {
            let fraction = (read as f64 / total as f64) as f32;
            let _ = sink.send(Event::Files(FilesEvent::Busy { what: format!("Extracting {what}"), fraction }));
        }
        std::thread::sleep(PROGRESS);
    };
    let mut said = String::new();
    if let Some(mut e) = child.stderr.take() {
        let _ = e.read_to_string(&mut said);
    }
    match status {
        Ok(s) if s.success() => Ok(format!("Extracted {what} into {}", name_of(&into))),
        result => {
            let _ = remove(&into);
            let why = said.lines().rev().find(|l| !l.trim().is_empty()).map(str::trim).map(str::to_owned);
            Err(format!("Extracting {what}: {}", why.unwrap_or_else(|| result.map_or_else(|e| e.to_string(), |s| s.to_string()))))
        }
    }
}

/// How far process `pid` has read `file` (its open file's offset), if it has it open.
fn read_so_far(pid: u32, file: &Path) -> Option<u64> {
    let fds = PathBuf::from(format!("/proc/{pid}/fd"));
    let fd = fs::read_dir(&fds).ok()?.flatten().find(|e| fs::read_link(e.path()).is_ok_and(|t| t == file))?;
    let info = fs::read_to_string(PathBuf::from(format!("/proc/{pid}/fdinfo")).join(fd.file_name())).ok()?;
    info.lines().find_map(|l| l.strip_prefix("pos:")).and_then(|p| p.trim().parse().ok())
}

/// The bytes of the files in `path`, all the way down.
fn size(path: &Path) -> u64 {
    match path.symlink_metadata() {
        Ok(m) if m.is_dir() => fs::read_dir(path).map(|d| d.flatten().map(|e| size(&e.path())).sum()).unwrap_or(0),
        Ok(m) if m.is_file() => m.len(),
        _ => 0,
    }
}

struct Progress<'a, S> {
    what: String,
    total: u64,
    done: u64,
    last: Instant,
    sink: &'a S,
}

impl<S: Sink> Progress<'_, S> {
    fn add(&mut self, bytes: u64) {
        self.done += bytes;
        if self.last.elapsed() >= PROGRESS {
            self.last = Instant::now();
            let fraction = (self.done as f64 / self.total as f64) as f32;
            let _ = self.sink.send(Event::Files(FilesEvent::Busy { what: self.what.clone(), fraction }));
        }
    }
}

/// `from` (a file, a link, a folder with everything in it) copied to `to`, which isn't there.
fn copy(from: &Path, to: &Path, progress: &mut Progress<impl Sink>) -> io::Result<()> {
    let meta = from.symlink_metadata()?;
    if meta.file_type().is_symlink() {
        return std::os::unix::fs::symlink(fs::read_link(from)?, to);
    }
    if meta.is_dir() {
        fs::create_dir(to)?;
        for e in fs::read_dir(from)?.flatten() {
            copy(&e.path(), &to.join(e.file_name()), progress)?;
        }
        return fs::set_permissions(to, meta.permissions());
    }
    let mut src = fs::File::open(from)?;
    let mut dst = fs::OpenOptions::new().write(true).create_new(true).open(to)?;
    let mut buf = vec![0u8; 1 << 20];
    loop {
        let n = src.read(&mut buf)?;
        if n == 0 {
            break;
        }
        dst.write_all(buf.get(..n).unwrap_or(&[]))?;
        progress.add(n as u64);
    }
    dst.sync_all()?;
    fs::set_permissions(to, meta.permissions())
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::indexing_slicing)]
mod tests {
    use super::*;
    use std::sync::mpsc;

    fn scratch(name: &str) -> clamshell_core::kit::testdir::TempDir {
        clamshell_core::kit::testdir::TempDir::new(&format!("files-{name}"))
    }

    #[test]
    fn pastes_never_replace() {
        let dir = scratch("paste");
        fs::create_dir_all(dir.join("from/sub")).unwrap();
        fs::write(dir.join("from/sub/f.txt"), "hello").unwrap();
        fs::write(dir.join("a.txt"), "a").unwrap();
        let (tx, _rx) = mpsc::channel();
        assert_eq!(paste(&dir.join("a.txt"), &dir, false, &tx).unwrap(), "Copied a.txt");
        assert_eq!(fs::read_to_string(dir.join("a (2).txt")).unwrap(), "a");
        paste(&dir.join("from"), &dir.join("from/sub"), false, &tx).unwrap_err();
        fs::create_dir(dir.join("to")).unwrap();
        paste(&dir.join("from"), &dir.join("to"), true, &tx).unwrap();
        assert_eq!(fs::read_to_string(dir.join("to/from/sub/f.txt")).unwrap(), "hello");
        assert!(!dir.join("from").exists(), "cut: moved");
    }

    #[test]
    fn encodings() {
        assert_eq!(name_of(&free_name(Path::new("/nonexistent"), "a.txt")), "a.txt");
    }
}
