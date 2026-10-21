//! The sticks calibrated: each axis' centre (where it rests on this controller) and extremes (as far as it
//! goes each way), which vary from unit to unit (PB-OS found the Thor's travel between about 600 and 2000
//! counts from the centre). Raw values are mapped through them to the range the controller reports
//! ([`Sticks::correct`]) before anything reads them, and an extreme passed widens ([`Sticks::widen`]).
//!
//! Calibrating ([`Calibrator`]): the sticks let go (a still second: the centres), then turned around their
//! edges until every axis has gone far each way, then let go again; or given up after a while.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

/// The axes: the left stick's x and y, the right stick's x and y.
pub const AXES: usize = 4;

/// One axis' raw values: as far as it goes down, where it rests, as far as it goes up.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct StickAxis {
    pub low: i32,
    pub centre: i32,
    pub high: i32,
}

/// A controller's sticks, calibrated.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Sticks {
    pub axes: [StickAxis; AXES],
}

/// The calibrations, by controller (its name).
pub type Calibrations = BTreeMap<String, Sticks>;

impl Sticks {
    /// Axis `i`'s raw `value` in the controller's range (`min`..`max`, centred on their middle): the
    /// centre to the middle, each extreme to its end, clamped.
    pub fn correct(&self, i: usize, value: i32, min: i32, max: i32) -> i32 {
        let Some(a) = self.axes.get(i) else { return value };
        let mid = (min + max) / 2;
        let (span, out) = if value >= a.centre { (a.high - a.centre, max - mid) } else { (a.centre - a.low, mid - min) };
        if span <= 0 {
            return value;
        }
        let scaled = mid as i64 + (value - a.centre) as i64 * out as i64 / span as i64;
        scaled.clamp(min as i64, max as i64) as i32
    }

    /// Axis `i` went to `value`: past an extreme, that extreme moves out to it (a stick that goes further
    /// than when it was calibrated). Whether it moved.
    pub fn widen(&mut self, i: usize, value: i32) -> bool {
        let Some(a) = self.axes.get_mut(i) else { return false };
        if value > a.high {
            a.high = value;
        } else if value < a.low {
            a.low = value;
        } else {
            return false;
        }
        true
    }
}

/// Still: every axis within this share of half the controller's range since the last move (40 counts
/// on the Thor's ±1024).
const STILL: f32 = 0.04;
/// Still this long: the centres.
const CENTRE_FOR: u64 = 1_000;
/// An axis has been turned far enough each way: this share of the way from the centre to the end of the
/// controller's range.
const FAR: f32 = 0.5;
/// Then let go: still this long, and it is done.
const DONE_FOR: u64 = 500;
/// Given up after this long.
const GIVE_UP: u64 = 20_000;

/// Where a calibration is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Phase {
    /// Waiting for the sticks to rest (to take the centres).
    LetGo,
    /// The centres taken: turning the sticks around their edges.
    Turn,
    /// Every axis turned far each way: let go to finish.
    Release,
}

/// What a calibration shows while it goes: the phase, the axes as they are, the ranges reached, and the
/// controller's range (to draw them in).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Progress {
    pub phase: Phase,
    pub now: [i32; AXES],
    pub seen: [StickAxis; AXES],
    pub min: i32,
    pub max: i32,
}

/// A calibration going: fed the axes' raw values with their time (ms).
#[derive(Debug, Clone)]
pub struct Calibrator {
    min: i32,
    max: i32,
    started: Option<u64>,
    phase: Phase,
    /// The axes' values since the last move, and since when.
    still: Option<(u64, [i32; AXES], [i32; AXES])>,
    seen: [StickAxis; AXES],
    now: [i32; AXES],
}

/// What a sample did to a calibration.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Step {
    Going(Progress),
    Done(Sticks),
    /// Given up (not let go, or not turned far enough in time).
    Failed,
}

impl Calibrator {
    /// A calibration of sticks whose range is `min`..`max`.
    pub fn new(min: i32, max: i32) -> Calibrator {
        let mid = (min + max) / 2;
        let axis = StickAxis { low: mid, centre: mid, high: mid };
        Calibrator { min, max, started: None, phase: Phase::LetGo, still: None, seen: [axis; AXES], now: [mid; AXES] }
    }

