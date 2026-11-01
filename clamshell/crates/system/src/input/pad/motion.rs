//! The gyro, where the pad thread gets angular velocity from. A [`MotionSource`] gives a file descriptor
//! to wait on, and samples of degrees per second on three axes with their time. There are two sources:
//!
//! - An evdev motion sensor ([`EvdevMotion`]), which is how the kernel's controller drivers expose their
//!   IMU. It is a device with INPUT_PROP_ACCELEROMETER, with the gyro on ABS_RX/RY/RZ, the resolution in
//!   units per degree per second in the axis' info, and MSC_TIMESTAMP in microseconds with each report.
//!   The sensor used is the one whose name or physical path pairs with the controller's, or else the only
//!   motion sensor there is. It isn't grabbed, since nothing else reads it as input and the idle watch
//!   ignores accelerometers.
//! - The Thor's sensor hub (`sensor_hub`), read directly over QRTR.

use std::io;
use std::os::fd::{AsFd, BorrowedFd};
use std::time::{Duration, Instant};

use evdev::raw_stream::RawDevice;
use evdev::{AbsoluteAxisCode, EventType, MiscCode, PropType};

/// Angular velocity in degrees per second, about the device's axes held as a handheld in landscape:
/// pitch about the left-right axis (ABS_RX), yaw about the up axis (ABS_RY), roll about the axis
/// toward the player (ABS_RZ).
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub(crate) struct Rate {
    pub(crate) pitch: f32,
    pub(crate) yaw: f32,
    pub(crate) roll: f32,
}

impl Rate {
    fn map(self, f: impl Fn(f32) -> f32) -> Rate {
        Rate { pitch: f(self.pitch), yaw: f(self.yaw), roll: f(self.roll) }
    }

    fn zip(self, o: Rate, f: impl Fn(f32, f32) -> f32) -> Rate {
        Rate { pitch: f(self.pitch, o.pitch), yaw: f(self.yaw, o.yaw), roll: f(self.roll, o.roll) }
    }

    fn axes(self) -> [f32; 3] {
        [self.pitch, self.yaw, self.roll]
    }
}

/// The device is lying still when every axis is slower than this, in degrees per second including the
/// offset ...
const STILL: f32 = 5.0;
/// ... no axis moves over a range wider than this ...
const STILL_SPREAD: f32 = 0.5;
/// ... for this many seconds. The average over that time is then the offset.
const STILL_FOR: f32 = 1.5;
/// A requested calibration takes this many seconds, and fails if an axis moves over a wider range than
/// `CALIBRATE_SPREAD`.
const CALIBRATE_FOR: f32 = 1.0;
const CALIBRATE_SPREAD: f32 = 2.0;
/// A requested calibration that hasn't ended by this long after it was asked for fails: the gyro sends
/// no samples (a sensor hub whose service isn't up yet).
const ASK_WAIT: Duration = Duration::from_secs(3);

/// Samples over a stretch of time: their sum, count and time, and each axis' lowest and highest value.
#[derive(Debug, Clone, Copy, PartialEq)]
struct Run {
    sum: Rate,
    n: u32,
    time: f32,
    low: Rate,
    high: Rate,
}

impl Run {
    fn start(r: Rate) -> Run {
        Run { sum: r, n: 1, time: 0.0, low: r, high: r }
    }

    fn add(&mut self, r: Rate, dt: f32) {
        self.sum = self.sum.zip(r, |a, b| a + b);
        self.n += 1;
        self.time += dt;
        self.low = self.low.zip(r, f32::min);
        self.high = self.high.zip(r, f32::max);
    }

    /// The widest range an axis moved over.
    fn spread(&self) -> f32 {
        self.high.zip(self.low, |h, l| h - l).axes().into_iter().fold(0.0, f32::max)
    }

    fn mean(&self) -> Rate {
        let n = self.n.max(1) as f32;
        self.sum.map(|v| v / n)
    }
}

/// The gyro's reading at rest. This offset is something real sensors have, and it drifts with
/// temperature. It is taken off every sample. It is learnt whenever the device lies still for a moment
/// ([`STILL`], [`STILL_SPREAD`], [`STILL_FOR`]). It is also learnt on request ([`Calibration::ask`]) over
/// the next [`CALIBRATE_FOR`] seconds, as long as the device is held still enough.
#[derive(Debug, Clone, Default, PartialEq)]
pub(crate) struct Calibration {
    bias: Rate,
    still: Option<Run>,
    asked: Asked,
    /// When a requested calibration fails if it hasn't ended ([`ASK_WAIT`]).
    asked_until: Option<Instant>,
}

/// A requested calibration: none, waiting for its first sample, or gathering samples.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
enum Asked {
    #[default]
    No,
    First,
    Gathering(Run),
}

