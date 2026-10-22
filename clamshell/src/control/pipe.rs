//! The control pipe itself: writing a command to it (`clamshell ctl`) and the thread that reads them.

use std::io::BufRead;
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

/// Reads commands from the pipe at `path`, one per line, and sends their events. The pipe is created if it
/// doesn't exist, and created again if it is deleted.
pub fn spawn(path: PathBuf, sink: impl Sink) {
    create(&path);
    let spawned = worker::detached("control", move || {
        // Each writer closes the pipe after its line, so it is opened again for the next one.
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
            for line in std::io::BufReader::new(file).lines().map_while(|l| l.inspect_err(|e| log::warn!("control: reading {}: {e}", path.display())).ok()) {
                match parse(&line) {
                    Ok(events) => {
                        if events.into_iter().try_for_each(|e| sink.send(e)).is_err() {
                            return;
                        }
                    }
                    Err(e) => log::warn!("control: {line:?}: {e}"),
                }
            }
        }
    });
    if let Err(e) = spawned {
        log::warn!("control: {e}");
    }
}
