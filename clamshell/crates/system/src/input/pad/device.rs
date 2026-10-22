//! The real controller and our virtual devices: a pad identified as an Xbox 360 controller (for games)
//! and a mouse with a keyboard's keys (the pad mouse, typing, what a keymap presses).

use std::io;

use evdev::raw_stream::RawDevice;
use evdev::uinput::VirtualDevice;
use evdev::{AbsInfo, AbsoluteAxisCode, AttributeSet, BusType, FFEffectCode, InputEvent, InputId, KeyCode, RelativeAxisCode, UinputAbsSetup};

use super::remap::{AYN_KEY, BACK_KEY};
use super::route::{Axis, PadSnapshot, Ranges, mouse_device_keys};
use crate::os::evdev::device::{self, DeviceSpec};
use crate::os::evdev::{Press, Range, Raw, keyset};

const VIRTUAL_PAD: &str = "clamshell pad (Xbox 360)";
/// The virtual pad's USB ids: a wired Xbox 360 controller's.
pub const VIRTUAL_IDS: (u16, u16) = (0x045e, 0x028e);
const VIRTUAL_MOUSE: &str = "clamshell pad mouse";

/// The configured controller, or the first device with gamepad buttons and two sticks (not our own).
pub fn find(spec: &DeviceSpec) -> io::Result<RawDevice> {
    let gamepad = |d: &RawDevice| {
        d.supported_keys().is_some_and(|k| k.contains(KeyCode::BTN_SOUTH)) && d.supported_absolute_axes().is_some_and(|a| a.contains(AbsoluteAxisCode::ABS_RX))
    };
    device::open(spec, "controller", gamepad, &[VIRTUAL_PAD])
}

/// The device the Thor's AYN button is on (its GPIO keys), read alongside the controller for that one
/// key. Not grabbed: other keys on it go where they would.
pub struct AynKey {
    device: RawDevice,
}

impl AynKey {
    const NAME: &'static str = "gpio-keys-ayn";

    pub fn find() -> Option<AynKey> {
        let device =
            evdev::raw_stream::enumerate().map(|(_, d)| d).find(|d| d.name() == Some(Self::NAME) && d.supported_keys().is_some_and(|k| k.contains(AYN_KEY)))?;
        let nonblocking = rustix::fs::fcntl_getfl(&device).and_then(|f| rustix::fs::fcntl_setfl(&device, f | rustix::fs::OFlags::NONBLOCK));
        if let Err(e) = nonblocking {
            log::warn!("pad: AYN button: {e}");
            return None;
        }
        log::info!("pad: the AYN button from {}", Self::NAME);
        Some(AynKey { device })
    }

    pub fn device(&self) -> &RawDevice {
        &self.device
    }

    /// Appends the AYN button's presses waiting, each with its report.
    pub fn fetch(&mut self, out: &mut Vec<Raw>) -> io::Result<()> {
        match self.device.fetch_events() {
            Ok(events) => {
                for e in events.filter(|e| e.event_type() == evdev::EventType::KEY && e.code() == AYN_KEY.0) {
                    out.extend([Raw::from(e), Raw::REPORT]);
                }
                Ok(())
            }
            Err(e) if e.kind() == io::ErrorKind::WouldBlock => Ok(()),
            Err(e) => Err(e),
        }
    }
}

/// A controller's buttons and axes (each with its range and current value).
#[derive(Debug, Clone)]
pub struct Capabilities {
    pub keys: Vec<KeyCode>,
    pub axes: Vec<(AbsoluteAxisCode, AbsInfo)>,
}

/// The same buttons and axes with the same ranges: the axes' current values don't count (a controller
/// back with a stick a little off is the same controller).
impl PartialEq for Capabilities {
    fn eq(&self, other: &Self) -> bool {
        let shape = |(code, a): &(AbsoluteAxisCode, AbsInfo)| (*code, a.minimum(), a.maximum(), a.fuzz(), a.flat(), a.resolution());
        self.keys == other.keys && self.axes.iter().map(shape).eq(other.axes.iter().map(shape))
    }
}

impl Capabilities {
    pub fn of(real: &RawDevice) -> io::Result<Capabilities> {
        let abs = real.get_abs_state()?;
        let axes = real
            .supported_absolute_axes()
            .map(|set| {
                set.iter()
                    .filter_map(|code| abs.get(usize::from(code.0)).map(|a| (code, AbsInfo::new(a.value, a.minimum, a.maximum, a.fuzz, a.flat, a.resolution))))
                    .collect()
            })
            .unwrap_or_default();
        let keys = real.supported_keys().map(|k| k.iter().collect()).unwrap_or_default();
        Ok(Capabilities { keys, axes })
    }

