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
    /// The Steam game it belongs to, if Steam started it (see [`Server::tell_game_mode`]).
    pub steam_app: Option<u32>,
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
/// for it, and the answers come back to [`Server::turn`].
pub(crate) struct Identities {
    identify: Arc<dyn Fn(u32) -> Process + Send + Sync>,
    /// Wakes the event loop for an answer.
    wake: Arc<dyn Fn() + Send + Sync>,
    /// Each answer, by the token it was asked with.
    send: mpsc::Sender<(u64, Process)>,
    answers: mpsc::Receiver<(u64, Process)>,
    waiting: Vec<(u64, Waiting)>,
    next: u64,
}

impl Identities {
    pub(crate) fn new(identify: impl Fn(u32) -> Process + Send + Sync + 'static, wake: Arc<dyn Fn() + Send + Sync>) -> Identities {
        let (send, answers) = mpsc::channel();
        Identities { identify: Arc::new(identify), wake, send, answers, waiting: Vec::new(), next: 0 }
    }

    /// Identifies process `pid` for `waiting`, which [`Identities::answered`] gives back with the answer.
    pub(crate) fn ask(&mut self, pid: u32, waiting: Waiting) {
        let token = self.next;
        self.next = self.next.wrapping_add(1);
        self.waiting.push((token, waiting));
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

    /// The next answer that came, with what waited for it.
    pub(crate) fn answered(&mut self) -> Option<(Waiting, Process)> {
        while let Ok((token, process)) = self.answers.try_recv() {
            if let Some(index) = self.waiting.iter().position(|(t, _)| *t == token) {
                return Some((self.waiting.swap_remove(index).1, process));
            }
        }
        None
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
    /// A client gone meanwhile is taken in all the same, and ends at the next dispatch.
    pub(crate) fn admit_identified(&mut self, stream: UnixStream, process: Process) {
        let game = self.core.state().process_game(process.unit.as_deref(), process.steam_app);
        let told = game.map(|(resolution, _)| clamshell_state::Reading::Told { unit: process.unit.clone(), steam_app: process.steam_app, resolution });
        insert_client(&mut self.dh, stream, ClientState { process: Some(process), game, ..ClientState::default() });
        if let Some(told) = told {
            self.core.handle(clamshell_state::Event::Reading(told));
        }
    }

    /// Hands out the processes identified since the last turn.
    pub(crate) fn take_identified(&mut self) {
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