impl Calibration {
    /// Calibrates from the next samples, asked for at `now`. [`Calibration::correct`] says how it went, or
    /// [`Calibration::expire`] that it failed for want of samples.
    pub(crate) fn ask(&mut self, now: Instant) {
        self.asked = Asked::First;
        self.asked_until = Some(now + ASK_WAIT);
    }

    /// When a requested calibration fails if it hasn't ended.
    pub(crate) fn deadline(&self) -> Option<Instant> {
        self.asked_until
    }

    /// Ends a requested calibration whose time is up at `now`. Returns whether it did: it failed.
    pub(crate) fn expire(&mut self, now: Instant) -> bool {
        if self.asked_until.is_none_or(|until| now < until) {
            return false;
        }
        self.asked = Asked::No;
        self.asked_until = None;
        true
    }

    /// Corrects a sample's rate, `dt` seconds after the last sample, by taking off the offset. When a
    /// requested calibration ends, also returns whether it succeeded.
    pub(crate) fn correct(&mut self, rate: Rate, dt: f32) -> (Rate, Option<bool>) {
        let mut done = None;
        let gathering = match self.asked {
            Asked::No => None,
            Asked::First => Some(Run::start(rate)),
            Asked::Gathering(mut r) => {
                r.add(rate, dt);
                Some(r)
            }
        };
        if let Some(r) = gathering {
            self.asked = Asked::Gathering(r);
            if r.time >= CALIBRATE_FOR {
                let ok = r.spread() <= CALIBRATE_SPREAD;
                if ok {
                    self.bias = r.mean();
                }
                self.asked = Asked::No;
                self.asked_until = None;
                self.still = None;
                done = Some(ok);
            }
        } else if rate.axes().iter().all(|v| v.abs() < STILL) {
            match &mut self.still {
                Some(r) => r.add(rate, dt),
                None => self.still = Some(Run::start(rate)),
            }
            match self.still {
                Some(r) if r.spread() > STILL_SPREAD => self.still = Some(Run::start(rate)),
                Some(r) if r.time >= STILL_FOR => {
                    self.bias = r.mean();
                    self.still = None;
                }
                _ => {}
            }
        } else {
            self.still = None;
        }
        (rate.zip(self.bias, |v, b| v - b), done)
    }
}

/// One reading: the rate, and when it was taken by the sensor's clock.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct Sample {
    pub(crate) rate: Rate,
    pub(crate) at: Duration,
}

/// Where angular velocity comes from.
pub(crate) trait MotionSource: Send {
    /// The file descriptor to wait on for samples.
    fn fd(&self) -> BorrowedFd<'_>;

    /// Appends the waiting samples, if any.
    fn fetch(&mut self, out: &mut Vec<Sample>) -> io::Result<()>;
}

/// A motion sensor's evdev device.
pub(crate) struct EvdevMotion {
    device: RawDevice,
    /// Units per degree per second, for RX, RY and RZ.
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

/// What tells an input device from others: its name, physical path and unique id (a Bluetooth
/// controller's address), each empty if it has none.
#[derive(Debug, Clone, Copy, Default)]
pub(crate) struct Ident<'a> {
    pub(crate) name: &'a str,
    pub(crate) phys: &'a str,
    pub(crate) uniq: &'a str,
}

impl<'a> Ident<'a> {
    fn of(d: &'a RawDevice) -> Ident<'a> {
        Ident { name: d.name().unwrap_or(""), phys: d.physical_path().unwrap_or(""), uniq: d.unique_name().unwrap_or("") }
    }
}

/// Whether `motion` is `controller`'s. Where both have a unique id, it decides: a Bluetooth device's
/// physical path is the adapter's address, the same for every controller, and two of one model share a
/// name. Otherwise it is if its name starts with the controller's, since drivers call it `<controller>
/// Motion Sensors`, or if their physical paths differ only in the last part.
fn pairs(controller: Ident, motion: Ident) -> bool {
    let (c_uniq, m_uniq) = (controller.uniq.trim(), motion.uniq.trim());
    if !c_uniq.is_empty() && !m_uniq.is_empty() {
        return c_uniq == m_uniq;
    }
    let stem = |p: &str| p.rsplit_once('/').map_or(p, |(s, _)| s).to_owned();
    (!controller.name.is_empty() && motion.name.starts_with(controller.name)) || (!controller.phys.is_empty() && stem(controller.phys) == stem(motion.phys))
}

impl EvdevMotion {
    /// The motion sensor of the controller `controller`, or else the only one there is.
    pub(crate) fn find(controller: Ident) -> Option<EvdevMotion> {
        let sensors: Vec<RawDevice> = evdev::raw_stream::enumerate().map(|(_, d)| d).filter(is_motion).collect();
        let paired = sensors.iter().position(|d| pairs(controller, Ident::of(d)));
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
        crate::os::set_nonblocking(&device)?;
        log::info!("pad: gyro from {}", device.name().unwrap_or("a motion sensor"));
        Ok(EvdevMotion { device, resolution, raw, stamp: None })
    }
}

impl MotionSource for EvdevMotion {
    fn fd(&self) -> BorrowedFd<'_> {
        self.device.as_fd()
    }

