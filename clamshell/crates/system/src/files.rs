//! Browsed dirs and file operations (see clamshell-core's `files`). The browse thread lists the folder
//! shelf has open and lists it again whenever it changes (inotify), and notices removable filesystems
//! mounted and unmounted (udisks2's, under `/run/media`). The files thread does the operations one after
//! another: rename, new folder, paste (a copy, or a move; across filesystems a copy then a delete, with
//! its progress), the Trash (freedesktop's: the home Trash, or a filesystem's own `.Trash-<uid>`), delete,
//! eject (`udisksctl`), extract (`bsdtar`, its progress read from how far it has read the archive).
//! An archive is browsed as a folder through its mount (see [`crate::archives`]).

use std::fs;
use std::io::{self, Read, Write};
use std::mem::MaybeUninit;
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};
use std::sync::mpsc::TryRecvError;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use clamshell_core::files::{Browse, BrowseCommand, FileOp, FilesEvent};
use clamshell_core::{Browsed, Event, Game};
use rustix::fs::inotify;

use crate::Sink;
use crate::library::content;
use crate::library::runners::{self, Runner};
use crate::worker::{Bell, Worker};

/// A paste's progress, at most this often.
const PROGRESS: Duration = Duration::from_millis(250);
/// Where udisks2 mounts removable filesystems.
const MEDIA: &str = "/run/media/";

/// Where a browse thread reads and what it does with videos: library.json (how files open), the home dir
/// (`~/`), the database (videos' lengths and frames, as for scanned ones), and how to have those not
/// read yet read (in the background: they come as metadata).
pub struct Browsing {
    pub config: Option<PathBuf>,
    pub home: Option<PathBuf>,
    pub db: Option<PathBuf>,
    pub probe: Box<dyn Fn(Vec<crate::meta::Wanted>) + Send>,
    /// Where archives' indexes are kept (see [`crate::archives`]).
    pub cache: Option<PathBuf>,
}

