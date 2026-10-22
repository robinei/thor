//! The sticks' calibration in the pad thread (see `clamshell_core::device::sticks`): the controller's raw stick
//! values corrected through its calibration as they come in ([`Sticks::correct`]; an extreme passed
//! widens it, for this session), before the router or anything else reads them; and a calibration
//! asked for, fed the raw values (the router doesn't get the sticks meanwhile, so turning them doesn't
//! move the focus around), its progress sent about ten times a second, its result kept and sent.

use std::time::{Duration, Instant};

use clamshell_core::device::sticks::{AXES, Calibrator, Step, Sticks};
use evdev::AbsoluteAxisCode;

use super::route::PadSnapshot;
use crate::input::{Range, Raw};

/// The sticks' axes, in the calibration's order: the left stick's x and y, the right stick's.
const STICK_AXES: [AbsoluteAxisCode; AXES] = [AbsoluteAxisCode::ABS_X, AbsoluteAxisCode::ABS_Y, AbsoluteAxisCode::ABS_RX, AbsoluteAxisCode::ABS_RY];
/// A calibration's progress is sent this often.
const PROGRESS_EVERY: Duration = Duration::from_millis(100);
/// While a calibration goes, the pad thread wakes this often to sample the sticks, so the still seconds
/// count when the sticks send nothing.
const SAMPLE_EVERY: Duration = Duration::from_millis(30);

fn index(code: AbsoluteAxisCode) -> Option<usize> {
    STICK_AXES.iter().position(|&c| c == code)
}

/// A controller's sticks: their raw values now, their calibration (if any), one going.
pub struct StickState {
    range: Range,
    raw: [i32; AXES],
    pub sticks: Option<Sticks>,
    calibrating: Option<Calibrating>,
}

/// A calibration going: the calibrator, when it started, and when its progress was last sent.
struct Calibrating {
    calibrator: Calibrator,
    started: Instant,
    reported: Instant,
}

/// What a calibration said.
pub enum Said {
    Progress(clamshell_core::device::sticks::Progress),
    Done(Option<Sticks>),
}

impl StickState {
    /// For a controller whose sticks' range is `range`, its axes now as `snapshot` has them, calibrated as
    /// `sticks` (the snapshot's sticks corrected).
    pub fn new(range: Range, sticks: Option<Sticks>, snapshot: &mut PadSnapshot) -> StickState {
        let raw = STICK_AXES.map(|c| snapshot.value(c).unwrap_or((range.min + range.max) / 2));
        let state = StickState { range, raw, sticks, calibrating: None };
        for (i, &code) in STICK_AXES.iter().enumerate() {
            if let Some(&v) = raw.get(i) {
                snapshot.update(Raw::abs(code, state.corrected(i, v)));
            }
        }
        state
    }

    /// Whether a calibration goes.
    pub fn calibrating(&self) -> bool {
        self.calibrating.is_some()
    }

    fn corrected(&self, i: usize, v: i32) -> i32 {
        self.sticks.map_or(v, |s| s.correct(i, v, self.range.min, self.range.max))
    }

    /// A calibration from now.
    pub fn calibrate(&mut self, now: Instant) {
        self.calibrating = Some(Calibrating { calibrator: Calibrator::new(self.range.min, self.range.max), started: now, reported: now });
    }

    /// The controller's events: its sticks' kept as raw values, and corrected (an extreme passed
    /// widening the calibration); while a calibration goes, taken out (the rest stay).
    pub fn take(&mut self, raws: &mut Vec<Raw>) {
        let calibrating = self.calibrating.is_some();
        raws.retain_mut(|r| {
            let Some((code, value)) = r.as_abs() else { return true };
            let Some(i) = index(code) else { return true };
            if let Some(slot) = self.raw.get_mut(i) {
                *slot = value;
            }
            if calibrating {
                return false;
            }
            if let Some(s) = self.sticks.as_mut() {
                s.widen(i, value);
            }
            *r = Raw::abs(code, self.corrected(i, value));
            true
        });
    }

