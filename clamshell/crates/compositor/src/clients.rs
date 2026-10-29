//! Clients as they connect: who they are and their protocol state. A client is admitted once its
//! process is identified, off the compositor's thread ([`Identities`]), because who it is decides what it
//! sees from its first request: a game's client sees only the top screen, and is told its game's mode.

use std::os::unix::net::UnixStream;
use std::sync::{Arc, mpsc};

use smithay::reexports::wayland_server::{
    DisplayHandle,
    backend::{ClientData, ClientId, DisconnectReason},
};
use smithay::wayland::compositor::CompositorClientState;

use crate::server::Server;

/// Identity read once from the client's credentials by the system-side resolver.
#[derive(Default, Clone, Debug, PartialEq)]
pub struct Process {
    pub unit: Option<String>,
    pub pids: Vec<u32>,
    /// The game its launcher marked it with, if one did (see `clamshell_state::session::launchers`).
    pub game: Option<String>,
}

#[derive(Default)]
pub(crate) struct ClientState {
    pub compositor: CompositorClientState,
    /// Its process, identified before it was admitted (see [`Server::admit`]); none for Xwayland and a
    /// client whose process is unknown.
    pub process: Option<Process>,
    /// For a game's process, its resolution when it connected and whether it shows a window on the second
    /// screen (see [`Server::admit`]).
    pub game: Option<(clamshell_state::settings::Resolution, bool)>,
    /// Xwayland, the only client that sees `xwayland_shell_v1`.
    pub xwayland: bool,
}

impl ClientState {
    /// Whether the client sees only the top screen: a game's, unless it shows a window on the second screen
    /// as a dual-screen emulator does.
    pub(crate) fn top_only(client: &smithay::reexports::wayland_server::Client) -> bool {
        client.get_data::<ClientState>().and_then(|state| state.game).is_some_and(|(_, dual)| !dual)
    }
}
impl ClientData for ClientState {
    fn initialized(&self, _: ClientId) {}
    fn disconnected(&self, id: ClientId, reason: DisconnectReason) {
        match reason {
            DisconnectReason::ProtocolError(error) => log::warn!("compositor: client {id:?} disconnected for protocol error: {error:?}"),
            reason => log::debug!("compositor: client {id:?} disconnected: {reason:?}"),
        }
    }
}

pub(crate) fn insert_client(dh: &mut DisplayHandle, stream: UnixStream, state: ClientState) -> Option<smithay::reexports::wayland_server::Client> {
    dh.insert_client(stream, Arc::new(state)).inspect_err(|error| log::warn!("compositor: accept client: {error}")).ok()
}

/// What waits for a process to be identified.
pub(crate) enum Waiting {
    /// A client that connected, not yet admitted.
    Client(UnixStream),
    /// An X11 window, by its id, not yet opened (see `x11::wm`).
    X11Window(i64),
}

/// Processes identified off the compositor's thread: reading `/proc/<pid>/environ` takes the process's
/// mmap lock, which a process stuck in a page fault (on a FUSE archive, say) can hold for long. Each
/// process is read on a short-lived thread of its own, so one stuck process holds up only what waits
/// for it, and the answers come back to [`Server::turn`]. What asks for a process already being read
/// joins that read, so the threads are bounded by the processes being read at once: a stuck process
/// costs one, however often its program connects or maps windows meanwhile.
pub(crate) struct Identities {
    identify: Arc<dyn Fn(u32) -> Process + Send + Sync>,
    /// Wakes the event loop for an answer.
    wake: Arc<dyn Fn() + Send + Sync>,
    /// Each answer, by the token it was asked with.
    send: mpsc::Sender<(u64, Process)>,
    answers: mpsc::Receiver<(u64, Process)>,
    /// The reads under way, each with what waits for it.
    asks: Vec<Ask>,
    /// What waited for a read answered, with the answer, not yet handed out.
    answered: Vec<(Waiting, Process)>,
    next: u64,
}

/// A read of process `pid` under way, asked with `token`, and what waits for it.
struct Ask {
    pid: u32,
    token: u64,
    waiting: Vec<Waiting>,
}

impl Identities {
    pub(crate) fn new(identify: impl Fn(u32) -> Process + Send + Sync + 'static, wake: Arc<dyn Fn() + Send + Sync>) -> Identities {
        let (send, answers) = mpsc::channel();
        Identities { identify: Arc::new(identify), wake, send, answers, asks: Vec::new(), answered: Vec::new(), next: 0 }
    }

    /// Identifies process `pid` for `waiting`, which [`Identities::answered`] gives back with the answer:
    /// joining the read of it under way, if there is one, else starting one.
    pub(crate) fn ask(&mut self, pid: u32, waiting: Waiting) {
        if let Some(ask) = self.asks.iter_mut().find(|ask| ask.pid == pid) {
            ask.waiting.push(waiting);
            return;
        }
        let token = self.next;
        self.next = self.next.wrapping_add(1);
        self.asks.push(Ask { pid, token, waiting: vec![waiting] });
        let (identify, sender, wake) = (self.identify.clone(), self.send.clone(), self.wake.clone());
        // At the ordinary priority, not the compositor thread's raised one (see `clamshell_ui::threads`).
        let spawned = clamshell_ui::threads::start("identify", move || {
            if sender.send((token, identify(pid))).is_ok() {
                wake();
            }
        });
        if let Err(e) = spawned {
            log::warn!("compositor: process {pid} not identified, so taken as no game's: {e}");
            let _ = self.send.send((token, Process::default()));
        }
    }

