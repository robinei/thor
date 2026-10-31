//! Frame pacing: the frame callbacks (a capped window's at its cap's pace) and the FIFO barriers let go
//! with them, the commits timed with commit timing, the frame rate sent to the state, and presentation
//! feedback. Nested feedback uses a software submission timestamp, without hardware timing claims.

use std::time::Instant;

use smithay::{
    backend::renderer::element::RenderElementStates,
    reexports::{wayland_protocols::wp::presentation_time::server::wp_presentation_feedback::Kind, wayland_server::Resource},
    utils::{Monotonic, Time},
    wayland::presentation::{PresentationFeedbackCachedState, PresentationFeedbackCallback, Refresh},
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
    /// The clients whose blockers a frame's callbacks or a commit timing's release cleared, told once each;
    /// kept, so that pacing a frame allocates nothing.
    cleared: Vec<smithay::reexports::wayland_server::Client>,
    /// The feedback of a frame submitted to the nested host, kept so that it allocates nothing.
    #[cfg(any(feature = "nested", test))]
    submitted: Feedback,
}

/// The presentation feedback of a frame queued on a screen: taken as the frame is queued
/// ([`Server::take_feedback`]), since a client may commit again before it shows, whose feedback is then
/// the next frame's; given at its flip ([`Server::present_flip`]), or discarded if it never shows.
#[derive(Default)]
pub(crate) struct Feedback {
    /// Each callback, whether its surface is shown from its own buffer, and whether on the second screen.
    callbacks: Vec<(PresentationFeedbackCallback, bool, bool)>,
    /// The windows whose picture it shows, for their frame rates.
    windows: Vec<i64>,
}

impl Feedback {
    /// Its frame won't show: the clients are told.
    pub(crate) fn discard(&mut self) {
        for (callback, ..) in self.callbacks.drain(..) {
            callback.discarded();
        }
        self.windows.clear();
    }
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
    #[cfg(any(feature = "nested", test))]
    pub(crate) fn present_submission(&mut self, rendered: &RenderElementStates, time: Time<Monotonic>) {
        let mut feedback = std::mem::take(&mut self.pacing.submitted);
        self.take_feedback(rendered, None, &mut feedback);
        // Winit exposes neither a display timestamp nor a refresh sequence.
        self.give_feedback(&mut feedback, time, Refresh::Unknown, 0, Kind::empty());
        self.pacing.submitted = feedback;
    }

    /// A page flip on a screen showed the frame `feedback` was taken for: its time (the hardware's,
    /// `hw_clock`, or else the clock's as the flip was seen), the screen's refresh and the flip's sequence.
    pub(crate) fn present_flip(&mut self, feedback: &mut Feedback, time: Time<Monotonic>, hw_clock: bool, refresh: std::time::Duration, sequence: u64) {
        let clock = if hw_clock { Kind::HwClock } else { Kind::empty() };
        self.give_feedback(feedback, time, Refresh::Fixed(refresh), sequence, Kind::Vsync | Kind::HwCompletion | clock);
    }

    /// Takes into `feedback` the presentation feedback of the windows' surfaces `rendered` drew on a screen
    /// (`down`; None: both, nested), as its frame is queued; what it held before, never shown, is
    /// discarded.
    pub(crate) fn take_feedback(&mut self, rendered: &RenderElementStates, screen: Option<bool>, feedback: &mut Feedback) {
        feedback.discard();
        self.place();
        for client in &self.windows {
            let Some(down) = self.scene.window(client.id).map(|placed| placed.below) else { continue };
            if screen.is_some_and(|screen| screen != down) {
                continue;
            }
            if client.window.picture_surface().is_some_and(|picture| rendered.element_was_presented(&picture)) {
                feedback.windows.push(client.id);
            }
            client.window.with_surfaces(|surface, states| {
                if !rendered.element_was_presented(surface) {
                    return;
                }
                // Shown from its own buffer (see `window_scanout`): without a copy.
                let zero_copy = rendered
                    .element_render_state(surface)
                    .is_some_and(|state| state.presentation_state == smithay::backend::renderer::element::RenderElementPresentationState::ZeroCopy);
                let mut cached = states.cached_state.get::<PresentationFeedbackCachedState>();
                feedback.callbacks.extend(cached.current().callbacks.drain(..).map(|callback| (callback, zero_copy, down)));
            });
        }
    }

