//! A turn's timing: when each screen is next due, and the screens' power and refresh.

use super::*;

/// When the loop wakes next, `due` as the screens' turns say, with `composed` the commit count composed
/// and `commits` the count now: commits let through after composing (a FIFO barrier the cap let go) are
/// composed at once, not at the next wake-up for something else.
pub(super) fn after_compose(due: Option<Instant>, composed: u64, commits: u64, now: Instant) -> Option<Instant> {
    if commits != composed { Some(now) } else { due }
}

/// When the loop wakes for a commit timed for `at` (the clock's times since boot), its screen's next
/// frame `next` and its refresh `interval`, given the loop's `now` and the clock's `clock`: a refresh before
/// it is due, to compose it in time; or, within a refresh of it already, at the next frame, when the one
/// shown then is composed (not at once again, which would turn the loop until then).
pub(super) fn timed_wake(now: Instant, clock: Duration, next: Duration, at: Duration, interval: Duration) -> Instant {
    let wait = at.saturating_sub(clock).saturating_sub(interval);
    now + if wait.is_zero() { next.saturating_sub(clock) } else { wait }
}

/// The item whose rate (`rate`) is nearest `hz`, the first of those as near.
pub(super) fn nearest<T>(items: &[T], rate: impl Fn(&T) -> u32, hz: u32) -> Option<&T> {
    items.iter().min_by_key(|item| rate(item).abs_diff(hz))
}

/// The earlier of two times, either or both absent.
pub(super) fn earlier(a: Option<Instant>, b: Option<Instant>) -> Option<Instant> {
    match (a, b) {
        (Some(a), Some(b)) => Some(a.min(b)),
        (a, b) => a.or(b),
    }
}

/// What decides a screen's turn in [`step`].
#[derive(Debug, Clone, Copy)]
pub(super) struct Timing {
    /// A frame is queued, waiting for its flip.
    pub(super) in_flight: bool,
    /// A frame is drawn, its commit waiting for the GPU to finish it.
    pub(super) rendering: bool,
    /// Its part of the scene waits to be composed (for its composition deadline).
    pub(super) composing: bool,
    /// What it shows.
    pub(super) shows: Shows,
    /// Its part of the scene changed since its last frame.
    pub(super) dirty: bool,
    /// What its overlay and cursor planes show changed since its last commit.
    pub(super) layers_dirty: bool,
    pub(super) on: bool,
    /// Whether its flips pace its windows (see [`pacer`]): else another screen answers them.
    pub(super) paces: bool,
    /// When frame callbacks held back (under a frame rate cap) are due.
    pub(super) held: Option<Instant>,
    /// Whether the clients' commits have been answered with their frame callbacks.
    pub(super) answered: bool,
    /// The soonest an idle screen answers commits again (at most once a refresh).
    pub(super) next_callbacks: Option<Instant>,
}

impl Timing {
    /// `screen`'s, with the server's commit count `commits`.
    pub(super) fn of(screen: &Screen, commits: u64) -> Timing {
        Timing {
            in_flight: screen.surface.in_flight(),
            rendering: screen.surface.rendering(),
            composing: screen.needs_compose && !matches!(screen.shows, Shows::Own | Shows::Window(_)),
            shows: screen.shows,
            dirty: screen.dirty,
            layers_dirty: screen.layers_dirty,
            on: screen.on,
            paces: screen.paces,
            held: screen.held,
            answered: screen.answered == commits,
            next_callbacks: screen.next_callbacks,
        }
    }

    /// Whether a frame of its own is under way: queued, drawing, or, on, waiting for its composition
    /// deadline. The new content's frame callbacks aren't answered as an idle commit's meanwhile.
    fn pending(&self) -> bool {
        self.in_flight || self.rendering || (self.on && self.composing)
    }
}

/// What a screen's turn does.
#[derive(Debug, Default, PartialEq, Eq)]
pub(super) struct Turn {
    /// Send the frame callbacks held back, now due.
    pub(super) send_held: bool,
    /// What it commits, or answers.
    pub(super) does: Does,
    /// When to wake for this screen, unless something comes before.
    pub(super) wake: Option<Instant>,
}

/// What a screen commits in its turn, or whether it answers its clients.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub(super) enum Does {
    #[default]
    Nothing,
    /// A frame composed: the one drawing committed once the GPU is done, or one drawn now (see `present`).
    Frame,
    /// Its own buffer, drawn with what changed in ClamShell's view (see `present_ui`).
    Own,
    /// The window shown from its own buffer (see `present_window`).
    Window,
    /// The frame shown, again, with its overlay and cursor planes as they now are (see `present_layers`).
    Layers,
    /// The clients' commits answered with their frame callbacks, the screen being idle.
    Answer,
}

/// A screen's turn. A frame drawing is committed once the GPU is done with it; a screen showing its own
/// buffers commits them whenever it is free; otherwise, while a frame of its own is under way (see
/// [`Timing::pending`]) nothing but a change of its planes, committed with the frame shown when it is free
/// (its flip sends what is held: a wake now would only turn the loop until it); else the held callbacks
/// once due, and a frame if its part changed and it is on; else, with commits not answered, their
/// callbacks at most once a refresh. A screen that doesn't pace its windows only shows a frame when its
/// part changed.
pub(super) fn screen_turn(timing: Timing, now: Instant) -> Turn {
    let mut turn = Turn::default();
    let (mut present, mut answer) = (false, false);
    if !timing.pending() {
        if timing.paces {
            match timing.held {
                Some(at) if at <= now => turn.send_held = true,
                held => turn.wake = held,
            }
            if timing.dirty && timing.on {
                present = true;
            } else if !timing.answered {
                match timing.next_callbacks {
                    Some(at) if at > now => turn.wake = earlier(turn.wake, Some(at)),
                    _ => answer = true,
                }
            }
        } else {
            present = timing.dirty && timing.on;
        }
    }
    let free = timing.on && !timing.in_flight;
    turn.does = match timing.shows {
        _ if timing.on && timing.rendering => Does::Frame,
        Shows::Own if free => Does::Own,
        Shows::Own => Does::Nothing,
        Shows::Window(_) if present => Does::Window,
        _ if present => Does::Frame,
        _ if timing.layers_dirty && free => Does::Layers,
        _ if answer => Does::Answer,
        _ => Does::Nothing,
    };
    turn
}

/// What of a screen's state its power changes.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub(super) struct Power {
    pub(super) on: bool,
    pub(super) dirty: bool,
}

/// A screen turned on or off as `wanted`, if that changes it: turned on it is to be drawn whole, since its
/// last frame is gone. (Either way no frame is in flight any more: see `outputs`.)
pub(super) fn switched(power: Power, wanted: bool) -> Option<Power> {
    (power.on != wanted).then_some(Power { on: wanted, dirty: power.dirty || wanted })
}

/// The mode for a refresh rate of `hz` asked for: `start` for 0 (the screen's own), else the one of
/// `modes` whose rate is nearest (see [`nearest`]), or `start` if there are none.
pub(super) fn mode_for<'a, T>(modes: &'a [T], start: &'a T, rate: impl Fn(&T) -> u32, hz: u32) -> &'a T {
    if hz == 0 { start } else { nearest(modes, rate, hz).unwrap_or(start) }
}
