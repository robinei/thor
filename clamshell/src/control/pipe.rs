//! The control pipe itself: writing a command to it (`clamshell ctl`) and the thread that reads them.

use std::io::{BufRead, Read};
use std::path::{Path, PathBuf};

use clamshell_system::Sink;
use clamshell_system::os::worker;

use super::parse;

/// Sends `line` to the running ClamShell's control pipe (`clamshell ctl`). If ClamShell isn't reading the
/// pipe, this fails instead of waiting.
pub fn send(path: Option<&PathBuf>, line: &str) -> std::io::Result<()> {
    use std::io::Write;
    use std::os::unix::fs::OpenOptionsExt;
    let path = path.ok_or_else(|| std::io::Error::other("no control pipe (no XDG_RUNTIME_DIR)"))?;
    // Opened non-blocking so that, if nobody reads the pipe, it fails at once (ENXIO) instead of waiting.
    let mut pipe = std::fs::OpenOptions::new()
        .write(true)
        .custom_flags(rustix::fs::OFlags::NONBLOCK.bits() as i32)
        .open(path)
        .map_err(|e| std::io::Error::new(e.kind(), format!("{}: {e} (is ClamShell running?)", path.display())))?;
    writeln!(pipe, "{line}")
}

/// How long to wait before trying a pipe that couldn't be opened again.
const RETRY: std::time::Duration = std::time::Duration::from_secs(5);

/// Creates the pipe at `path` if it doesn't exist. Something else there is replaced: read as the pipe,
/// a file would end at once, every time it is opened again. Whether `path` is a pipe now.
fn create(path: &Path) -> bool {
    use std::os::unix::fs::FileTypeExt;
    match std::fs::symlink_metadata(path) {
        Ok(meta) if meta.file_type().is_fifo() => return true,
        Ok(_) => {
            log::warn!("control: {} isn't a pipe; replacing it with one", path.display());
            if let Err(e) = std::fs::remove_file(path) {
                log::warn!("control: {}: {e}", path.display());
                return false;
            }
        }
        Err(_) => {}
    }
    match rustix::fs::mknodat(rustix::fs::CWD, path, rustix::fs::FileType::Fifo, rustix::fs::Mode::from_raw_mode(0o600), 0) {
        Ok(()) => true,
        Err(e) => {
            log::warn!("control: {}: {e}", path.display());
            false
        }
    }
}

/// The longest command line taken. A longer one is skipped, up to its end, with a warning.
const LONGEST: usize = 4096;

/// How often the reader looks whether the pipe is still there, while nothing is sent.
const LOOK_EVERY: std::time::Duration = std::time::Duration::from_secs(5);

/// Reads commands from the pipe at `path`, one per line, and sends their events. The pipe is created if it
/// doesn't exist, and created again if it is deleted or replaced. The thread is started again after a
/// panic.
pub fn spawn(path: PathBuf, sink: impl Sink) {
    spawn_looking(path, sink, LOOK_EVERY);
}

/// [`spawn`], looking whether the pipe is still there every `every`.
fn spawn_looking(path: PathBuf, sink: impl Sink, every: std::time::Duration) {
    // The pipe is opened for reading and writing, once. With a writer always there, itself, it never
    // reads as ended between two writers, so a line sent while it would have been opened again can't
    // fail; and `send` still fails at once while ClamShell isn't running, since nobody holds it open.
    let spawned = worker::lasting("control", move || {
        loop {
            if !create(&path) {
                std::thread::sleep(RETRY);
                continue;
            }
            let file = match std::fs::OpenOptions::new().read(true).write(true).open(&path) {
                Ok(file) => file,
                Err(e) => {
                    log::warn!("control: {}: {e}; trying again", path.display());
                    std::thread::sleep(RETRY);
                    continue;
                }
            };
            let Some(watched) = Watched::new(file, &path, every) else {
                std::thread::sleep(RETRY);
                continue;
            };
            if read(std::io::BufReader::new(watched), &path, &sink).is_err() {
                return;
            }
        }
    });
    if let Err(e) = spawned {
        log::warn!("control: {e}");
    }
}

/// The pipe opened, read until the pipe at its path is another or gone, which reads as its end.
struct Watched<'a> {
    file: std::fs::File,
    path: &'a Path,
    inode: (u64, u64),
    every: std::time::Duration,
}

