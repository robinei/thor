//! Input devices: which one to use ([`DeviceSpec`], from the environment), finding it ([`open`]), and
//! keeping it grabbed while reading it ([`Grabbed`]).

use std::fmt;
use std::io;
use std::path::PathBuf;
use std::time::Duration;

use evdev::raw_stream::RawDevice;
use evdev::{AbsoluteAxisCode, EventType, KeyCode, SynchronizationCode};
use rustix::event::{PollFd, PollFlags};

use super::Raw;

/// Which device to use.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DeviceSpec {
    /// Its /dev/input path.
    Path(PathBuf),
    /// Part of its name.
    Named(String),
    /// The first that looks right.
    Auto,
}

impl DeviceSpec {
    /// A value starting with '/' is a path, and anything else is part of a name. No value means the first
    /// device that looks right.
    pub fn parse(value: Option<&str>) -> DeviceSpec {
        match value {
            Some(path) if path.starts_with('/') => DeviceSpec::Path(path.into()),
            Some(name) => DeviceSpec::Named(name.to_owned()),
            None => DeviceSpec::Auto,
        }
    }
}

impl fmt::Display for DeviceSpec {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            DeviceSpec::Path(path) => write!(f, "{}", path.display()),
            DeviceSpec::Named(name) => write!(f, "{name}"),
            DeviceSpec::Auto => write!(f, "auto"),
        }
    }
}

/// Opens the device `spec` asks for, or else the first that `looks_right`. When searching, a device named
/// in `ours`, one of our own virtual devices, is never chosen. If none is found, the error is of kind
/// NotFound (`no <what>`).
pub fn open(spec: &DeviceSpec, what: &str, looks_right: impl Fn(&RawDevice) -> bool, ours: &[&str]) -> io::Result<RawDevice> {
    if let DeviceSpec::Path(path) = spec {
        return RawDevice::open(path);
    }
    let not_ours = |d: &RawDevice| d.name().is_none_or(|n| !ours.contains(&n));
    let candidates = evdev::raw_stream::enumerate().map(|(_, d)| d).filter(|d| {
        not_ours(d)
            && match spec {
                DeviceSpec::Named(name) => d.name().is_some_and(|n| n.contains(name.as_str())),
                _ => looks_right(d),
            }
    });
    // When matching by name, prefer a device that also looks right, because a controller's motion sensor
    // shares its name.
    let mut first = None;
    for d in candidates {
        if looks_right(&d) {
            return Ok(d);
        }
        first.get_or_insert(d);
    }
    first.ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, format!("no {what} ({spec})")))
}

/// Axes and their values.
type AxisValues = Vec<(AbsoluteAxisCode, i32)>;

/// A grabbed device, read without blocking. Nothing else sees its events while we hold it, and it is let
/// go when dropped.
pub struct Grabbed {
    device: RawDevice,
    /// The kernel dropped events (SYN_DROPPED), so everything until the next report is left out.
    skipping: bool,
}

impl Grabbed {
    pub fn new(mut device: RawDevice) -> io::Result<Grabbed> {
        device.grab()?;
        let flags = rustix::fs::fcntl_getfl(&device)?;
        rustix::fs::fcntl_setfl(&device, flags | rustix::fs::OFlags::NONBLOCK)?;
        Ok(Grabbed { device, skipping: false })
    }

    pub fn device(&self) -> &RawDevice {
        &self.device
    }

    /// Waits up to `timeout` for events to read, or less if a signal interrupts the wait.
    pub fn wait(&self, timeout: Duration) -> io::Result<()> {
        let timeout = crate::os::sys::timespec(timeout);
        let mut fds = [PollFd::new(&self.device, PollFlags::IN)];
        match rustix::event::poll(&mut fds, Some(&timeout)) {
            Ok(_) | Err(rustix::io::Errno::INTR) => Ok(()),
            Err(e) => Err(e.into()),
        }
    }

    /// Appends the waiting events, if any, to `out`. Returns whether the kernel dropped some because its
    /// buffer overran (SYN_DROPPED). In that case what was read no longer adds up to the device's state
    /// (see [`Grabbed::state`]). The partial report after a drop is left out, as the kernel asks.
    pub fn fetch(&mut self, out: &mut Vec<Raw>) -> io::Result<bool> {
        let events = match self.device.fetch_events() {
            Ok(events) => events,
            Err(e) if e.kind() == io::ErrorKind::WouldBlock => return Ok(false),
            Err(e) => return Err(e),
        };
        let mut dropped = false;
        for raw in events.map(Raw::from) {
            if raw.kind == EventType::SYNCHRONIZATION && raw.code == SynchronizationCode::SYN_DROPPED.0 {
                (dropped, self.skipping) = (true, true);
            } else if self.skipping {
                self.skipping = raw != Raw::REPORT;
            } else {
                out.push(raw);
            }
        }
        Ok(dropped)
    }

    /// The device's current state: the keys held and the axes' values.
    pub fn state(&self) -> io::Result<(Vec<KeyCode>, AxisValues)> {
        let keys = self.device.get_key_state()?.iter().collect();
        let abs = self.device.get_abs_state()?;
        let axes = self
            .device
            .supported_absolute_axes()
            .map(|set| set.iter().filter_map(|code| abs.get(usize::from(code.0)).map(|a| (code, a.value))).collect())
            .unwrap_or_default();
        Ok((keys, axes))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn specs_from_the_environment() {
        assert_eq!(DeviceSpec::parse(Some("/dev/input/event3")), DeviceSpec::Path("/dev/input/event3".into()));
        assert_eq!(DeviceSpec::parse(Some("Xbox")), DeviceSpec::Named("Xbox".into()));
        assert_eq!(DeviceSpec::parse(None), DeviceSpec::Auto);
        assert_eq!(DeviceSpec::Auto.to_string(), "auto");
    }
}
