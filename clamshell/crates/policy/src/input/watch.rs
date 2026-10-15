//! Watching the input devices: every one is read (not grabbed, so the compositor and logind still get
//! everything), for two things. Input on any of them, for idle (sent at most every [`ACTIVITY_EVERY`]
//! while there is some): keys, buttons, relative motion, and absolute axes that moved a real distance (a
//! resting stick's jitter isn't input). And, in the device's own session, the device's hardware keys:
//! volume, mute and brightness on small button devices (gpio-keys, a headset's buttons), never on a
//! full keyboard, whose keys are the desktop's business. Devices that come and go are picked up within
//! [`RESCAN_EVERY`]. Grabbed devices (the controller, the touchscreen in touchpad mode) say nothing here;
//! their input reaches the state through their own paths and the virtual devices they feed.

use std::collections::HashMap;
use std::io;
use std::path::PathBuf;
use std::time::{Duration, Instant};

use clamshell_core::hardware::DeviceKey;
use clamshell_core::idle::IdleEvent;
use clamshell_core::Event;
use evdev::raw_stream::RawDevice;
use evdev::{AbsoluteAxisCode, EventType, KeyCode, PropType};
use rustix::event::{PollFd, PollFlags, Timespec};

use super::{Press, Raw, Range};
use crate::worker;
use crate::{Closed, Sink};

/// Input is reported at most this often.
const ACTIVITY_EVERY: Duration = Duration::from_secs(3);
/// New devices are looked for this often.
const RESCAN_EVERY: Duration = Duration::from_secs(5);
/// An absolute axis counts as input once it has moved this far (of its range) since it last counted.
const ABS_MOVE: f32 = 0.05;

/// A device read for input.
struct Watched {
    path: PathBuf,
    device: RawDevice,
    /// Its volume and brightness keys are the device's (see the module doc).
    hardware_keys: bool,
    /// Each absolute axis' range and the value it last counted at.
    axes: HashMap<u16, (Range, i32)>,
}

impl Watched {
    fn open(path: PathBuf, device: RawDevice) -> Option<Watched> {
        // Accelerometers report all the time: not input.
        if device.properties().contains(PropType::ACCELEROMETER) {
            return None;
        }
        let flags = rustix::fs::fcntl_getfl(&device).ok()?;
        rustix::fs::fcntl_setfl(&device, flags | rustix::fs::OFlags::NONBLOCK).ok()?;
        let keys = device.supported_keys();
        let has = |k: KeyCode| keys.is_some_and(|set| set.contains(k));
        let hardware_keys = (has(KeyCode::KEY_VOLUMEUP) || has(KeyCode::KEY_BRIGHTNESSUP)) && !has(KeyCode::KEY_A);
        let state = device.get_abs_state().ok();
        let axes = device
            .supported_absolute_axes()
            .map(|set| {
                set.iter()
                    .filter_map(|axis| {
                        let info = state.as_ref()?.get(usize::from(axis.0))?;
                        Some((axis.0, (Range { min: info.minimum, max: info.maximum }, info.value)))
                    })
                    .collect()
            })
            .unwrap_or_default();
        Some(Watched { path, device, hardware_keys, axes })
    }

    /// Whether `raw` is input (and remembers where an axis counted).
    fn is_input(&mut self, raw: Raw) -> bool {
        match raw.kind {
            EventType::KEY | EventType::RELATIVE => true,
            EventType::ABSOLUTE => match self.axes.get_mut(&raw.code) {
                Some((range, last)) => {
                    let moved = (raw.value - *last).unsigned_abs() as f32 / (range.max - range.min).max(1) as f32;
                    if moved >= ABS_MOVE {
                        *last = raw.value;
                    }
                    moved >= ABS_MOVE
                }
                // An axis it didn't say it had (or a slot change): counts.
                None => AbsoluteAxisCode(raw.code) != AbsoluteAxisCode::ABS_MT_SLOT,
            },
            _ => false,
        }
    }
}

