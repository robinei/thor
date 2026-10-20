//! The fan. Nothing in the Thor's kernel drives it (its device tree links no thermal zone to it; it runs
//! at the speed it was given at boot), so the fan daemon does: `clamshell-fan`, a system service started
//! early in boot, sets its speed from the hottest CPU or GPU sensor every [`TICK`]. ClamShell's Fan
//! setting picks the [`Mode`], written to [`MODE_FILE`] (which the image makes writable by the session's
//! user); without one, the daemon runs [`Mode::Auto`].

use std::time::Duration;

/// Where the mode is written: its name, as [`Mode::name`] has it.
pub const MODE_FILE: &str = "/run/clamshell-fan";
/// How often the daemon reads the sensors and sets the speed.
pub const TICK: Duration = Duration::from_secs(3);

/// The fan's speed as the hardware takes it (hwmon's `pwm1`): a duty cycle, 0 off to 255 full.
pub type Duty = u8;

/// Full speed in every mode at or above this (°C).
pub const HOT: f32 = 90.0;
/// When no sensor can be read.
pub const SAFE: Duty = 128;
/// The slowest the fan runs at (Armada's `min_pwm`): slowing down never goes below it but straight to off.
const MIN_RUNNING: Duty = 51;
/// Turned off only this far (°C) below a curve's first point, so that it doesn't start and stop around it.
const OFF_BELOW: f32 = 5.0;
/// Slowing down goes at most this much a tick (speeding up goes straight to the curve).
const STEP_DOWN: Duty = 8;

/// Each mode's curve: (°C, duty) points, rising; linear between them, off below the first, full above the
/// last. Auto is near Thorch's and Armada's moderate curves (theirs start at 45-55 °C); Quiet starts later
/// and stays slower.
const AUTO: [(f32, Duty); 5] = [(50.0, 51), (60.0, 77), (70.0, 128), (80.0, 204), (85.0, 255)];
const QUIET: [(f32, Duty); 4] = [(60.0, 51), (70.0, 77), (80.0, 153), (88.0, 255)];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    Auto,
    Quiet,
    Max,
}

impl Mode {
    /// [`crate::settings::Setting::Fan`]'s value: 0 auto, 1 quiet, 2 max.
    pub fn of(value: u16) -> Mode {
        match value {
            0 => Mode::Auto,
            1 => Mode::Quiet,
            _ => Mode::Max,
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Mode::Auto => "auto",
            Mode::Quiet => "quiet",
            Mode::Max => "max",
        }
    }

    /// A mode file's contents; anything else: none.
    pub fn parse(text: &str) -> Option<Mode> {
        [Mode::Auto, Mode::Quiet, Mode::Max].into_iter().find(|m| m.name() == text.trim())
    }

    fn curve(self) -> &'static [(f32, Duty)] {
        match self {
            Mode::Auto => &AUTO,
            Mode::Quiet => &QUIET,
            Mode::Max => &[],
        }
    }
}

/// The speed for the next tick, in `mode` at `temp` (the hottest sensor, °C; `None`: none could be read),
/// the fan running at `now`.
pub fn next(mode: Mode, temp: Option<f32>, now: Duty) -> Duty {
    let Some(t) = temp else {
        return SAFE;
    };
    let curve = mode.curve();
    let (Some(&(first_t, first)), Some(&(last_t, _))) = (curve.first(), curve.last()) else {
        return Duty::MAX;
    };
    let target = if t >= HOT || t >= last_t {
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
        return target;
    }
    let down = now.saturating_sub(STEP_DOWN).max(target);
    if down < MIN_RUNNING { target } else { down }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn modes_by_setting_and_by_name() {
        assert_eq!(Mode::of(0), Mode::Auto);
        assert_eq!(Mode::of(1), Mode::Quiet);
        assert_eq!(Mode::of(2), Mode::Max);
        for mode in [Mode::Auto, Mode::Quiet, Mode::Max] {
            assert_eq!(Mode::parse(&format!("{}\n", mode.name())), Some(mode));
        }
        assert_eq!(Mode::parse(""), None);
    }

    #[test]
    fn the_curve_between_its_points() {
        assert_eq!(next(Mode::Auto, Some(40.0), 0), 0, "cool: off");
        assert_eq!(next(Mode::Auto, Some(50.0), 0), 51);
        assert_eq!(next(Mode::Auto, Some(65.0), 0), 103, "half way from 77 to 128");
        assert_eq!(next(Mode::Auto, Some(85.0), 0), 255);
        assert_eq!(next(Mode::Quiet, Some(65.0), 0), 64, "quiet: slower");
        assert_eq!(next(Mode::Quiet, Some(55.0), 0), 0);
    }

    #[test]
    fn full_when_hot_or_asked_safe_when_blind() {
        assert_eq!(next(Mode::Max, Some(30.0), 0), 255);
        assert_eq!(next(Mode::Quiet, Some(HOT), 0), 255);
        assert_eq!(next(Mode::Auto, None, 0), SAFE);
        assert_eq!(next(Mode::Quiet, None, 255), SAFE, "blind: safe at once, not stepping down");
    }

    #[test]
    fn slows_down_gradually_and_stops_below_the_curve() {
        assert_eq!(next(Mode::Auto, Some(50.0), 200), 192, "a step at a time");
        assert_eq!(next(Mode::Auto, Some(48.0), 51), 51, "just below the first point: still running");
        assert_eq!(next(Mode::Auto, Some(48.0), 0), 0, "and not started there");
        assert_eq!(next(Mode::Auto, Some(44.0), 51), 0, "well below: off");
        assert_eq!(next(Mode::Auto, Some(44.0), 55), 0, "off rather than below the slowest");
        assert_eq!(next(Mode::Auto, Some(44.0), 100), 92);
    }
}
