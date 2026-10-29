//! The real controllers and our virtual devices: a pad per player identified as an Xbox 360 controller for
//! games, and a mouse with a keyboard's keys for the pad mouse, typing and what keymaps press.

use std::io;

use evdev::raw_stream::RawDevice;
use evdev::uinput::VirtualDevice;
use evdev::{AbsInfo, AbsoluteAxisCode, AttributeSet, BusType, FFEffectCode, InputEvent, InputId, KeyCode, RelativeAxisCode, UinputAbsSetup};

use super::codes::AYN_KEY;
use super::merge::Normalise;
use super::route::{Axis, PadSnapshot, mouse_device_keys};
use super::rumble::Effects;
use crate::os::evdev::device::DeviceSpec;
use crate::os::evdev::{Press, Range, Raw, keyset};
use clamshell_device::input::{AuxiliaryKey as AuxiliaryHint, Controls};

const VIRTUAL_PAD: &str = "clamshell pad (Xbox 360)";
/// The virtual pad's USB ids, which are a wired Xbox 360 controller's.
pub(crate) const VIRTUAL_IDS: (u16, u16) = (0x045e, 0x028e);
const VIRTUAL_MOUSE: &str = "clamshell pad mouse";

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
        if let Err(e) = crate::os::set_nonblocking(&device) {
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
        let Some(events) = crate::os::ready(self.device.fetch_events())? else { return Ok(()) };
        for e in events.filter(|e| e.event_type() == evdev::EventType::KEY && e.code() == self.code.0) {
            out.extend([Raw { kind: evdev::EventType::KEY, code: AYN_KEY.0, value: e.value() }, Raw::REPORT]);
        }
        Ok(())
    }
}

/// A controller's buttons and axes, each axis with its range and current value.
#[derive(Debug, Clone)]
pub(crate) struct Capabilities {
    keys: Vec<KeyCode>,
    axes: Vec<(AbsoluteAxisCode, AbsInfo)>,
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

    /// How its axes become the virtual pad's.
    pub(crate) fn normalise(&self) -> Normalise {
        let dpad = self.keys.iter().any(|k| DPAD_KEYS.contains(k));
        Normalise::new(self.axes.iter().map(|(code, a)| (*code, Range { min: a.minimum(), max: a.maximum() })), dpad)
    }

    /// Its sticks' range, read from the left stick's x axis.
    pub(crate) fn stick_range(&self) -> Range {
        self.axes.iter().find(|(c, _)| *c == AbsoluteAxisCode::ABS_X).map_or(Range::STICK, |(_, a)| Range { min: a.minimum(), max: a.maximum() })
    }

    /// The axes as they are now, raw.
    pub(crate) fn snapshot(&self) -> PadSnapshot {
        PadSnapshot::new(self.axes.iter().map(|(code, a)| Axis { code: *code, value: a.value(), rest: a.value() }).collect())
    }
}

/// The D-pad's buttons, on controllers that have it as buttons. The virtual pad has the hat instead.
const DPAD_KEYS: [KeyCode; 4] = [KeyCode::BTN_DPAD_UP, KeyCode::BTN_DPAD_DOWN, KeyCode::BTN_DPAD_LEFT, KeyCode::BTN_DPAD_RIGHT];

/// The virtual pad's buttons: an Xbox 360 pad's, as xpad gives them. Its D-pad is the hat, since SDL
/// takes a pad with both for one whose D-pad is its buttons, and finds none.
pub(crate) const PAD_KEYS: [KeyCode; 11] = [
    KeyCode::BTN_SOUTH,
    KeyCode::BTN_EAST,
    KeyCode::BTN_NORTH,
    KeyCode::BTN_WEST,
    KeyCode::BTN_TL,
    KeyCode::BTN_TR,
    KeyCode::BTN_SELECT,
    KeyCode::BTN_START,
    KeyCode::BTN_MODE,
    KeyCode::BTN_THUMBL,
    KeyCode::BTN_THUMBR,
];

/// A player's virtual pad, identified as a wired Xbox 360 controller so SDL, Wine and Steam map it
/// without configuration, with games' rumble effects on it. It has an Xbox 360 pad's buttons and axes
/// whatever the controllers are ([`PAD_KEYS`], [`super::merge::PAD_AXES`]), so it never has to be made
/// again for another controller, and a game keeps it while controllers come and go.
pub(crate) struct VirtualPad {
    pub(crate) device: VirtualDevice,
    /// Games' rumble effects on it.
    pub(crate) effects: Effects,
    /// The events being emitted, kept to be used again.
    scratch: Vec<InputEvent>,
}

