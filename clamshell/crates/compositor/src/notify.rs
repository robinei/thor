//! What ClamShell tells systemd when it runs as a service (`Type=notify`, `WatchdogSec=`): that it is up
//! once its loop has turned once, with its socket listening and the system threads started; every few
//! seconds that the loop is still turning; and a status line for `systemctl status`: what runs.
//!
//! If the loop stops, through a deadlock or a call that never returns, systemd restarts ClamShell after
//! the watchdog's time. What it launched runs on in its own scopes, and the restarted ClamShell finds it.
//! Outside systemd there is no socket and nothing is sent.

use std::os::unix::net::UnixDatagram;
use std::time::{Duration, Instant};

/// How often the loop reports that it is turning. This is well within any watchdog time; the service
/// gives 30 s.
const EVERY: Duration = Duration::from_secs(5);

pub struct Notifier {
    socket: Option<(UnixDatagram, std::path::PathBuf)>,
    ready: bool,
    last: Option<Instant>,
    /// The status last sent: running entries, windows, whether Xwayland runs.
    status: Option<(usize, usize, bool)>,
}

impl Notifier {
    /// systemd's socket, if ClamShell runs as a notifying service. `NOTIFY_SOCKET` must be a path. An
    /// abstract socket (`@…`) isn't the user manager's and isn't used.
    pub fn new() -> Notifier {
        let path = std::env::var_os("NOTIFY_SOCKET").map(std::path::PathBuf::from).filter(|p| p.is_absolute());
        let socket = path.and_then(|p| UnixDatagram::unbound().inspect_err(|e| log::warn!("notify {}: {e}", p.display())).ok().map(|s| (s, p)));
        Notifier { socket, ready: false, last: None, status: None }
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

    /// What `systemctl status` shows under the unit, sent when it changes: the running entries, the
    /// windows, and whether Xwayland runs.
    pub fn status(&mut self, running: usize, windows: usize, xwayland: bool) {
        if self.socket.is_none() || self.status.replace((running, windows, xwayland)) == Some((running, windows, xwayland)) {
            return;
        }
        let xwayland = if xwayland { "Xwayland running" } else { "Xwayland not started" };
        self.send(&format!("STATUS={running} running, {windows} windows, {xwayland}"));
    }

    /// When the loop is next to turn at the latest, for the next report: never outside systemd.
    pub fn deadline(&self) -> Option<Instant> {
        self.socket.as_ref()?;
        Some(self.last.map_or_else(Instant::now, |t| t + EVERY))
    }

    fn send(&self, state: &str) {
        if let Some((socket, path)) = &self.socket
            && let Err(e) = socket.send_to(state.as_bytes(), path)
        {
            log::warn!("notify {state}: {e}");
        }
    }
}
