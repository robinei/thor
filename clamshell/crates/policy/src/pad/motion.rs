//! The gyro: where the pad thread gets angular velocity from ([`MotionSource`]: a file descriptor to wait
//! on, and samples of degrees per second on three axes with their time). The one source so far is an
//! evdev motion sensor ([`EvdevMotion`]), as the kernel's controller drivers expose their IMU: a device
//! with INPUT_PROP_ACCELEROMETER, the gyro on ABS_RX/RY/RZ with its resolution (units per degree per
//! second) in the axis' info, and MSC_TIMESTAMP (microseconds) with each report. Found next to the
//! controller: the motion device whose name or physical path pairs with the controller's, else the only
//! motion sensor there is. (The Thor's own sensor, IIO or evdev, is unknown until it is in hand; it
//! plugs in here.) The motion device isn't grabbed: nothing else reads it as input, and the idle watch
//! ignores accelerometers.

use std::io;
use std::os::fd::{AsFd, BorrowedFd};
use std::time::Duration;

use evdev::raw_stream::RawDevice;
use evdev::{AbsoluteAxisCode, EventType, MiscCode, PropType};

/// Angular velocity in degrees per second, about the device's axes held as a handheld in landscape:
/// pitch about the left-right axis (ABS_RX), yaw about the up axis (ABS_RY), roll about the axis
/// toward the player (ABS_RZ).
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct Rate {
    pub pitch: f32,
    pub yaw: f32,
    pub roll: f32,
}

/// One reading: the rate and when it was taken (the sensor's clock).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Sample {
    pub rate: Rate,
    pub at: Duration,
}

/// Where angular velocity comes from.
pub trait MotionSource: Send {
    /// To wait on for samples.
    fn fd(&self) -> BorrowedFd<'_>;

    /// Appends the samples waiting (none: nothing).
    fn fetch(&mut self, out: &mut Vec<Sample>) -> io::Result<()>;
}

/// A motion sensor's evdev device.
pub struct EvdevMotion {
    device: RawDevice,
    /// Units per degree per second, for RX, RY, RZ.
    resolution: [f32; 3],
    /// The axes' values as last reported.
    raw: [i32; 3],
    /// The report's MSC_TIMESTAMP, if it had one.
    stamp: Option<Duration>,
}

const GYRO: [AbsoluteAxisCode; 3] = [AbsoluteAxisCode::ABS_RX, AbsoluteAxisCode::ABS_RY, AbsoluteAxisCode::ABS_RZ];

/// Whether a device is a motion sensor with a gyro.
fn is_motion(d: &RawDevice) -> bool {
    d.properties().contains(PropType::ACCELEROMETER) && d.supported_absolute_axes().is_some_and(|a| GYRO.iter().all(|&g| a.contains(g)))
}

/// Whether `motion` is `controller`'s: its name starts with the controller's (drivers call it
/// "<controller> Motion Sensors"), or their physical paths differ only in the last part.
pub fn pairs(controller: (&str, &str), motion: (&str, &str)) -> bool {
    let (c_name, c_phys) = controller;
    let (m_name, m_phys) = motion;
    let stem = |p: &str| p.rsplit_once('/').map_or(p, |(s, _)| s).to_owned();
    (!c_name.is_empty() && m_name.starts_with(c_name)) || (!c_phys.is_empty() && stem(c_phys) == stem(m_phys))
}

impl EvdevMotion {
    /// The motion sensor of the controller named `name` with physical path `phys`, else the only one
    /// there is.
    pub fn find(name: &str, phys: &str) -> Option<EvdevMotion> {
        let sensors: Vec<RawDevice> = evdev::raw_stream::enumerate().map(|(_, d)| d).filter(is_motion).collect();
        let paired = sensors.iter().position(|d| pairs((name, phys), (d.name().unwrap_or(""), d.physical_path().unwrap_or(""))));
        let chosen = match paired {
            Some(i) => sensors.into_iter().nth(i),
            None if sensors.len() == 1 => sensors.into_iter().next(),
            None => None,
        }?;
        EvdevMotion::new(chosen).map_err(|e| log::warn!("motion sensor: {e}")).ok()
    }

