//! The virtual touchpad.

use std::io;

use evdev::uinput::VirtualDevice;
use evdev::{AbsInfo, AbsoluteAxisCode, AttributeSet, KeyCode, PropType, UinputAbsSetup};

use super::pointing::SLOTS;
use crate::os::evdev::{Rect, keyset};

/// The virtual touchpad's keys. One is a dummy button, so libinput takes it as a touchpad with buttons
/// and taps for clicks. The others are the touch and finger-count keys.
const TOUCHPAD_KEYS: [KeyCode; 6] =
    [KeyCode::BTN_LEFT, KeyCode::BTN_TOUCH, KeyCode::BTN_TOOL_FINGER, KeyCode::BTN_TOOL_DOUBLETAP, KeyCode::BTN_TOOL_TRIPLETAP, KeyCode::BTN_TOOL_QUADTAP];

/// A touchpad the size of `area`, in screen pixels, with `px_per_mm` pixels per millimetre.
pub(crate) fn virtual_touchpad(area: Rect, px_per_mm: (i32, i32)) -> io::Result<VirtualDevice> {
    let (w, h) = (area.width().max(1.0) as i32, area.height().max(1.0) as i32);
    let (res_x, res_y) = px_per_mm;
    let axis = |code, max, res| UinputAbsSetup::new(code, AbsInfo::new(0, 0, max, 0, 0, res));
    let props: AttributeSet<PropType> = [PropType::POINTER].into_iter().collect();
    VirtualDevice::builder()?
        .name("clamshell touchpad")
        .with_properties(&props)?
        .with_keys(&keyset(&TOUCHPAD_KEYS))?
        .with_absolute_axis(&axis(AbsoluteAxisCode::ABS_X, w, res_x))?
        .with_absolute_axis(&axis(AbsoluteAxisCode::ABS_Y, h, res_y))?
        .with_absolute_axis(&axis(AbsoluteAxisCode::ABS_MT_SLOT, SLOTS as i32 - 1, 0))?
        .with_absolute_axis(&axis(AbsoluteAxisCode::ABS_MT_TRACKING_ID, 65535, 0))?
        .with_absolute_axis(&axis(AbsoluteAxisCode::ABS_MT_POSITION_X, w, res_x))?
        .with_absolute_axis(&axis(AbsoluteAxisCode::ABS_MT_POSITION_Y, h, res_y))?
        .build()
}
