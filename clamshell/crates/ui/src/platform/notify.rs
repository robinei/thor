//! What ClamShell tells systemd when it runs as its service (`Type=notify`, `WatchdogSec=`): that it is
//! up (its screens drawn once), and every few seconds that its main loop still turns. A loop that stops
//! (a deadlock, a call that never returns) is restarted by systemd after the watchdog's time; what it
//! launched runs on in its own scopes, and the ClamShell started again finds it. Outside systemd (a
//! desktop's nested session) there is no socket: nothing is sent.

use std::os::unix::net::UnixDatagram;
use std::time::{Duration, Instant};

/// How often the loop says it turns: well within any watchdog time (the service gives 30 s).
const EVERY: Duration = Duration::from_secs(5);

pub struct Notifier {
    socket: Option<(UnixDatagram, std::path::PathBuf)>,
    ready: bool,
    last: Option<Instant>,
}

impl Notifier {
    /// systemd's socket, if ClamShell runs as a notifying service (`NOTIFY_SOCKET`: a path; an abstract
    /// one, `@…`, isn't the user manager's and isn't used).
    pub fn new() -> Notifier {
        let path = std::env::var_os("NOTIFY_SOCKET").map(std::path::PathBuf::from).filter(|p| p.is_absolute());
        let socket = path.and_then(|p| UnixDatagram::unbound().inspect_err(|e| log::warn!("notify {}: {e}", p.display())).ok().map(|s| (s, p)));
        Notifier { socket, ready: false, last: None }
    }

    /// After a turn of the main loop: up (once), and alive (at most every few seconds).
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
