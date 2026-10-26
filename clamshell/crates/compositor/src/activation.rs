//! A recent token delegates focus through the ordinary window activation rule.

use std::time::{Duration, Instant};

use clamshell_state::WindowEvent;
use smithay::{
    input::Seat,
    reexports::wayland_server::{Resource, protocol::wl_surface::WlSurface},
    wayland::xdg_activation::{XdgActivationHandler, XdgActivationState, XdgActivationToken, XdgActivationTokenData},
};

use crate::server::Server;

const TOKEN_LIFETIME: Duration = Duration::from_secs(5);

impl Server {
    pub(crate) fn expire_activation_tokens(&mut self, now: Instant) {
        self.protocols.activation.retain_tokens(|_, data| now.saturating_duration_since(data.timestamp) < TOKEN_LIFETIME);
    }
}

impl XdgActivationHandler for Server {
    fn activation_state(&mut self) -> &mut XdgActivationState {
        &mut self.protocols.activation
    }

    fn token_created(&mut self, _: XdgActivationToken, data: XdgActivationTokenData) -> bool {
        let Some((serial, seat)) = data.serial else { return false };
        let Some(keyboard) = self.seat.get_keyboard() else { return false };
        let focused = self
            .core
            .state()
            .session()
            .keyboard_focus()
            .and_then(|id| self.windows.iter().find(|window| window.id == id && window.mapped))
            .and_then(|window| window.window.toplevel())
            .and_then(|surface| surface.wl_surface().client())
            .map(|client| client.id());
        if focused.is_none() || focused != data.client_id {
            return false;
        }
        Seat::<Self>::from_resource(&seat).as_ref() == Some(&self.seat)
            && keyboard.last_enter().is_some_and(|entered| serial.is_no_older_than(&entered))
            && keyboard.current_focus().and_then(|surface| surface.client()).map(|client| client.id()) == data.client_id
            && data.surface.as_ref().is_none_or(|surface| surface.client().map(|client| client.id()) == data.client_id)
    }

    fn request_activation(&mut self, token: XdgActivationToken, data: XdgActivationTokenData, surface: WlSurface) {
        // Consume even a rejected request, so a token never grants repeated focus changes.
        if !self.protocols.activation.remove_token(&token) {
            return;
        }
        if data.timestamp.elapsed() >= TOKEN_LIFETIME || self.core.state().session().keyboard_focus().is_none() {
            return;
        }
        let Some(id) = self.surface_id(&surface) else { return };
        if self.core.state().session().window(id).is_none() {
            return;
        }
        self.core.handle(WindowEvent::Activate(id).into());
        self.sync_windows();
    }
}
