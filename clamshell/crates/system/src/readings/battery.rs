//! The battery tab's readings while it is open: the charge over the last day from UPower on the system
//! bus, which records the battery's level as it changes, and what is using the machine now
//! ([`super::usage`]). They are sampled every two seconds and sent as [`Reading::Battery`].

use std::fmt;
use std::io;
use std::sync::mpsc::Receiver;
use std::time::{Duration, Instant};

use super::usage::Sampler;
use crate::os::dbus::{self, Bus, Kept, Path};
use crate::os::worker::{self, Next, Shown, Worker};
use crate::{Closed, Sink};
use clamshell_state::status::battery::{Battery, Charge};
use clamshell_state::{Event, Reading};

const SAMPLE_EVERY: Duration = Duration::from_secs(2);
const HISTORY_EVERY: Duration = Duration::from_secs(60);
/// The history's span and how many points to ask for.
const HISTORY_SPAN: u32 = 24 * 3600;
const HISTORY_POINTS: u32 = 240;
/// UPower's device type (UpDeviceKind) for a battery.
const UPOWER_BATTERY: u32 = 2;

/// A history point's state (UpDeviceState). Points with an unknown state are gaps, with a level of 0.
#[repr(u32)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ChargeState {
    Unknown = 0,
    Charging = 1,
}

const UPOWER: &str = "org.freedesktop.UPower";
const UPOWER_DEVICE: &str = "org.freedesktop.UPower.Device";

#[derive(Debug)]
enum Error {
    Bus(dbus::Error),
    NoBattery,
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::Bus(e) => write!(f, "{e}"),
            Error::NoBattery => write!(f, "UPower has no battery"),
        }
    }
}

impl From<dbus::Error> for Error {
    fn from(e: dbus::Error) -> Self {
        Error::Bus(e)
    }
}

/// Starts the battery thread, which samples what uses the battery while the battery tab is shown.
pub fn spawn(sink: impl Sink) -> io::Result<Worker<Shown>> {
    Worker::spawn("battery", move |commands| {
        let _ = sample(&commands, &sink);
    })
}

/// Samples every two seconds while the tab is shown, and the history once a minute, until the command
/// channel or the UI is gone.
fn sample(commands: &Receiver<Shown>, sink: &impl Sink) -> Result<(), Closed> {
    let mut sampling = false;
    let mut sampler = Sampler::default();
    let mut battery = Battery::default();
    let mut last_history: Option<Instant> = None;
    let mut upower = Kept::system();
    loop {
        match worker::next(commands, sampling.then_some(SAMPLE_EVERY)) {
            Next::Command(Shown(shown)) => {
                sampling = shown;
                sampler = Sampler::default();
                last_history = None;
                battery.spenders.clear();
            }
            Next::Elapsed => {}
            Next::Disconnected => return Ok(()),
        }
        if !sampling {
            continue;
        }
        if last_history.is_none_or(|t| t.elapsed() >= HISTORY_EVERY) {
            last_history = Some(Instant::now());
            match history(&mut upower) {
                Ok(h) => {
                    battery.history = h;
                    battery.has_history = true;
                }
                Err(e) => {
                    log::debug!("battery: no charge history: {e}");
                    battery.has_history = false;
                }
            }
        }
        battery.spenders = sampler.sample();
        sink.send(Event::Reading(Reading::Battery(battery.clone())))?;
    }
}

/// The battery's charge over the last day, oldest first, from UPower. The connection is kept between
/// calls.
fn history(upower: &mut Kept) -> Result<Vec<Charge>, Error> {
    let charges = read_history(upower.get()?);
    if let Err(Error::Bus(e)) = &charges {
        upower.failed(e);
    }
    charges
}

fn read_history(bus: &mut Bus) -> Result<Vec<Charge>, Error> {
    let (devices,): (Vec<Path<'static>>,) = bus.call(UPOWER, "/org/freedesktop/UPower", UPOWER, "EnumerateDevices", ())?;
    for path in devices {
        if bus.get::<u32>(UPOWER, &path, UPOWER_DEVICE, "Type")? != UPOWER_BATTERY || !bus.get::<bool>(UPOWER, &path, UPOWER_DEVICE, "PowerSupply")? {
            continue;
        }
        // Points of (time, value, state), newest first.
        let (points,): (Vec<(u32, f64, u32)>,) = bus.call(UPOWER, &path, UPOWER_DEVICE, "GetHistory", ("charge", HISTORY_SPAN, HISTORY_POINTS))?;
        return Ok(charges(points));
    }
    Err(Error::NoBattery)
}

/// UPower's history points, newest first, as charges, oldest first. Points with an unknown state are
/// gaps with a level of 0 and are dropped.
fn charges(points: Vec<(u32, f64, u32)>) -> Vec<Charge> {
    let mut out: Vec<Charge> = points
        .into_iter()
        .filter(|&(_, _, state)| state != ChargeState::Unknown as u32)
        .map(|(at, level, state)| Charge { at: u64::from(at), level: level.round().clamp(0.0, 100.0) as u8, charging: state == ChargeState::Charging as u32 })
        .collect();
    out.sort_by_key(|c| c.at);
    out
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    #[test]
    fn upower_history_without_its_gaps_oldest_first() {
        let points = vec![(300, 51.0, 2), (200, 52.0, 1), (200, 0.0, 0), (100, 53.4, 2)];
        let levels: Vec<(u64, u8, bool)> = charges(points).iter().map(|c| (c.at, c.level, c.charging)).collect();
        assert_eq!(levels, [(100, 53, false), (200, 52, true), (300, 51, false)]);
    }
}

#[cfg(test)]
mod live {
    use super::*;

    /// With UPower running, as on a desktop, reads the charge history or says why there is none. Nothing
    /// is changed. Run it by hand with `--ignored --nocapture`.
    #[test]
    #[ignore = "needs UPower"]
    fn reads_upower() {
        match history(&mut Kept::system()) {
            Ok(charges) => println!("{} points, the last {:?}", charges.len(), charges.last()),
            Err(e) => println!("no history: {e}"),
        }
    }
}
