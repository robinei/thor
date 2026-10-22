//! The pipe itself: commands written to it (`clamshell ctl`), and the thread reading them.

use std::io::BufRead;
use std::path::PathBuf;

use clamshell_system::Sink;
use clamshell_system::os::worker;

use super::parse;

/// Reads commands from the pipe, one per line, for as long as the program runs.
/// Sends `line` to the running ClamShell's control pipe (`clamshell ctl`). Without ClamShell reading it,
/// an error rather than waiting.
pub fn send(path: Option<&PathBuf>, line: &str) -> std::io::Result<()> {
    use std::io::Write;
    use std::os::unix::fs::OpenOptionsExt;
    let path = path.ok_or_else(|| std::io::Error::other("no control pipe (no XDG_RUNTIME_DIR)"))?;
    // Non-blocking: opening a pipe nobody reads fails at once (ENXIO) instead of waiting for a reader.
    let mut pipe = std::fs::OpenOptions::new()
        .write(true)
        .custom_flags(rustix::fs::OFlags::NONBLOCK.bits() as i32)
        .open(path)
        .map_err(|e| std::io::Error::new(e.kind(), format!("{}: {e} (is ClamShell running?)", path.display())))?;
    writeln!(pipe, "{line}")
}

/// Reads commands from the pipe at `path` (made if it isn't there) and sends their events.
pub fn spawn(path: PathBuf, sink: impl Sink) {
    if !path.exists()
        && let Err(e) = rustix::fs::mknodat(rustix::fs::CWD, &path, rustix::fs::FileType::Fifo, rustix::fs::Mode::from_raw_mode(0o600), 0)
    {
        log::warn!("control: {}: {e}", path.display());
    }
    let spawned = worker::detached("control", move || {
        // Each writer closes the pipe after its line: open it again for the next.
        loop {
            let file = match std::fs::File::open(&path) {
                Ok(file) => file,
                Err(e) => {
                    log::warn!("control: {}: {e}", path.display());
                    return;
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
