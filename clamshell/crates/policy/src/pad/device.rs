//! The real controller and our virtual devices: the pad games see ([`GamePad`]: an Xbox 360 controller
//! through uinput, or a DualSense or Switch Pro Controller through uhid) and a mouse with a few keys
//! (the pad mouse, typing).

use std::io;

use evdev::raw_stream::RawDevice;
use evdev::uinput::VirtualDevice;
use evdev::{AbsInfo, AbsoluteAxisCode, AttributeSet, BusType, InputEvent, InputId, KeyCode, RelativeAxisCode, UinputAbsSetup};

use super::hid::{VirtualHid, dualsense, motion, procon};
use super::route::{Axis, MOUSE_DEVICE_KEYS, PadSnapshot, Ranges};
use super::rumble::RumbleSlot;
use super::shaping::PadKind;
use crate::input::device::{self, DeviceSpec};
use crate::input::{Press, Range, Raw, keyset};

pub const VIRTUAL_PAD: &str = "clamshell pad (Xbox 360)";
const VIRTUAL_MOUSE: &str = "clamshell pad mouse";

/// The configured controller, or the first device with gamepad buttons and two sticks (not our own).
pub fn find(spec: &DeviceSpec) -> io::Result<RawDevice> {
    let gamepad = |d: &RawDevice| {
        d.supported_keys().is_some_and(|k| k.contains(KeyCode::BTN_SOUTH)) && d.supported_absolute_axes().is_some_and(|a| a.contains(AbsoluteAxisCode::ABS_RX))
    };
    let ours = [VIRTUAL_PAD, dualsense::NAME, dualsense::MOTION_NAME, dualsense::TOUCHPAD_NAME, procon::NAME, procon::IMU_NAME];
    device::open(spec, "controller", gamepad, &ours)
}

/// A controller's buttons and axes (each with its range and current value).
#[derive(Debug, Clone, PartialEq)]
pub struct Capabilities {
    pub keys: Vec<KeyCode>,
    pub axes: Vec<(AbsoluteAxisCode, AbsInfo)>,
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

/// The pad games see. Either takes the same events (the controller's, shaped); the router doesn't
/// know which it feeds.
pub enum GamePad {
    Xbox(VirtualDevice),
    Hid(PadKind, VirtualHid),
}

impl GamePad {
    fn new(kind: PadKind, caps: &Capabilities, rumble: &RumbleSlot) -> io::Result<GamePad> {
        Ok(match kind {
            PadKind::Xbox => GamePad::Xbox(virtual_pad(caps)?),
            PadKind::PlayStation => GamePad::Hid(kind, VirtualHid::new(Box::new(dualsense::DualSense::default()), caps.ranges(), motion::default_source(), rumble.clone())?),
            PadKind::Nintendo => GamePad::Hid(kind, VirtualHid::new(Box::new(procon::ProController::default()), caps.ranges(), motion::default_source(), rumble.clone())?),
        })
    }

    pub fn kind(&self) -> PadKind {
        match self {
            GamePad::Xbox(_) => PadKind::Xbox,
            GamePad::Hid(kind, _) => *kind,
        }
    }

    pub fn emit(&mut self, events: &[Raw]) -> io::Result<()> {
        match self {
            GamePad::Xbox(device) => emit(device, events),
            GamePad::Hid(_, device) => device.emit(events),
        }
    }
}

/// The virtual devices, kept across the controller's comings and goings (an unplug, an error), so games
/// never see their pad go: made for the first controller, made again only for one with other buttons.
/// The pad is made again, of the other kind, when the settings in force ask for it.
pub struct VirtualDevices {
    pub made_for: Capabilities,
    pub pad: GamePad,
    pub mouse: VirtualDevice,
    /// The pad untouched: every button up, every axis at rest (shown while no controller is there).
    neutral: Vec<Raw>,
    rumble: RumbleSlot,
}

impl VirtualDevices {
    pub fn new(caps: Capabilities, kind: PadKind, rumble: RumbleSlot) -> io::Result<VirtualDevices> {
        let mut neutral: Vec<Raw> = caps.keys.iter().map(|&k| Raw::key(k, Press::Up)).collect();
        neutral.extend(caps.axes.iter().map(|(code, a)| Raw::abs(*code, rest(a))));
        neutral.push(Raw::REPORT);
        let pad = GamePad::new(kind, &caps, &rumble).or_else(|e| {
            log::warn!("pad: no {kind:?} pad ({e}); games get the Xbox one");
            GamePad::new(PadKind::Xbox, &caps, &rumble)
        })?;
        Ok(VirtualDevices { pad, mouse: virtual_mouse()?, made_for: caps, neutral, rumble })
    }

    /// The pad made again as `kind` if it is the other (games see one controller unplugged and another
    /// plugged in), at rest; whether it was. If the new one can't be made, the old one stays.
    pub fn set_kind(&mut self, kind: PadKind) -> io::Result<bool> {
        if self.pad.kind() == kind {
            return Ok(false);
        }
        match GamePad::new(kind, &self.made_for, &self.rumble) {
            Ok(pad) => self.pad = pad,
            Err(e) => {
                log::warn!("pad: no {kind:?} pad ({e}); games keep the {:?} one", self.pad.kind());
                return Ok(false);
            }
        }
        self.rest()?;
        Ok(true)
    }

    /// The controller is gone: the virtual pad stays, at rest.
    pub fn rest(&mut self) -> io::Result<()> {
        self.pad.emit(&self.neutral)
    }
}

/// Emits events on a virtual device.
pub fn emit(device: &mut VirtualDevice, events: &[Raw]) -> io::Result<()> {
    let events: Vec<InputEvent> = events.iter().map(|&r| r.into()).collect();
    device.emit(&events)
}

/// A pad with the real one's buttons and axes, identified as a wired Xbox 360 controller so SDL, Wine
/// and Steam map it without configuration.
fn virtual_pad(caps: &Capabilities) -> io::Result<VirtualDevice> {
    let mut builder = VirtualDevice::builder()?.name(VIRTUAL_PAD).input_id(InputId::new(BusType::BUS_USB, 0x045e, 0x028e, 0x0110));
    if !caps.keys.is_empty() {
        builder = builder.with_keys(&keyset(&caps.keys))?;
    }
    for &(code, info) in &caps.axes {
        builder = builder.with_absolute_axis(&UinputAbsSetup::new(code, info))?;
    }
    builder.build()
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
    VirtualDevice::builder()?.name(VIRTUAL_MOUSE).with_keys(&keyset(&MOUSE_DEVICE_KEYS))?.with_relative_axes(&rel)?.build()
}