    /// The axes' raw values `now`, at `at` ms.
    pub fn sample(&mut self, now: [i32; AXES], at: u64) -> Step {
        let started = *self.started.get_or_insert(at);
        self.now = now;
        // Still since when: every axis within STILL of its range over that time.
        let still_for = match &mut self.still {
            Some((since, low, high)) => {
                let wide = now.iter().zip(low.iter_mut().zip(high.iter_mut())).any(|(&v, (l, h))| {
                    *l = (*l).min(v);
                    *h = (*h).max(v);
                    (*h - *l) as f32 > (self.max - self.min) as f32 / 2.0 * STILL
                });
                if wide {
                    self.still = Some((at, now, now));
                    0
                } else {
                    at - *since
                }
            }
            None => {
                self.still = Some((at, now, now));
                0
            }
        };
        match self.phase {
            Phase::LetGo if still_for >= CENTRE_FOR => {
                for (a, &v) in self.seen.iter_mut().zip(now.iter()) {
                    *a = StickAxis { low: v, centre: v, high: v };
                }
                self.phase = Phase::Turn;
            }
            Phase::Turn => {
                for (a, &v) in self.seen.iter_mut().zip(now.iter()) {
                    a.low = a.low.min(v);
                    a.high = a.high.max(v);
                }
                let mid = (self.min + self.max) / 2;
                let reach =
                    |a: &StickAxis| (a.high - a.centre) as f32 >= (self.max - mid) as f32 * FAR && (a.centre - a.low) as f32 >= (mid - self.min) as f32 * FAR;
                if self.seen.iter().all(reach) {
                    self.phase = Phase::Release;
                    self.still = Some((at, now, now));
                }
            }
            Phase::Release if still_for >= DONE_FOR => return Step::Done(Sticks { axes: self.seen }),
            _ => {}
        }
        if at.saturating_sub(started) > GIVE_UP {
            return Step::Failed;
        }
        Step::Going(self.progress())
    }

    pub fn progress(&self) -> Progress {
        Progress { phase: self.phase, now: self.now, seen: self.seen, min: self.min, max: self.max }
    }
}

#[cfg(test)]
#[allow(clippy::panic)]
mod tests {
    use super::*;

    #[test]
    fn a_raw_value_maps_through_its_centre_and_extremes() {
        let s = Sticks { axes: [StickAxis { low: -800, centre: 30, high: 900 }; AXES] };
        assert_eq!(s.correct(0, 30, -1024, 1024), 0, "the centre to the middle");
        assert_eq!(s.correct(0, 900, -1024, 1024), 1024, "the high extreme to the end");
        assert_eq!(s.correct(0, -800, -1024, 1024), -1024);
        assert_eq!(s.correct(0, 1300, -1024, 1024), 1024, "past it: clamped");
        assert_eq!(s.correct(0, 465, -1024, 1024), 512, "halfway");
        assert_eq!(s.correct(9, 465, -1024, 1024), 465, "no such axis: as it is");
    }

    #[test]
    fn an_extreme_passed_widens() {
        let mut s = Sticks { axes: [StickAxis { low: -800, centre: 0, high: 800 }; AXES] };
        assert!(s.widen(1, 950));
        assert_eq!(s.axes[1].high, 950);
        assert!(!s.widen(1, 100), "within: nothing");
        assert!(s.widen(1, -900));
        assert_eq!(s.axes[1].low, -900);
    }

    /// Feeds `c` the same values every 10 ms for `ms`, from `at`; the last step.
    fn hold(c: &mut Calibrator, now: [i32; AXES], at: &mut u64, ms: u64) -> Step {
        let mut step = Step::Failed;
        for _ in 0..ms / 10 {
            step = c.sample(now, *at);
            *at += 10;
        }
        step
    }

    #[test]
    fn let_go_turned_around_then_let_go_calibrates() {
        let mut c = Calibrator::new(-1024, 1024);
        let mut at = 0;
        let rest = [20, -15, 5, 0];
        let Step::Going(p) = hold(&mut c, rest, &mut at, 1_100) else { panic!("going") };
        assert_eq!(p.phase, Phase::Turn, "a still second: the centres");
        // Each stick to each side.
        for (x, y) in [(800, 0), (-700, 0), (0, 900), (0, -750)] {
            hold(&mut c, [x, y, x, y], &mut at, 100);
        }
        let Step::Going(p) = hold(&mut c, rest, &mut at, 100) else { panic!("going") };
        assert_eq!(p.phase, Phase::Release, "every axis far each way");
        let Step::Done(s) = hold(&mut c, rest, &mut at, 600) else { panic!("done") };
        assert_eq!(s.axes[0], StickAxis { low: -700, centre: 20, high: 800 });
        assert_eq!(s.axes[1], StickAxis { low: -750, centre: -15, high: 900 });
    }

    #[test]
    fn not_turned_far_enough_in_time_fails() {
        let mut c = Calibrator::new(-1024, 1024);
        let mut at = 0;
        hold(&mut c, [0; AXES], &mut at, 1_100);
        // Only a little each way.
        let mut step = Step::Failed;
        while at < 21_000 {
            step = hold(&mut c, [200, -200, 200, -200], &mut at, 100);
            hold(&mut c, [-200, 200, -200, 200], &mut at, 100);
        }
        assert_eq!(step, Step::Failed);
    }

    #[test]
    fn moving_sticks_keep_it_from_taking_the_centres() {
        let mut c = Calibrator::new(-1024, 1024);
        let mut at = 0;
        for i in 0..200 {
            let v = if i % 2 == 0 { 300 } else { -300 };
            if let Step::Going(p) = c.sample([v, 0, 0, 0], at) {
                assert_eq!(p.phase, Phase::LetGo);
            }
            at += 10;
        }
    }
}
