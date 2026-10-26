//! Xwayland starts on the first display connection. Socket readiness drives all adapter work.
mod properties;
pub(crate) mod selection;
pub(crate) mod shell;
mod socket;
mod transport;
mod wm;

use crate::server::{ClientState, Server};
use command_fds::CommandFdExt;
use smithay::reexports::{
    calloop::{Interest, LoopHandle, Mode, PostAction, RegistrationToken, generic::Generic},
    wayland_server::{Client, DisplayHandle, backend::DisconnectReason},
};
use std::{
    cell::Cell,
    io::{self, Read},
    os::{
        fd::{AsFd, AsRawFd, OwnedFd},
        unix::net::UnixStream,
    },
    process::{Child, Command, Stdio},
    rc::Rc,
    sync::Arc,
    time::{Duration, Instant},
};
use transport::Transport;

pub(crate) struct Supervisor {
    handle: LoopHandle<'static, Server>,
    socket: socket::Socket,
    listener: Option<RegistrationToken>,
    demand: Rc<Cell<bool>>,
    retry: Option<Instant>,
    running: Option<Running>,
}
struct Running {
    child: Child,
    client: Client,
    ready_socket: UnixStream,
    display_bytes: Vec<u8>,
    display: Option<u32>,
    transport: Transport,
    wm: wm::Wm,
    ready_logged: bool,
    deadline: Instant,
    stopping: bool,
    sources: Vec<RegistrationToken>,
    ready_source: Option<RegistrationToken>,
    write_source: Option<RegistrationToken>,
    selection_sources: Vec<selection::IoSource>,
}
fn watch(handle: &LoopHandle<'static, Server>, fd: &impl AsFd, interest: Interest) -> io::Result<RegistrationToken> {
    let fd = fd.as_fd().try_clone_to_owned()?;
    handle.insert_source(Generic::new(fd, interest, Mode::Level), |_, _, _| Ok(PostAction::Continue)).map_err(|e| io::Error::other(e.error))
}
impl Supervisor {
    pub fn new(handle: LoopHandle<'static, Server>) -> io::Result<Self> {
        let mut supervisor = Self { handle, socket: socket::Socket::reserve()?, listener: None, demand: Rc::new(Cell::new(false)), retry: None, running: None };
        supervisor.arm()?;
        log::info!("compositor: X11 socket ready display=:{} (Xwayland dormant)", supervisor.socket.number);
        Ok(supervisor)
    }
    pub fn display(&self) -> String {
        format!(":{}", self.socket.number)
    }
    fn arm(&mut self) -> io::Result<()> {
        let demand = self.demand.clone();
        let fd = self.socket.listener.as_fd().try_clone_to_owned()?;
        self.listener = Some(
            self.handle
                .insert_source(Generic::new(fd, Interest::READ, Mode::Level), move |_, _, _| {
                    demand.set(true);
                    Ok(PostAction::Disable)
                })
                .map_err(|e| io::Error::other(e.error))?,
        );
        Ok(())
    }
    /// Whether Xwayland runs (it starts on the first X11 client).
    pub fn running(&self) -> bool {
        self.running.is_some()
    }