impl VirtualPad {
    pub(crate) fn new() -> io::Result<VirtualPad> {
        // Rumble, as an Xbox 360 pad has it. Games' effects come back on its file, which is read without
        // blocking.
        let ff: AttributeSet<FFEffectCode> = [FFEffectCode::FF_RUMBLE].into_iter().collect();
        let mut builder = VirtualDevice::builder()?
            .name(VIRTUAL_PAD)
            .input_id(InputId::new(BusType::BUS_USB, VIRTUAL_IDS.0, VIRTUAL_IDS.1, 0x0110))
            .with_ff(&ff)?
            .with_ff_effects_max(super::rumble::EFFECTS_MAX)
            .with_keys(&keyset(&PAD_KEYS))?;
        for (i, code) in super::merge::PAD_AXES.iter().enumerate() {
            let r = super::merge::pad_range(i);
            // A stick's noise and dead zone as xpad gives them.
            let (fuzz, flat) = if r == Range::STICK { (16, 128) } else { (0, 0) };
            builder = builder.with_absolute_axis(&UinputAbsSetup::new(*code, AbsInfo::new(0, r.min, r.max, fuzz, flat, 0)))?;
        }
        let device = builder.build()?;
        crate::os::set_nonblocking(&device)?;
        Ok(VirtualPad { device, effects: Effects::default(), scratch: Vec::new() })
    }

    pub(crate) fn emit(&mut self, events: &[Raw]) -> io::Result<()> {
        emit(&mut self.device, &mut self.scratch, events)
    }

    /// Every button up and every axis at rest, when its controllers are gone or a run ended in a panic.
    pub(crate) fn rest(&mut self) -> io::Result<()> {
        let mut neutral = [Raw::REPORT; PAD_KEYS.len() + super::merge::PAD_AXES.len() + 1];
        for (slot, raw) in
            neutral.iter_mut().zip(PAD_KEYS.iter().map(|&k| Raw::key(k, Press::Up)).chain(super::merge::PAD_AXES.iter().map(|&a| Raw::abs(a, 0))))
        {
            *slot = raw;
        }
        self.emit(&neutral)
    }
}

/// The virtual mouse with a keyboard's keys, for the pad mouse, typing and what keymaps press. Every
/// player's keymap presses on it.
pub(crate) struct VirtualMouse {
    device: VirtualDevice,
    scratch: Vec<InputEvent>,
    /// Every key and button up.
    up: Vec<Raw>,
}

impl VirtualMouse {
    pub(crate) fn new() -> io::Result<VirtualMouse> {
        let mut up: Vec<Raw> = mouse_device_keys().into_iter().map(|k| Raw::key(k, Press::Up)).collect();
        up.push(Raw::REPORT);
        Ok(VirtualMouse { device: virtual_mouse()?, scratch: Vec::new(), up })
    }

    pub(crate) fn emit(&mut self, events: &[Raw]) -> io::Result<()> {
        emit(&mut self.device, &mut self.scratch, events)
    }

    /// Lets go of every key and button.
    pub(crate) fn rest(&mut self) -> io::Result<()> {
        emit(&mut self.device, &mut self.scratch, &self.up)
    }
}

/// The candidates for controllers: the one CLAMSHELL_GAMEPAD names, or else every device other than our
/// own with gamepad buttons and two sticks, each with its /dev/input path.
pub(crate) fn candidates(spec: &DeviceSpec) -> Vec<(std::path::PathBuf, RawDevice)> {
    let not_ours = |d: &RawDevice| d.name().is_none_or(|n| n != VIRTUAL_PAD && n != VIRTUAL_MOUSE);
    match spec {
        DeviceSpec::Path(path) => match RawDevice::open(path) {
            Ok(d) => vec![(path.clone(), d)],
            Err(e) => {
                log::debug!("pad: {}: {e}", path.display());
                Vec::new()
            }
        },
        DeviceSpec::Named(name) => {
            let named: Vec<_> = evdev::raw_stream::enumerate().filter(|(_, d)| not_ours(d) && d.name().is_some_and(|n| n.contains(name.as_str()))).collect();
            // A controller's motion sensor shares its name: the one that looks like a gamepad first.
            let first = named.iter().position(|(_, d)| gamepad(d)).unwrap_or(0);
            named.into_iter().nth(first).into_iter().collect()
        }
        DeviceSpec::Auto => evdev::raw_stream::enumerate().filter(|(_, d)| not_ours(d) && gamepad(d)).collect(),
    }
}

/// Whether a device looks like a gamepad: buttons and two sticks.
fn gamepad(d: &RawDevice) -> bool {
    d.supported_keys().is_some_and(|k| k.contains(KeyCode::BTN_SOUTH)) && d.supported_absolute_axes().is_some_and(|a| a.contains(AbsoluteAxisCode::ABS_RX))
}

