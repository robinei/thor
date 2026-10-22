//! The daemon's curve. Nothing in the Thor's kernel drives the fan (its device tree links no thermal zone
//! to it; it runs at the speed it was given at boot), so the daemon sets its speed from the CPU and GPU
//! sensors every [`TICK`], as Armada's armada-powerd does: the mean of the hottest few ([`temperature`]),
//! smoothed ([`smooth`]), on the [`Mode`]'s curve, the speed changing a step at a time ([`next`]).

use std::time::Duration;

use clamshell_core::fan::Mode;

/// How often the daemon reads the sensors and sets the speed.
pub const TICK: Duration = Duration::from_secs(3);

/// The fan's speed as the hardware takes it (hwmon's `pwm1`): a duty cycle, 0 off to 255 full. (The
/// device tree's `cooling-levels` for it stop at 150; nothing uses them, and full speed stays available.)
pub type Duty = u8;

/// Full speed at once, in every mode, at or above this (°C): past the curves' ends, so a step at a time
/// is no longer worth waiting for.
pub const HOT: f32 = 95.0;
/// When no sensor can be read, and when the daemon stops (`clamshell-fan --safe`).
pub const SAFE: Duty = 128;
/// The slowest the fan runs at (Armada's `min_pwm`): starting goes straight to it, slowing down never goes
/// below it but straight to off.
const MIN_RUNNING: Duty = 51;
/// Turned off only this far (°C) below a curve's first point, so that it doesn't start and stop around
/// it. (Off at all is ours: Armada's curves never stop the fan.)
const OFF_BELOW: f32 = 5.0;
/// Speeding up goes at most this much a tick, slowing down at most [`STEP_DOWN`] (Armada's ramp).
const STEP_UP: Duty = 36;
const STEP_DOWN: Duty = 6;
/// The sensors averaged: the hottest this many.
const HOTTEST: usize = 3;
/// How much of a new reading [`smooth`] takes.
const SMOOTHING: f32 = 0.5;

/// Each mode's curve: (°C, duty) points, rising; linear between them, off below the first, full above the
/// last. Auto is Armada's moderate curve; Quiet starts later and stays slower.
const AUTO: [(f32, Duty); 6] = [(55.0, 51), (72.0, 77), (78.0, 102), (84.0, 153), (90.0, 204), (96.0, 255)];
const QUIET: [(f32, Duty); 5] = [(60.0, 51), (78.0, 77), (85.0, 102), (90.0, 153), (95.0, 204)];

/// `mode`'s curve: (°C, duty) points, rising; none for Max (full speed).
fn curve(mode: Mode) -> &'static [(f32, Duty)] {
    match mode {
        Mode::Auto => &AUTO,
        Mode::Quiet => &QUIET,
        Mode::Max => &[],
    }
}

/// The temperature the fan follows, from the CPU and GPU sensors' readings (°C): the mean of the hottest
/// [`HOTTEST`], those believable (-40 to 150 °C) only; none if none is.
pub fn temperature(readings: impl IntoIterator<Item = f32>) -> Option<f32> {
    let mut hot: Vec<f32> = readings.into_iter().filter(|&c| c > -40.0 && c < 150.0).collect();
    hot.sort_by(|a, b| b.total_cmp(a));
    hot.truncate(HOTTEST);
    (!hot.is_empty()).then(|| hot.iter().sum::<f32>() / hot.len() as f32)
}

/// A new [`temperature`] smoothed with the last (so that a spike moves the fan less); none read: none.
pub fn smooth(was: Option<f32>, now: Option<f32>) -> Option<f32> {
    let now = now?;
    Some(was.map_or(now, |was| was + (now - was) * SMOOTHING))
}

