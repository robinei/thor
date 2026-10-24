//! The daemon's fan curve.
//!
//! Nothing in the Thor's kernel drives the fan. Its device tree links no thermal zone to it, so it keeps
//! the speed it was given at boot. The daemon therefore sets the speed from the CPU and GPU sensors every
//! [`TICK`], as Armada's armada-powerd does. It takes the mean of the hottest few sensors
//! ([`temperature`]), smooths it ([`smooth`]), looks it up on the [`Mode`]'s curve, and changes the speed
//! a step at a time ([`next`]).

use std::time::Duration;

use clamshell_device::fan::Fan;
use clamshell_fan_mode::Mode;

/// How often the daemon reads the sensors and sets the speed.
pub const TICK: Duration = Duration::from_secs(3);

/// The fan's speed as the hardware takes it in hwmon's `pwm1`: a duty cycle from 0 (off) to 255 (full).
/// The device tree's `cooling-levels` for the fan stop at 150, but nothing uses them and full speed is
/// still available.
pub type Duty = u8;

/// How many of the hottest sensors are averaged.
const HOTTEST: usize = 3;
/// How much of a new reading [`smooth`] takes.
const SMOOTHING: f32 = 0.5;

/// Each mode selects a curve from the board policy. Max always runs at full speed.
fn curve(fan: &Fan, mode: Mode) -> &'static [(u16, Duty)] {
    match mode {
        Mode::Auto => fan.auto,
        Mode::Quiet => fan.quiet,
        Mode::Max => &[],
    }
}

/// The temperature the fan follows, from the CPU and GPU sensor readings in °C. It is the mean of the
/// [`HOTTEST`] hottest believable readings, those between -40 and 150 °C. None if no reading is believable.
pub fn temperature(readings: impl IntoIterator<Item = f32>) -> Option<f32> {
    let mut hot: Vec<f32> = readings.into_iter().filter(|&c| c > -40.0 && c < 150.0).collect();
    hot.sort_by(|a, b| b.total_cmp(a));
    hot.truncate(HOTTEST);
    (!hot.is_empty()).then(|| hot.iter().sum::<f32>() / hot.len() as f32)
}

/// A new [`temperature`] smoothed with the previous one, so that a spike moves the fan less. None if
/// nothing was read.
pub fn smooth(was: Option<f32>, now: Option<f32>) -> Option<f32> {
    let now = now?;
    Some(was.map_or(now, |was| was + (now - was) * SMOOTHING))
}

/// The speed for the next tick, in `mode`, with the fan running at `now`. `temp` is in °C as [`smooth`]
/// gives it, or `None` if no sensor could be read.
pub fn next(fan: &Fan, mode: Mode, temp: Option<f32>, now: Duty) -> Duty {
    let curve = curve(fan, mode);
    let (Some(&(first_t, first)), Some(&(last_t, _))) = (curve.first(), curve.last()) else {
        return Duty::MAX;
    };
    let Some(t) = temp else {
        return fan.safe_duty;
    };
    if t >= f32::from(fan.emergency_celsius) {
        return Duty::MAX;
    }
    let (first_t, last_t) = (f32::from(first_t), f32::from(last_t));
    let target = if t >= last_t {
        Duty::MAX
    } else if t < first_t {
        if now > 0 && t >= first_t - f32::from(fan.off_below) { first } else { 0 }
    } else {
        curve
            .windows(2)
            .find_map(|w| match *w {
                [(t0, d0), (t1, d1)] if t < f32::from(t1) => Some(d0 as f32 + (d1 as f32 - d0 as f32) * (t - f32::from(t0)) / (f32::from(t1) - f32::from(t0))),
                _ => None,
            })
            .map_or(Duty::MAX, |d| d.round() as Duty)
    };
    if target >= now {
        return target.min(now.saturating_add(fan.step_up).max(fan.min_running));
    }
    let down = now.saturating_sub(fan.step_down).max(target);
    if down < fan.min_running { target } else { down }
}

#[cfg(test)]
mod tests {
    use super::*;
    const HOT: f32 = Fan::thor().emergency_celsius as f32;
    const SAFE: Duty = Fan::thor().safe_duty;
    fn next(mode: Mode, temp: Option<f32>, now: Duty) -> Duty {
        super::next(&Fan::thor(), mode, temp, now)
    }

    #[test]
    fn a_different_board_policy_changes_interpolation_and_fallback_without_device_checks() {
        let fan = Fan { auto: &[(10, 100), (20, 200)], safe_duty: 222, emergency_celsius: 70, min_running: 1, step_up: 255, step_down: 255, ..Fan::thor() };
        assert_eq!(super::next(&fan, Mode::Auto, Some(15.0), 0), 150);
        assert_eq!(super::next(&fan, Mode::Auto, None, 0), 222);
        assert_eq!(super::next(&fan, Mode::Auto, Some(70.0), 0), 255);
    }
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
        // Starting from a speed it can reach in one step, so the result is the curve itself.
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