/// A hardware key's meaning, if `raw` is one pressed (or held: it repeats).
fn device_key(raw: Raw) -> Option<DeviceKey> {
    let (key, press) = raw.as_key()?;
    if !press.held() {
        return None;
    }
    Some(match key {
        KeyCode::KEY_VOLUMEUP => DeviceKey::VolumeUp,
        KeyCode::KEY_VOLUMEDOWN => DeviceKey::VolumeDown,
        KeyCode::KEY_MUTE if press == Press::Down => DeviceKey::Mute,
        KeyCode::KEY_BRIGHTNESSUP => DeviceKey::BrightnessUp,
        KeyCode::KEY_BRIGHTNESSDOWN => DeviceKey::BrightnessDown,
        _ => return None,
    })
}

/// Starts watching. `hardware_keys`: take the device's hardware keys (its own session only).
pub fn spawn(hardware_keys: bool, sink: impl Sink) -> io::Result<()> {
    worker::detached("input watch", move || {
        if let Err(Closed) = watch(hardware_keys, &sink) {
            log::debug!("input watch: the UI is gone");
        }
    })
}

fn watch(take_keys: bool, sink: &impl Sink) -> Result<(), Closed> {
    let mut devices: Vec<Watched> = Vec::new();
    let mut scanned: Option<Instant> = None;
    let mut reported: Option<Instant> = None;
    let mut watching = None;
    let mut raws = Vec::new();
    loop {
        if scanned.is_none_or(|t| t.elapsed() >= RESCAN_EVERY) {
            scanned = Some(Instant::now());
            for (path, device) in evdev::raw_stream::enumerate() {
                if !devices.iter().any(|d| d.path == path)
                    && let Some(w) = Watched::open(path, device)
                {
                    log::debug!("input watch: {}{}", w.device.name().unwrap_or("?"), if w.hardware_keys { " (hardware keys)" } else { "" });
                    devices.push(w);
                }
            }
            // Whether there is anything to watch (no permission to read /dev/input: nothing).
            let now = !devices.is_empty();
            if watching != Some(now) {
                watching = Some(now);
                if !now {
                    log::info!("input watch: no input device can be read; idle relies on the compositor");
                }
                sink.send(Event::Idle(IdleEvent::Watching(now)))?;
            }
        }
        wait(&devices, RESCAN_EVERY);
        let mut input = false;
        let mut keys = Vec::new();
        devices.retain_mut(|d| {
            raws.clear();
            match d.device.fetch_events() {
                Ok(events) => raws.extend(events.map(Raw::from)),
                Err(e) if e.kind() == io::ErrorKind::WouldBlock => {}
                // Unplugged.
                Err(_) => return false,
            }
            for &raw in &raws {
                input |= d.is_input(raw);
                if take_keys && d.hardware_keys {
                    keys.extend(device_key(raw));
                }
            }
            true
        });
        for key in keys {
            sink.send(Event::Device(key))?;
        }
        if input && reported.is_none_or(|t| t.elapsed() >= ACTIVITY_EVERY) {
            reported = Some(Instant::now());
            sink.send(Event::Idle(IdleEvent::DeviceInput))?;
        }
    }
}

/// Waits until a device has events, at most `timeout`.
fn wait(devices: &[Watched], timeout: Duration) {
    let mut fds: Vec<PollFd> = devices.iter().map(|d| PollFd::new(&d.device, PollFlags::IN)).collect();
    let timeout = Timespec { tv_sec: timeout.as_secs() as i64, tv_nsec: i64::from(timeout.subsec_nanos()) };
    if fds.is_empty() {
        std::thread::sleep(Duration::from_secs(timeout.tv_sec as u64));
        return;
    }
    let _ = rustix::event::poll(&mut fds, Some(&timeout));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hardware_keys_and_their_presses() {
        let key = |code: KeyCode, press: Press| Raw::key(code, press);
        assert_eq!(device_key(key(KeyCode::KEY_VOLUMEUP, Press::Down)), Some(DeviceKey::VolumeUp));
        assert_eq!(device_key(key(KeyCode::KEY_VOLUMEDOWN, Press::Repeat)), Some(DeviceKey::VolumeDown), "held: repeats");
        assert_eq!(device_key(key(KeyCode::KEY_VOLUMEUP, Press::Up)), None);
        assert_eq!(device_key(key(KeyCode::KEY_MUTE, Press::Repeat)), None, "mute toggles once");
        assert_eq!(device_key(key(KeyCode::KEY_A, Press::Down)), None);
    }
}