/// The browse thread: lists (and watches) the folder each command names (see [`Browsing`]).
pub fn spawn_browse(browsing: Browsing, sink: impl Sink) -> io::Result<Worker<BrowseCommand>> {
    let Browsing { config, home, db, probe, cache } = browsing;
    Worker::spawn_belled("browse", move |commands, bell| {
        let mut archives = crate::archives::Mounts::new(cache);
        let mut in_use: Vec<PathBuf> = Vec::new();
        let notify = inotify::init(inotify::CreateFlags::NONBLOCK | inotify::CreateFlags::CLOEXEC).map_err(|e| log::warn!("browse: inotify: {e}")).ok();
        let flags = inotify::WatchFlags::CREATE
            | inotify::WatchFlags::DELETE
            | inotify::WatchFlags::MOVED_FROM
            | inotify::WatchFlags::MOVED_TO
            | inotify::WatchFlags::CLOSE_WRITE
            | inotify::WatchFlags::ATTRIB
            | inotify::WatchFlags::DELETE_SELF
            | inotify::WatchFlags::MOVE_SELF;
        // (The folder open, the dir listed for it, its watch.)
        let mut open: Option<(Browse, PathBuf, Option<i32>)> = None;
        let mut mounts = media_mounts();
        // (The kernel marks the mount table read through this file when it changes: POLLPRI.)
        let mount_table = fs::File::open("/proc/self/mounts").map_err(|e| log::warn!("browse: mounts: {e}")).ok();
        // (An archive's: its mount's.)
        let list = |b: &Browse, dir: &Path| {
            let config = config.as_deref().map(content::read_config).unwrap_or_default().0;
            let (runners, _) = runners::in_force(&config.runners);
            let mut entries = list(dir, &b.folder, &runners, home.as_deref(), b.hidden);
            let wanted = known_videos(&mut entries, db.as_deref());
            if !wanted.is_empty() {
                probe(wanted);
            }
            sink.send(Event::Files(FilesEvent::Listed { folder: b.folder.clone(), entries }))
        };
        loop {
            // Asleep until a command, the folder or the mounts change.
            let (folder_changed, mounts_changed) = wait(&bell, notify.as_ref(), mount_table.as_ref());
            // (A command and a change may come together: the change is looked at too.)
            match commands.try_recv() {
                Ok(BrowseCommand::InUse(files)) => {
                    in_use = files;
                    let open_dir = open.as_ref().map(|(b, ..)| b.dir.as_path());
                    archives.keep(&in_use.iter().map(PathBuf::as_path).chain(open_dir).collect::<Vec<_>>());
                    bell.ring();
                }
                Ok(BrowseCommand::Open(browse)) => {
                    if let (Some(fd), Some((_, _, Some(wd)))) = (notify.as_ref(), open.take()) {
                        let _ = inotify::remove_watch(fd, wd);
                    }
                    if let Some(fd) = notify.as_ref() {
                        changed(fd);
                    }
                    let Some(b) = browse else { continue };
                    // (An archive: mounted, its mount listed; one that can't be, empty, and why told.)
                    let dir = match b.dir.is_file() {
                        true => archives.mount(&b.dir).unwrap_or_else(|e| {
                            let _ = sink.send(Event::Files(FilesEvent::Done(Err(e))));
                            PathBuf::new()
                        }),
                        false => b.dir.clone(),
                    };
                    let wd = notify
                        .as_ref()
                        .and_then(|fd| inotify::add_watch(fd, dir.as_path(), flags).map_err(|e| log::debug!("browse: {}: {e}", dir.display())).ok());
                    if list(&b, &dir).is_err() {
                        return;
                    }
                    archives.keep(&in_use.iter().map(PathBuf::as_path).chain([b.dir.as_path()]).collect::<Vec<_>>());
                    open = Some((b, dir, wd));
                    // (Another command may be waiting: the bell was cleared for all of them.)
                    bell.ring();
                }
                Err(TryRecvError::Empty) => {
                    if folder_changed
                        && let (Some(fd), Some((b, dir, _))) = (notify.as_ref(), open.as_ref())
                        && changed(fd)
                        && list(b, dir).is_err()
                    {
                        return;
                    }
                }
                Err(TryRecvError::Disconnected) => return,
            }
            if mounts_changed {
                let now = media_mounts();
                if now != mounts {
                    mounts = now;
                    if sink.send(Event::Files(FilesEvent::Mounts)).is_err() {
                        return;
                    }
                }
            }
        }
    })
}

/// Waits for the browse thread's bell, the folder's inotify events or a change of the mounts; which of
/// the last two came.
fn wait(bell: &Bell, notify: Option<&rustix::fd::OwnedFd>, mounts: Option<&fs::File>) -> (bool, bool) {
    use rustix::event::{PollFd, PollFlags};
    let mut fds = vec![PollFd::from_borrowed_fd(bell.fd(), PollFlags::IN)];
    let folder = notify.map(|fd| {
        fds.push(PollFd::new(fd, PollFlags::IN));
        fds.len() - 1
    });
    let table = mounts.map(|f| {
        fds.push(PollFd::new(f, PollFlags::PRI));
        fds.len() - 1
    });
    if rustix::event::poll(&mut fds, None).is_err() {
        // (Interrupted: looked at as if anything changed.)
        return (true, true);
    }
    bell.clear();
    let came = |i: Option<usize>, what: PollFlags| i.and_then(|i| fds.get(i)).is_some_and(|f| f.revents().intersects(what));
    (came(folder, PollFlags::IN), came(table, PollFlags::PRI | PollFlags::ERR))
}

/// Whether inotify has events (all read).
fn changed(fd: &rustix::fd::OwnedFd) -> bool {
    let mut buf = [MaybeUninit::uninit(); 4096];
    let mut reader = inotify::Reader::new(fd, &mut buf);
    let mut any = false;
    while reader.next().is_ok() {
        any = true;
    }
    any
}

