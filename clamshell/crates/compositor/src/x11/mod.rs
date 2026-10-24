//! Xwayland is a disposable child. All X11 I/O is non-blocking and belongs to this adapter.

mod transport;

use crate::server::ClientState;
use command_fds::CommandFdExt;
use smithay::reexports::wayland_server::{Client, DisplayHandle, backend::DisconnectReason};
use std::{
    io::{self, Read},
    os::{
        fd::{AsRawFd, OwnedFd},
        unix::net::UnixStream,
    },
    process::{Child, Command, Stdio},
    sync::Arc,
    time::{Duration, Instant},
};
use transport::Transport;

const RETRY: Duration = Duration::from_secs(1);
const POLL: Duration = Duration::from_millis(20);

pub(crate) struct Supervisor {
    running: Option<Running>,
    next: Instant,
}
struct Running {
    child: Child,
    client: Client,
    ready_socket: UnixStream,
    display_bytes: Vec<u8>,
    display: Option<u32>,
    transport: Transport,
    ready_logged: bool,
    deadline: Instant,
    stopping: bool,
}

impl Supervisor {
    pub fn new(now: Instant) -> Self {
        Self { running: None, next: now }
    }
    pub fn deadline(&self) -> Instant {
        self.next
    }

    pub fn turn(&mut self, dh: &mut DisplayHandle, now: Instant) {
        if now < self.next {
            return;
        }
        self.next = now + POLL;
        if let Some(running) = self.running.as_mut() {
            match running.child.try_wait() {
                Ok(Some(status)) => {
                    log::warn!("compositor: Xwayland exited: {status}; restarting");
                    dh.backend_handle().kill_client(running.client.id(), DisconnectReason::ConnectionClosed);
                    self.running = None;
                    self.next = now + RETRY;
                }
                Ok(None) if running.stopping => {}
                Ok(None) => {
                    if let Err(error) = running.turn(now) {
                        log::warn!("compositor: Xwayland: {error}; restarting");
                        running.stop(dh);
                    }
                }
                Err(error) => {
                    log::warn!("compositor: checking Xwayland: {error}; stopping");
                    running.stop(dh);
                }
            }
            return;
        }
        match Running::spawn(dh, now) {
            Ok(running) => self.running = Some(running),
            Err(error) => {
                log::warn!("compositor: starting Xwayland: {error}; retrying");
                self.next = now + RETRY;
            }
        }
    }
}

impl Running {
    fn spawn(dh: &mut DisplayHandle, now: Instant) -> io::Result<Self> {
        let (wayland, wayland_child) = UnixStream::pair()?;
        let (wm, wm_child) = UnixStream::pair()?;
        let (ready_socket, ready_child) = UnixStream::pair()?;
        ready_socket.set_nonblocking(true)?;
        let mut command = Command::new("Xwayland");
        command.env_clear();
        for key in ["PATH", "XDG_RUNTIME_DIR"] {
            if let Some(value) = std::env::var_os(key) {
                command.env(key, value);
            }
        }
        command.env("WAYLAND_SOCKET", wayland_child.as_raw_fd().to_string());
        command.args([
            "-rootless",
            "-terminate",
            "-nolisten",
            "tcp",
            "-displayfd",
            &ready_child.as_raw_fd().to_string(),
            "-wm",
            &wm_child.as_raw_fd().to_string(),
        ]);
        command.stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::inherit());
        let transport = Transport::new(wm, now)?;
        let client = dh.insert_client(wayland, Arc::new(ClientState::default()))?;
        let descriptors: Vec<OwnedFd> = vec![wayland_child.into(), wm_child.into(), ready_child.into()];
        let child = match command.preserved_fds(descriptors).spawn() {
            Ok(child) => child,
            Err(error) => {
                dh.backend_handle().kill_client(client.id(), DisconnectReason::ConnectionClosed);
                return Err(error);
            }
        };
        log::info!("compositor: Xwayland started pid={}", child.id());
        Ok(Self {
            child,
            client,
            ready_socket,
            display_bytes: Vec::new(),
            display: None,
            transport,
            ready_logged: false,
            deadline: now + Duration::from_secs(5),
            stopping: false,
        })
    }

    fn turn(&mut self, now: Instant) -> io::Result<()> {
        if self.display.is_none() {
            if now >= self.deadline {
                return Err(io::Error::new(io::ErrorKind::TimedOut, "startup timed out"));
            }
            let mut buffer = [0u8; 32];
            match self.ready_socket.read(&mut buffer) {
                Ok(0) => return Err(io::Error::new(io::ErrorKind::UnexpectedEof, "displayfd closed before readiness")),
                Ok(n) => self.display_bytes.extend(buffer.get(..n).ok_or_else(|| io::Error::other("invalid displayfd read"))?),
                Err(error) if matches!(error.kind(), io::ErrorKind::WouldBlock | io::ErrorKind::Interrupted) => {}
                Err(error) => return Err(error),
            }
            if self.display_bytes.contains(&b'\n') {
                self.display = Some(
                    std::str::from_utf8(&self.display_bytes)
                        .ok()
                        .and_then(|s| s.trim().parse().ok())
                        .ok_or_else(|| io::Error::other("invalid display number"))?,
                );
            } else if self.display_bytes.len() > 16 {
                return Err(io::Error::other("display number exceeds limit"));
            }
        }
        self.transport.turn(now)?;
        if self.transport.ready && self.display.is_some() && !self.ready_logged {
            log::info!("compositor: Xwayland X11 transport ready display=:{} pid={}", self.display.unwrap_or_default(), self.child.id());
            self.ready_logged = true;
        }
        Ok(())
    }

    fn stop(&mut self, dh: &DisplayHandle) {
        self.stopping = true;
        dh.backend_handle().kill_client(self.client.id(), DisconnectReason::ConnectionClosed);
        if let Err(error) = self.child.kill() {
            log::debug!("compositor: killing Xwayland: {error}");
        }
    }
}
impl Drop for Supervisor {
    fn drop(&mut self) {
        if let Some(mut running) = self.running.take() {
            let _ = running.child.kill();
            // Child exit is reaped away from the compositor loop, including a stopped Xwayland.
            if let Err(error) = std::thread::Builder::new().name("xwayland-reap".into()).spawn(move || {
                let _ = running.child.wait();
            }) {
                log::warn!("compositor: starting Xwayland reaper: {error}");
            }
        }
    }
}

#[cfg(test)]
mod tests;
