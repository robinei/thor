//! The analog side of the keymap: speeds from the binding's 0-100, the sticks' tuning (deadzone, full
//! point, curve, inversion), and the gyro's turning, tightening and acceleration.

use clamshell_state::keymap::Binding;

use super::super::motion::Rate;

/// Pointer speed of a stick at full deflection in pixels per second, at speeds 0, 25, 50, 75 and 100.
/// Speeds in between are in proportion. Below full deflection, the binding's deadzone and curve apply.
pub(super) const STICK_POINTER: [f32; 5] = [500.0, 900.0, 1400.0, 2100.0, 3000.0];
/// The gyro on the pointer: pixels per degree turned, at the same speeds.
pub(super) const GYRO_POINTER: [f32; 5] = [6.0, 10.0, 15.0, 22.0, 32.0];
/// The gyro on a stick: degrees per second for full deflection, at the same speeds.
pub(super) const GYRO_STICK: [f32; 5] = [240.0, 160.0, 100.0, 60.0, 35.0];
/// Which way tilting the device like a steering wheel (roll) turns compared with rotating it (yaw), when
/// a binding turns by tilting. The Thor's sign is still to be tested on the device
/// (docs/thor-gameplan.md).
pub(super) const ROLL_TURNS: f32 = 1.0;
/// The value for a speed from 0 to 100 in a table of five values, for speeds 0, 25, 50, 75 and 100.
/// Speeds in between are in proportion.
pub(super) fn step(table: &[f32; 5], speed: u8) -> f32 {
    let at = f32::from(speed.min(100)) / 25.0;
    let i = (at.floor() as usize).min(3);
    let (Some(a), Some(b)) = (table.get(i), table.get(i + 1)) else {
        return 1.0;
    };
    a + (b - a) * (at - i as f32)
}

/// A value from 0 to 1 with its deadzone and full point applied, both in percent. It is 0 up to the
/// deadzone, 1 from the full point, and stretched in between.
pub(super) fn range(v: f32, deadzone: u8, full: u8) -> f32 {
    let (dz, full) = (f32::from(deadzone) / 100.0, f32::from(full.max(deadzone.saturating_add(1))) / 100.0);
    ((v - dz) / (full - dz).max(0.01)).clamp(0.0, 1.0)
}

/// A stick's position, each axis -1 to 1 with y down, tuned by its binding. The axes are inverted as set,
/// and the distance from the middle goes through the deadzone, the full point and the curve. The
/// direction is kept.
pub(super) fn tune_stick((x, y): (f32, f32), b: &Binding) -> (f32, f32) {
    let (x, y) = (if b.invert_x { -x } else { x }, if b.invert_y { -y } else { y });
    if b.deadzone == 0 && b.full >= 100 && b.curve == 0 {
        return (x, y);
    }
    let len = x.hypot(y);
    if len <= 0.0 {
        return (0.0, 0.0);
    }
    let tuned = range(len.min(1.0), b.deadzone, b.full).powf(1.0 + 2.0 * f32::from(b.curve) / 100.0);
    (x / len * tuned, y / len * tuned)
}

/// The signs for the gyro's motion across and up/down, inverted as its binding says.
pub(super) fn gyro_signs(b: &Binding) -> (f32, f32) {
    (if b.invert_x { -1.0 } else { 1.0 }, if b.invert_y { -1.0 } else { 1.0 })
}

/// The gyro's turning in degrees per second, inverted as the binding says, and tightened. Across is yaw,
/// plus roll when the binding turns by tilting, and up and down is pitch. Turning slower than the
/// tightening is scaled down in proportion, so half the tightening turns at a quarter. That way a hand's
/// tremor fades without a hard edge.
pub(super) fn gyro_turn(b: &Binding, r: Rate) -> (f32, f32) {
    let (sx, sy) = gyro_signs(b);
    let across = if b.twist { r.yaw + ROLL_TURNS * r.roll } else { r.yaw };
    let (x, y) = (sx * across, sy * r.pitch);
    let tight = f32::from(b.tighten) / 10.0;
    let speed = x.hypot(y);
    let k = if tight > 0.0 && speed < tight { speed / tight } else { 1.0 };
    (x * k, y * k)
}

/// The gyro's acceleration: 1 for slow turns, rising to 3 at 100 % for turns as fast as its "fastest
/// at".
pub(super) fn gyro_gain(b: &Binding, (x, y): (f32, f32)) -> f32 {
    let at = f32::from(b.accel_at.max(1));
    1.0 + 2.0 * f32::from(b.accel) / 100.0 * (x.hypot(y) / at).min(1.0)
}

/// A stick's deflection, each axis -1 to 1, kept within the circle and starting at `min` percent. That
/// way a game's own stick deadzone doesn't swallow small motion.
pub(super) fn stick_from(x: f32, y: f32, min: u8) -> (f32, f32) {
    let m = x.hypot(y);
    if m <= 0.0 {
        return (0.0, 0.0);
    }
    let min = f32::from(min) / 100.0;
    let to = min + (1.0 - min) * m.min(1.0);
    (x / m * to, y / m * to)
}
