//! The real controller and our virtual devices: a pad identified as an Xbox 360 controller for games,
//! and a mouse with a keyboard's keys for the pad mouse, typing and what a keymap presses.

use std::io;

use evdev::raw_stream::RawDevice;
use evdev::uinput::VirtualDevice;
use evdev::{AbsInfo, AbsoluteAxisCode, AttributeSet, BusType, FFEffectCode, InputEvent, InputId, KeyCode, RelativeAxisCode, UinputAbsSetup};

use super::remap::{AYN_KEY, BACK_KEY};
use super::route::{Axis, PadSnapshot, Ranges, mouse_device_keys};
use crate::os::evdev::device::{self, DeviceSpec};
use crate::os::evdev::{Press, Range, Raw, keyset};
use clamshell_device::input::{AuxiliaryKey as AuxiliaryHint, Controls};

const VIRTUAL_PAD: &str = "clamshell pad (Xbox 360)";
/// The virtual pad's USB ids, which are a wired Xbox 360 controller's.
pub(crate) const VIRTUAL_IDS: (u16, u16) = (0x045e, 0x028e);
const VIRTUAL_MOUSE: &str = "clamshell pad mouse";

/// The configured controller, or else the first device other than our own with gamepad buttons and two
/// sticks.
pub(crate) fn find(spec: &DeviceSpec) -> io::Result<RawDevice> {
    let gamepad = |d: &RawDevice| {
        d.supported_keys().is_some_and(|k| k.contains(KeyCode::BTN_SOUTH)) && d.supported_absolute_axes().is_some_and(|a| a.contains(AbsoluteAxisCode::ABS_RX))
    };
    device::open(spec, "controller", gamepad, &[VIRTUAL_PAD])
}

/// The device the Thor's AYN button is on, its GPIO keys. It is read alongside the controller for that one
/// key. It isn't grabbed, so other keys on it go where they would.
pub(crate) struct AuxiliaryKey {
    device: RawDevice,
    code: KeyCode,
}

impl AuxiliaryKey {
    pub(crate) fn find(hint: Option<AuxiliaryHint>) -> Option<AuxiliaryKey> {
        let hint = hint?;
        let code = KeyCode(hint.code);
        let device = evdev::raw_stream::enumerate()
            .map(|(_, device)| device)
            .find(|device| device.name() == Some(hint.device_name) && device.supported_keys().is_some_and(|keys| keys.contains(code)))?;
        let nonblocking = rustix::fs::fcntl_getfl(&device).and_then(|f| rustix::fs::fcntl_setfl(&device, f | rustix::fs::OFlags::NONBLOCK));
        if let Err(e) = nonblocking {
            log::warn!("pad: AYN button: {e}");
            return None;
        }
        log::info!("pad: the auxiliary button from {}", hint.device_name);
        Some(AuxiliaryKey { device, code })
    }

    pub(crate) fn device(&self) -> &RawDevice {
        &self.device
    }

    /// Appends the AYN button's waiting presses, each with its report.
    pub(crate) fn fetch(&mut self, out: &mut Vec<Raw>) -> io::Result<()> {
        match self.device.fetch_events() {
            Ok(events) => {
                for e in events.filter(|e| e.event_type() == evdev::EventType::KEY && e.code() == self.code.0) {
                    out.extend([Raw { kind: evdev::EventType::KEY, code: AYN_KEY.0, value: e.value() }, Raw::REPORT]);
                }
                Ok(())
            }
            Err(e) if e.kind() == io::ErrorKind::WouldBlock => Ok(()),
            Err(e) => Err(e),
        }
    }
}

/// A controller's buttons and axes, each axis with its range and current value.
#[derive(Debug, Clone)]
pub(crate) struct Capabilities {
    keys: Vec<KeyCode>,
    axes: Vec<(AbsoluteAxisCode, AbsInfo)>,
}

