//! Watching the input devices. Every device is read but not grabbed, so the compositor and logind still
//! get everything. The devices are read for two things:
//!
//! - Input on any of them, for idle. It is sent at most every [`ACTIVITY_EVERY`] while there is some.
//!   Input means keys, buttons, relative motion, and absolute axes that moved a real distance, since a
//!   resting stick's jitter isn't input.
//! - In the device's own session, the device's hardware keys. These are volume, mute and brightness on
//!   small button devices such as gpio-keys or a headset's buttons, never on a full keyboard, whose keys
//!   are the desktop's business. The lid is read too, for the power key's wakeup
//!   ([`crate::power::lid`]).
//!
//! Devices that come and go are picked up as /dev/input changes, through inotify, or without it within
//! [`RESCAN_EVERY`]. Grabbed devices such as the controller show nothing here. Their input reaches the
//! state through their own paths and the virtual devices they feed.

use std::collections::HashMap;
use std::io;
use std::path::PathBuf;
use std::time::{Duration, Instant};

use clamshell_state::Event;
use clamshell_state::HardwareKey;
use clamshell_state::idle::IdleEvent;
use evdev::raw_stream::RawDevice;
use evdev::{AbsoluteAxisCode, EventType, KeyCode, PropType, SwitchCode, SynchronizationCode};
use std::os::fd::OwnedFd;

use rustix::event::{PollFd, PollFlags};
use rustix::fs::inotify;

use crate::os::evdev::{Press, Range, Raw};
use crate::os::worker;
use crate::power::lid;
use crate::{Closed, Sink};

/// Input is reported at most this often.
const ACTIVITY_EVERY: Duration = Duration::from_secs(3);
/// How often new devices are looked for without inotify.
const RESCAN_EVERY: Duration = Duration::from_secs(5);
/// An absolute axis counts as input once it has moved this fraction of its range since it last counted.
const ABS_MOVE: f32 = 0.05;

/// A device read for input.
struct Watched {
    path: PathBuf,
    device: RawDevice,
    /// Whether its volume and brightness keys are the device's (see the module docs).
    hardware_keys: bool,
    /// Whether it has the lid's switch.
    lid: bool,
    /// Each absolute axis' range and the value it last counted at.
    axes: HashMap<u16, (Range, i32)>,
}

impl Watched {
    fn open(path: PathBuf, device: RawDevice) -> Option<Watched> {
        // Accelerometers report all the time, which isn't input.
        if device.properties().contains(PropType::ACCELEROMETER) {
            return None;
        }
        let nonblocking = rustix::fs::fcntl_getfl(&device).and_then(|f| rustix::fs::fcntl_setfl(&device, f | rustix::fs::OFlags::NONBLOCK));
        if let Err(e) = nonblocking {
            log::debug!("input watch: {}: {e}", path.display());
            return None;
        }
        let keys = device.supported_keys();
        let has = |k: KeyCode| keys.is_some_and(|set| set.contains(k));
        let hardware_keys = (has(KeyCode::KEY_VOLUMEUP) || has(KeyCode::KEY_BRIGHTNESSUP)) && !has(KeyCode::KEY_A);
        let state = device.get_abs_state().inspect_err(|e| log::debug!("input watch: {}: axes: {e}", path.display())).ok();
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
        let lid = device.supported_switches().is_some_and(|set| set.contains(SwitchCode::SW_LID));
        Some(Watched { path, device, hardware_keys, lid, axes })
    }

    /// Whether `raw` is input. Remembers the value an axis last counted at.
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
                // An axis the device didn't say it had counts, except a slot change.
                None => AbsoluteAxisCode(raw.code) != AbsoluteAxisCode::ABS_MT_SLOT,
            },
            _ => false,
        }
    }
}

/// A hardware key's meaning, if `raw` is one pressed or held. Holding it repeats.
fn device_key(raw: Raw) -> Option<HardwareKey> {
    let (key, press) = raw.as_key()?;
    if !press.held() {
        return None;
    }
    Some(match key {
        KeyCode::KEY_VOLUMEUP => HardwareKey::VolumeUp,
        KeyCode::KEY_VOLUMEDOWN => HardwareKey::VolumeDown,
        KeyCode::KEY_MUTE if press == Press::Down => HardwareKey::Mute,
        KeyCode::KEY_BRIGHTNESSUP => HardwareKey::BrightnessUp,
        KeyCode::KEY_BRIGHTNESSDOWN => HardwareKey::BrightnessDown,
        _ => return None,
    })
}

/// A non-blocking inotify watch of /dev/input. It sees a device's node appear and udev give it its
/// permissions, and either means look again. The pad thread has a watch of its own.
pub(crate) fn devices_watch() -> Option<rustix::fd::OwnedFd> {
    let fd = inotify::init(inotify::CreateFlags::NONBLOCK | inotify::CreateFlags::CLOEXEC).inspect_err(|e| log::warn!("inotify: {e}")).ok()?;
    inotify::add_watch(&fd, "/dev/input", inotify::WatchFlags::CREATE | inotify::WatchFlags::ATTRIB | inotify::WatchFlags::MOVED_TO)
        .inspect_err(|e| log::warn!("inotify: watching /dev/input: {e}"))
        .ok()?;
    Some(fd)
}

