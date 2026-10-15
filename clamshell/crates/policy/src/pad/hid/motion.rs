//! Where the virtual HID pads' gyro and accelerometer readings come from: a [`MotionSource`]. For
//! now only [`Level`], a device lying level at rest, until the Thor's own sensor is known (an IIO
//! device under /sys/bus/iio, or an evdev node with INPUT_PROP_ACCELEROMETER): a source reading it goes
//! here, implementing the same trait, and is picked in [`default_source`].

use std::time::Duration;

/// One reading, in the DualSense's frame: x to the right, y up out of the face, z towards the player.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Motion {
    /// Rotation in degrees per second about x (pitch), y (yaw) and z (roll).
    pub gyro: [f32; 3],
    /// Acceleration in g, gravity included: lying face up, at rest, y is +1.
    pub accel: [f32; 3],
}

impl Motion {
    /// At rest, lying level face up.
    pub const LEVEL: Motion = Motion { gyro: [0.0; 3], accel: [0.0, 1.0, 0.0] };
}

/// A motion sensor, read for each input report.
pub trait MotionSource: Send {
    /// The reading now.
    fn read(&mut self) -> Motion;

    /// How often the pad should report readings while nothing else changes; `None`: only with the
    /// pad's own changes (a source whose readings never change).
    fn every(&self) -> Option<Duration>;
}

/// A device lying level, at rest: no rotation, 1 g down.
#[derive(Debug, Clone, Copy, Default)]
pub struct Level;

impl MotionSource for Level {
    fn read(&mut self) -> Motion {
        Motion::LEVEL
    }

    fn every(&self) -> Option<Duration> {
        None
    }
}

/// The source the virtual HID pads use.
pub fn default_source() -> Box<dyn MotionSource> {
    Box::new(Level)
}