/// The speed for the next tick, in `mode` at `temp` (°C, as [`smooth`] gives it; `None`: no sensor could
/// be read), the fan running at `now`.
pub fn next(mode: Mode, temp: Option<f32>, now: Duty) -> Duty {
    let curve = curve(mode);
    let (Some(&(first_t, first)), Some(&(last_t, _))) = (curve.first(), curve.last()) else {
        return Duty::MAX;
    };
    let Some(t) = temp else {
        return SAFE;
    };
    if t >= HOT {
        return Duty::MAX;
    }
    let target = if t >= last_t {
        Duty::MAX
    } else if t < first_t {
        if now > 0 && t >= first_t - OFF_BELOW { first } else { 0 }
    } else {
        curve
            .windows(2)
            .find_map(|w| match *w {
                [(t0, d0), (t1, d1)] if t < t1 => Some(d0 as f32 + (d1 as f32 - d0 as f32) * (t - t0) / (t1 - t0)),
                _ => None,
            })
            .map_or(Duty::MAX, |d| d.round() as Duty)
    };
    if target >= now {
        return target.min(now.saturating_add(STEP_UP).max(MIN_RUNNING));
    }
    let down = now.saturating_sub(STEP_DOWN).max(target);
    if down < MIN_RUNNING { target } else { down }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_mean_of_the_hottest_sensors_smoothed() {
        assert_eq!(temperature([70.0, 40.0, 80.0, 60.0, 500.0]), Some(70.0), "80, 70 and 60; 500 isn't believed");
        assert_eq!(temperature([50.0]), Some(50.0));
        assert_eq!(temperature([f32::NAN, -273.0]), None);
        assert_eq!(smooth(None, Some(60.0)), Some(60.0));
        assert_eq!(smooth(Some(60.0), Some(80.0)), Some(70.0), "half way");
        assert_eq!(smooth(Some(60.0), None), None);
    }

    #[test]
    fn the_curve_between_its_points() {
        // (From a speed it may jump to: the curve itself.)
        assert_eq!(next(Mode::Auto, Some(40.0), 0), 0, "cool: off");
        assert_eq!(next(Mode::Auto, Some(55.0), 0), 51);
        assert_eq!(next(Mode::Auto, Some(81.0), 128), 128, "half way from 102 to 153");
        assert_eq!(next(Mode::Auto, Some(96.0), 255), 255);
        assert_eq!(next(Mode::Quiet, Some(81.0), 88), 88, "quiet: slower (77 to 102 over 78 to 85 °C)");
        assert_eq!(next(Mode::Quiet, Some(58.0), 0), 0);
    }

    #[test]
    fn speeds_up_a_step_at_a_time_but_starts_at_the_slowest() {
        assert_eq!(next(Mode::Auto, Some(90.0), 0), 51, "starting: the slowest");
        assert_eq!(next(Mode::Auto, Some(90.0), 51), 87);
        assert_eq!(next(Mode::Auto, Some(90.0), 200), 204);
        assert_eq!(next(Mode::Auto, Some(HOT), 51), 255, "hot: full at once");
    }

    #[test]
    fn full_when_asked_safe_when_blind() {
        assert_eq!(next(Mode::Max, Some(30.0), 0), 255);
        assert_eq!(next(Mode::Max, None, 0), 255, "max even blind");
        assert_eq!(next(Mode::Quiet, Some(HOT), 0), 255);
        assert_eq!(next(Mode::Auto, None, 0), SAFE);
        assert_eq!(next(Mode::Quiet, None, 255), SAFE, "blind: safe at once, not stepping down");
    }

    #[test]
    fn slows_down_gradually_and_stops_below_the_curve() {
        assert_eq!(next(Mode::Auto, Some(55.0), 200), 194, "a step at a time");
        assert_eq!(next(Mode::Auto, Some(53.0), 51), 51, "just below the first point: still running");
        assert_eq!(next(Mode::Auto, Some(53.0), 0), 0, "and not started there");
        assert_eq!(next(Mode::Auto, Some(49.0), 51), 0, "well below: off");
        assert_eq!(next(Mode::Auto, Some(49.0), 55), 0, "off rather than below the slowest");
        assert_eq!(next(Mode::Auto, Some(49.0), 100), 94);
    }
}