    /// A calibration going, at `now`: what it says (progress now and then, or its end), and when to look
    /// again.
    pub fn step(&mut self, now: Instant) -> (Option<Said>, Option<Instant>) {
        let Some(Calibrating { calibrator, started, reported }) = self.calibrating.as_mut() else { return (None, None) };
        let ms = u64::try_from(now.saturating_duration_since(*started).as_millis()).unwrap_or(u64::MAX);
        match calibrator.sample(self.raw, ms) {
            Step::Going(p) => {
                let said = (now >= *reported + PROGRESS_EVERY).then(|| {
                    *reported = now;
                    Said::Progress(p)
                });
                (said, Some(now + SAMPLE_EVERY))
            }
            Step::Done(sticks) => {
                self.calibrating = None;
                self.sticks = Some(sticks);
                (Some(Said::Done(Some(sticks))), None)
            }
            Step::Failed => {
                self.calibrating = None;
                (Some(Said::Done(None)), None)
            }
        }
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::indexing_slicing)]
mod tests {
    use super::*;
    use clamshell_core::device::sticks::StickAxis;

    fn state(sticks: Option<Sticks>) -> StickState {
        let axes = STICK_AXES.iter().map(|&code| super::super::route::Axis { code, value: 0, rest: 0 }).collect();
        StickState::new(Range { min: -1024, max: 1024 }, sticks, &mut PadSnapshot::new(axes))
    }

    #[test]
    fn stick_values_are_corrected_and_the_rest_left_alone() {
        let sticks = Sticks { axes: [StickAxis { low: -800, centre: 20, high: 820 }; AXES] };
        let mut s = state(Some(sticks));
        let mut raws = vec![Raw::abs(AbsoluteAxisCode::ABS_X, 820), Raw::abs(AbsoluteAxisCode::ABS_Z, 300), Raw::abs(AbsoluteAxisCode::ABS_RY, 20)];
        s.take(&mut raws);
        assert_eq!(raws, vec![Raw::abs(AbsoluteAxisCode::ABS_X, 1024), Raw::abs(AbsoluteAxisCode::ABS_Z, 300), Raw::abs(AbsoluteAxisCode::ABS_RY, 0)]);
        // Past the extreme: it widens (the next value there maps to the end).
        let mut raws = vec![Raw::abs(AbsoluteAxisCode::ABS_X, 900)];
        s.take(&mut raws);
        assert_eq!(s.sticks.unwrap().axes[0].high, 900);
    }

    #[test]
    fn uncalibrated_values_pass_as_they_are() {
        let mut s = state(None);
        let mut raws = vec![Raw::abs(AbsoluteAxisCode::ABS_X, 777)];
        s.take(&mut raws);
        assert_eq!(raws, vec![Raw::abs(AbsoluteAxisCode::ABS_X, 777)]);
    }

    #[test]
    fn a_calibration_takes_the_sticks_from_the_router_and_reports_its_end() {
        let mut s = state(None);
        let t0 = Instant::now();
        s.calibrate(t0);
        let mut raws = vec![Raw::abs(AbsoluteAxisCode::ABS_X, 10), Raw::abs(AbsoluteAxisCode::ABS_Z, 300)];
        s.take(&mut raws);
        assert_eq!(raws, vec![Raw::abs(AbsoluteAxisCode::ABS_Z, 300)], "the sticks the calibration's alone");
        // Let go a second, each stick to each side, let go again.
        let mut at = t0;
        let mut feed = |s: &mut StickState, v: [i32; 4], ms: u64| -> Option<Said> {
            let mut last = None;
            for _ in 0..ms / 30 {
                let mut raws: Vec<Raw> = STICK_AXES.iter().zip(v).map(|(&c, v)| Raw::abs(c, v)).collect();
                s.take(&mut raws);
                at += Duration::from_millis(30);
                if let (Some(said), _) = s.step(at) {
                    last = Some(said);
                }
            }
            last
        };
        feed(&mut s, [0; 4], 1_100);
        for (x, y) in [(800, 0), (-800, 0), (0, 800), (0, -800)] {
            feed(&mut s, [x, y, x, y], 150);
        }
        let said = feed(&mut s, [0; 4], 700);
        assert!(matches!(said, Some(Said::Done(Some(_)))), "calibrated");
        assert!(!s.calibrating());
        assert!(s.sticks.is_some(), "and kept for the controller");
    }
}
