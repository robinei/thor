//! The files thread. It runs the operations one after another, with their progress: rename, new folder,
//! paste, the Trash, delete, eject and extract.

use std::fs;
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use clamshell_state::Event;
use clamshell_state::library_view::files::{FileOp, FilesEvent};

use super::trash::{restore, trash, trash_info};
use crate::Sink;
use crate::os::worker::Worker;

/// How long ejecting a card may take.
const EJECT_LIMIT: Duration = Duration::from_secs(60);

/// How often, at most, a paste's progress is sent.
const PROGRESS: Duration = Duration::from_millis(250);

/// How long bsdtar may read nothing more of an archive before it is taken as stuck, stopped, and the
/// extraction failed. It reads as it goes, so this is long only for a device that stopped answering.
const EXTRACT_STALL: Duration = Duration::from_secs(120);

/// Starts the files thread. It runs each operation, then sends what was done or what went wrong.
/// `data_home` is where the home Trash is.
pub(crate) fn spawn_ops(data_home: Option<PathBuf>, sink: impl Sink) -> io::Result<Worker<FileOp>> {
    Worker::spawn(clamshell_state::Thread::Files.name(), move |ops| {
        for op in ops {
            log::info!("files: {op:?}");
            let result = run(&op, data_home.as_deref(), &sink);
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

fn run(op: &FileOp, data_home: Option<&Path>, sink: &impl Sink) -> Result<String, String> {
    match op {
        FileOp::Rename { path, name } => {
            let to = path.with_file_name(name);
            match rename_new(path, &to) {
                Ok(()) => Ok(format!("Renamed to {name}")),
                Err(e) if e.kind() == io::ErrorKind::AlreadyExists => Err(format!("{name} is there already")),
                Err(e) => Err(failed("Renaming", path, &e)),
            }
        }
        FileOp::NewFolder { dir, name } => {
            fs::create_dir(dir.join(name)).map_err(|e| failed("Making", &dir.join(name), &e))?;
            Ok(format!("Made {name}"))
        }
        FileOp::Paste { path, into, cut } => paste(path, into, *cut, sink),
        FileOp::Extract { archive, name } => extract(archive, name, sink),
        FileOp::Trash(path) => {
            trash(path, data_home)?;
            Ok(format!("Moved {} to the Trash", name_of(path)))
        }
        FileOp::Restore(path) => {
            let to = restore(path)?;
            Ok(format!("Restored {}", to.display()))
        }
        FileOp::Delete(path) => {
            remove(path).map_err(|e| failed("Deleting", path, &e))?;
            // In the Trash, its record goes too.
            if let Some(info) = trash_info(path)
                && let Err(e) = fs::remove_file(&info)
            {
                log::warn!("files: removing {}: {e}", info.display());
            }
            Ok(format!("Deleted {}", name_of(path)))
        }
        FileOp::EmptyTrash(files) => empty_trash(files),
        FileOp::Eject(device) => {
            super::mounts::eject(device, EJECT_LIMIT).map_err(|e| format!("Ejecting {device}: {}", e.message()))?;
            Ok(format!("{device} can be taken out"))
        }
    }
}

/// Empties the Trash whose `files` folder is `files`: each entry is deleted with its record, so one that
/// can't be keeps its record and can still be restored. Records left with no entry are removed too. The
/// failures are counted and the first said.
fn empty_trash(files: &Path) -> Result<String, String> {
    let mut left: Vec<String> = Vec::new();
    for entry in fs::read_dir(files).map_err(|e| failed("Reading", files, &e))? {
        let entry = match entry {
            Ok(entry) => entry.path(),
            Err(e) => {
                log::warn!("files: {}: {e}", files.display());
                left.push(format!("Reading {}: {e}", name_of(files)));
                continue;
            }
        };
        if let Err(e) = remove(&entry) {
            log::warn!("files: deleting {}: {e}", entry.display());
            left.push(failed("Deleting", &entry, &e));
            continue;
        }
        if let Some(info) = trash_info(&entry)
            && let Err(e) = fs::remove_file(&info)
        {
            log::warn!("files: removing {}: {e}", info.display());
        }
    }
    if let Some(info) = files.parent().map(|t| t.join("info")) {
        match fs::read_dir(&info) {
            Ok(entries) => {
                for record in entries.filter_map(|e| e.inspect_err(|e| log::warn!("files: {}: {e}", info.display())).ok()).map(|e| e.path()) {
                    let entry = record.file_stem().map(|n| files.join(n));
                    if entry.is_some_and(|e| e.symlink_metadata().is_err())
                        && let Err(err) = fs::remove_file(&record)
                    {
                        log::warn!("files: removing {}: {err}", record.display());
                    }
                }
            }
            Err(e) if e.kind() == io::ErrorKind::NotFound => log::debug!("files: {}: {e}", info.display()),
            Err(e) => log::warn!("files: {}: {e}", info.display()),
        }
    }
    match left.as_slice() {
        [] => Ok("Emptied the Trash".to_owned()),
        [one] => Err(one.clone()),
        [first, rest @ ..] => Err(format!("{first}, and {} more left in the Trash", rest.len())),
    }
}

/// What went wrong doing `what` to `path`.
pub(super) fn failed(what: &str, path: &Path, e: &io::Error) -> String {
    format!("{what} {}: {e}", name_of(path))
}

/// Deletes a file, or a folder with everything in it.
fn remove(path: &Path) -> io::Result<()> {
    match path.symlink_metadata()?.is_dir() {
        true => fs::remove_dir_all(path),
        false => fs::remove_file(path),
    }
}

/// A free path in `dir` for `name`: the name itself, or else `stem (2).ext`, `stem (3).ext` and so on.
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

/// Renames `from` to `to`, which must not exist: one that appears meanwhile, made by another program, is
/// never replaced (`AlreadyExists`). A filesystem that can't rename so (some FUSE ones) gets a plain
/// rename after a look.
fn rename_new(from: &Path, to: &Path) -> io::Result<()> {
    use rustix::fs::{CWD, RenameFlags, renameat_with};
    match renameat_with(CWD, from, CWD, to, RenameFlags::NOREPLACE) {
        Ok(()) => Ok(()),
        Err(rustix::io::Errno::INVAL | rustix::io::Errno::NOSYS) => {
            log::debug!("files: {}: no rename without replacing here: renamed after a look", to.display());
            if to.symlink_metadata().is_ok() {
                return Err(io::ErrorKind::AlreadyExists.into());
            }
            fs::rename(from, to)
        }
        Err(e) => Err(e.into()),
    }
}

/// Moves `from` into `dir` under a free name for `name` ([`free_name`]), never replacing what appears
/// meanwhile: the next free name is taken then. Returns where it went.
pub(super) fn move_to_free(from: &Path, dir: &Path, name: &str) -> io::Result<PathBuf> {
    // A bound, in case the folder keeps filling as fast.
    for _ in 0..100 {
        let to = free_name(dir, name);
        match rename_new(from, &to) {
            Ok(()) => return Ok(to),
            Err(e) if e.kind() == io::ErrorKind::AlreadyExists => continue,
            Err(e) => return Err(e),
        }
    }
    Err(io::ErrorKind::AlreadyExists.into())
}

/// Copies `path` into `into`, or moves it if `cut`, under a free name. A copy's progress is sent.
fn paste(path: &Path, into: &Path, cut: bool, sink: &impl Sink) -> Result<String, String> {
    let name = name_of(path);
    // A folder can't go into itself, also through a symlink to either. A symlink is copied as a link.
    let resolved = |p: &Path| p.canonicalize().unwrap_or_else(|_| p.to_path_buf());
    let folder = path.symlink_metadata().is_ok_and(|m| m.is_dir());
    if into.starts_with(path) || (folder && resolved(into).starts_with(resolved(path))) {
        return Err(format!("{name} can't go into itself"));
    }
    if cut && path.parent() == Some(into) {
        return Ok(format!("{name} is there already"));
    }
    if cut {
        // Across filesystems a move is a copy, then the original removed; any other failure is the move's.
        match move_to_free(path, into, &name) {
            Ok(_) => return Ok(format!("Moved {name}")),
            Err(e) if e.kind() == io::ErrorKind::CrossesDevices => {
                log::debug!("files: moving {} into {}: {e}: copying it", path.display(), into.display());
            }
            Err(e) => return Err(format!("Moving {name}: {e}")),
        }
    }
    let to = free_name(into, &name);
    let mut progress = Progress {
        what: format!("{} {name}", if cut { "Moving" } else { "Copying" }),
        total: size(path).max(1),
        done: 0,
        last: Instant::now(),
        sink,
        left_out: 0,
    };
    copy_over(path, &to, cut, &mut progress)
}

/// Copies `path` to `to`, then removes `path` if `cut`. What was copied is on the disk, synced once for
/// its filesystem, before the original goes. Pipes, sockets and devices can't be copied: a copy says how
/// many were left out, and a move with any keeps its original, since they would be lost.
fn copy_over(path: &Path, to: &Path, cut: bool, progress: &mut Progress<impl Sink>) -> Result<String, String> {
    let name = name_of(path);
    let copied = copy(path, to, progress).and_then(|()| {
        let dir = to.parent().unwrap_or(Path::new("/"));
        Ok(rustix::fs::syncfs(fs::File::open(dir)?)?)
    });
    if let Err(e) = copied {
        if let Err(e) = remove(to) {
            log::warn!("files: removing {}: {e}", to.display());
        }
        return Err(format!("Copying {name}: {e}"));
    }
    let left_out = match progress.left_out {
        0 => String::new(),
        1 => "1 pipe, socket or device".to_owned(),
        n => format!("{n} pipes, sockets or devices"),
    };
    match (cut, left_out.is_empty()) {
        (true, true) => {
            remove(path).map_err(|e| format!("Moving {name}: copied, but the original stays: {e}"))?;
            Ok(format!("Moved {name}"))
        }
        (true, false) => Err(format!("Moving {name}: copied, but the original stays: {left_out} in it can't be moved")),
        (false, true) => Ok(format!("Copied {name}")),
        (false, false) => Ok(format!("Copied {name}, without {left_out}")),
    }
}

/// Extracts `archive` with bsdtar into a new folder `name` beside it, with " (2)" added if the name is
/// taken. The progress sent is how far bsdtar has read the archive.
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
        if let Err(e) = fs::remove_dir(&into) {
            log::warn!("files: removing {}: {e}", into.display());
        }
        format!("Extracting {what}: bsdtar: {e}")
    })?;
    let total = archive.metadata().map_or(1, |m| m.len().max(1));
    // Read as it comes, since a full pipe would stall bsdtar.
    let errors = crate::os::command::drain("bsdtar", child.stderr.take());
    let mut stall = Stall::new(Instant::now());
    // bsdtar's open file is the archive's resolved path: through a symlinked folder it differs.
    let opened = archive.canonicalize().unwrap_or_else(|_| archive.to_path_buf());
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break Ok(status),
            Ok(None) => {}
            Err(e) => break Err(e),
        }
        let read = read_so_far(child.id(), &opened);
        if let Some(read) = read {
            let fraction = (read as f64 / total as f64) as f32;
            let _ = sink.send(Event::Files(FilesEvent::Busy { what: format!("Extracting {what}"), fraction }));
        }
        if stall.stuck(read, Instant::now()) {
            log::warn!("files: bsdtar read nothing of {} for {} s: stopped", archive.display(), EXTRACT_STALL.as_secs());
            if let Err(e) = child.kill().and_then(|()| child.wait().map(drop)) {
                log::warn!("files: stopping bsdtar: {e}");
            }
            break Err(io::Error::other(format!("bsdtar read nothing for {} s, stopped", EXTRACT_STALL.as_secs())));
        }
        std::thread::sleep(PROGRESS);
    };
    let said = String::from_utf8_lossy(&errors.recv_timeout(Duration::from_secs(1)).unwrap_or_default()).into_owned();
    match status {
        Ok(s) if s.success() => Ok(format!("Extracted {what} into {}", name_of(&into))),
        result => {
            if let Err(e) = remove(&into) {
                log::warn!("files: removing {}: {e}", into.display());
            }
            let why = said.lines().rev().find(|l| !l.trim().is_empty()).map(str::trim).map(str::to_owned);
            Err(format!("Extracting {what}: {}", why.unwrap_or_else(|| result.map_or_else(|e| e.to_string(), |s| s.to_string()))))
        }
    }
}