/// The entries of `dir` (folder `folder`'s: their ids it, `/`, their names), folders first, then by
/// name; hidden ones (`.name`) only if `hidden`. A picture is its own art; a file opens by the first of
/// `runners` that opens its name (`~/` in its command against `home`).
pub fn list(dir: &Path, folder: &str, runners: &[Runner], home: Option<&Path>, hidden: bool) -> Vec<Game> {
    let Ok(read) = fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut games: Vec<Game> = read
        .flatten()
        .filter_map(|e| {
            let name = e.file_name().into_string().ok().filter(|n| hidden || !n.starts_with('.'))?;
            let path = e.path();
            // (Through a link: what it points at.)
            let meta = fs::metadata(&path).ok();
            let is_dir = meta.as_ref().is_some_and(fs::Metadata::is_dir);
            let size = meta.as_ref().filter(|m| m.is_file()).map(fs::Metadata::len);
            let modified = meta.and_then(|m| m.modified().ok()).and_then(|t| t.duration_since(UNIX_EPOCH).ok()).map_or(0, |d| d.as_secs());
            let mut game = Game::new(&format!("{folder}/{name}"), &name);
            // (An archive: browsed as a folder, through its mount; a runner that opens it kept, for
            // Open in.)
            let archive = !is_dir && crate::archives::is_archive(&name);
            // (An archive's: only a runner whose own files it is, a ROM's zip; not one for any file.)
            let opener = runners::open(runners, &path, home).filter(|_| !is_dir);
            if let Some((launch, runner)) = opener.filter(|_| !archive || runners.iter().find(|r| r.opens(&name)).is_some_and(|r| r.opens_as_its_own(&name))) {
                game.launch = launch;
                game.runner = Some(runner);
            }
            if is_dir {
                (game.runner, game.playlist) = enqueued(&path, runners, home);
            }
            // (Its picture as a scanned entry's would be; else what it is.)
            game.box_front = content::picture_of(&path);
            if game.box_front.is_none() {
                game.glyph = Some(
                    if is_dir {
                        "folder"
                    } else if archive {
                        "archive"
                    } else {
                        "file"
                    }
                    .into(),
                );
            }
            let locked = archive || (is_dir && rustix::fs::access(&path, rustix::fs::Access::WRITE_OK).is_err());
            game.file = Some(path);
            game.browsed = Some(Browsed { folder: is_dir || archive, archive, locked, size, modified, ..Browsed::default() });
            Some(game)
        })
        .collect();
    games.sort_by(|a, b| (!a.is_group(), &a.sort_key, &a.title).cmp(&(!b.is_group(), &b.sort_key, &b.title)));
    games
}

/// What enqueueing folder `dir` (not listed) is by: the runner that enqueues its first file one can
/// enqueue, and its playlist (an `.m3u` in it), from a look at its first names (`~/` the home dir).
fn enqueued(dir: &Path, runners: &[Runner], home: Option<&Path>) -> (Option<clamshell_library::RunnerOf>, Option<PathBuf>) {
    let (mut runner, mut playlist) = (None, None);
    let Ok(read) = fs::read_dir(dir) else {
        return (None, None);
    };
    for e in read.flatten().take(PEEK) {
        let name = e.file_name().to_string_lossy().to_lowercase();
        if name.starts_with('.') {
            continue;
        }
        if playlist.is_none() && (name.ends_with(".m3u") || name.ends_with(".m3u8")) {
            playlist = Some(e.path());
        }
        if runner.is_none() {
            runner = runners::opener(runners, &name).filter(|r| r.enqueue.is_some()).map(|r| r.of(home));
        }
        if runner.is_some() && playlist.is_some() {
            break;
        }
    }
    let playlist = playlist.filter(|_| runner.is_some());
    (runner, playlist)
}

