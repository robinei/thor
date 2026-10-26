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

/// Creates the pipe at `path` if it doesn't exist.
fn create(path: &Path) {
    if !path.exists()
        && let Err(e) = rustix::fs::mknodat(rustix::fs::CWD, path, rustix::fs::FileType::Fifo, rustix::fs::Mode::from_raw_mode(0o600), 0)
    {
        log::warn!("control: {}: {e}", path.display());
    }
}

/// The longest command line taken. A longer one is skipped, up to its end, with a warning.
const LONGEST: usize = 4096;

/// Reads commands from the pipe at `path`, one per line, and sends their events. The pipe is created if it
/// doesn't exist, and created again if it is deleted. The thread is started again after a panic.
pub fn spawn(path: PathBuf, sink: impl Sink) {
    create(&path);
    // Each writer closes the pipe after its line, so it is opened again for the next one.
    let spawned = worker::lasting("control", move || {
        loop {
            let file = match std::fs::File::open(&path) {
                Ok(file) => file,
                Err(e) => {
                    log::warn!("control: {}: {e}; trying again", path.display());
                    std::thread::sleep(RETRY);
                    create(&path);
                    continue;
                }
            };
            if read(std::io::BufReader::new(file), &path, &sink).is_err() {
                return;
            }
        }
    });
    if let Err(e) = spawned {
        log::warn!("control: {e}");
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
}
