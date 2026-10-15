//! The real controller and our virtual devices: a pad identified as an Xbox 360 controller (for games)
//! and a mouse with a keyboard's keys (the pad mouse, typing, what a keymap presses).

use std::io;

use evdev::raw_stream::RawDevice;
use evdev::uinput::VirtualDevice;
use evdev::{AbsInfo, AbsoluteAxisCode, AttributeSet, BusType, FFEffectCode, InputEvent, InputId, KeyCode, RelativeAxisCode, UinputAbsSetup};

use super::remap::is_back_key;
use super::route::{Axis, PadSnapshot, Ranges, mouse_device_keys};
use crate::input::device::{self, DeviceSpec};
use crate::input::{Press, Range, Raw, keyset};

pub const VIRTUAL_PAD: &str = "clamshell pad (Xbox 360)";
const VIRTUAL_MOUSE: &str = "clamshell pad mouse";

/// The configured controller, or the first device with gamepad buttons and two sticks (not our own).
pub fn find(spec: &DeviceSpec) -> io::Result<RawDevice> {
    let gamepad = |d: &RawDevice| {
        d.supported_keys().is_some_and(|k| k.contains(KeyCode::BTN_SOUTH)) && d.supported_absolute_axes().is_some_and(|a| a.contains(AbsoluteAxisCode::ABS_RX))
    };
    device::open(spec, "controller", gamepad, &[VIRTUAL_PAD])
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
        neutral.extend(caps.axes.iter().map(|(code, a)| Raw::abs(*code, rest(a))));
        neutral.push(Raw::REPORT);
        Ok(VirtualDevices { pad: virtual_pad(&caps)?, mouse: virtual_mouse()?, made_for: caps, neutral })
    }

    /// The controller is gone: the virtual pad stays, at rest.
    pub fn rest(&mut self) -> io::Result<()> {
        emit(&mut self.pad, &self.neutral)
    }
}

/// Emits events on a virtual device.
pub fn emit(device: &mut VirtualDevice, events: &[Raw]) -> io::Result<()> {
    let events: Vec<InputEvent> = events.iter().map(|&r| r.into()).collect();
    device.emit(&events)
}

/// The real pad's keys the virtual one has: all but the layer key (ClamShell's, never the game's).
fn pad_keys(caps: &Capabilities) -> impl Iterator<Item = KeyCode> + '_ {
    caps.keys.iter().copied().filter(|&k| !is_back_key(k))
}

/// A pad with the real one's buttons and axes, identified as a wired Xbox 360 controller so SDL, Wine
/// and Steam map it without configuration.
fn virtual_pad(caps: &Capabilities) -> io::Result<VirtualDevice> {
    // Rumble, as an Xbox 360 pad has it (games' effects come back on its file, read without blocking).
    let ff: AttributeSet<FFEffectCode> = [FFEffectCode::FF_RUMBLE].into_iter().collect();
    let mut builder = VirtualDevice::builder()?
        .name(VIRTUAL_PAD)
        .input_id(InputId::new(BusType::BUS_USB, 0x045e, 0x028e, 0x0110))
        .with_ff(&ff)?
        .with_ff_effects_max(super::rumble::EFFECTS_MAX);
    let keys: Vec<KeyCode> = pad_keys(caps).collect();
    if !keys.is_empty() {
        builder = builder.with_keys(&keyset(&keys))?;
    }
    for &(code, info) in &caps.axes {
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