    pub fn deadline(&self) -> Option<Instant> {
        self.retry
            .into_iter()
            .chain(
                self.running
                    .iter()
                    .filter(|r| !r.stopping)
                    .flat_map(|r| r.transport.deadline().into_iter().chain(r.wm.deadline()).chain(r.display.is_none().then_some(r.deadline))),
            )
            .min()
    }
    pub fn turn(&mut self, server: &mut Server, now: Instant) {
        if let Some(running) = self.running.as_mut() {
            match running.child.try_wait() {
                Ok(Some(status)) => {
                    log::warn!("compositor: Xwayland exited: {status}; awaiting next X11 client");
                    running.remove_sources(&self.handle);
                    running.wm.clear(server);
                    server.dh.backend_handle().kill_client(running.client.id(), DisconnectReason::ConnectionClosed);
                    self.running = None;
                    self.retry = None;
                    if let Err(error) = self.arm() {
                        log::error!("compositor: rearming X11 listener: {error}");
                    }
                }
                Ok(None) if running.stopping => {
                    if running.sources.is_empty() {
                        self.retry = Some(now + Duration::from_secs(1));
                    }
                }
                Ok(None) => {
                    if let Err(error) = running.turn(&self.handle, server, now) {
                        log::warn!("compositor: Xwayland: {error}; stopping");
                        running.wm.clear(server);
                        running.stop(&server.dh);
                        // Keep only pidfd readiness while SIGKILL is being reaped.
                        running.remove_io_sources(&self.handle);
                    }
                }
                Err(error) => {
                    log::warn!("compositor: checking Xwayland: {error}");
                    running.stop(&server.dh);
                }
            }
            return;
        }
        server.x11_close.clear();
        server.x11_popup_dismiss.clear();
        if !self.demand.get() || self.retry.is_some_and(|deadline| now < deadline) {
            return;
        }
        self.retry = None;
        if let Some(token) = self.listener.take() {
            self.handle.remove(token);
        }
        match Running::spawn(&mut server.dh, &self.socket, now) {
            Ok(mut running) => {
                if let Err(error) = running.register(&self.handle) {
                    log::warn!("compositor: registering Xwayland: {error}");
                    running.stop(&server.dh);
                    // If pidfd registration itself failed, keep a deadline only until this child is reaped.
                    if running.sources.is_empty() {
                        self.retry = Some(now + Duration::from_secs(1));
                    }
                }
                self.running = Some(running);
                self.demand.set(false);
            }
            Err(error) => {
                log::warn!("compositor: starting Xwayland: {error}; retrying");
                self.retry = Some(now + Duration::from_secs(1));
            }
        }
    }
}
impl Running {
    fn spawn(dh: &mut DisplayHandle, socket: &socket::Socket, now: Instant) -> io::Result<Self> {
        let (wayland, wayland_child) = UnixStream::pair()?;
        let (wm, wm_child) = UnixStream::pair()?;
        let (ready_socket, ready_child) = UnixStream::pair()?;
        let listener = socket.listener.as_fd().try_clone_to_owned()?;
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
            &format!(":{}", socket.number),
            "-rootless",
            "-terminate",
            "-nolisten",
            "tcp",
            "-listenfd",
            &listener.as_raw_fd().to_string(),
            "-displayfd",
            &ready_child.as_raw_fd().to_string(),
            "-wm",
            &wm_child.as_raw_fd().to_string(),
        ]);
        command.stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::inherit());
        let transport = Transport::new(wm, now)?;
        let client = dh.insert_client(wayland, Arc::new(ClientState { xwayland: true, ..ClientState::default() }))?;
        let descriptors: Vec<OwnedFd> = vec![wayland_child.into(), wm_child.into(), ready_child.into(), listener];
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
            wm: wm::Wm::default(),
            ready_logged: false,
            deadline: now + Duration::from_secs(5),
            stopping: false,
            sources: Vec::new(),
            ready_source: None,
            write_source: None,
            selection_sources: Vec::new(),
        })
    }
    fn register(&mut self, handle: &LoopHandle<'static, Server>) -> io::Result<()> {
        let pid = i32::try_from(self.child.id())
            .ok()
            .filter(|pid| *pid > 0)
            .and_then(rustix::process::Pid::from_raw)
            .ok_or_else(|| io::Error::other("invalid child pid"))?;
        let pidfd = rustix::process::pidfd_open(pid, rustix::process::PidfdFlags::empty())?;
        self.sources.push(watch(handle, &pidfd, Interest::READ)?);
        self.sources.push(watch(handle, self.transport.fd(), Interest::READ)?);
        self.ready_source = Some(watch(handle, &self.ready_socket, Interest::READ)?);
        self.update_writing(handle)
    }
    fn update_writing(&mut self, handle: &LoopHandle<'static, Server>) -> io::Result<()> {
        if self.transport.writing() && self.write_source.is_none() {
            self.write_source = Some(watch(handle, self.transport.fd(), Interest::WRITE)?);
        } else if !self.transport.writing()
            && let Some(token) = self.write_source.take()
        {
            handle.remove(token);
        }
        Ok(())
    }
    fn turn(&mut self, handle: &LoopHandle<'static, Server>, server: &mut Server, now: Instant) -> io::Result<()> {
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
                if let Some(token) = self.ready_source.take() {
                    handle.remove(token);
                }
            } else if self.display_bytes.len() > 16 {
                return Err(io::Error::other("display number exceeds limit"));
            }
        }
        self.transport.turn(now)?;
        self.wm.turn(&mut self.transport, server, now)?;
        self.wm.sync_selection_sources(handle, &mut self.selection_sources)?;
        self.update_writing(handle)?;
        if self.wm.ready && self.display.is_some() && !self.ready_logged {
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
    fn remove_io_sources(&mut self, handle: &LoopHandle<'static, Server>) {
        selection::Bridge::remove_sources(handle, &mut self.selection_sources);
        if let Some(token) = self.ready_source.take() {
            handle.remove(token);
        }
        if let Some(token) = self.write_source.take() {
            handle.remove(token);
        }
        if self.sources.len() > 1
            && let Some(token) = self.sources.pop()
        {
            handle.remove(token);
        }
    }
    fn remove_sources(&mut self, handle: &LoopHandle<'static, Server>) {
        self.remove_io_sources(handle);
        for token in self.sources.drain(..) {
            handle.remove(token);
        }
    }
}
impl Drop for Supervisor {
    fn drop(&mut self) {
        if let Some(token) = self.listener.take() {
            self.handle.remove(token);
        }
        if let Some(mut running) = self.running.take() {
            running.remove_sources(&self.handle);
            let _ = running.child.kill();
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