/// Whether bsdtar is stuck: how far it has read hasn't moved for [`EXTRACT_STALL`]. A position that
/// can't be read counts as no move.
struct Stall {
    read: Option<u64>,
    since: Instant,
}

impl Stall {
    fn new(now: Instant) -> Stall {
        Stall { read: None, since: now }
    }

    /// Takes how far it has read now; whether it is stuck.
    fn stuck(&mut self, read: Option<u64>, now: Instant) -> bool {
        if read.is_some() && read != self.read {
            (self.read, self.since) = (read, now);
        }
        now.saturating_duration_since(self.since) >= EXTRACT_STALL
    }
}

/// How far process `pid` has read `file`, from the open file's offset, if it has it open.
fn read_so_far(pid: u32, file: &Path) -> Option<u64> {
    let fds = PathBuf::from(format!("/proc/{pid}/fd"));
    let fd = fs::read_dir(&fds)
        .inspect_err(|e| log::debug!("files: {}: {e}", fds.display()))
        .ok()?
        .flatten()
        .find(|e| fs::read_link(e.path()).is_ok_and(|t| t == file))?;
    let info_path = PathBuf::from(format!("/proc/{pid}/fdinfo")).join(fd.file_name());
    let info = fs::read_to_string(&info_path).inspect_err(|e| log::debug!("files: {}: {e}", info_path.display())).ok()?;
    info.lines().find_map(|l| l.strip_prefix("pos:")).and_then(|p| p.trim().parse().ok())
}

