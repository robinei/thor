//! The pad mouse. The left stick moves the pointer and the right stick scrolls. A or the right trigger
//! is the left click, B the right click and Y the middle click. The D-pad sends arrow keys.

use std::time::{Duration, Instant};

use evdev::{AbsoluteAxisCode, KeyCode, RelativeAxisCode};

use super::route::{Mapping, Out, Ranges, release_mouse_keys};
use crate::os::evdev::{Press, Raw, hat_keys};

/// Stick deflection below this fraction of full is ignored, since sticks don't rest exactly at zero.
const DEADZONE: f32 = 0.15;
/// Pointer speed at full deflection, in pixels per second. The curve is quadratic below that, so small
/// deflections give fine control.
const MAX_SPEED: f32 = 1800.0;
/// Scroll speed at full deflection, in wheel notches per second.
const MAX_SCROLL: f32 = 18.0;
/// High-resolution wheel units per notch, by the kernel's convention.
const HI_RES_NOTCH: f32 = 120.0;
/// A trigger pressed past this fraction of full is a held button.
const TRIGGER: f32 = 0.5;
/// How often motion is sent while a stick is deflected: 125 Hz, a common mouse rate.
pub const MOTION_EVERY: Duration = Duration::from_millis(8);

/// A stick vector after the radial deadzone and the curve. Its length is 0 at the deadzone's edge and 1
/// at full deflection.
fn shape((x, y): (f32, f32)) -> (f32, f32) {
    let len = x.hypot(y);
    if len <= DEADZONE {
        return (0.0, 0.0);
    }
    let scaled = ((len.min(1.0) - DEADZONE) / (1.0 - DEADZONE)).powi(2);
    (x / len * scaled, y / len * scaled)
}

#[derive(Debug)]
pub struct PadMouse {
    ranges: Ranges,
    left: (f32, f32),
    right: (f32, f32),
    trigger_down: bool,
    /// Fractions of a pixel and of a wheel unit not yet sent.
    carry: (f32, f32),
    scroll_carry: (f32, f32),
    /// When motion was last sent.
    last: Option<Instant>,
}

impl PadMouse {
    pub fn new(ranges: Ranges) -> Self {
        PadMouse { ranges, left: (0.0, 0.0), right: (0.0, 0.0), trigger_down: false, carry: (0.0, 0.0), scroll_carry: (0.0, 0.0), last: None }
    }

    /// Whether a stick is out of its deadzone, in which case motion needs ticks.
    pub fn moving(&self) -> bool {
        shape(self.left) != (0.0, 0.0) || shape(self.right) != (0.0, 0.0)
    }

    /// Motion for `dt` of stick deflection: the pointer from the left stick, scrolling from the right.
    pub fn motion(&mut self, dt: Duration, out: &mut Out) {
        let secs = dt.as_secs_f32();
        let (mx, my) = shape(self.left);
        self.carry.0 += mx * MAX_SPEED * secs;
        self.carry.1 += my * MAX_SPEED * secs;
        let (dx, dy) = (self.carry.0.trunc(), self.carry.1.trunc());
        self.carry = (self.carry.0 - dx, self.carry.1 - dy);

        let (sx, sy) = shape(self.right);
        // Stick down (positive y) scrolls down, which is a negative wheel value.
        self.scroll_carry.0 += sx * MAX_SCROLL * HI_RES_NOTCH * secs;
        self.scroll_carry.1 -= sy * MAX_SCROLL * HI_RES_NOTCH * secs;
        let (hx, hy) = (self.scroll_carry.0.trunc(), self.scroll_carry.1.trunc());
        self.scroll_carry = (self.scroll_carry.0 - hx, self.scroll_carry.1 - hy);

        let start = out.mouse.len();
        for (axis, value) in
            [(RelativeAxisCode::REL_X, dx), (RelativeAxisCode::REL_Y, dy), (RelativeAxisCode::REL_WHEEL_HI_RES, hy), (RelativeAxisCode::REL_HWHEEL_HI_RES, hx)]
        {
            if value != 0.0 {
                out.mouse.push(Raw::rel(axis, value as i32));
            }
        }
        if out.mouse.len() > start {
            out.mouse.push(Raw::REPORT);
        }
    }

    fn button(&self, button: KeyCode, press: Press, out: &mut Out) {
        // A repeat means the button is held, since a mouse button has no repeats.
        let press = if press.held() { Press::Down } else { Press::Up };
        out.mouse.extend([Raw::key(button, press), Raw::REPORT]);
    }
}

impl Mapping for PadMouse {
    /// Sticks are only read here. Buttons, the trigger and the D-pad map at once.
    fn event(&mut self, raw: Raw, _: Instant, out: &mut Out) {
        if let Some((axis, value)) = raw.as_abs() {
            match axis {
                AbsoluteAxisCode::ABS_X => self.left.0 = self.ranges.stick.centered(value),
                AbsoluteAxisCode::ABS_Y => self.left.1 = self.ranges.stick.centered(value),
                AbsoluteAxisCode::ABS_RX => self.right.0 = self.ranges.stick.centered(value),
                AbsoluteAxisCode::ABS_RY => self.right.1 = self.ranges.stick.centered(value),
                AbsoluteAxisCode::ABS_RZ => {
                    let down = self.ranges.trigger.unit(value) > TRIGGER;
                    if down != self.trigger_down {
                        self.trigger_down = down;
                        self.button(KeyCode::BTN_LEFT, if down { Press::Down } else { Press::Up }, out);
                    }
                }
                axis => out.mouse.extend(hat_keys(axis, value).into_iter().flatten()),
            }
            return;
        }
        match raw.as_key() {
            Some((KeyCode::BTN_SOUTH, press)) => self.button(KeyCode::BTN_LEFT, press, out),
            Some((KeyCode::BTN_EAST, press)) => self.button(KeyCode::BTN_RIGHT, press, out),
            Some((KeyCode::BTN_NORTH, press)) => self.button(KeyCode::BTN_MIDDLE, press, out),
            _ => {}
        }
    }

