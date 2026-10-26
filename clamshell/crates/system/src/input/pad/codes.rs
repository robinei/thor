//! The controller's codes: which evdev codes each button, D-pad direction, stick and trigger of the
//! keymap's inputs has on the controller and on the pad games see, and the Thor's Back and AYN roles.
//! The keymap ([`super::remap`]), the virtual devices ([`super::device`]) and listening for an input
//! ([`super::listen`]) read them.

use clamshell_state::keymap::{Button, Dir, MouseButton, Side};
use evdev::{AbsoluteAxisCode, KeyCode};

use crate::os::evdev::Raw;

/// A stick or trigger counts as pushed, as a direction or a button, past this.
pub(crate) const HALF: f32 = 0.5;

/// The normalised Back role. Physical codes are translated from the device profile before routing.
pub(crate) const BACK_KEY: KeyCode = KeyCode(clamshell_device::input::BACK_ROLE);

/// The normalised auxiliary-button role, emitted by the profile's GPIO-key reader.
pub(crate) const AYN_KEY: KeyCode = KeyCode(clamshell_device::input::AUXILIARY_ROLE);

/// The controller's codes for a button. The back buttons go by several names.
pub(super) fn button_codes(b: Button) -> &'static [KeyCode] {
    match b {
        Button::A => &[KeyCode::BTN_SOUTH],
        Button::B => &[KeyCode::BTN_EAST],
        Button::X => &[KeyCode::BTN_WEST],
        Button::Y => &[KeyCode::BTN_NORTH],
        Button::LB => &[KeyCode::BTN_TL],
        Button::RB => &[KeyCode::BTN_TR],
        Button::View => &[KeyCode::BTN_SELECT],
        Button::Menu => &[KeyCode::BTN_START],
        Button::LS => &[KeyCode::BTN_THUMBL],
        Button::RS => &[KeyCode::BTN_THUMBR],
        Button::Back => &[BACK_KEY],
        Button::Ayn => &[AYN_KEY],
        // BTN_TRIGGER_HAPPY1-4 are xpad's paddles, and BTN_GRIPL/R/L2/R2 newer kernels' back buttons.
        Button::L4 => &[KeyCode::BTN_TRIGGER_HAPPY1, KeyCode(0x224)],
        Button::R4 => &[KeyCode::BTN_TRIGGER_HAPPY2, KeyCode(0x225)],
        Button::L5 => &[KeyCode::BTN_TRIGGER_HAPPY3, KeyCode(0x226)],
        Button::R5 => &[KeyCode::BTN_TRIGGER_HAPPY4, KeyCode(0x227)],
    }
}

/// The code a button has on the pad games see. That is its own code, except that AYN is Guide there.
pub(super) fn pad_code(b: Button) -> Option<KeyCode> {
    match b {
        Button::Ayn => Some(KeyCode::BTN_MODE),
        b => button_codes(b).first().copied(),
    }
}

/// The button a controller's code is, if it is one.
pub(crate) fn button_of(code: KeyCode) -> Option<Button> {
    Button::ALL.into_iter().find(|b| button_codes(*b).contains(&code))
}

/// The D-pad as buttons, for controllers without a hat.
pub(super) fn dpad_code(d: Dir) -> KeyCode {
    match d {
        Dir::Up => KeyCode::BTN_DPAD_UP,
        Dir::Down => KeyCode::BTN_DPAD_DOWN,
        Dir::Left => KeyCode::BTN_DPAD_LEFT,
        Dir::Right => KeyCode::BTN_DPAD_RIGHT,
    }
}

pub(crate) fn stick_axes(s: Side) -> (AbsoluteAxisCode, AbsoluteAxisCode) {
    match s {
        Side::Left => (AbsoluteAxisCode::ABS_X, AbsoluteAxisCode::ABS_Y),
        Side::Right => (AbsoluteAxisCode::ABS_RX, AbsoluteAxisCode::ABS_RY),
    }
}

pub(crate) fn trigger_axis(s: Side) -> AbsoluteAxisCode {
    match s {
        Side::Left => AbsoluteAxisCode::ABS_Z,
        Side::Right => AbsoluteAxisCode::ABS_RZ,
    }
}

pub(super) const HAT: [AbsoluteAxisCode; 2] = [AbsoluteAxisCode::ABS_HAT0X, AbsoluteAxisCode::ABS_HAT0Y];

pub(super) fn mouse_code(m: MouseButton) -> KeyCode {
    match m {
        MouseButton::Left => KeyCode::BTN_LEFT,
        MouseButton::Right => KeyCode::BTN_RIGHT,
        MouseButton::Middle => KeyCode::BTN_MIDDLE,
        MouseButton::Back => KeyCode::BTN_SIDE,
        MouseButton::Forward => KeyCode::BTN_EXTRA,
    }
}

/// The mouse buttons a keymap may press. The virtual mouse has them.
pub(crate) fn mouse_codes() -> [KeyCode; 5] {
    MouseButton::ALL.map(mouse_code)
}

/// Whether the remapping handles this event. If not, it passes through.
pub(crate) fn known(raw: Raw) -> bool {
    if let Some((code, _)) = raw.as_key() {
        return button_of(code).is_some() || Dir::ALL.into_iter().any(|d| dpad_code(d) == code);
    }
    raw.as_abs().is_some_and(|(axis, _)| {
        HAT.contains(&axis) || Side::ALL.into_iter().any(|s| trigger_axis(s) == axis || stick_axes(s).0 == axis || stick_axes(s).1 == axis)
    })
}
