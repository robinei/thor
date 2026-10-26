//! Calibrating the sticks. The sticks are let go, and a still second gives the centres. Then they are
//! turned around their edges until every axis has gone far each way, and then let go again. If that
//! doesn't happen in time, the calibration is given up. The result is a
//! `clamshell_state::device::sticks::Sticks`.

use clamshell_state::device::sticks::{AXES, Phase, Progress, StickAxis, Sticks};

/// The sticks count as still when every axis has stayed within this fraction of half the controller's
/// range since the last move. That is 40 counts on the Thor's ±1024.
const STILL: f32 = 0.04;
/// How long, in ms, the sticks must be still to take the centres.
const CENTRE_FOR: u64 = 1_000;
/// How far each way an axis must be turned, as a fraction of the way from the centre to the end of the
/// controller's range.
const FAR: f32 = 0.5;
/// After turning, the sticks are let go. Once they are still this long, in ms, it is done.
const DONE_FOR: u64 = 500;
/// The calibration is given up after this long, in ms.
const GIVE_UP: u64 = 20_000;

/// A running calibration. It is fed the axes' raw values with their time in ms.
#[derive(Debug, Clone)]
pub(crate) struct Calibrator {
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
pub(crate) enum Step {
    Going(Progress),
    Done(Sticks),
    /// Given up, because the sticks weren't let go or weren't turned far enough in time.
    Failed,
}

impl Calibrator {
    /// A calibration of sticks whose range is `min`..`max`.
    pub(crate) fn new(min: i32, max: i32) -> Calibrator {
        let mid = (min + max) / 2;
        let axis = StickAxis { low: mid, centre: mid, high: mid };
        Calibrator { min, max, started: None, phase: Phase::LetGo, still: None, seen: [axis; AXES], now: [mid; AXES] }
    }

    /// The axes' raw values `now`, at `at` ms.
    pub(crate) fn sample(&mut self, now: [i32; AXES], at: u64) -> Step {
        let started = *self.started.get_or_insert(at);
        self.now = now;
        // How long the sticks have been still, meaning every axis within STILL of its range over that
        // time.
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
                self.widen(now);
                let mid = (self.min + self.max) / 2;
                let reach =
                    |a: &StickAxis| (a.high - a.centre) as f32 >= (self.max - mid) as f32 * FAR && (a.centre - a.low) as f32 >= (mid - self.min) as f32 * FAR;
                if self.seen.iter().all(reach) {
                    self.phase = Phase::Release;
                    self.still = Some((at, now, now));
                }
            }
            // Still widening, since the turning goes on past halfway, out to the edges.
            Phase::Release if still_for >= DONE_FOR => return Step::Done(Sticks { axes: self.seen }),
            Phase::Release => self.widen(now),
            _ => {}
        }
        if at.saturating_sub(started) > GIVE_UP {
            return Step::Failed;
        }
        Step::Going(self.progress())
    }

    /// Widens the extremes reached out to `now`.
    fn widen(&mut self, now: [i32; AXES]) {
        for (a, &v) in self.seen.iter_mut().zip(now.iter()) {
            a.low = a.low.min(v);
            a.high = a.high.max(v);
        }
    }

    fn progress(&self) -> Progress {
        Progress { phase: self.phase, now: self.now, seen: self.seen, min: self.min, max: self.max }
    }
}

#[cfg(test)]
#[allow(clippy::panic)]
mod tests {
    use super::*;

    /// Feeds `c` the same values every 10 ms for `ms`, starting at `at`. Returns the last step.
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
        let Step::Going(p) = hold(&mut c, [900, 0, 900, 0], &mut at, 100) else { panic!("going") };
        assert_eq!(p.phase, Phase::Release, "every axis far each way");
        assert_eq!(p.seen[0].high, 900, "and still widening past it");
        hold(&mut c, [800, 0, 800, 0], &mut at, 100);
        let Step::Done(s) = hold(&mut c, rest, &mut at, 600) else { panic!("done") };
        assert_eq!(s.axes[0], StickAxis { low: -700, centre: 20, high: 900 });
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