impl<'a> Watched<'a> {
    fn new(file: std::fs::File, path: &'a Path, every: std::time::Duration) -> Option<Watched<'a>> {
        use std::os::unix::fs::MetadataExt;
        let meta = file.metadata().inspect_err(|e| log::warn!("control: {}: {e}; trying again", path.display())).ok()?;
        Some(Watched { file, path, inode: (meta.dev(), meta.ino()), every })
    }

    /// Whether the pipe at the path is still the one opened.
    fn there(&self) -> bool {
        use std::os::unix::fs::MetadataExt;
        std::fs::symlink_metadata(self.path).is_ok_and(|m| (m.dev(), m.ino()) == self.inode)
    }
}

impl Read for Watched<'_> {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        use rustix::event::{PollFd, PollFlags, Timespec};
        let every = Timespec { tv_sec: i64::try_from(self.every.as_secs()).unwrap_or(i64::MAX), tv_nsec: i64::from(self.every.subsec_nanos()) };
        loop {
            let mut fds = [PollFd::new(&self.file, PollFlags::IN)];
            match rustix::event::poll(&mut fds, Some(&every)) {
                Ok(0) if !self.there() => {
                    log::warn!("control: {} was removed or replaced: made again", self.path.display());
                    return Ok(0);
                }
                Ok(0) | Err(rustix::io::Errno::INTR) => {}
                Ok(_) => return self.file.read(buf),
                Err(e) => return Err(e.into()),
            }
        }
    }
}

/// Reads command lines from `from` until it ends, and sends their events. A line longer than [`LONGEST`]
/// is skipped. `Err` when the UI is gone.
fn read(mut from: impl BufRead, path: &Path, sink: &impl Sink) -> Result<(), clamshell_system::Closed> {
    let mut line = Vec::new();
    loop {
        line.clear();
        match (&mut from).take(LONGEST as u64 + 1).read_until(b'\n', &mut line) {
            Ok(0) => return Ok(()),
            Ok(_) => {}
            Err(e) => {
                log::warn!("control: reading {}: {e}", path.display());
                return Ok(());
            }
        }
        if line.last() != Some(&b'\n') && line.len() > LONGEST {
            log::warn!("control: a line longer than {LONGEST} bytes: skipped");
            skip_line(&mut from);
            continue;
        }
        let text = String::from_utf8_lossy(&line);
        let text = text.trim_end_matches(['\n', '\r']);
        match parse(text) {
            Ok(events) => events.into_iter().try_for_each(|e| sink.send(e))?,
            Err(e) => log::warn!("control: {text:?}: {e}"),
        }
    }
}

/// Reads past the rest of the line, without keeping it.
fn skip_line(from: &mut impl BufRead) {
    loop {
        let (used, ended) = match from.fill_buf() {
            Ok([]) | Err(_) => return,
            Ok(buf) => match buf.iter().position(|b| *b == b'\n') {
                Some(at) => (at + 1, true),
                None => (buf.len(), false),
            },
        };
        from.consume(used);
        if ended {
            return;
        }
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    /// Lines are taken one by one; one too long is skipped to its end, and the next is taken.
    #[test]
    fn a_line_too_long_is_skipped() {
        let long = "x".repeat(LONGEST * 3);
        let input = format!("home\n{long}\nhome\n");
        let (sink, events) = std::sync::mpsc::channel();
        read(std::io::BufReader::with_capacity(64, input.as_bytes()), Path::new("pipe"), &sink).unwrap();
        assert_eq!(events.try_iter().count(), 2 * parse("home").unwrap().len());
    }

    /// Waits until a line sent to the pipe at `path` is taken, the reader having opened it.
    fn until_read(path: &Path) {
        let start = std::time::Instant::now();
        while send(Some(&path.to_path_buf()), "home").is_err() {
            assert!(start.elapsed() < std::time::Duration::from_secs(5), "the reader opened the pipe");
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
    }

    /// Lines sent one right after another all go through: none is sent while the reader has the pipe
    /// closed between two writers.
    #[test]
    fn lines_sent_back_to_back_all_go_through() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("control");
        let (sink, events) = std::sync::mpsc::channel();
        spawn_looking(path.clone(), sink, std::time::Duration::from_millis(100));
        until_read(&path);
        let lines = 2000;
        let failed = (0..lines).filter(|_| send(Some(&path), "home").is_err()).count();
        assert_eq!(failed, 0);
        let each = parse("home").unwrap().len();
        let got = (0..(lines + 1) * each).filter(|_| events.recv_timeout(std::time::Duration::from_secs(5)).is_ok()).count();
        assert_eq!(got, (lines + 1) * each);
    }

    /// A pipe removed is made again, and read.
    #[test]
    fn a_pipe_removed_is_made_again() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("control");
        let (sink, events) = std::sync::mpsc::channel();
        spawn_looking(path.clone(), sink, std::time::Duration::from_millis(50));
        until_read(&path);
        events.recv_timeout(std::time::Duration::from_secs(5)).unwrap();
        std::fs::remove_file(&path).unwrap();
        until_read(&path);
        events.recv_timeout(std::time::Duration::from_secs(5)).unwrap();
    }

    /// A file where the pipe goes is replaced with the pipe, rather than read to its end over and over.
    #[test]
    fn a_file_in_the_pipes_place_is_replaced() {
        use std::os::unix::fs::FileTypeExt;
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("control");
        std::fs::write(&path, "home\n").unwrap();
        assert!(create(&path));
        assert!(std::fs::symlink_metadata(&path).unwrap().file_type().is_fifo());
        assert!(create(&path), "and kept");
    }
}