    fn range(&self, code: AbsoluteAxisCode) -> Range {
        self.axes.iter().find(|(c, _)| *c == code).map_or(Range::STICK, |(_, a)| Range { min: a.minimum(), max: a.maximum() })
    }

    pub fn ranges(&self) -> Ranges {
        Ranges { stick: self.range(AbsoluteAxisCode::ABS_X), trigger: self.range(AbsoluteAxisCode::ABS_RZ) }
    }

    /// The axes as they are now.
    pub fn snapshot(&self) -> PadSnapshot {
        PadSnapshot::new(self.axes.iter().map(|(code, a)| Axis { code: *code, value: a.value(), rest: rest(a) }).collect())
    }
}

/// An axis at rest: a stick at 0, a trigger at its minimum.
fn rest(axis: &AbsInfo) -> i32 {
    if axis.minimum() < 0 { 0 } else { axis.minimum() }
}

/// The virtual devices, kept across the controller's comings and goings (an unplug, an error), so games
/// never see their pad go: made for the first controller, made again only for one with other buttons.
pub struct VirtualDevices {
    pub made_for: Capabilities,
    pub pad: VirtualDevice,
    pub mouse: VirtualDevice,
    /// The pad untouched: every button up, every axis at rest (shown while no controller is there).
    neutral: Vec<Raw>,
}

impl VirtualDevices {
    pub fn new(caps: Capabilities) -> io::Result<VirtualDevices> {
        let mut neutral: Vec<Raw> = pad_keys(&caps).map(|k| Raw::key(k, Press::Up)).collect();
        neutral.extend(pad_axes(&caps).iter().map(|(code, a)| Raw::abs(*code, rest(a))));
        neutral.push(Raw::REPORT);
        Ok(VirtualDevices { pad: virtual_pad(&caps)?, mouse: virtual_mouse()?, made_for: caps, neutral })
    }

    /// The controller is gone: the virtual pad stays, at rest.
    pub fn rest(&mut self) -> io::Result<()> {
        emit(&mut self.pad, &self.neutral)
    }
}

/// Whether the controller (by its name and physical path) is driven by xpad or rsinput (the Thor's own
/// pad), which name their face buttons' codes by an Xbox pad's letters (the left one sends BTN_X: the code
/// Linux calls BTN_NORTH; PB-OS found rsinput's top one sending BTN_WEST, BTN_Y) rather than by their
/// place, as Linux has it (BTN_WEST the left one, BTN_NORTH the top one) and ClamShell does.
pub fn letter_codes(name: &str, phys: &str) -> bool {
    let inputs = match std::fs::read_dir("/sys/class/input") {
        Ok(inputs) => inputs,
        Err(e) => {
            log::warn!("pad: reading /sys/class/input: {e}");
            return false;
        }
    };
    let read = |p: std::path::PathBuf| {
        std::fs::read_to_string(&p).map(|s| s.trim().to_owned()).inspect_err(|e| log::trace!("pad: reading {}: {e}", p.display())).unwrap_or_default()
    };
    let entries = inputs.filter_map(|e| e.inspect_err(|e| log::debug!("pad: reading /sys/class/input: {e}")).ok());
    entries.map(|e| e.path().join("device")).any(|dev| {
        read(dev.join("name")) == name
            && read(dev.join("phys")) == phys
            && std::fs::read_link(dev.join("device/driver")).is_ok_and(|d| d.file_name().is_some_and(|n| n == "xpad" || n == "rsinput"))
    })
}

/// X and Y between the two conventions (the same swap either way): in, a [`letter_codes`] controller's
/// events to ClamShell's; out, ClamShell's to the virtual pad's, which games read as an Xbox 360 pad's
/// (xpad's: by letter).
pub fn swap_xy(events: &mut [Raw]) {
    for e in events.iter_mut().filter(|e| e.kind == evdev::EventType::KEY) {
        e.code = swap_key(KeyCode(e.code)).0;
    }
}

/// [`swap_xy`] for one key.
pub fn swap_key(key: KeyCode) -> KeyCode {
    match key {
        KeyCode::BTN_NORTH => KeyCode::BTN_WEST,
        KeyCode::BTN_WEST => KeyCode::BTN_NORTH,
        key => key,
    }
}

/// Emits events on a virtual device.
pub fn emit(device: &mut VirtualDevice, events: &[Raw]) -> io::Result<()> {
    let events: Vec<InputEvent> = events.iter().map(|&r| r.into()).collect();
    device.emit(&events)
}

/// The D-pad's buttons, on controllers that have it as buttons.
const DPAD_KEYS: [KeyCode; 4] = [KeyCode::BTN_DPAD_UP, KeyCode::BTN_DPAD_DOWN, KeyCode::BTN_DPAD_LEFT, KeyCode::BTN_DPAD_RIGHT];