/// A controller's key, by which its player is remembered: its USB ids and its unique id where it has one
/// (a Bluetooth controller's address, some USB controllers' serial number), or else its name. A controller
/// without a unique id is the same controller in any port; two such of one kind share a key, and so a
/// player.
pub(crate) fn key(vendor: u16, product: u16, uniq: Option<&str>, name: &str) -> String {
    match uniq.map(str::trim).filter(|u| !u.is_empty()) {
        Some(uniq) => format!("{vendor:04x}:{product:04x}:{uniq}"),
        None => format!("{vendor:04x}:{product:04x}:{name}"),
    }
}

/// The kernel driver of the controller named `name` at `phys`, from /sys/class/input, if it is found.
pub(crate) fn driver(name: &str, phys: &str) -> Option<String> {
    let inputs = match std::fs::read_dir("/sys/class/input") {
        Ok(inputs) => inputs,
        Err(e) => {
            log::warn!("pad: reading /sys/class/input: {e}");
            return None;
        }
    };
    let read = |p: std::path::PathBuf| {
        std::fs::read_to_string(&p).map(|s| s.trim().to_owned()).inspect_err(|e| log::trace!("pad: reading {}: {e}", p.display())).unwrap_or_default()
    };
    let entries = inputs.filter_map(|e| e.inspect_err(|e| log::debug!("pad: reading /sys/class/input: {e}")).ok());
    entries
        .map(|e| e.path().join("device"))
        .find(|dev| read(dev.join("name")) == name && read(dev.join("phys")) == phys)
        .and_then(|dev| std::fs::read_link(dev.join("device/driver")).ok().and_then(|d| d.file_name().and_then(|n| n.to_str()).map(str::to_owned)))
}

/// Whether a controller driven by `driver` gives its face buttons by letter: xpad and rsinput, the Thor's
/// own pad, give the codes by an Xbox pad's letters rather than by place. The left button sends BTN_X,
/// which is the code Linux calls BTN_NORTH, and PB-OS found rsinput's top button sending BTN_WEST, which is
/// BTN_Y. Linux and ClamShell go by place: BTN_WEST is the left button and BTN_NORTH the top one.
pub(crate) fn letter_codes(controls: Controls, driver: Option<&str>) -> bool {
    driver.is_some_and(|d| controls.letter_code_drivers.contains(&d))
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

/// Emits events on a virtual device, through `scratch`, which keeps its room for the next.
fn emit(device: &mut VirtualDevice, scratch: &mut Vec<InputEvent>, events: &[Raw]) -> io::Result<()> {
    scratch.clear();
    scratch.extend(events.iter().map(|&r| InputEvent::from(r)));
    device.emit(scratch)
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

pub(crate) fn normalise_controls(controls: Controls, raws: &mut [Raw]) {
    for raw in raws.iter_mut().filter(|raw| raw.kind == evdev::EventType::KEY) {
        raw.code = controls.normalise_button(raw.code);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::os::evdev::Press;

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

    #[test]
    fn control_normalisation_preserves_other_keys_and_axis_codes() {
        let controls = Controls { back_key: Some(123), auxiliary: None, letter_code_drivers: &[], power_key: None, builtin_driver: None };
        let mut events =
            [Raw::key(KeyCode(123), Press::Down), Raw::key(KeyCode::BTN_SOUTH, Press::Down), Raw { kind: evdev::EventType::ABSOLUTE, code: 123, value: 1 }];
        normalise_controls(controls, &mut events);
        assert_eq!(events.first(), Some(&Raw::key(super::super::codes::BACK_KEY, Press::Down)));
        assert_eq!(events.get(1), Some(&Raw::key(KeyCode::BTN_SOUTH, Press::Down)));
        assert_eq!(events.get(2).map(|event| event.code), Some(123));
        assert!(!PAD_KEYS.contains(&super::super::codes::BACK_KEY), "Back is a keymap input, not the virtual pad's");
        assert!(PAD_KEYS.contains(&KeyCode::BTN_MODE), "Guide, for the AYN button and Home with Back");
    }

    /// A controller is known by its USB ids and its unique id, or its name without one.
    #[test]
    fn a_controller_is_known_by_its_ids_and_unique_id_or_name() {
        assert_eq!(key(0x045e, 0x0b13, Some("aa:bb:cc:dd:ee:ff"), "Xbox Wireless Controller"), "045e:0b13:aa:bb:cc:dd:ee:ff");
        assert_eq!(key(0x2020, 0x3001, Some(" "), "AYN Odin2 Gamepad"), "2020:3001:AYN Odin2 Gamepad", "a blank unique id is none");
        assert_eq!(key(0x2dc8, 0x6003, None, "8BitDo Pro 2"), "2dc8:6003:8BitDo Pro 2");
    }

    #[test]
    fn letter_codes_by_driver() {
        let controls = Controls::thor();
        assert!(letter_codes(controls, Some("xpad")) && letter_codes(controls, Some("rsinput")));
        assert!(!letter_codes(controls, Some("hid-playstation")) && !letter_codes(controls, None));
    }
}
