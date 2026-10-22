//! What ClamShell tells systemd when it runs as its service (`Type=notify`, `WatchdogSec=`). It says it
//! is up once its screens are drawn, and every few seconds that its main loop is still turning.
//!
//! If the loop stops, through a deadlock or a call that never returns, systemd restarts ClamShell after
//! the watchdog's time. What it launched runs on in its own scopes, and the restarted ClamShell finds it.
//! Outside systemd, as in a desktop's nested session, there is no socket and nothing is sent.

use std::os::unix::net::UnixDatagram;
use std::time::{Duration, Instant};

/// How often the loop reports that it is turning. This is well within any watchdog time; the service
/// gives 30 s.
const EVERY: Duration = Duration::from_secs(5);

pub struct Notifier {
    socket: Option<(UnixDatagram, std::path::PathBuf)>,
    ready: bool,
    last: Option<Instant>,
}

impl Notifier {
    /// systemd's socket, if ClamShell runs as a notifying service. `NOTIFY_SOCKET` must be a path. An
    /// abstract socket (`@…`) isn't the user manager's and isn't used.
    pub fn new() -> Notifier {
        let path = std::env::var_os("NOTIFY_SOCKET").map(std::path::PathBuf::from).filter(|p| p.is_absolute());
        let socket = path.and_then(|p| UnixDatagram::unbound().inspect_err(|e| log::warn!("notify {}: {e}", p.display())).ok().map(|s| (s, p)));
        Notifier { socket, ready: false, last: None }
    }

    /// Called after each turn of the main loop. Reports that the program is up the first time, and that
    /// it is alive at most every few seconds.
    pub fn turned(&mut self) {
        if !std::mem::replace(&mut self.ready, true) {
            self.send("READY=1");
        }
        if self.last.is_none_or(|t| t.elapsed() >= EVERY) {
            self.last = Some(Instant::now());
            self.send("WATCHDOG=1");
        }
    }

    fn send(&self, state: &str) {
        if let Some((socket, path)) = &self.socket
            && let Err(e) = socket.send_to(state.as_bytes(), path)
        {
            log::warn!("notify {state}: {e}");
        }
    }
}