/// The real pad's keys the virtual one has: all but Back (an input of keymaps, nothing as on the pad)
/// and the D-pad's buttons (its D-pad is a hat, as an Xbox 360 pad's: SDL takes a pad with both for one
/// whose D-pad is its buttons, and finds none); and Guide (the AYN button as on the pad; home's with
/// Back) though the controller has none.
fn pad_keys(caps: &Capabilities) -> impl Iterator<Item = KeyCode> + '_ {
    let guide = (!caps.keys.contains(&KeyCode::BTN_MODE)).then_some(KeyCode::BTN_MODE);
    caps.keys.iter().copied().filter(|&k| k != BACK_KEY && !DPAD_KEYS.contains(&k)).chain(guide)
}

/// The real pad's axes the virtual one has: all, and the D-pad's hat though the controller has none.
fn pad_axes(caps: &Capabilities) -> Vec<(AbsoluteAxisCode, AbsInfo)> {
    let mut axes = caps.axes.clone();
    for hat in [AbsoluteAxisCode::ABS_HAT0X, AbsoluteAxisCode::ABS_HAT0Y] {
        if !axes.iter().any(|(code, _)| *code == hat) {
            axes.push((hat, AbsInfo::new(0, -1, 1, 0, 0, 0)));
        }
    }
    axes
}

/// A pad with the real one's buttons and axes, identified as a wired Xbox 360 controller so SDL, Wine
/// and Steam map it without configuration.
fn virtual_pad(caps: &Capabilities) -> io::Result<VirtualDevice> {
    // Rumble, as an Xbox 360 pad has it (games' effects come back on its file, read without blocking).
    let ff: AttributeSet<FFEffectCode> = [FFEffectCode::FF_RUMBLE].into_iter().collect();
    let mut builder = VirtualDevice::builder()?
        .name(VIRTUAL_PAD)
        .input_id(InputId::new(BusType::BUS_USB, VIRTUAL_IDS.0, VIRTUAL_IDS.1, 0x0110))
        .with_ff(&ff)?
        .with_ff_effects_max(super::rumble::EFFECTS_MAX);
    let keys: Vec<KeyCode> = pad_keys(caps).collect();
    if !keys.is_empty() {
        builder = builder.with_keys(&keyset(&keys))?;
    }
    for (code, info) in pad_axes(caps) {
        builder = builder.with_absolute_axis(&UinputAbsSetup::new(code, info))?;
    }
    let pad = builder.build()?;
    let flags = rustix::fs::fcntl_getfl(&pad)?;
    rustix::fs::fcntl_setfl(&pad, flags | rustix::fs::OFlags::NONBLOCK)?;
    Ok(pad)
}

fn virtual_mouse() -> io::Result<VirtualDevice> {
    let rel: AttributeSet<RelativeAxisCode> = [
        RelativeAxisCode::REL_X,
        RelativeAxisCode::REL_Y,
        RelativeAxisCode::REL_WHEEL,
        RelativeAxisCode::REL_HWHEEL,
        RelativeAxisCode::REL_WHEEL_HI_RES,
        RelativeAxisCode::REL_HWHEEL_HI_RES,
    ]
    .into_iter()
    .collect();
    VirtualDevice::builder()?.name(VIRTUAL_MOUSE).with_keys(&keyset(&mouse_device_keys()))?.with_relative_axes(&rel)?.build()
}

#[cfg(test)]
mod letter_tests {
    use super::*;
    use crate::os::evdev::Press;

    #[test]
    fn a_controller_back_with_a_stick_moved_is_the_same() {
        let caps = |x: i32| Capabilities { keys: vec![KeyCode::BTN_SOUTH], axes: vec![(AbsoluteAxisCode::ABS_X, AbsInfo::new(x, -1024, 1024, 16, 32, 0))] };
        assert_eq!(caps(0), caps(37));
        let other = Capabilities { axes: vec![(AbsoluteAxisCode::ABS_X, AbsInfo::new(0, -1408, 1408, 16, 32, 0))], ..caps(0) };
        assert_ne!(caps(0), other, "another range");
    }

    #[test]
    fn x_and_y_swap_between_letters_and_places() {
        let mut events =
            [Raw::key(KeyCode::BTN_NORTH, Press::Down), Raw::key(KeyCode::BTN_WEST, Press::Up), Raw::key(KeyCode::BTN_SOUTH, Press::Down), Raw::REPORT];
        swap_xy(&mut events);
        assert_eq!(
            events,
            [Raw::key(KeyCode::BTN_WEST, Press::Down), Raw::key(KeyCode::BTN_NORTH, Press::Up), Raw::key(KeyCode::BTN_SOUTH, Press::Down), Raw::REPORT]
        );
    }
}
