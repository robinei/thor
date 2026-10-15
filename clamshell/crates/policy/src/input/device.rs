//! Input devices: which one to use ([`DeviceSpec`], from the environment), finding it ([`open`]), and
//! holding it grabbed while reading it ([`Grabbed`]).

use std::fmt;
use std::io;
use std::path::PathBuf;
use std::time::Duration;

use evdev::raw_stream::RawDevice;
use rustix::event::{PollFd, PollFlags, Timespec};

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
    /// A path (starting with '/'), else part of a name; nothing: the first that looks right.
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

/// The device `spec` asks for, else the first that `looks_right`; by name, never one named in `ours`
/// (our own virtual devices). Not found: an error of kind NotFound ("no <what>").
pub fn open(spec: &DeviceSpec, what: &str, looks_right: impl Fn(&RawDevice) -> bool, ours: &[&str]) -> io::Result<RawDevice> {
    if let DeviceSpec::Path(path) = spec {
        return RawDevice::open(path);
    }
    let not_ours = |d: &RawDevice| d.name().is_none_or(|n| !ours.contains(&n));
    evdev::raw_stream::enumerate()
        .map(|(_, d)| d)
        .find(|d| {
            not_ours(d)
                && match spec {
                    DeviceSpec::Named(name) => d.name().is_some_and(|n| n.contains(name.as_str())),
                    _ => looks_right(d),
                }
        })
        .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, format!("no {what} ({spec})")))
}

/// A device grabbed (nothing else sees its events while we hold it; let go when dropped) and read
/// without blocking.
pub struct Grabbed {
    device: RawDevice,
}

impl Grabbed {
    pub fn new(mut device: RawDevice) -> io::Result<Grabbed> {
        device.grab()?;
        let flags = rustix::fs::fcntl_getfl(&device)?;
        rustix::fs::fcntl_setfl(&device, flags | rustix::fs::OFlags::NONBLOCK)?;
        Ok(Grabbed { device })
    }

    pub fn device(&self) -> &RawDevice {
        &self.device
    }

    /// Waits up to `timeout` for events to read (less if a signal interrupts the wait).
    pub fn wait(&self, timeout: Duration) -> io::Result<()> {
        let timeout = Timespec { tv_sec: i64::try_from(timeout.as_secs()).unwrap_or(i64::MAX), tv_nsec: i64::from(timeout.subsec_nanos()) };
        let mut fds = [PollFd::new(&self.device, PollFlags::IN)];
        match rustix::event::poll(&mut fds, Some(&timeout)) {
            Ok(_) | Err(rustix::io::Errno::INTR) => Ok(()),
            Err(e) => Err(e.into()),
        }
    }

    /// Appends the events waiting to `out` (none waiting: nothing).
    pub fn fetch(&mut self, out: &mut Vec<Raw>) -> io::Result<()> {
        match self.device.fetch_events() {
            Ok(events) => {
                out.extend(events.map(Raw::from));
                Ok(())
            }
            Err(e) if e.kind() == io::ErrorKind::WouldBlock => Ok(()),
            Err(e) => Err(e),
        }
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
