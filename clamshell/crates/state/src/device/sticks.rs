//! Stick calibration. For each axis it records the centre, where the axis rests on this controller, and
//! the extremes, how far it goes each way. These vary from unit to unit: PB-OS found the Thor's travel
//! between about 600 and 2000 counts from the centre. Raw values are mapped through them to the range the
//! controller reports ([`Sticks::correct`]) before anything reads them. When an axis goes past an
//! extreme, the extreme widens ([`Sticks::widen`]).
//!
//! The pad thread runs a calibration (`clamshell_system`'s `input::pad::calibrator`) and sends its
//! [`Progress`] to show and the [`Sticks`] it ends with.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

/// The number of axes: the left stick's x and y, and the right stick's x and y.
pub const AXES: usize = 4;

/// One axis' raw values: how far it goes down, where it rests, and how far it goes up.
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

/// The calibrations, by controller name.
pub type Calibrations = BTreeMap<String, Sticks>;

impl Sticks {
    /// Maps axis `i`'s raw `value` into the controller's range, `min`..`max`, centred on its middle. The
    /// centre maps to the middle and each extreme to its end, and the result is clamped.
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

    /// Axis `i` went to `value`. If that is past an extreme, the extreme moves out to it, for a stick that
    /// goes further than when it was calibrated. Returns whether it moved.
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

/// Where a calibration is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Phase {
    /// Waiting for the sticks to rest, to take the centres.
    LetGo,
    /// The centres are taken, and the sticks are being turned around their edges.
    Turn,
    /// Every axis has been turned far each way. Let go to finish.
    Release,
}

/// What a calibration shows while it runs: the phase, the axes as they are, the ranges reached, and the
/// controller's range to draw them in.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Progress {
    pub phase: Phase,
    pub now: [i32; AXES],
    pub seen: [StickAxis; AXES],
    pub min: i32,
    pub max: i32,
}

#[cfg(test)]
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
}