    fn fetch(&mut self, out: &mut Vec<Sample>) -> io::Result<()> {
        let Some(events) = crate::os::ready(self.device.fetch_events())? else { return Ok(()) };
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

/// The time between two samples in seconds. A first sample, or a clock that wrapped (MSC_TIMESTAMP is 32
/// bits of microseconds), gives 0. A long gap is capped at 50 ms.
pub(crate) fn interval(last: Option<Duration>, at: Duration) -> f32 {
    const MAX: f32 = 0.05;
    match last {
        Some(last) if at > last => at.saturating_sub(last).as_secs_f32().min(MAX),
        _ => 0.0,
    }
}

#[cfg(test)]
mod tests {

    #[test]
    fn the_offset_is_learnt_at_rest_and_on_request() {
        let r = |yaw: f32| Rate { pitch: 0.2, yaw, roll: -0.3 };
        let mut c = Calibration::default();
        let dt = 0.004;
        // While moving, nothing is learnt.
        for i in 0..500 {
            c.correct(r(if i % 2 == 0 { 3.0 } else { -3.0 }), dt);
        }
        assert_eq!(c.correct(r(0.0), dt).0, r(0.0));
        // Still for 1.5 s at an offset, so the offset is learnt.
        for _ in 0..400 {
            c.correct(r(0.8), dt);
        }
        let (now, _) = c.correct(r(0.8), dt);
        assert!(now.axes().iter().all(|v| v.abs() < 0.01), "{now:?}");
        // On request, the next second held still is taken, and success is reported.
        c.ask(Instant::now());
        let mut done = None;
        for _ in 0..300 {
            done = done.or(c.correct(r(1.5), dt).1);
        }
        assert_eq!(done, Some(true));
        assert!(c.correct(r(1.5), dt).0.yaw.abs() < 0.01);
        // On request while moving, nothing is taken.
        c.ask(Instant::now());
        let mut done = None;
        for i in 0..300 {
            done = done.or(c.correct(r(if i % 50 < 25 { 10.0 } else { -10.0 }), dt).1);
        }
        assert_eq!(done, Some(false));
        assert!(c.correct(r(1.5), dt).0.yaw.abs() < 0.01, "the offset kept");
    }

    use super::*;

    /// A calibration asked for that gets no samples fails once its time is up, and doesn't end later
    /// when samples come. (It waited for samples for good, and "Gyro calibrated" showed up minutes later.)
    #[test]
    fn a_calibration_without_samples_fails_in_time() {
        let t0 = Instant::now();
        let mut c = Calibration::default();
        assert!(!c.expire(t0), "nothing asked");
        c.ask(t0);
        assert_eq!(c.deadline(), Some(t0 + ASK_WAIT));
        assert!(!c.expire(t0 + ASK_WAIT / 2));
        assert!(c.expire(t0 + ASK_WAIT), "failed");
        assert!(!c.expire(t0 + ASK_WAIT * 2), "once");
        let still = Rate { pitch: 0.1, yaw: 0.1, roll: 0.1 };
        assert!((0..400).all(|_| c.correct(still, 0.004).1.is_none()), "samples later end nothing");
        // One with samples ends before its time, and has nothing left to fail.
        c.ask(t0);
        assert!((0..400).any(|_| c.correct(still, 0.004).1 == Some(true)));
        assert_eq!(c.deadline(), None);
        assert!(!c.expire(t0 + ASK_WAIT));
    }

    #[test]
    fn a_motion_sensor_pairs_with_its_controller() {
        let at = |name, phys| Ident { name, phys, uniq: "" };
        let pad = at("Sony Interactive Entertainment DualSense Wireless Controller", "usb-0000:00:14.0-2/input3");
        assert!(pairs(pad, at("Sony Interactive Entertainment DualSense Wireless Controller Motion Sensors", "")));
        assert!(pairs(at("x", "usb-0000:00:14.0-2/input0"), at("y", "usb-0000:00:14.0-2/input1")), "the same port");
        assert!(!pairs(at("x", "usb-1/input0"), at("y", "usb-2/input0")));
        assert!(!pairs(at("", ""), at("y", "")), "nothing to pair by");
        // Two DualSenses over Bluetooth: the same names, the adapter's address as the physical path, each
        // its own address as its unique id. (By name and path, either took the first sensor found.)
        let name = "DualSense Wireless Controller";
        let sensors = "DualSense Wireless Controller Motion Sensors";
        let bt = |name, uniq| Ident { name, phys: "e8:48:b8:c8:20:00", uniq };
        assert!(pairs(bt(name, "a0:5a:5c:00:00:01"), bt(sensors, "a0:5a:5c:00:00:01")));
        assert!(!pairs(bt(name, "a0:5a:5c:00:00:01"), bt(sensors, "a0:5a:5c:00:00:02")), "the other controller's");
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
