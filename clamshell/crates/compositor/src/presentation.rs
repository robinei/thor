//! Nested feedback uses a software submission timestamp, without hardware timing claims.

use smithay::{
    backend::renderer::element::RenderElementStates,
    reexports::wayland_protocols::wp::presentation_time::server::wp_presentation_feedback::Kind,
    utils::{Monotonic, Time},
    wayland::presentation::{PresentationFeedbackCachedState, Refresh},
};

use crate::server::Server;

impl Server {
    /// Called only after a successful host swap. Hidden or occluded surfaces retain their feedback
    /// until they appear, are superseded by another commit, or are destroyed.
    pub(crate) fn present_submission(&self, rendered: &RenderElementStates, time: Time<Monotonic>) {
        for client in self.windows.iter().filter(|client| client.mapped && self.visible(client.id)) {
            let [top, bottom] = &self.outputs;
            let output = if self.core.state().session().window(client.id).is_some_and(|window| window.down) { bottom } else { top };
            client.window.with_surfaces(|surface, states| {
                if !rendered.element_was_presented(surface) {
                    return;
                }
                let mut cached = states.cached_state.get::<PresentationFeedbackCachedState>();
                let current = cached.current();
                for callback in current.callbacks.drain(..) {
                    // Winit exposes neither a display timestamp nor a refresh sequence.
                    callback.presented(output, time, Refresh::Unknown, 0, Kind::empty());
                }
            });
        }
    }
}