/// Equal means the same buttons and axes with the same ranges. The axes' current values don't count, so
/// a controller that comes back with a stick a little off is the same controller.
impl PartialEq for Capabilities {
    fn eq(&self, other: &Self) -> bool {
        let shape = |(code, a): &(AbsoluteAxisCode, AbsInfo)| (*code, a.minimum(), a.maximum(), a.fuzz(), a.flat(), a.resolution());
        self.keys == other.keys && self.axes.iter().map(shape).eq(other.axes.iter().map(shape))
    }
}

impl Capabilities {
    pub(crate) fn of(real: &RawDevice) -> io::Result<Capabilities> {
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

    pub(crate) fn ranges(&self) -> Ranges {
        Ranges { stick: self.range(AbsoluteAxisCode::ABS_X), trigger: self.range(AbsoluteAxisCode::ABS_RZ) }
    }

    /// The axes as they are now.
    pub(crate) fn snapshot(&self) -> PadSnapshot {
        PadSnapshot::new(self.axes.iter().map(|(code, a)| Axis { code: *code, value: a.value(), rest: rest(a) }).collect())
    }
}

/// An axis at rest: a stick at 0, a trigger at its minimum.
fn rest(axis: &AbsInfo) -> i32 {
    if axis.minimum() < 0 { 0 } else { axis.minimum() }
}

/// The virtual devices, kept while the controller comes and goes through an unplug or an error, so games
/// never see their pad go. They are made for the first controller, and made again only for one with
/// other buttons or axes.
pub(crate) struct VirtualDevices {
    pub(crate) made_for: Capabilities,
    pub(crate) pad: VirtualDevice,
    pub(crate) mouse: VirtualDevice,
    /// The untouched pad, with every button up and every axis at rest. It is shown while no controller is
    /// there.
    neutral: Vec<Raw>,
}

impl VirtualDevices {
    pub(crate) fn new(caps: Capabilities, controls: Controls) -> io::Result<VirtualDevices> {
        let mut neutral: Vec<Raw> = pad_keys(&caps, controls).map(|k| Raw::key(k, Press::Up)).collect();
        neutral.extend(pad_axes(&caps).iter().map(|(code, a)| Raw::abs(*code, rest(a))));
        neutral.push(Raw::REPORT);
        Ok(VirtualDevices { pad: virtual_pad(&caps, controls)?, mouse: virtual_mouse()?, made_for: caps, neutral })
    }

    /// Puts the virtual pad at rest when the controller is gone. The pad itself stays.
    pub(crate) fn rest(&mut self) -> io::Result<()> {
        emit(&mut self.pad, &self.neutral)
    }
}

/// Whether the controller, found by its name and physical path, is driven by xpad or by rsinput, the
/// Thor's own pad. These drivers give the face buttons' codes by an Xbox pad's letters rather than by
/// place. The left button sends BTN_X, which is the code Linux calls BTN_NORTH, and PB-OS found rsinput's
/// top button sending BTN_WEST, which is BTN_Y. Linux and ClamShell go by place: BTN_WEST is the left
/// button and BTN_NORTH the top one.
pub(crate) fn letter_codes(controls: Controls, name: &str, phys: &str) -> bool {
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
            && std::fs::read_link(dev.join("device/driver"))
                .is_ok_and(|d| d.file_name().and_then(|name| name.to_str()).is_some_and(|driver| controls.letter_code_drivers.contains(&driver)))
    })
}

/// Swaps X and Y between the two conventions. The swap is the same either way. Coming in, it turns a
/// [`letter_codes`] controller's events into ClamShell's. Going out, it turns ClamShell's into the
/// virtual pad's, which games read as an Xbox 360 pad's, by letter as xpad does.
pub(crate) fn swap_xy(events: &mut [Raw]) {
    for e in events.iter_mut().filter(|e| e.kind == evdev::EventType::KEY) {
        e.code = swap_key(KeyCode(e.code)).0;
    }
}

