//! The touchscreen, and the virtual touchpad that stands for the area below the bar.

use std::io;

use evdev::raw_stream::RawDevice;
use evdev::uinput::VirtualDevice;
use evdev::{AbsInfo, AbsoluteAxisCode, AttributeSet, KeyCode, PropType, UinputAbsSetup};

use super::router::{Axes, SLOTS};
use crate::input::device::{self, DeviceSpec};
use crate::input::{Range, Rect, keyset};

/// The virtual touchpad's keys: one dummy button, so libinput takes it as a touchpad with buttons and
/// taps for clicks, and the touch and finger-count keys.
const TOUCHPAD_KEYS: [KeyCode; 6] = [
    KeyCode::BTN_LEFT,
    KeyCode::BTN_TOUCH,
    KeyCode::BTN_TOOL_FINGER,
    KeyCode::BTN_TOOL_DOUBLETAP,
    KeyCode::BTN_TOOL_TRIPLETAP,
    KeyCode::BTN_TOOL_QUADTAP,
];

/// The configured touchscreen, or the first direct-touch multitouch device.
pub fn find(spec: &DeviceSpec) -> io::Result<RawDevice> {
    let touchscreen = |d: &RawDevice| {
        d.properties().contains(PropType::DIRECT) && d.supported_absolute_axes().is_some_and(|a| a.contains(AbsoluteAxisCode::ABS_MT_POSITION_X))
    };
    device::open(spec, "touchscreen", touchscreen, &[])
}

/// The touchscreen's coordinate ranges.
pub fn axes(device: &RawDevice) -> io::Result<Axes> {
    let abs = device.get_abs_state()?;
    let range = |code: AbsoluteAxisCode| abs.get(usize::from(code.0)).map_or(Range { min: 0, max: 1 }, |a| Range { min: a.minimum, max: a.maximum });
    Ok(Axes { x: range(AbsoluteAxisCode::ABS_MT_POSITION_X), y: range(AbsoluteAxisCode::ABS_MT_POSITION_Y) })
}

/// A touchpad the size of the pad area (in screen pixels, `px_per_mm` of them per millimetre).
pub fn virtual_touchpad(area: Rect, px_per_mm: f32) -> io::Result<VirtualDevice> {
    let (w, h) = (area.width().max(1.0) as i32, area.height().max(1.0) as i32);
    let res = px_per_mm.round().max(1.0) as i32;
    let axis = |code, max, res| UinputAbsSetup::new(code, AbsInfo::new(0, 0, max, 0, 0, res));
    let props: AttributeSet<PropType> = [PropType::POINTER].into_iter().collect();
    VirtualDevice::builder()?
        .name("clamshell touchpad")
        .with_properties(&props)?
        .with_keys(&keyset(&TOUCHPAD_KEYS))?
        .with_absolute_axis(&axis(AbsoluteAxisCode::ABS_X, w, res))?
        .with_absolute_axis(&axis(AbsoluteAxisCode::ABS_Y, h, res))?
        .with_absolute_axis(&axis(AbsoluteAxisCode::ABS_MT_SLOT, SLOTS as i32 - 1, 0))?
        .with_absolute_axis(&axis(AbsoluteAxisCode::ABS_MT_TRACKING_ID, 65535, 0))?
        .with_absolute_axis(&axis(AbsoluteAxisCode::ABS_MT_POSITION_X, w, res))?
        .with_absolute_axis(&axis(AbsoluteAxisCode::ABS_MT_POSITION_Y, h, res))?
        .build()
}
