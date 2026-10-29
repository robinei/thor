//! Input devices: which one to use ([`DeviceSpec`], from the environment), and keeping one grabbed while
//! reading it ([`Grabbed`]).

use std::fmt;
use std::io;
use std::path::PathBuf;

use evdev::raw_stream::RawDevice;
use evdev::{AbsoluteAxisCode, EventType, KeyCode, SynchronizationCode};

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

/// Axes and their values.
type AxisValues = Vec<(AbsoluteAxisCode, i32)>;

/// A grabbed device, read without blocking. Nothing else sees its events while we hold it, and it is let
/// go when dropped.
pub(crate) struct Grabbed {
    device: RawDevice,
    /// The kernel dropped events (SYN_DROPPED), so everything until the next report is left out.
    skipping: bool,
}

impl Grabbed {
    pub(crate) fn new(mut device: RawDevice) -> io::Result<Grabbed> {
        device.grab()?;
        crate::os::set_nonblocking(&device)?;
        Ok(Grabbed { device, skipping: false })
    }

    pub(crate) fn device(&self) -> &RawDevice {
        &self.device
    }

    /// Appends the waiting events, if any, to `out`. Returns whether the kernel dropped some because its
    /// buffer overran (SYN_DROPPED). In that case what was read no longer adds up to the device's state
    /// (see [`Grabbed::state`]). The partial report after a drop is left out, as the kernel asks.
    pub(crate) fn fetch(&mut self, out: &mut Vec<Raw>) -> io::Result<bool> {
        let Some(events) = crate::os::ready(self.device.fetch_events())? else { return Ok(false) };
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
    pub(crate) fn state(&self) -> io::Result<(Vec<KeyCode>, AxisValues)> {
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