/// How many names of a folder [`enqueued`] looks at.
const PEEK: usize = 256;

/// The videos (mpv's) among `entries`: their lengths and frames as the database has them (read before,
/// here or as scanned entries'); those it hasn't, to read.
fn known_videos(entries: &mut [Game], db: Option<&Path>) -> Vec<crate::meta::Wanted> {
    if !entries.iter().any(|g| g.mpv_file().is_some()) {
        return Vec::new();
    }
    let probes = db.and_then(|f| crate::store::Store::open(f).ok()).and_then(|s| s.probes().ok()).unwrap_or_default();
    let mut wanted = Vec::new();
    for g in entries.iter_mut() {
        let Some(file) = g.mpv_file().map(Path::to_path_buf) else {
            continue;
        };
        match probes.get(&*g.id).filter(|p| p.frame.as_ref().is_none_or(|f| f.exists())) {
            Some(p) => {
                g.duration = p.seconds;
                g.shot.clone_from(&p.frame);
                // (A video's frame is its picture, not the file glyph.)
                g.glyph = None;
            }
            None => {
                g.glyph = None;
                wanted.push(crate::meta::Wanted {
                    id: g.id.clone(),
                    key: g.key.clone(),
                    title: g.title.clone(),
                    file: Some(file),
                    how: crate::meta::How::Duration,
                    art: true,
                });
            }
        }
    }
    wanted
}

/// A removable filesystem mounted: its device, where, its type, label and UUID (empty if none).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Mount {
    pub device: String,
    pub dir: PathBuf,
    pub fs: String,
    pub label: String,
    pub uuid: String,
}

/// The removable filesystems mounted (under `/run/media`), in the order they were mounted.
pub fn media_mounts() -> Vec<Mount> {
    let text = fs::read_to_string("/proc/self/mounts").unwrap_or_default();
    let named = |dir: &str| -> Vec<(String, PathBuf)> {
        fs::read_dir(dir)
            .map(|d| d.flatten().filter_map(|e| Some((unescape(&e.file_name().into_string().ok()?), fs::canonicalize(e.path()).ok()?))).collect())
            .unwrap_or_default()
    };
    let (labels, uuids) = (named("/dev/disk/by-label"), named("/dev/disk/by-uuid"));
    text.lines()
        .filter_map(|line| {
            let mut fields = line.split(' ');
            let (device, dir, fs) = (fields.next()?, unescape(fields.next()?), fields.next()?);
            if !dir.starts_with(MEDIA) {
                return None;
            }
            let real = fs::canonicalize(device).ok();
            let find = |names: &[(String, PathBuf)]| names.iter().find(|(_, p)| Some(p) == real.as_ref()).map(|(n, _)| n.clone()).unwrap_or_default();
            let dir = PathBuf::from(dir);
            let label =
                Some(find(&labels)).filter(|l| !l.is_empty()).unwrap_or_else(|| dir.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default());
            Some(Mount { device: device.to_owned(), fs: fs.to_owned(), label, uuid: find(&uuids), dir })
        })
        .collect()
}