    /// Sends motion at most every [`MOTION_EVERY`], for the time since the last. Ticks are wanted while a
    /// stick is deflected.
    fn tick(&mut self, now: Instant, out: &mut Out) -> Option<Instant> {
        let last = *self.last.get_or_insert(now);
        if now.duration_since(last) >= MOTION_EVERY {
            self.motion(now.duration_since(last), out);
            self.last = Some(now);
        }
        let last = self.last.unwrap_or(now);
        self.moving().then_some(last + MOTION_EVERY)
    }

    fn release(&mut self, out: &mut Out) {
        release_mouse_keys(out);
        *self = PadMouse::new(self.ranges);
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::indexing_slicing)]
mod tests {
    use super::*;
    use crate::os::evdev::Range;

    fn mouse() -> PadMouse {
        PadMouse::new(Ranges { stick: Range::STICK, trigger: Range { min: 0, max: 1023 } })
    }

    fn feed(m: &mut PadMouse, raw: Raw, out: &mut Out) {
        m.event(raw, Instant::now(), out);
    }

    fn sum(out: &Out, code: RelativeAxisCode) -> i32 {
        out.mouse.iter().filter(|r| *r == &Raw::rel(code, r.value)).map(|r| r.value).sum()
    }

    #[test]
    fn a_resting_stick_does_not_move_the_pointer() {
        let mut m = mouse();
        let mut out = Out::default();
        feed(&mut m, Raw::abs(AbsoluteAxisCode::ABS_X, 3000), &mut out);
        assert!(!m.moving());
        m.motion(Duration::from_secs(1), &mut out);
        assert!(out.mouse.is_empty());
    }

    #[test]
    fn full_deflection_moves_at_max_speed() {
        let mut m = mouse();
        let mut out = Out::default();
        feed(&mut m, Raw::abs(AbsoluteAxisCode::ABS_X, 32767), &mut out);
        for _ in 0..125 {
            m.motion(MOTION_EVERY, &mut out);
        }
        let moved = sum(&out, RelativeAxisCode::REL_X);
        assert!((moved - MAX_SPEED as i32).abs() <= 2, "moved {moved}");
        assert_eq!(sum(&out, RelativeAxisCode::REL_Y), 0);
    }

    #[test]
    fn half_deflection_is_much_slower_than_full() {
        let mut m = mouse();
        let mut out = Out::default();
        feed(&mut m, Raw::abs(AbsoluteAxisCode::ABS_Y, 16384), &mut out);
        m.motion(Duration::from_secs(1), &mut out);
        let moved = sum(&out, RelativeAxisCode::REL_Y);
        assert!(moved > 0 && moved < MAX_SPEED as i32 / 3, "moved {moved}");
    }

    #[test]
    fn right_stick_down_scrolls_down() {
        let mut m = mouse();
        let mut out = Out::default();
        feed(&mut m, Raw::abs(AbsoluteAxisCode::ABS_RY, 32767), &mut out);
        m.motion(Duration::from_secs(1), &mut out);
        assert_eq!(sum(&out, RelativeAxisCode::REL_WHEEL_HI_RES), -(MAX_SCROLL * HI_RES_NOTCH) as i32);
    }

    #[test]
    fn ticks_come_at_the_motion_rate_while_deflected() {
        let mut m = mouse();
        let mut out = Out::default();
        let t0 = Instant::now();
        assert_eq!(m.tick(t0, &mut out), None, "at rest");
        feed(&mut m, Raw::abs(AbsoluteAxisCode::ABS_X, 32767), &mut out);
        assert_eq!(m.tick(t0 + Duration::from_millis(3), &mut out), Some(t0 + MOTION_EVERY), "not yet due");
        assert_eq!(m.tick(t0 + MOTION_EVERY, &mut out), Some(t0 + 2 * MOTION_EVERY));
        assert!(sum(&out, RelativeAxisCode::REL_X) > 0);
    }

    #[test]
    fn buttons_and_trigger_click() {
        let mut m = mouse();
        let mut out = Out::default();
        feed(&mut m, Raw::key(KeyCode::BTN_SOUTH, Press::Down), &mut out);
        feed(&mut m, Raw::key(KeyCode::BTN_EAST, Press::Down), &mut out);
        assert_eq!(out.mouse[0], Raw::key(KeyCode::BTN_LEFT, Press::Down));
        assert_eq!(out.mouse[2], Raw::key(KeyCode::BTN_RIGHT, Press::Down));
        out.mouse.clear();
        feed(&mut m, Raw::abs(AbsoluteAxisCode::ABS_RZ, 900), &mut out);
        feed(&mut m, Raw::abs(AbsoluteAxisCode::ABS_RZ, 950), &mut out);
        assert_eq!(out.mouse, vec![Raw::key(KeyCode::BTN_LEFT, Press::Down), Raw::REPORT], "one press, not one per report");
    }

    #[test]
    fn dpad_sends_arrows() {
        let mut m = mouse();
        let mut out = Out::default();
        feed(&mut m, Raw::abs(AbsoluteAxisCode::ABS_HAT0Y, -1), &mut out);
        assert!(out.mouse.contains(&Raw::key(KeyCode::KEY_UP, Press::Down)));
        out.mouse.clear();
        feed(&mut m, Raw::abs(AbsoluteAxisCode::ABS_HAT0Y, 0), &mut out);
        assert!(out.mouse.contains(&Raw::key(KeyCode::KEY_UP, Press::Up)));
    }
}