    /// Drops the clients waiting whose other end hung up: there is nothing left to admit. The read of their
    /// process goes on (a stuck one can't be ended), but what it holds is the thread alone.
    pub(crate) fn drop_hung_up(&mut self) {
        for ask in &mut self.asks {
            ask.waiting.retain(|waiting| {
                let Waiting::Client(stream) = waiting else { return true };
                let mut fds = [rustix::event::PollFd::new(stream, rustix::event::PollFlags::empty())];
                match rustix::event::poll(&mut fds, Some(&rustix::event::Timespec { tv_sec: 0, tv_nsec: 0 })) {
                    Ok(_) if fds[0].revents().intersects(rustix::event::PollFlags::HUP | rustix::event::PollFlags::ERR) => {
                        log::debug!("compositor: a client of process {} hung up while it was identified: dropped", ask.pid);
                        false
                    }
                    Ok(_) => true,
                    Err(e) => {
                        log::warn!("compositor: a client of process {} waiting to be identified can't be polled, kept: {e}", ask.pid);
                        true
                    }
                }
            });
        }
    }

    /// How many clients wait for their process to be identified.
    #[cfg(test)]
    pub(crate) fn clients_waiting(&self) -> usize {
        self.asks.iter().flat_map(|ask| &ask.waiting).filter(|waiting| matches!(waiting, Waiting::Client(_))).count()
    }

    /// The next thing that waited for a process that was identified, with the answer.
    pub(crate) fn answered(&mut self) -> Option<(Waiting, Process)> {
        while self.answered.is_empty()
            && let Ok((token, process)) = self.answers.try_recv()
        {
            if let Some(index) = self.asks.iter().position(|ask| ask.token == token) {
                let ask = self.asks.swap_remove(index);
                self.answered.extend(ask.waiting.into_iter().map(|waiting| (waiting, process.clone())));
            }
        }
        self.answered.pop()
    }
}

impl Server {
    /// Takes in a client that connected once its process is identified (see [`Identities`]), so that
    /// everything it asks for, from the outputs it is shown on, is answered knowing whose it is. Its
    /// requests wait in its socket meanwhile.
    pub(crate) fn admit(&mut self, stream: UnixStream) {
        match rustix::net::sockopt::socket_peercred(&stream) {
            Ok(credentials) => {
                let pid = credentials.pid.as_raw_nonzero().get().unsigned_abs();
                self.identities.ask(pid, Waiting::Client(stream));
            }
            Err(e) => {
                log::warn!("compositor: a client's process unknown, so it is taken as no game's: {e}");
                insert_client(&mut self.dh, stream, ClientState::default());
            }
        }
    }

    /// Takes in a client whose process was identified: a game's (see `running_of_process`) is shown only
    /// the top screen, at its resolution as it is now, so a change applies the next time the game starts.
    /// A client that hung up while it waited is dropped (see [`Identities::drop_hung_up`]); one gone since
    /// the last turn is taken in all the same, and ends at the next dispatch.
    pub(crate) fn admit_identified(&mut self, stream: UnixStream, process: Process) {
        let game = self.core.state().process_game(process.unit.as_deref(), process.game.as_deref());
        let told = game.map(|(resolution, _)| clamshell_state::Reading::Told { unit: process.unit.clone(), game: process.game.clone(), resolution });
        insert_client(&mut self.dh, stream, ClientState { process: Some(process), game, ..ClientState::default() });
        if let Some(told) = told {
            self.core.handle(clamshell_state::Event::Reading(told));
        }
    }

    /// Hands out the processes identified since the last turn, after dropping the clients waiting that
    /// hung up.
    pub(crate) fn take_identified(&mut self) {
        self.identities.drop_hung_up();
        while let Some((waiting, process)) = self.identities.answered() {
            match waiting {
                Waiting::Client(stream) => self.admit_identified(stream, process),
                Waiting::X11Window(id) => self.x11.identified.push((id, process)),
            }
        }
    }

    /// Tells a game's client that bound the top output the mode its resolution gives the screen, after the
    /// screen's own that Smithay sent, so a game choosing from the display modes (Wine's) finds it as the
    /// largest. The screen's
    /// mode itself never changes for a game: Wine lays its monitors out by their modes, and the second
    /// screen's monitor would move under the game's window.
    pub(crate) fn tell_game_mode(&mut self, output: &smithay::reexports::wayland_server::protocol::wl_output::WlOutput) {
        use smithay::reexports::wayland_server::{Resource, protocol::wl_output::Mode as WMode};
        let Some((resolution, _)) = output.client().and_then(|client| client.get_data::<ClientState>()?.game) else { return };
        let screen = (self.screens.top_width.max(0) as u32, self.screens.size.1.max(0) as u32);
        let (w, h) = resolution.size_on(screen);
        if (w, h) != screen && w > 0 && h > 0 {
            output.mode(WMode::Current, w as i32, h as i32, self.screens.refresh);
            if output.version() >= 2 {
                output.done();
            }
        }
    }
}