/// The total size in bytes of the files in `path`, including subfolders.
fn size(path: &Path) -> u64 {
    match path.symlink_metadata() {
        Ok(m) if m.is_dir() => {
            fs::read_dir(path).inspect_err(|e| log::debug!("files: {}: {e}", path.display())).map(|d| d.flatten().map(|e| size(&e.path())).sum()).unwrap_or(0)
        }
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
    /// The pipes, sockets and devices met, which have no contents to copy.
    left_out: usize,
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

/// Copies `from`, which is a file, a link or a folder with everything in it, to `to`, which must not
/// exist.
fn copy(from: &Path, to: &Path, progress: &mut Progress<impl Sink>) -> io::Result<()> {
    let meta = from.symlink_metadata()?;
    if meta.file_type().is_symlink() {
        return std::os::unix::fs::symlink(fs::read_link(from)?, to);
    }
    if meta.is_dir() {
        fs::create_dir(to)?;
        for e in fs::read_dir(from)? {
            let e = e?;
            copy(&e.path(), &to.join(e.file_name()), progress)?;
        }
        return fs::set_permissions(to, meta.permissions());
    }
    // A pipe, socket or device has no contents to copy: opening a pipe would wait for a writer.
    if !meta.is_file() {
        log::info!("files: {}: not a file or folder: not copied", from.display());
        progress.left_out += 1;
        return Ok(());
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
    fs::set_permissions(to, meta.permissions())
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::indexing_slicing)]
mod tests {
    use super::*;
    use std::sync::mpsc;

    /// bsdtar is stuck once how far it has read hasn't moved for the limit; a move, however small, starts
    /// the wait again, and a position that can't be read is no move.
    #[test]
    fn bsdtar_is_stuck_once_it_reads_nothing_for_the_limit() {
        let start = Instant::now();
        let at = |secs: u64| start + Duration::from_secs(secs);
        let limit = EXTRACT_STALL.as_secs();
        let mut stall = Stall::new(start);
        assert!(!stall.stuck(Some(10), at(1)));
        assert!(!stall.stuck(Some(10), at(limit)), "not yet: it moved at 1 s");
        assert!(!stall.stuck(Some(11), at(limit + 1)), "moved");
        assert!(!stall.stuck(None, at(2 * limit)));
        assert!(stall.stuck(None, at(2 * limit + 1)), "nothing read since");
    }

    #[test]
    fn pastes_never_replace() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path();
        fs::create_dir_all(dir.join("from/sub")).unwrap();
        fs::write(dir.join("from/sub/f.txt"), "hello").unwrap();
        fs::write(dir.join("a.txt"), "a").unwrap();
        let (tx, _rx) = mpsc::channel();
        assert_eq!(paste(&dir.join("a.txt"), dir, false, &tx).unwrap(), "Copied a.txt");
        assert_eq!(fs::read_to_string(dir.join("a (2).txt")).unwrap(), "a");
        paste(&dir.join("from"), &dir.join("from/sub"), false, &tx).unwrap_err();
        fs::create_dir(dir.join("to")).unwrap();
        paste(&dir.join("from"), &dir.join("to"), true, &tx).unwrap();
        assert_eq!(fs::read_to_string(dir.join("to/from/sub/f.txt")).unwrap(), "hello");
        assert!(!dir.join("from").exists(), "cut: moved");
    }

    /// A folder can't be pasted into itself through a symlink either; a pipe in a folder is left out
    /// rather than waited on.
    #[test]
    fn pastes_go_not_into_themselves_and_skip_pipes() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path();
        fs::create_dir_all(dir.join("games/sub")).unwrap();
        std::os::unix::fs::symlink(dir.join("games"), dir.join("link")).unwrap();
        let (tx, _rx) = mpsc::channel();
        paste(&dir.join("games"), &dir.join("link/sub"), false, &tx).unwrap_err();
        let fifo = dir.join("games/pipe");
        rustix::fs::mknodat(rustix::fs::CWD, &fifo, rustix::fs::FileType::Fifo, rustix::fs::Mode::from_raw_mode(0o600), 0).unwrap();
        fs::create_dir(dir.join("to")).unwrap();
        assert_eq!(paste(&dir.join("games"), &dir.join("to"), false, &tx).unwrap(), "Copied games, without 1 pipe, socket or device");
        assert!(dir.join("to/games/sub").is_dir() && dir.join("to/games/pipe").symlink_metadata().is_err());
    }

    /// A rename or a move never replaces what is there, also what appeared after the name was looked at:
    /// a rename is refused, a move takes the next free name.
    #[test]
    fn renames_and_moves_never_replace() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path();
        fs::write(dir.join("a.txt"), "a").unwrap();
        fs::write(dir.join("b.txt"), "b").unwrap();
        assert_eq!(rename_new(&dir.join("a.txt"), &dir.join("b.txt")).unwrap_err().kind(), io::ErrorKind::AlreadyExists);
        assert_eq!(fs::read_to_string(dir.join("b.txt")).unwrap(), "b", "kept");
        fs::create_dir(dir.join("to")).unwrap();
        fs::write(dir.join("to/a.txt"), "there").unwrap();
        assert_eq!(move_to_free(&dir.join("a.txt"), &dir.join("to"), "a.txt").unwrap(), dir.join("to/a (2).txt"));
        assert_eq!(fs::read_to_string(dir.join("to/a.txt")).unwrap(), "there");
        let (tx, _rx) = mpsc::channel();
        fs::write(dir.join("c.txt"), "c").unwrap();
        assert_eq!(run(&FileOp::Rename { path: dir.join("b.txt"), name: "c.txt".into() }, None, &tx), Err("c.txt is there already".into()));
        assert_eq!(fs::read_to_string(dir.join("c.txt")).unwrap(), "c");
    }

    /// Emptying the Trash deletes each entry with its record; one that can't be deleted keeps its record,
    /// the others go on, and the failure is said at the end. A record with no entry goes too.
    #[test]
    fn emptying_the_trash_keeps_the_records_of_what_stays() {
        use std::os::unix::fs::PermissionsExt;
        let tmp = tempfile::tempdir().unwrap();
        let trash = tmp.path().join("Trash");
        fs::create_dir_all(trash.join("files/stuck/locked")).unwrap();
        fs::create_dir_all(trash.join("info")).unwrap();
        fs::write(trash.join("files/stuck/locked/f"), "x").unwrap();
        fs::write(trash.join("files/z.txt"), "z").unwrap();
        for name in ["stuck", "z.txt", "gone.txt"] {
            fs::write(trash.join(format!("info/{name}.trashinfo")), "[Trash Info]\n").unwrap();
        }
        // A folder whose contents can't be deleted.
        fs::set_permissions(trash.join("files/stuck/locked"), fs::Permissions::from_mode(0o500)).unwrap();
        let emptied = empty_trash(&trash.join("files"));
        fs::set_permissions(trash.join("files/stuck/locked"), fs::Permissions::from_mode(0o700)).unwrap();
        assert!(emptied.is_err_and(|e| e.starts_with("Deleting stuck")));
        assert!(!trash.join("files/z.txt").exists(), "the others are deleted");
        let mut records: Vec<_> = fs::read_dir(trash.join("info")).unwrap().map(|e| e.unwrap().file_name()).collect();
        records.sort();
        assert_eq!(records, ["stuck.trashinfo"], "what stays keeps its record; the rest go");
    }

    /// A move made by copying (across filesystems) keeps its original when it holds a pipe, a socket or a
    /// device, which can't be copied and would be lost.
    #[test]
    fn a_move_by_copying_keeps_an_original_with_what_cant_be_copied() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path();
        fs::create_dir_all(dir.join("games")).unwrap();
        fs::write(dir.join("games/save"), "s").unwrap();
        let fifo = dir.join("games/pipe");
        rustix::fs::mknodat(rustix::fs::CWD, &fifo, rustix::fs::FileType::Fifo, rustix::fs::Mode::from_raw_mode(0o600), 0).unwrap();
        let (tx, _rx) = mpsc::channel();
        let mut progress = Progress { what: String::new(), total: 1, done: 0, last: Instant::now(), sink: &tx, left_out: 0 };
        let moved = copy_over(&dir.join("games"), &dir.join("copy"), true, &mut progress);
        assert!(moved.is_err_and(|e| e.contains("the original stays")));
        assert!(dir.join("games/pipe").symlink_metadata().is_ok(), "kept");
        assert_eq!(fs::read_to_string(dir.join("copy/save")).unwrap(), "s", "copied all it could");
        fs::remove_file(&fifo).unwrap();
        let mut progress = Progress { what: String::new(), total: 1, done: 0, last: Instant::now(), sink: &tx, left_out: 0 };
        assert_eq!(copy_over(&dir.join("games"), &dir.join("moved"), true, &mut progress).unwrap(), "Moved games");
        assert!(!dir.join("games").exists());
    }

    #[test]
    fn encodings() {
        assert_eq!(name_of(&free_name(Path::new("/nonexistent"), "a.txt")), "a.txt");
    }
}