/// [`swap_xy`] for one key.
pub(crate) fn swap_key(key: KeyCode) -> KeyCode {
    match key {
        KeyCode::BTN_NORTH => KeyCode::BTN_WEST,
        KeyCode::BTN_WEST => KeyCode::BTN_NORTH,
        key => key,
    }
}

/// Emits events on a virtual device.
pub(crate) fn emit(device: &mut VirtualDevice, events: &[Raw]) -> io::Result<()> {
    let events: Vec<InputEvent> = events.iter().map(|&r| r.into()).collect();
    device.emit(&events)
}

/// The D-pad's buttons, on controllers that have it as buttons.
const DPAD_KEYS: [KeyCode; 4] = [KeyCode::BTN_DPAD_UP, KeyCode::BTN_DPAD_DOWN, KeyCode::BTN_DPAD_LEFT, KeyCode::BTN_DPAD_RIGHT];

/// The real pad's keys that the virtual one has. That is all of them except Back, which is a keymap input
/// that does nothing as on the pad, and the D-pad's buttons. The virtual D-pad is a hat, as on an Xbox 360
/// pad, because SDL takes a pad with both for one whose D-pad is its buttons, and finds none. Guide is
/// added even if the controller has none, for the AYN button as on the pad and for Home with Back.
fn pad_keys(caps: &Capabilities, controls: Controls) -> impl Iterator<Item = KeyCode> + '_ {
    let guide = (!caps.keys.contains(&KeyCode::BTN_MODE)).then_some(KeyCode::BTN_MODE);
    caps.keys.iter().copied().filter(move |&k| k != BACK_KEY && controls.back_key != Some(k.0) && !DPAD_KEYS.contains(&k)).chain(guide)
}

/// The real pad's axes that the virtual one has: all of them, plus the D-pad's hat even if the controller
/// has none.
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
fn virtual_pad(caps: &Capabilities, controls: Controls) -> io::Result<VirtualDevice> {
    // Rumble, as an Xbox 360 pad has it. Games' effects come back on its file, which is read without
    // blocking.
    let ff: AttributeSet<FFEffectCode> = [FFEffectCode::FF_RUMBLE].into_iter().collect();
    let mut builder = VirtualDevice::builder()?
        .name(VIRTUAL_PAD)
        .input_id(InputId::new(BusType::BUS_USB, VIRTUAL_IDS.0, VIRTUAL_IDS.1, 0x0110))
        .with_ff(&ff)?
        .with_ff_effects_max(super::rumble::EFFECTS_MAX);
    let keys: Vec<KeyCode> = pad_keys(caps, controls).collect();
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

pub(crate) fn normalise_controls(controls: Controls, raws: &mut [Raw]) {
    for raw in raws.iter_mut().filter(|raw| raw.kind == evdev::EventType::KEY) {
        raw.code = controls.normalise_button(raw.code);
    }
}
#[cfg(test)]
mod profile_tests {
    use super::*;
    #[test]
    fn control_normalisation_preserves_other_keys_and_axis_codes() {
        let controls = Controls { back_key: Some(123), auxiliary: None, letter_code_drivers: &[] };
        let mut events =
            [Raw::key(KeyCode(123), Press::Down), Raw::key(KeyCode::BTN_SOUTH, Press::Down), Raw { kind: evdev::EventType::ABSOLUTE, code: 123, value: 1 }];
        normalise_controls(controls, &mut events);
        assert_eq!(events.first(), Some(&Raw::key(BACK_KEY, Press::Down)));
        assert_eq!(events.get(1), Some(&Raw::key(KeyCode::BTN_SOUTH, Press::Down)));
        assert_eq!(events.get(2).map(|event| event.code), Some(123));
        let caps = Capabilities { keys: vec![KeyCode(123), KeyCode::BTN_SOUTH], axes: Vec::new() };
        assert_eq!(pad_keys(&caps, controls).collect::<Vec<_>>(), [KeyCode::BTN_SOUTH, KeyCode::BTN_MODE]);
    }
}
