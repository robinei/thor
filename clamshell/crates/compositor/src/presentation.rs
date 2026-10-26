//! Nested feedback uses a software submission timestamp, without hardware timing claims.

use smithay::{
    backend::renderer::element::RenderElementStates,
    reexports::wayland_protocols::wp::presentation_time::server::wp_presentation_feedback::Kind,
    utils::{Monotonic, Time},
    wayland::presentation::{PresentationFeedbackCachedState, Refresh},
};

use crate::server::Server;

/// The commits of a surface's content in the frame queued for its screen and in the one last shown (see
/// [`Server::frame_queued`]).
#[derive(Default)]
struct Latched {
    queued: Option<smithay::backend::renderer::utils::CommitCounter>,
    shown: Option<smithay::backend::renderer::utils::CommitCounter>,
}

/// Whether a surface's current content has been shown, or it has never been drawn: a FIFO barrier set
/// with it may then be let go, and the next content can't replace it unseen.
pub(crate) fn content_shown(states: &smithay::wayland::compositor::SurfaceData) -> bool {
    let current = states
        .data_map
        .get::<smithay::backend::renderer::utils::RendererSurfaceStateUserData>()
        .and_then(|state| state.lock().ok().map(|state| state.current_commit()));
    let latched = states.data_map.get::<std::sync::Mutex<Latched>>().and_then(|latched| latched.lock().ok().map(|l| (l.queued, l.shown)));
    match latched {
        Some((None, None)) | None => true,
        Some((_, shown)) => shown.is_some() && shown == current,
    }
}

impl Server {
    /// Notes the commit each surface of the windows on a screen (`down`; None: both, nested) had when
    /// `rendered` drew it, for the frame queued there; it counts as shown once [`Server::frame_shown`] says.
    pub(crate) fn frame_queued(&self, rendered: &RenderElementStates, down: Option<bool>) {
        self.latch(rendered, down, |latched, commit| latched.queued = Some(commit));
    }

    /// The frame queued on a screen is shown: its surfaces' content (see [`Server::frame_queued`]).
    pub(crate) fn frame_shown(&self, rendered: &RenderElementStates, down: Option<bool>) {
        self.latch(rendered, down, |latched, _| latched.shown = latched.queued);
    }

    fn latch(&self, rendered: &RenderElementStates, down: Option<bool>, f: impl Fn(&mut Latched, smithay::backend::renderer::utils::CommitCounter)) {
        let state = self.core.state();
        for client in &self.windows {
            let on_down = state.session().window(client.id).is_some_and(|window| window.down);
            if down.is_some_and(|down| down != on_down) {
                continue;
            }
            client.window.with_surfaces(|surface, states| {
                if rendered.element_render_state(surface).is_none() {
                    return;
                }
                let commit = states
                    .data_map
                    .get::<smithay::backend::renderer::utils::RendererSurfaceStateUserData>()
                    .and_then(|state| state.lock().ok().map(|state| state.current_commit()));
                states.data_map.insert_if_missing_threadsafe(|| std::sync::Mutex::new(Latched::default()));
                if let (Some(commit), Some(mut latched)) = (commit, states.data_map.get::<std::sync::Mutex<Latched>>().and_then(|l| l.lock().ok())) {
                    f(&mut latched, commit);
                }
            });
        }
    }

    /// Called only after a successful host swap. Hidden or occluded surfaces retain their feedback
    /// until they appear, are superseded by another commit, or are destroyed.
    pub(crate) fn present_submission(&mut self, rendered: &RenderElementStates, time: Time<Monotonic>) {
        // Winit exposes neither a display timestamp nor a refresh sequence.
        self.feedback(rendered, time, None, Refresh::Unknown, 0, Kind::empty());
    }

    /// A page flip on the top screen or the second (`down`): the hardware's time, the screen's refresh
    /// and the flip's sequence, for the windows shown there.
    pub(crate) fn present_flip(&mut self, rendered: &RenderElementStates, time: Time<Monotonic>, down: bool, refresh: std::time::Duration, sequence: u64) {
        self.feedback(rendered, time, Some(down), Refresh::Fixed(refresh), sequence, Kind::Vsync | Kind::HwClock | Kind::HwCompletion);
    }

    /// The windows shown get their presentation feedback, and a frame counted if a new buffer was shown.
    fn feedback(&mut self, rendered: &RenderElementStates, time: Time<Monotonic>, screen: Option<bool>, refresh: Refresh, sequence: u64, kind: Kind) {
        let shown: Vec<_> = self.windows.iter().map(|client| client.mapped && self.visible(client.id)).collect();
        let state = self.core.state();
        for (client, shown) in self.windows.iter_mut().zip(shown) {
            if !shown {
                continue;
            }
            let down = state.session().window(client.id).is_some_and(|window| window.down);
            if screen.is_some_and(|screen| screen != down) {
                continue;
            }
            let [top, bottom] = &self.outputs;
            let output = if down { bottom } else { top };
            if client.window.root().is_some_and(|root| rendered.element_was_presented(&root)) {
                client.frames.shown(time.into());
            }
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
