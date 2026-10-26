//! A press confirmed by hand, for the root helper that sets the first password (`thor-account`, through
//! `clamshell device confirm`): Volume Up pressed on a physical input device. A virtual device doesn't
//! count, since any member of the input group can make one through uinput and press its keys.

use std::fs;
use std::io;
use std::os::fd::AsFd;
use std::path::Path;
use std::time::{Duration, Instant};

use evdev::{EventSummary, KeyCode};
use rustix::event::{PollFd, PollFlags};

/// Waits up to `within` for Volume Up to be pressed on a physical input device. Returns whether it was.
/// An error when there is no such device.
pub fn volume_up(within: Duration) -> io::Result<bool> {
    let mut devices = Vec::new();
    for entry in fs::read_dir("/dev/input")? {
        let path = entry?.path();
        let Some(event) = path.file_name().and_then(|n| n.to_str()).filter(|n| n.starts_with("event")) else {
            continue;
        };
        let device = fs::canonicalize(format!("/sys/class/input/{event}/device"));
        if device.as_deref().is_ok_and(|d| !is_virtual(d)) {
            match evdev::Device::open(&path) {
                Ok(d) if d.supported_keys().is_some_and(|keys| keys.contains(KeyCode::KEY_VOLUMEUP)) => devices.push(d),
                Ok(_) => {}
                Err(e) => log::debug!("confirm: {}: {e}", path.display()),
            }
        }
    }
    if devices.is_empty() {
        return Err(io::Error::other("no physical input device has Volume Up"));
    }
    for device in &devices {
        device.set_nonblocking(true)?;
    }
    let deadline = Instant::now() + within;
    loop {
        let left = deadline.saturating_duration_since(Instant::now());
        if left.is_zero() {
            return Ok(false);
        }
        let timeout = rustix::event::Timespec::try_from(left).map_err(io::Error::other)?;
        let mut fds: Vec<PollFd> = devices.iter().map(|d| PollFd::from_borrowed_fd(d.as_fd(), PollFlags::IN)).collect();
        match rustix::event::poll(&mut fds, Some(&timeout)) {
            Ok(_) | Err(rustix::io::Errno::INTR) => {}
            Err(e) => return Err(e.into()),
        }
        drop(fds);
        for device in &mut devices {
            let Ok(events) = device.fetch_events() else { continue };
            for event in events {
                if let EventSummary::Key(_, KeyCode::KEY_VOLUMEUP, 1) = event.destructure() {
                    return Ok(true);
                }
            }
        }
    }
}

/// Whether the input device at `device` (its sysfs folder, links resolved) is virtual, as uinput's are.
fn is_virtual(device: &Path) -> bool {
    device.starts_with("/sys/devices/virtual")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_uinput_device_is_virtual_and_a_platform_one_isnt() {
        assert!(is_virtual(Path::new("/sys/devices/virtual/input/input42")));
        assert!(!is_virtual(Path::new("/sys/devices/platform/gpio-keys/input/input3")));
    }
}
