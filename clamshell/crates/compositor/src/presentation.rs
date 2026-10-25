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
        // Winit exposes neither a display timestamp nor a refresh sequence.
        self.feedback(rendered, time, None, Refresh::Unknown, 0, Kind::empty());
    }

    /// A page flip on the top screen or the second (`down`): the hardware's time, the screen's refresh
    /// and the flip's sequence, for the windows shown there.
    pub(crate) fn present_flip(&self, rendered: &RenderElementStates, time: Time<Monotonic>, down: bool, refresh: std::time::Duration, sequence: u64) {
        self.feedback(rendered, time, Some(down), Refresh::Fixed(refresh), sequence, Kind::Vsync | Kind::HwClock | Kind::HwCompletion);
    }

    fn feedback(&self, rendered: &RenderElementStates, time: Time<Monotonic>, screen: Option<bool>, refresh: Refresh, sequence: u64, kind: Kind) {
        for client in self.windows.iter().filter(|client| client.mapped && self.visible(client.id)) {
            let down = self.core.state().session().window(client.id).is_some_and(|window| window.down);
            if screen.is_some_and(|screen| screen != down) {
                continue;
            }
            let [top, bottom] = &self.outputs;
            let output = if down { bottom } else { top };
            client.window.with_surfaces(|surface, states| {
                if !rendered.element_was_presented(surface) {
                    return;
                }
                let mut cached = states.cached_state.get::<PresentationFeedbackCachedState>();
                let current = cached.current();
                for callback in current.callbacks.drain(..) {
                    callback.presented(output, time, refresh, sequence, kind);
                }
            });
        }
    }
}