    /// The windows `feedback` was taken for get it, and a frame counted for each that showed a new buffer.
    fn give_feedback(&mut self, feedback: &mut Feedback, time: Time<Monotonic>, refresh: Refresh, sequence: u64, kind: Kind) {
        let [top, bottom] = &self.screens.outputs;
        for (callback, zero_copy, down) in feedback.callbacks.drain(..) {
            let kind = if zero_copy { kind | Kind::ZeroCopy } else { kind };
            callback.presented(if down { bottom } else { top }, time, refresh, sequence, kind);
        }
        for id in feedback.windows.drain(..) {
            if let Some(client) = self.windows.iter_mut().find(|client| client.id == id) {
                client.frames.shown(time.into());
            }
        }
    }

    /// Frame callbacks for the windows shown on the top screen or the second (`down`; None: both, as
    /// nested), a capped window's at its cap's pace (see [`crate::frames::Frames::pace`]) on a screen
    /// showing a frame every `refresh`, and with them their surfaces' FIFO barriers, whose held commits
    /// then go ahead. Returns when a callback held back is due.
    pub fn frame_callbacks(&mut self, down: Option<bool>, refresh: std::time::Duration, now: Instant) -> Option<Instant> {
        self.place();
        let [top, bottom] = &self.screens.outputs;
        let time = self.started.elapsed();
        let mut held: Option<Instant> = None;
        for client in self.windows.iter_mut() {
            let Some(on_down) = self.scene.window(client.id).map(|placed| placed.below) else { continue };
            if down.is_some_and(|down| down != on_down) {
                continue;
            }
            // A capped window uses its turn only while it waits for a callback or on a FIFO barrier, so
            // that one drawing slower than its cap isn't held back further.
            let cap = client.shown.fps_cap;
            let mut waiting = false;
            client.window.with_surfaces(|_, states| {
                waiting |= !states.cached_state.get::<smithay::wayland::compositor::SurfaceAttributes>().current().frame_callbacks.is_empty()
                    || states.cached_state.get::<smithay::wayland::fifo::FifoBarrierCachedState>().current().barrier.is_some();
            });
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
                            if let Some(owner) = surface.client().filter(|owner| !self.pacing.cleared.iter().any(|c| c.id() == owner.id())) {
                                self.pacing.cleared.push(owner);
                            }
                        }
                    });
                }
                Err(due) => held = Some(held.map_or(due, |held| held.min(due))),
            }
        }
        self.tell_cleared();
        held
    }

    /// Tells the clients in `cleared` that their blockers cleared, so that their held commits go ahead.
    fn tell_cleared(&mut self) {
        use smithay::wayland::compositor::CompositorHandler;
        let dh = self.dh.clone();
        let mut cleared = std::mem::take(&mut self.pacing.cleared);
        for owner in cleared.drain(..) {
            self.client_compositor_state(&owner).blocker_cleared(self, &dh);
        }
        self.pacing.cleared = cleared;
    }

    /// Lets the commits timed (`wp_commit_timing`) for no later than `shown`, when the frame about to be
    /// composed is shown, go ahead for the windows on a screen (`down`; None: both, nested; the top screen
    /// also takes the windows not shown), and a capped window's whatever their time: its cap paces
    /// it, and Mesa's timing, which takes a frame shown later than it asked as one missed, would move its
    /// times on further each frame the cap holds back. Returns the earliest time a commit is still held for.
    pub fn release_timed_commits(
        &mut self,
        shown: smithay::utils::Time<smithay::utils::Monotonic>,
        down: Option<bool>,
    ) -> Option<smithay::utils::Time<smithay::utils::Monotonic>> {
        use smithay::wayland::commit_timing::{CommitTimerBarrierStateUserData, Timestamp};
        let mut held: Option<Timestamp> = None;
        let state = self.core.state();
        for client in &self.windows {
            let on_down = state.session().below(client.id);
            if down.is_some_and(|down| down != on_down) {
                continue;
            }
            let until = if client.shown.fps_cap.is_some() {
                Timestamp::from(smithay::utils::Time::<smithay::utils::Monotonic>::from(std::time::Duration::from_secs(1 << 40)))
            } else {
                shown.into()
            };
            client.window.with_surfaces(|surface, states| {
                let Some(mut timers) = states.data_map.get::<CommitTimerBarrierStateUserData>().and_then(|timers| timers.lock().ok()) else { return };
                if timers.signal_until(until)
                    && let Some(owner) = surface.client().filter(|owner| !self.pacing.cleared.iter().any(|c| c.id() == owner.id()))
                {
                    self.pacing.cleared.push(owner);
                }
                if let Some(next) = timers.next_deadline() {
                    held = Some(held.map_or(next, |held| held.min(next)));
                }
            });
        }
        self.tell_cleared();
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