    fn new(device: RawDevice) -> io::Result<EvdevMotion> {
        let abs = device.get_abs_state()?;
        let info = |axis: AbsoluteAxisCode| abs.get(usize::from(axis.0)).copied();
        let resolution = GYRO.map(|g| info(g).map_or(1.0, |i| i.resolution.max(1) as f32));
        let raw = GYRO.map(|g| info(g).map_or(0, |i| i.value));
        let flags = rustix::fs::fcntl_getfl(&device)?;
        rustix::fs::fcntl_setfl(&device, flags | rustix::fs::OFlags::NONBLOCK)?;
        log::info!("pad: gyro from {}", device.name().unwrap_or("a motion sensor"));
        Ok(EvdevMotion { device, resolution, raw, stamp: None })
    }
}

impl MotionSource for EvdevMotion {
    fn fd(&self) -> BorrowedFd<'_> {
        self.device.as_fd()
    }

    fn fetch(&mut self, out: &mut Vec<Sample>) -> io::Result<()> {
        let events = match self.device.fetch_events() {
            Ok(events) => events,
            Err(e) if e.kind() == io::ErrorKind::WouldBlock => return Ok(()),
            Err(e) => return Err(e),
        };
        for e in events {
            match e.event_type() {
                EventType::ABSOLUTE => {
                    if let Some(i) = GYRO.iter().position(|g| g.0 == e.code())
                        && let Some(v) = self.raw.get_mut(i)
                    {
                        *v = e.value();
                    }
                }
                EventType::MISC if e.code() == MiscCode::MSC_TIMESTAMP.0 => {
                    self.stamp = Some(Duration::from_micros(u64::from(e.value() as u32)));
                }
                EventType::SYNCHRONIZATION => {
                    let at = self.stamp.take().unwrap_or_else(|| e.timestamp().duration_since(std::time::UNIX_EPOCH).unwrap_or_default());
                    let [p, y, r] = [0, 1, 2].map(|i| self.raw.get(i).copied().unwrap_or(0) as f32 / self.resolution.get(i).copied().unwrap_or(1.0));
                    out.push(Sample { rate: Rate { pitch: p, yaw: y, roll: r }, at });
                }
                _ => {}
            }
        }
        Ok(())
    }
}

/// The time between two samples, as seconds; a first sample, a clock that wrapped (MSC_TIMESTAMP is 32
/// bits of microseconds) or a long gap counts as one sample period at most.
pub fn interval(last: Option<Duration>, at: Duration) -> f32 {
    const MAX: f32 = 0.05;
    match last {
        Some(last) if at > last => (at - last).as_secs_f32().min(MAX),
        _ => 0.0,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_motion_sensor_pairs_with_its_controller() {
        let pad = ("Sony Interactive Entertainment DualSense Wireless Controller", "usb-0000:00:14.0-2/input3");
        assert!(pairs(pad, ("Sony Interactive Entertainment DualSense Wireless Controller Motion Sensors", "")));
        assert!(pairs(("x", "usb-0000:00:14.0-2/input0"), ("y", "usb-0000:00:14.0-2/input1")), "the same port");
        assert!(!pairs(("x", "usb-1/input0"), ("y", "usb-2/input0")));
        assert!(!pairs(("", ""), ("y", "")), "nothing to pair by");
    }

    #[test]
    fn intervals_between_samples() {
        let ms = Duration::from_millis;
        assert_eq!(interval(None, ms(10)), 0.0, "the first sample moves nothing");
        assert!((interval(Some(ms(10)), ms(14)) - 0.004).abs() < 1e-6);
        assert_eq!(interval(Some(ms(10)), ms(2)), 0.0, "the clock wrapped");
        assert_eq!(interval(Some(ms(0)), ms(5000)), 0.05, "a gap");
    }
}