/// Starts watching. With `hardware_keys` it takes the device's hardware keys, which is for the device's
/// own session only.
pub(crate) fn spawn(hardware_keys: bool, sink: impl Sink) -> io::Result<()> {
    worker::lasting(clamshell_state::Thread::InputWatch.name(), move || {
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
    let notify = devices_watch();
    let mut look = true;
    loop {
        if look || (notify.is_none() && scanned.is_none_or(|t| t.elapsed() >= RESCAN_EVERY)) {
            scanned = Some(Instant::now());
            for (path, device) in evdev::raw_stream::enumerate() {
                if !devices.iter().any(|d| d.path == path)
                    && let Some(w) = Watched::open(path, device)
                {
                    log::debug!("input watch: {}{}", w.device.name().unwrap_or("?"), if w.hardware_keys { " (hardware keys)" } else { "" });
                    if take_keys && w.lid {
                        lid::closed(lid_switch(&w));
                    }
                    devices.push(w);
                }
            }
            // Whether there is anything to watch. Without permission to read /dev/input there is nothing.
            let now = !devices.is_empty();
            if watching != Some(now) {
                watching = Some(now);
                if !now {
                    log::info!("input watch: no input device can be read; idle relies on the compositor");
                }
                sink.send(Event::Idle(IdleEvent::Watching(now)))?;
            }
        }
        look = wait(&devices, notify.as_ref());
        let mut input = false;
        let mut keys = Vec::new();
        devices.retain_mut(|d| {
            raws.clear();
            match d.device.fetch_events() {
                Ok(events) => raws.extend(events.map(Raw::from)),
                Err(e) if e.kind() == io::ErrorKind::WouldBlock => {}
                // Unplugged.
                Err(e) => {
                    log::debug!("input watch: {}: {e}", d.path.display());
                    return false;
                }
            }
            for &raw in &raws {
                input |= d.is_input(raw);
                if take_keys && d.hardware_keys {
                    keys.extend(device_key(raw));
                }
                if take_keys && d.lid && raw.kind == EventType::SWITCH && raw.code == SwitchCode::SW_LID.0 {
                    lid::closed(raw.value != 0);
                }
                // Events were dropped, perhaps the lid's among them, so read the lid as it is now.
                if take_keys && d.lid && raw.kind == EventType::SYNCHRONIZATION && raw.code == SynchronizationCode::SYN_DROPPED.0 {
                    lid::closed(lid_switch(d));
                }
            }
            true
        });
        for key in keys {
            sink.send(Event::HardwareKey(key))?;
        }
        if input && reported.is_none_or(|t| t.elapsed() >= ACTIVITY_EVERY) {
            reported = Some(Instant::now());
            sink.send(Event::Idle(IdleEvent::DeviceInput))?;
        }
    }
}

/// Whether the lid's switch on `w` is closed. If it can't be read, it counts as open.
fn lid_switch(w: &Watched) -> bool {
    w.device
        .get_switch_state()
        .inspect_err(|e| log::warn!("input watch: {}: the lid switch: {e}", w.path.display()))
        .is_ok_and(|on| on.contains(SwitchCode::SW_LID))
}

/// Waits until a device has events or /dev/input changed (`notify`). Without `notify` it waits at most
/// [`RESCAN_EVERY`]. Returns whether to look for devices again.
fn wait(devices: &[Watched], notify: Option<&OwnedFd>) -> bool {
    let mut fds: Vec<PollFd> = devices.iter().map(|d| PollFd::new(&d.device, PollFlags::IN)).collect();
    let Some(notify) = notify else {
        let timeout = crate::os::sys::timespec(RESCAN_EVERY);
        if fds.is_empty() {
            std::thread::sleep(RESCAN_EVERY);
        } else {
            if let Err(e) = rustix::event::poll(&mut fds, Some(&timeout)) {
                log::debug!("input watch: polling: {e}");
            }
        }
        return false;
    };
    fds.push(PollFd::new(notify, PollFlags::IN));
    if let Err(e) = rustix::event::poll(&mut fds, None) {
        log::debug!("input watch: polling: {e}");
    }
    let changed = fds.last().is_some_and(|f| f.revents().contains(PollFlags::IN));
    if changed {
        let mut buf = [std::mem::MaybeUninit::uninit(); 4096];
        let mut reader = inotify::Reader::new(notify, &mut buf);
        while reader.next().is_ok() {}
    }
    changed
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hardware_keys_and_their_presses() {
        let key = |code: KeyCode, press: Press| Raw::key(code, press);
        assert_eq!(device_key(key(KeyCode::KEY_VOLUMEUP, Press::Down)), Some(HardwareKey::VolumeUp));
        assert_eq!(device_key(key(KeyCode::KEY_VOLUMEDOWN, Press::Repeat)), Some(HardwareKey::VolumeDown), "held: repeats");
        assert_eq!(device_key(key(KeyCode::KEY_VOLUMEUP, Press::Up)), None);
        assert_eq!(device_key(key(KeyCode::KEY_MUTE, Press::Repeat)), None, "mute toggles once");
        assert_eq!(device_key(key(KeyCode::KEY_A, Press::Down)), None);
    }
}
