//! A digital input's actuations: the small state machine that tells a press from a hold and a double
//! ([`Phase`]), and each actuation's on and off with its toggle and turbo ([`Beat`]).

use std::time::{Duration, Instant};

use clamshell_state::keymap::{Actuation, Binding, Input};

/// A second press within this of letting go makes a double.
pub(super) const DOUBLE_WINDOW: Duration = Duration::from_millis(300);
/// A press that is decided only after letting go, because the input has a hold or a double, is sent as
/// a tap this long.
pub(super) const TAP: Duration = Duration::from_millis(60);

/// Where a digital input's press is.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(super) enum Phase {
    Up,
    /// Down since then, and not yet known to be a press, a hold or a double.
    Deciding(Instant),
    /// Let go then after a short press. A second press soon makes a double.
    Waiting(Instant),
    /// An actuation that is on while held, or for a tap until `until`.
    On {
        act: Actuation,
        until: Option<Instant>,
    },
}

/// A binding's on and off: whether its actuation is on now, its toggle's state, and since when it has
/// been on. Turbo's beat counts from then.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub(super) struct Beat {
    pub(super) raw: bool,
    pub(super) latched: bool,
    pub(super) since: Option<Instant>,
}

impl Beat {
    /// Follows the actuation's current state, `raw`. A toggle flips each time the actuation starts.
    pub(super) fn follow(&mut self, raw: bool, toggle: bool, now: Instant) {
        let rising = raw && !self.raw;
        self.raw = raw;
        if toggle {
            if rising {
                self.latched = !self.latched;
                self.since = self.latched.then_some(now);
            }
        } else {
            self.latched = false;
            if rising {
                self.since = Some(now);
            }
        }
    }

    /// Whether the output is on: the actuation's or the toggle's state, beating at `turbo` per second.
    pub(super) fn on(&self, b: &Binding, now: Instant) -> bool {
        let on = if b.toggle { self.latched } else { self.raw };
        match (on, b.turbo, self.since) {
            (false, ..) => false,
            (true, 0, _) | (true, _, None) => true,
            (true, rate, Some(since)) => (now.saturating_duration_since(since).as_nanos() / half_beat(rate)).is_multiple_of(2),
        }
    }

    /// When its turbo next turns on or off. None if there is no turbo or it is off.
    pub(super) fn next_beat(&self, b: &Binding, now: Instant) -> Option<Instant> {
        let on = if b.toggle { self.latched } else { self.raw };
        let since = self.since.filter(|_| on && b.turbo > 0)?;
        let half = half_beat(b.turbo);
        let n = now.saturating_duration_since(since).as_nanos() / half + 1;
        Some(since + Duration::from_nanos(u64::try_from(n * half).unwrap_or(u64::MAX)))
    }
}

/// Half a turbo beat at `rate` per second, in nanoseconds. The output is on for one half and off for the
/// other.
pub(super) fn half_beat(rate: u8) -> u128 {
    500_000_000 / u128::from(rate.max(1))
}

/// A digital input's actuations: the phase of its press, and each actuation's beat.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(super) struct Actuator {
    pub(super) input: Input,
    pub(super) phase: Phase,
    pub(super) beats: [Beat; 4],
    /// When the phase moves on by itself: at a hold's time, a double's window or a tap's end.
    pub(super) due: Option<Instant>,
}

impl Actuator {
    pub(super) fn new(input: Input) -> Actuator {
        Actuator { input, phase: Phase::Up, beats: [Beat::default(); 4], due: None }
    }

    /// Moves the phase on, given whether the input is on at `now`, its hold's time if it has a hold's
    /// binding, and whether it has a double's.
    pub(super) fn step(&mut self, on: bool, hold: Option<Duration>, double: bool, now: Instant) {
        // A press during a tap starts a new one, so this goes round twice at most.
        for _ in 0..2 {
            let next = match self.phase {
                Phase::Up if on && hold.is_none() && !double => Phase::On { act: Actuation::Press, until: None },
                Phase::Up if on => Phase::Deciding(now),
                Phase::Up => Phase::Up,
                Phase::Deciding(at) if on => match hold {
                    Some(t) if now >= at + t => Phase::On { act: Actuation::Hold, until: None },
                    Some(_) => Phase::Deciding(at),
                    None if double && now < at + DOUBLE_WINDOW => Phase::Deciding(at),
                    // Held past the window with no hold, it is a press after all.
                    None => Phase::On { act: Actuation::Press, until: None },
                },
                Phase::Deciding(_) if double => Phase::Waiting(now),
                Phase::Deciding(_) => Phase::On { act: Actuation::Press, until: Some(now + TAP) },
                Phase::Waiting(_) if on => Phase::On { act: Actuation::Double, until: None },
                Phase::Waiting(at) if now >= at + DOUBLE_WINDOW => Phase::On { act: Actuation::Press, until: Some(now + TAP) },
                Phase::Waiting(at) => Phase::Waiting(at),
                Phase::On { until: None, .. } if !on => Phase::Up,
                Phase::On { until: Some(_), .. } if on => {
                    self.phase = Phase::Up;
                    continue;
                }
                Phase::On { until: Some(t), .. } if now >= t => Phase::Up,
                p @ Phase::On { .. } => p,
            };
            self.phase = next;
            break;
        }
        self.due = match self.phase {
            Phase::Deciding(at) => Some(hold.map_or(at + DOUBLE_WINDOW, |t| at + t)),
            Phase::Waiting(at) => Some(at + DOUBLE_WINDOW),
            Phase::On { until, .. } => until,
            Phase::Up => None,
        };
    }

    /// Whether `act` is on now, as the phase says.
    pub(super) fn actuated(&self, act: Actuation) -> bool {
        matches!(self.phase, Phase::On { act: a, .. } if a == act)
    }

    pub(super) fn beat(&self, act: Actuation) -> Beat {
        self.beats.get(act as usize).copied().unwrap_or_default()
    }
}
