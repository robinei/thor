//! What a list does after the finger dragging it lets go. It flings on at the finger's speed and slows
//! down with friction until it is slow or reaches an end. A list that rests on whole rows, such as the
//! shelf, then settles on the nearest row with a short ease-out. [`Trail`] measures the finger's speed as
//! it lets go.
//!
//! The code is pure: the caller steps it on a timer ([`FLING_EVERY`]) and scrolls the list by what each
//! step returns.

use std::time::{Duration, Instant};

use clamshell_core::{ListId, State};

use crate::toolkit::scroll::InList;

/// How often a fling steps: about once a frame.
pub(crate) const FLING_EVERY: Duration = Duration::from_millis(16);
/// How fast a fling slows down. The speed after t seconds is multiplied by e^(-t/τ), where τ = 1 / this.
const FRICTION: f32 = 3.0;
/// A drag let go faster than `START`, in rows per second, flings on. A fling slower than `STOP` ends.
const START: f32 = 2.0;
const STOP: f32 = 1.0;
/// How far back the speed at the release is measured.
const WINDOW: Duration = Duration::from_millis(100);
/// A list settling on a whole row moves this much of the remaining way each second, as an ease-out. It
/// is done once it is within [`SETTLED`] of the row.
const SETTLE_RATE: f32 = 14.0;
const SETTLED: f32 = 0.01;

/// Positions in a trail are at least this far apart, except for the newest. Events come in bursts, and
/// this keeps a speed from being measured over a single moment.
const SAMPLE_EVERY: Duration = Duration::from_millis(10);

/// A finger's last positions along y, newest first, [`SAMPLE_EVERY`] apart.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct Trail([Option<(Instant, f32)>; 8]);

impl Trail {
    pub fn new(now: Instant, y: f32) -> Trail {
        let mut trail = Trail([None; 8]);
        trail.0[0] = Some((now, y));
        trail
    }

    /// Records the finger at `y` at `now`. This adds a new position, unless it is too soon after the
    /// position before the newest; then the newest is moved here instead.
    pub fn follow(&mut self, now: Instant, y: f32) {
        let soon = self.0[1].is_some_and(|(t, _)| now.duration_since(t) < SAMPLE_EVERY);
        if !soon {
            self.0.rotate_right(1);
        }
        self.0[0] = Some((now, y));
    }

    /// The finger's speed along y, in pixels per second, over the last moments before `now`.
    pub fn speed(&self, now: Instant) -> f32 {
        let mut recent = self.0.iter().flatten().filter(|(t, _)| now.duration_since(*t) <= WINDOW);
        let (Some(newest), Some(oldest)) = (recent.clone().next(), recent.next_back()) else {
            return 0.0;
        };
        let dt = newest.0.duration_since(oldest.0).as_secs_f32();
        if dt <= 0.0 { 0.0 } else { (newest.1 - oldest.1) / dt }
    }
}

/// A list moving on by itself, and when it last stepped.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct Fling {
    pub list: ListId,
    motion: Motion,
    at: Instant,
}

#[derive(Debug, Clone, Copy, PartialEq)]
enum Motion {
    /// Rows per second.
    Speed(f32),
    /// The row offset it settles on.
    SettleOn(f32),
}

impl Fling {
    /// Called when a drag of `list` ends at `now`, with the finger's positions in `trail`. `released` is
    /// true if the finger let go rather than the drag being cancelled. The list flings on if the finger
    /// was let go fast enough. Otherwise a list on whole rows settles, and any other list does nothing.
    pub fn after_drag(list: InList, trail: &Trail, released: bool, state: &State, now: Instant) -> Option<Fling> {
        // The finger up moves the list down.
        let speed = -trail.speed(now) / list.row_h;
        let motion = if released && speed.abs() >= START {
            Motion::Speed(speed)
        } else if list.list.whole_rows() {
            Motion::SettleOn(state.list_offset(list.list).round())
        } else {
            return None;
        };
        Some(Fling { list: list.list, motion, at: now })
    }

    /// A step at `now`. Returns the rows to scroll by and whether the motion goes on.
    ///
    /// A fling moves by its speed for the time since the last step, then slows down. Once it is slow or at
    /// an end, it is done; a list on whole rows then settles on the row it came nearest to. A settling
    /// list moves part of the way each step, and the rest once it is close.
    pub fn step(&mut self, state: &State, now: Instant) -> (f32, bool) {
        let dt = now.duration_since(self.at).as_secs_f32();
        self.at = now;
        let list = self.list;
        let offset = state.list_offset(list);
        match &mut self.motion {
            Motion::Speed(speed) => {
                let rows = *speed * dt;
                *speed *= (-FRICTION * dt).exp();
                let done = speed.abs() < STOP || !state.list_can_scroll(list, rows);
                if done && list.whole_rows() {
                    self.motion = Motion::SettleOn((offset + rows).round());
                    return (rows, true);
                }
                (rows, !done)
            }
            Motion::SettleOn(to) => {
                let left = *to - offset;
                // At an end the list can't reach the row, so it is done.
                if left.abs() <= SETTLED || !state.list_can_scroll(list, left) { (left, false) } else { (left * (SETTLE_RATE * dt).min(1.0), true) }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_burst_of_positions_is_not_a_speed() {
        let t = Instant::now();
        let ms = |n: u64| t + Duration::from_millis(n);
        // A steady 1000 px/s, then a burst of events of the same motion, handled at once 1 ms apart.
        let mut trail = Trail::new(t, 0.0);
        for n in 1..=5 {
            trail.follow(ms(n * 10), n as f32 * 10.0);
        }
        for n in 1..=4 {
            trail.follow(ms(50 + n), 50.0 + n as f32);
        }
        let speed = trail.speed(ms(54));
        assert!((800.0..1200.0).contains(&speed), "{speed}");
    }
}
