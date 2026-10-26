//! Frame pacing: the frame callbacks (a capped window's at its cap's pace) and the FIFO barriers let go
//! with them, the commits timed with commit timing, the frame rate sent to the state, and presentation
//! feedback. Nested feedback uses a software submission timestamp, without hardware timing claims.

use std::time::Instant;

use smithay::{
    backend::renderer::element::RenderElementStates,
    reexports::{wayland_protocols::wp::presentation_time::server::wp_presentation_feedback::Kind, wayland_server::Resource},
    utils::{Monotonic, Time},
    wayland::{
        compositor::with_states,
        presentation::{PresentationFeedbackCachedState, Refresh},
    },
};

use crate::server::Server;

/// What the server counts and sends for pacing.
#[derive(Default)]
pub(crate) struct Pacing {
    /// Commits so far, for a host to tell when clients want frame callbacks without a visible change.
    pub commits: u64,
    /// When the frame rate of the window in front is next sent, while there is one to send, and what was
    /// last sent.
    pub frames_due: Option<Instant>,
    frames_sent: (Option<f32>, Option<f32>),
}

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
            let on_down = state.session().below(client.id);
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
            let down = state.session().below(client.id);
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

    /// Frame callbacks for the windows shown on the top screen or the second (`down`; None: both, as
    /// nested), a capped window's at its cap's pace (see [`crate::frames::Frames::pace`]) on a screen
    /// showing a frame every `refresh`, and with them their surfaces' FIFO barriers, whose held commits
    /// then go ahead. Returns when a callback held back is due.
    pub fn frame_callbacks(&mut self, down: Option<bool>, refresh: std::time::Duration, now: Instant) -> Option<Instant> {
        let shown: Vec<_> = self.windows.iter().map(|client| client.mapped && self.visible(client.id)).collect();
        let state = self.core.state();
        let [top, bottom] = &self.outputs;
        let time = self.started.elapsed();
        let mut held: Option<Instant> = None;
        let mut barriers: Vec<smithay::reexports::wayland_server::Client> = Vec::new();
        for (client, shown) in self.windows.iter_mut().zip(shown) {
            let on_down = state.session().below(client.id);
            if !shown || down.is_some_and(|down| down != on_down) {
                continue;
            }
            // A capped window uses its turn only while it waits for a callback or on a FIFO barrier, so
            // that one drawing slower than its cap isn't held back further.
            let cap = client.shown.fps_cap;
            let waiting = client.window.root().is_some_and(|root| {
                with_states(&root, |states| !states.cached_state.get::<smithay::wayland::compositor::SurfaceAttributes>().current().frame_callbacks.is_empty())
            });
            let mut barrier = false;
            client.window.with_surfaces(|_, states| {
                barrier |= states.cached_state.get::<smithay::wayland::fifo::FifoBarrierCachedState>().current().barrier.is_some();
            });
            let waiting = waiting || barrier;
            let paced = if cap.is_some() && !waiting { Ok(()) } else { client.frames.pace(cap, refresh, now) };
            match paced {
                Ok(()) => {
                    let output = if on_down { bottom } else { top };
                    client.window.send_frame(output, time, Some(std::time::Duration::ZERO), |_, _| Some(output.clone()));
                    client.window.with_surfaces(|surface, states| {
                        // Only once the content that set it has been shown: let go earlier, the next
                        // would replace it unseen.
                        if !content_shown(states) {
                            return;
                        }
                        let barrier = states.cached_state.get::<smithay::wayland::fifo::FifoBarrierCachedState>().current().barrier.take();
                        if let Some(barrier) = barrier {
                            barrier.signal();
                            if let Some(owner) = surface.client().filter(|owner| !barriers.iter().any(|c| c.id() == owner.id())) {
                                barriers.push(owner);
                            }
                        }
                    });
                }
                Err(due) => held = Some(held.map_or(due, |held| held.min(due))),
            }
        }
        let dh = self.dh.clone();
        for owner in barriers {
            use smithay::wayland::compositor::CompositorHandler;
            self.client_compositor_state(&owner).blocker_cleared(self, &dh);
        }
        held
    }

    /// Lets the windows' commits timed (`wp_commit_timing`) for no later than `shown`, when the frame about
    /// to be composed is shown, go ahead, and a capped window's whatever their time: its cap paces it, and
    /// Mesa's timing, which takes a frame shown later than it asked as one missed, would move its times on
    /// further each frame the cap holds back. Returns the earliest time a commit is still held for.
    pub fn release_timed_commits(&mut self, shown: smithay::utils::Time<smithay::utils::Monotonic>) -> Option<smithay::utils::Time<smithay::utils::Monotonic>> {
        use smithay::wayland::commit_timing::{CommitTimerBarrierStateUserData, Timestamp};
        let mut released: Vec<smithay::reexports::wayland_server::Client> = Vec::new();
        let mut held: Option<Timestamp> = None;
        for client in &self.windows {
            let until = if client.shown.fps_cap.is_some() {
                Timestamp::from(smithay::utils::Time::<smithay::utils::Monotonic>::from(std::time::Duration::from_secs(1 << 40)))
            } else {
                shown.into()
            };
            client.window.with_surfaces(|surface, states| {
                let Some(mut timers) = states.data_map.get::<CommitTimerBarrierStateUserData>().and_then(|timers| timers.lock().ok()) else { return };
                if timers.signal_until(until)
                    && let Some(owner) = surface.client().filter(|owner| !released.iter().any(|c| c.id() == owner.id()))
                {
                    released.push(owner);
                }
                if let Some(next) = timers.next_deadline() {
                    held = Some(held.map_or(next, |held| held.min(next)));
                }
            });
        }
        let dh = self.dh.clone();
        for owner in released {
            use smithay::wayland::compositor::CompositorHandler;
            self.client_compositor_state(&owner).blocker_cleared(self, &dh);
        }
        held.map(Into::into)
    }

    /// Sends the frame rate of the window on the top screen every [`crate::frames::REPORT`], while it
    /// draws: none for a window that drew nothing in the last second, so that the FPS tile measures no
    /// game rather than 0.
    pub(crate) fn report_frames(&mut self, now: Instant) {
        if self.pacing.frames_due.is_some_and(|due| due > now) {
            return;
        }
        let at = self.clock.now().into();
        let top = self.core.state().session().top_window().map(|w| w.id);
        let rate = self.windows.iter().find(|w| w.mapped && Some(w.id) == top).map(|w| w.frames.rate(at));
        let reading = rate.filter(|(fps, _)| *fps > 0.0).map_or((None, None), |(fps, ms)| (Some(fps), ms));
        if reading != self.pacing.frames_sent {
            self.pacing.frames_sent = reading;
            self.core.handle(clamshell_state::Event::Reading(clamshell_state::Reading::Frames { fps: reading.0, frame_ms: reading.1 }));
        }
        self.pacing.frames_due = reading.0.is_some().then(|| now + crate::frames::REPORT);
    }
}
