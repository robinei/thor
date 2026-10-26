//! Clients as they connect: their protocol state, and who they are, which decides what a game's sees.

use std::sync::Arc;

use smithay::reexports::wayland_server::{
    DisplayHandle,
    backend::{ClientData, ClientId, DisconnectReason},
};
use smithay::wayland::compositor::CompositorClientState;

use crate::server::Server;

/// Identity read once from the client's credentials by the system-side resolver.
#[derive(Default)]
pub struct Process {
    pub unit: Option<String>,
    pub pids: Vec<u32>,
    /// The Steam game it belongs to, if Steam started it (see [`Server::tell_game_mode`]).
    pub steam_app: Option<u32>,
}

#[derive(Default)]
pub(crate) struct ClientState {
    pub compositor: CompositorClientState,
    /// For a game's process, its resolution when it connected and whether it shows a window on the second
    /// screen (see [`Server::admit`]).
    pub game: std::sync::OnceLock<Option<(clamshell_state::settings::Resolution, bool)>>,
    /// Xwayland, the only client that sees `xwayland_shell_v1`.
    pub xwayland: bool,
}

impl ClientState {
    /// Whether the client sees only the top screen: a game's, unless it shows a window on the second screen
    /// as a dual-screen emulator does.
    pub(crate) fn top_only(client: &smithay::reexports::wayland_server::Client) -> bool {
        client.get_data::<ClientState>().and_then(|state| state.game.get().copied().flatten()).is_some_and(|(_, dual)| !dual)
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

pub(crate) fn insert_client(dh: &mut DisplayHandle, stream: std::os::unix::net::UnixStream) -> Option<smithay::reexports::wayland_server::Client> {
    dh.insert_client(stream, Arc::new(ClientState::default())).inspect_err(|error| log::warn!("compositor: accept client: {error}")).ok()
}

impl Server {
    /// Takes in a client that connected, knowing it by its process (see `running_of_process`): a game's is
    /// shown only the top screen, at its resolution as it is now, so a change applies the next time the
    /// game starts.
    pub(crate) fn admit(&mut self, stream: std::os::unix::net::UnixStream) {
        let Some(client) = insert_client(&mut self.dh, stream) else { return };
        let process = client
            .get_credentials(&self.dh)
            .inspect_err(|e| log::warn!("compositor: a client's process unknown, so it is taken as no game's: {e}"))
            .ok()
            .and_then(|c| u32::try_from(c.pid).ok())
            .map(|pid| (self.identify)(pid));
        let game = process.as_ref().and_then(|process| self.core.state().process_game(process.unit.as_deref(), process.steam_app));
        if let Some(state) = client.get_data::<ClientState>() {
            let _ = state.game.set(game);
        }
        if let (Some(process), Some((resolution, _))) = (process, game) {
            let told = clamshell_state::Reading::Told { unit: process.unit, steam_app: process.steam_app, resolution };
            self.core.handle(clamshell_state::Event::Reading(told));
        }
    }

    /// Tells a game's client that bound the top output the mode its resolution gives the screen, after the
    /// screen's own that Smithay sent, so a game choosing from the display modes (Wine's) finds it as the
    /// largest. The screen's
    /// mode itself never changes for a game: Wine lays its monitors out by their modes, and the second
    /// screen's monitor would move under the game's window.
    pub(crate) fn tell_game_mode(&mut self, output: &smithay::reexports::wayland_server::protocol::wl_output::WlOutput) {
        use smithay::reexports::wayland_server::{Resource, protocol::wl_output::Mode as WMode};
        let Some((resolution, _)) = output.client().and_then(|client| client.get_data::<ClientState>()?.game.get().copied().flatten()) else { return };
        let screen = (self.top_width.max(0) as u32, self.size.1.max(0) as u32);
        let (w, h) = resolution.size_on(screen);
        if (w, h) != screen && w > 0 && h > 0 {
            output.mode(WMode::Current, w as i32, h as i32, self.refresh);
            if output.version() >= 2 {
                output.done();
            }
        }
    }
}