/// `\040` (mounts) and `\x20` (udev's names) as the characters they stand for.
fn unescape(text: &str) -> String {
    let bytes = text.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while let Some(&b) = bytes.get(i) {
        let octal = bytes.get(i + 1..i + 4).filter(|d| d.iter().all(|c| (b'0'..=b'7').contains(c)));
        let hex = bytes
            .get(i + 1..i + 4)
            .filter(|d| d.first() == Some(&b'x'))
            .and_then(|d| std::str::from_utf8(d.get(1..)?).ok())
            .and_then(|h| u8::from_str_radix(h, 16).ok());
        match (b, octal, hex) {
            (b'\\', Some(d), _) => {
                out.push(d.iter().fold(0u8, |n, c| n.wrapping_mul(8).wrapping_add(c - b'0')));
                i += 4;
            }
            (b'\\', None, Some(h)) => {
                out.push(h);
                i += 4;
            }
            _ => {
                out.push(b);
                i += 1;
            }
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

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

fn name_of(path: &Path) -> String {
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
            let out = std::process::Command::new("udisksctl").args(["unmount", "-b", device]).output().map_err(|e| format!("udisksctl: {e}"))?;
            if !out.status.success() {
                return Err(format!("Ejecting {device}: {}", String::from_utf8_lossy(&out.stderr).trim()));
            }
            // (An SD card reader may not power off: unmounted is enough.)
            let _ = std::process::Command::new("udisksctl").args(["power-off", "-b", device]).output();
            Ok(format!("{device} can be taken out"))
        }
    }
}

/// What went wrong doing `what` to `path`.
fn failed(what: &str, path: &Path, e: &io::Error) -> String {
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
fn free_name(dir: &Path, name: &str) -> PathBuf {
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

/// The home Trash: `$XDG_DATA_HOME/Trash`, else `~/.local/share/Trash`.
fn home_trash(home: Option<&Path>) -> Option<PathBuf> {
    match std::env::var_os("XDG_DATA_HOME").filter(|d| !d.is_empty()) {
        Some(data) => Some(PathBuf::from(data).join("Trash")),
        None => home.map(|h| h.join(".local/share/Trash")),
    }
}

/// The filesystem's mount point `path` is on: the longest mount point it is under.
fn mount_point(path: &Path) -> PathBuf {
    let text = fs::read_to_string("/proc/self/mounts").unwrap_or_default();
    text.lines()
        .filter_map(|l| l.split(' ').nth(1))
        .map(|d| PathBuf::from(unescape(d)))
        .filter(|d| path.starts_with(d))
        .max_by_key(|d| d.as_os_str().len())
        .unwrap_or_else(|| PathBuf::from("/"))
}

/// `path` moved to the Trash: the home one where it is on the same filesystem, else its filesystem's
/// `.Trash-<uid>` (the path recorded relative to that filesystem's top).
fn trash(path: &Path, home: Option<&Path>) -> Result<(), String> {
    let path = path.canonicalize().map_err(|e| format!("{}: {e}", name_of(path)))?;
    let dev = |p: &Path| fs::metadata(p).map(|m| m.dev()).ok();
    let home_trash = home_trash(home).ok_or("no home dir")?;
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
    let _ = fs::set_permissions(&trash, fs::Permissions::from_mode(0o700));
    // A name free in both: the record made first (exclusively), then the move.
    let name = name_of(&path);
    let mut n = 1;
    loop {
        let candidate = if n == 1 { name.clone() } else { format!("{name}.{n}") };
        let record = info.join(format!("{candidate}.trashinfo"));
        let text = format!("[Trash Info]\nPath={}\nDeletionDate={}\n", percent_encode(&recorded.to_string_lossy()), deletion_date());
        match fs::OpenOptions::new().write(true).create_new(true).open(&record) {
            Ok(mut f) if files.join(&candidate).symlink_metadata().is_err() => {
                f.write_all(text.as_bytes()).map_err(|e| format!("{}: {e}", record.display()))?;
                if let Err(e) = fs::rename(&path, files.join(&candidate)) {
                    let _ = fs::remove_file(&record);
                    return Err(format!("Moving {name} to the Trash: {e}"));
                }
                return Ok(());
            }
            Ok(_) => {
                let _ = fs::remove_file(&record);
            }
            Err(e) if e.kind() == io::ErrorKind::AlreadyExists => {}
            Err(e) => return Err(format!("{}: {e}", record.display())),
        }
        n += 1;
    }
}

/// The Trash's record of `path`, if it is at the top of a Trash's `files`.
fn trash_info(path: &Path) -> Option<PathBuf> {
    let files = path.parent().filter(|p| p.file_name().is_some_and(|n| n == "files"))?;
    let info = files.parent()?.join("info").join(format!("{}.trashinfo", name_of(path)));
    info.exists().then_some(info)
}

/// `path` (at the top of a Trash's `files`) moved back where its record says it was (under a free name
/// if that is taken); where it went.
fn restore(path: &Path) -> Result<PathBuf, String> {
    let name = name_of(path);
    let info = trash_info(path).ok_or_else(|| format!("{name}: the Trash has no record of where it was"))?;
    let text = fs::read_to_string(&info).map_err(|e| format!("{}: {e}", info.display()))?;
    let recorded = text.lines().find_map(|l| l.strip_prefix("Path=")).map(percent_decode).ok_or_else(|| format!("{}: no Path", info.display()))?;
    let mut to = PathBuf::from(&recorded);
    // (A filesystem's own Trash records paths from its top.)
    if to.is_relative()
        && let Some(top) = info.parent().and_then(Path::parent).and_then(Path::parent)
    {
        to = top.join(to);
    }
    let parent = to.parent().ok_or_else(|| format!("{name}: no folder to go back to"))?.to_path_buf();
    fs::create_dir_all(&parent).map_err(|e| format!("{}: {e}", parent.display()))?;
    let to = free_name(&parent, &name_of(&to));
    fs::rename(path, &to).map_err(|e| format!("Restoring {name}: {e}"))?;
    let _ = fs::remove_file(info);
    Ok(to)
}

/// Now as the Trash records it, local time (`date`'s; UTC without it): 2026-10-18T14:03:00.
fn deletion_date() -> String {
    if let Some(out) = crate::command::output("date", &["+%Y-%m-%dT%H:%M:%S"]) {
        return String::from_utf8_lossy(&out).trim().to_owned();
    }
    let secs = SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_secs());
    let (y, mo, d, h, mi, s) = crate::device::civil(secs);
    format!("{y:04}-{mo:02}-{d:02}T{h:02}:{mi:02}:{s:02}")
}

/// A path for a Trash record: bytes but letters, digits, `/` and `-_.~` as `%XX`.
fn percent_encode(text: &str) -> String {
    text.bytes().map(|b| if b.is_ascii_alphanumeric() || b"/-_.~".contains(&b) { char::from(b).to_string() } else { format!("%{b:02X}") }).collect()
}

fn percent_decode(text: &str) -> String {
    let bytes = text.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while let Some(&b) = bytes.get(i) {
        let hex = bytes.get(i + 1..i + 3).and_then(|h| std::str::from_utf8(h).ok()).and_then(|h| u8::from_str_radix(h, 16).ok());
        match (b, hex) {
            (b'%', Some(h)) => {
                out.push(h);
                i += 3;
            }
            _ => {
                out.push(b);
                i += 1;
            }
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::indexing_slicing)]
mod tests {
    use super::*;
    use std::sync::mpsc;

    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("clamshell-files-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn a_listing_has_folders_first_and_files_open_by_the_sources() {
        let dir = scratch("list");
        fs::create_dir(dir.join("Zelda")).unwrap();
        for f in ["a.SFC", "b.txt", ".hidden", "cover.png"] {
            fs::write(dir.join(f), "x").unwrap();
        }
        fs::write(dir.join("tool.desktop"), "[Desktop Entry]\nType=Application\nName=Tool\nExec=tool --open %U\n").unwrap();
        let config: Vec<Runner> = serde_json::from_str(
            r#"[{ "name": "SNES", "match": ["*.sfc"], "launch": ["retroarch", "{path}"] },
                { "type": "desktop", "name": "Desktop", "match": ["*.desktop"] },
                { "name": "Other files", "match": ["*"], "launch": ["xdg-open", "{path}"] }]"#,
        )
        .unwrap();
        let (runners, _) = runners::checked(&config, &|_| true);
        let games = list(&dir, "browse:Files/Home", &runners, None, false);
        let names: Vec<&str> = games.iter().map(|g| g.title.as_str()).collect();
        assert_eq!(names, ["Zelda", "a.SFC", "b.txt", "cover.png", "tool.desktop"]);
        assert_eq!(&*games[0].id, "browse:Files/Home/Zelda");
        assert!(games[0].is_group());
        let argv = |g: &Game| match &g.launch {
            clamshell_core::Launch::Program { argv, .. } => argv.clone(),
            _ => vec![],
        };
        assert_eq!(argv(&games[1])[0], "retroarch", "by the runner's pattern, any case");
        assert_eq!(games[1].runner.as_ref().map(|r| (&*r.key, &*r.name)), Some(("runner:snes", "SNES")));
        assert_eq!(argv(&games[2])[0], "xdg-open");
        assert_eq!(games[3].box_front.as_deref(), Some(dir.join("cover.png").as_path()), "a picture is its own art");
        assert_eq!(games[2].browsed.as_ref().unwrap().size, Some(1));
        assert_eq!(argv(&games[4]), ["tool", "--open"], "a desktop file: its own command");
        // Pictures as a scan's: a folder's cover in it, a file's beside it, an image itself; else its glyph.
        fs::write(dir.join("Zelda/cover.jpg"), "x").unwrap();
        fs::write(dir.join("b.jpg"), "x").unwrap();
        let games = list(&dir, "browse:Files/Home", &runners, None, false);
        let by = |name: &str| games.iter().find(|g| g.title == name).unwrap();
        assert_eq!(by("Zelda").box_front.as_deref(), Some(dir.join("Zelda/cover.jpg").as_path()));
        assert_eq!(by("b.txt").box_front.as_deref(), Some(dir.join("b.jpg").as_path()), "beside it");
        assert_eq!(by("b.jpg").box_front.as_deref(), Some(dir.join("b.jpg").as_path()), "itself");
        assert_eq!((by("a.SFC").box_front.as_ref(), by("a.SFC").glyph.as_deref()), (None, Some("file")));
        assert_eq!(by("tool.desktop").runner.as_ref().map(|r| &*r.name), Some("Desktop"));
        let _ = fs::remove_dir_all(&dir);
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
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_trash_records_where_things_were_and_restores_them() {
        let dir = scratch("trash");
        let home = dir.join("home");
        fs::create_dir_all(home.join("docs")).unwrap();
        fs::write(home.join("docs/a b.txt"), "x").unwrap();
        // (XDG_DATA_HOME may be set: the home Trash from it, then.)
        let trash_dir = home_trash(Some(&home)).unwrap();
        if !trash_dir.starts_with(&home) {
            return;
        }
        trash(&home.join("docs/a b.txt"), Some(&home)).unwrap();
        assert!(!home.join("docs/a b.txt").exists());
        let record = fs::read_to_string(trash_dir.join("info/a b.txt.trashinfo")).unwrap();
        assert!(record.contains("Path=") && record.contains("a%20b.txt"), "{record}");
        // The same name again: another.
        fs::write(home.join("docs/a b.txt"), "y").unwrap();
        trash(&home.join("docs/a b.txt"), Some(&home)).unwrap();
        assert_eq!(fs::read_dir(trash_dir.join("files")).unwrap().count(), 2);
        let to = restore(&trash_dir.join("files/a b.txt")).unwrap();
        assert_eq!(to, home.join("docs/a b.txt").canonicalize().unwrap());
        assert!(!trash_dir.join("info/a b.txt.trashinfo").exists());
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn escapes_and_encodings() {
        assert_eq!(unescape(r"/run/media/u/My\040Card"), "/run/media/u/My Card");
        assert_eq!(unescape(r"My\x20Card"), "My Card");
        assert_eq!(percent_decode(&percent_encode("/a b/ø%.txt")), "/a b/ø%.txt");
        assert_eq!(name_of(&free_name(Path::new("/nonexistent"), "a.txt")), "a.txt");
    }
}
