//! What a list does after the finger that dragged it lets go: flings on at the finger's speed, slowing
//! down with friction until slow or at an end; a list that rests on whole rows (shelf) then settles on
//! the nearest one with a short ease-out. Also the finger's speed as it let go ([`Trail`]). Pure: the
//! caller steps it on a timer ([`FLING_EVERY`]) and scrolls the list by what each step says.

use std::time::{Duration, Instant};

use clamshell_core::{ListId, State};

use crate::views::scroll::InList;

/// A step: about a frame.
pub(crate) const FLING_EVERY: Duration = Duration::from_millis(16);
/// A fling slows down by this much each second (its speed times e^(-t/τ), τ = 1 / this).
const FRICTION: f32 = 3.0;
/// Let go faster than this (rows per second), a drag flings on; a fling slower than the stop speed ends.
const START: f32 = 2.0;
const STOP: f32 = 1.0;
/// How far back the speed at the release is measured.
const WINDOW: Duration = Duration::from_millis(100);
/// A list settling on a whole row moves this much of the way each second (an ease-out), done once
/// within [`SETTLED`] of it.
const SETTLE_RATE: f32 = 14.0;
const SETTLED: f32 = 0.01;

/// A finger's last positions along y, newest first.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct Trail([Option<(Instant, f32)>; 4]);

impl Trail {
    pub fn new(now: Instant, y: f32) -> Trail {
        Trail([Some((now, y)), None, None, None])
    }

    pub fn follow(&mut self, now: Instant, y: f32) {
        self.0.rotate_right(1);
        self.0[0] = Some((now, y));
    }

    /// The finger's speed along y (pixels per second) over the last moments before `now`.
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
    /// A drag of `list` ends at `now` with the finger at `trail` (`released`: let go, not cancelled): it
    /// flings on if the finger moved fast enough, else a list on whole rows settles; else nothing.
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

    /// A step at `now`: the rows to scroll by, and whether it goes on. A fling moves by its speed for
    /// the time since the last step and slows down; once slow or at an end it is done, or on whole rows
    /// settles on the row it comes nearest to. A settling list moves part of the way, the rest once close.
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
                // (At an end it can't reach the row: done.)
                if left.abs() <= SETTLED || !state.list_can_scroll(list, left) { (left, false) } else { (left * (SETTLE_RATE * dt).min(1.0), true) }
            }
        }
    }
}
