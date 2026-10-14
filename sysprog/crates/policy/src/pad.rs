//! The controller path (docs/plans.md, "Owning the input path"): the real Xbox-style controller is
//! grabbed for good, and games get a virtual Xbox 360 pad that is always there. Normally every event is
//! copied straight through. With the pad mouse on (a toggle, independent of the second screen's mode),
//! the same events drive a virtual mouse instead: left stick moves the pointer, right stick scrolls, A
//! or the right trigger clicks left, B right, X middle, the D-pad sends arrow keys; the virtual pad
//! meanwhile shows a neutral controller, so games never see it disappear. Button layers (the system
//! layer, remaps) go in this same path later.
//!
//! Not yet: forwarding rumble from the virtual pad to the real one (uinput force-feedback uploads).

use std::io;
use std::sync::mpsc::{Receiver, TryRecvError};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use evdev::raw_stream::RawDevice;
use evdev::uinput::VirtualDevice;
use evdev::{AbsInfo, AbsoluteAxisCode, AttributeSet, BusType, EventType, InputEvent, InputId, KeyCode, RelativeAxisCode, UinputAbsSetup};

/// Stick deflection below this (of full) is ignored: sticks don't rest exactly at zero.
const DEADZONE: f32 = 0.15;
/// Pointer speed at full deflection, pixels per second; the curve is quadratic below that, so small
/// deflections give fine control.
const MAX_SPEED: f32 = 1800.0;
/// Scroll speed at full deflection, wheel notches per second.
const MAX_SCROLL: f32 = 18.0;
/// High-resolution wheel units per notch (the kernel's convention).
const HI_RES_NOTCH: f32 = 120.0;
/// A trigger pressed past this (of full) is a held button.
const TRIGGER: f32 = 0.5;
/// How often motion is sent while a stick is deflected (125 Hz, a common mouse rate).
const MOTION_EVERY: Duration = Duration::from_millis(8);

const EV_SYN: u16 = EventType::SYNCHRONIZATION.0;
const EV_KEY: u16 = EventType::KEY.0;
const EV_REL: u16 = EventType::RELATIVE.0;
const EV_ABS: u16 = EventType::ABSOLUTE.0;

/// A raw event for the virtual mouse.
pub type Out = (u16, u16, i32);

/// Normalises an axis to -1..1 (sticks) or 0..1 (triggers) from its raw range.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Range {
    pub min: i32,
    pub max: i32,
}

impl Range {
    fn centered(self, v: i32) -> f32 {
        let mid = (self.min as f32 + self.max as f32) / 2.0;
        let half = ((self.max - self.min) as f32 / 2.0).max(1.0);
        ((v as f32 - mid) / half).clamp(-1.0, 1.0)
    }

    fn unit(self, v: i32) -> f32 {
        ((v - self.min) as f32 / (self.max - self.min).max(1) as f32).clamp(0.0, 1.0)
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Ranges {
    pub stick: Range,
    pub trigger: Range,
}

/// The pad-to-mouse mapping. Pure: pad events in, mouse events out, motion on `tick`.
#[derive(Debug)]
pub struct PadMouse {
    ranges: Ranges,
    left: (f32, f32),
    right: (f32, f32),
    trigger_down: bool,
    /// Fractions of a pixel and of a wheel unit not yet sent.
    carry: (f32, f32),
    scroll_carry: (f32, f32),
}

/// A stick vector after the radial deadzone and the curve: 0 at the deadzone's edge, 1 at full.
fn shape((x, y): (f32, f32)) -> (f32, f32) {
    let len = x.hypot(y);
    if len <= DEADZONE {
        return (0.0, 0.0);
    }
    let scaled = ((len.min(1.0) - DEADZONE) / (1.0 - DEADZONE)).powi(2);
    (x / len * scaled, y / len * scaled)
}

impl PadMouse {
    pub fn new(ranges: Ranges) -> Self {
        PadMouse { ranges, left: (0.0, 0.0), right: (0.0, 0.0), trigger_down: false, carry: (0.0, 0.0), scroll_carry: (0.0, 0.0) }
    }

    /// Whether a stick is out of its deadzone (motion needs ticks).
    pub fn moving(&self) -> bool {
        shape(self.left) != (0.0, 0.0) || shape(self.right) != (0.0, 0.0)
    }

    /// One pad event; buttons map at once.
    pub fn event(&mut self, kind: u16, code: u16, value: i32, out: &mut Vec<Out>) {
        let button = |out: &mut Vec<Out>, code: KeyCode| {
            out.push((EV_KEY, code.0, value.clamp(0, 1)));
            out.push((EV_SYN, 0, 0));
        };
        match (kind, code) {
            (EV_ABS, c) if c == AbsoluteAxisCode::ABS_X.0 => self.left.0 = self.ranges.stick.centered(value),
            (EV_ABS, c) if c == AbsoluteAxisCode::ABS_Y.0 => self.left.1 = self.ranges.stick.centered(value),
            (EV_ABS, c) if c == AbsoluteAxisCode::ABS_RX.0 => self.right.0 = self.ranges.stick.centered(value),
            (EV_ABS, c) if c == AbsoluteAxisCode::ABS_RY.0 => self.right.1 = self.ranges.stick.centered(value),
            (EV_ABS, c) if c == AbsoluteAxisCode::ABS_RZ.0 => {
                let down = self.ranges.trigger.unit(value) > TRIGGER;
                if down != self.trigger_down {
                    self.trigger_down = down;
                    out.push((EV_KEY, KeyCode::BTN_LEFT.0, i32::from(down)));
                    out.push((EV_SYN, 0, 0));
                }
            }
            (EV_ABS, c) if c == AbsoluteAxisCode::ABS_HAT0X.0 || c == AbsoluteAxisCode::ABS_HAT0Y.0 => {
                let (minus, plus) = if c == AbsoluteAxisCode::ABS_HAT0X.0 {
                    (KeyCode::KEY_LEFT, KeyCode::KEY_RIGHT)
                } else {
                    (KeyCode::KEY_UP, KeyCode::KEY_DOWN)
                };
                // The hat reports -1, 0 or 1; releasing reports 0 for both directions.
                out.push((EV_KEY, minus.0, i32::from(value < 0)));
                out.push((EV_KEY, plus.0, i32::from(value > 0)));
                out.push((EV_SYN, 0, 0));
            }
            (EV_KEY, c) if c == KeyCode::BTN_SOUTH.0 => button(out, KeyCode::BTN_LEFT),
            (EV_KEY, c) if c == KeyCode::BTN_EAST.0 => button(out, KeyCode::BTN_RIGHT),
            (EV_KEY, c) if c == KeyCode::BTN_NORTH.0 => button(out, KeyCode::BTN_MIDDLE),
            _ => {}
        }
    }

    /// Releases everything the mapping holds down (when the pad mouse turns off).
    pub fn reset(&mut self, out: &mut Vec<Out>) {
        for code in [KeyCode::BTN_LEFT, KeyCode::BTN_RIGHT, KeyCode::BTN_MIDDLE, KeyCode::KEY_UP, KeyCode::KEY_DOWN, KeyCode::KEY_LEFT, KeyCode::KEY_RIGHT] {
            out.push((EV_KEY, code.0, 0));
        }
        out.push((EV_SYN, 0, 0));
        *self = PadMouse::new(self.ranges);
    }

    /// Motion for `dt` of stick deflection: pointer from the left stick, scrolling from the right.
    pub fn tick(&mut self, dt: Duration, out: &mut Vec<Out>) {
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

        let start = out.len();
        if dx != 0.0 {
            out.push((EV_REL, RelativeAxisCode::REL_X.0, dx as i32));
        }
        if dy != 0.0 {
            out.push((EV_REL, RelativeAxisCode::REL_Y.0, dy as i32));
        }
        if hy != 0.0 {
            out.push((EV_REL, RelativeAxisCode::REL_WHEEL_HI_RES.0, hy as i32));
        }
        if hx != 0.0 {
            out.push((EV_REL, RelativeAxisCode::REL_HWHEEL_HI_RES.0, hx as i32));
        }
        if out.len() > start {
            out.push((EV_SYN, 0, 0));
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Command {
    MouseOn,
    MouseOff,
}

/// Owns the controller whenever one is connected (`wanted`: its /dev/input path or part of its name;
/// `None`: the first device that looks like a gamepad), looking again every second when there is none.
pub fn spawn(wanted: Option<String>, commands: Receiver<Command>) -> io::Result<JoinHandle<()>> {
    thread::Builder::new().name("pad".into()).spawn(move || {
        let mut mouse_on = false;
        loop {
            match run(wanted.as_deref(), &commands, &mut mouse_on) {
                Ok(()) => return,
                Err(e) => log::debug!("pad: {e}"),
            }
            match commands.recv_timeout(Duration::from_secs(1)) {
                Ok(command) => mouse_on = command == Command::MouseOn,
                Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {}
                Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => return,
            }
        }
    })
}

/// The controller's current state, to blank the virtual pad (pad mouse on) and restore it (off).
struct Held {
    keys: Vec<u16>,
    /// (code, value, neutral value) per axis.
    axes: Vec<(u16, i32, i32)>,
}

impl Held {
    fn update(&mut self, kind: u16, code: u16, value: i32) {
        if kind == EV_KEY {
            self.keys.retain(|&k| k != code);
            if value != 0 {
                self.keys.push(code);
            }
        } else if kind == EV_ABS
            && let Some(axis) = self.axes.iter_mut().find(|a| a.0 == code)
        {
            axis.1 = value;
        }
    }

    /// Events that make a pad look like this state (`real`) or like an untouched one.
    fn events(&self, real: bool, out: &mut Vec<InputEvent>) {
        for &k in &self.keys {
            out.push(InputEvent::new(EV_KEY, k, i32::from(real)));
        }
        for &(code, value, neutral) in &self.axes {
            out.push(InputEvent::new(EV_ABS, code, if real { value } else { neutral }));
        }
        out.push(InputEvent::new(EV_SYN, 0, 0));
    }
}

/// Runs while the controller is connected: Ok when the program is going away, Err when the controller
/// is gone (or none was found).
fn run(wanted: Option<&str>, commands: &Receiver<Command>, mouse_on: &mut bool) -> io::Result<()> {
    let mut real = find(wanted)?;
    let abs = real.get_abs_state()?;
    // Each supported axis with its current value and range.
    let axes: Vec<(AbsoluteAxisCode, AbsInfo)> = real
        .supported_absolute_axes()
        .map(|set| {
            set.iter()
                .filter_map(|code| {
                    abs.get(usize::from(code.0))
                        .map(|a| (code, AbsInfo::new(a.value, a.minimum, a.maximum, a.fuzz, a.flat, a.resolution)))
                })
                .collect()
        })
        .unwrap_or_default();
    let range = |code: AbsoluteAxisCode| {
        axes.iter()
            .find(|(c, _)| *c == code)
            .map_or(Range { min: -32768, max: 32767 }, |(_, a)| Range { min: a.minimum(), max: a.maximum() })
    };
    let ranges = Ranges { stick: range(AbsoluteAxisCode::ABS_X), trigger: range(AbsoluteAxisCode::ABS_RZ) };
    let mut held = Held {
        keys: Vec::new(),
        axes: axes
            .iter()
            .map(|(code, a)| (code.0, a.value(), if a.minimum() < 0 { 0 } else { a.minimum() }))
            .collect(),
    };
    let mut virtual_pad = virtual_pad(&real, &axes)?;
    let mut mouse = virtual_mouse()?;
    real.grab()?;
    let flags = rustix::fs::fcntl_getfl(&real)?;
    rustix::fs::fcntl_setfl(&real, flags | rustix::fs::OFlags::NONBLOCK)?;
    log::info!("pad: {} grabbed; games see the virtual pad", real.name().unwrap_or("controller"));

    let mut mapping = PadMouse::new(ranges);
    let mut out = Vec::new();
    let mut to_pad = Vec::new();
    let mut to_mouse = Vec::new();
    let mut last = Instant::now();
    if *mouse_on {
        held.events(false, &mut to_pad);
    }
    loop {
        loop {
            match commands.try_recv() {
                Ok(Command::MouseOn) if !*mouse_on => {
                    *mouse_on = true;
                    held.events(false, &mut to_pad);
                    log::info!("pad mouse on");
                }
                Ok(Command::MouseOff) if *mouse_on => {
                    *mouse_on = false;
                    mapping.reset(&mut out);
                    held.events(true, &mut to_pad);
                    log::info!("pad mouse off");
                }
                Ok(_) => {}
                Err(TryRecvError::Empty) => break,
                Err(TryRecvError::Disconnected) => return Ok(()),
            }
        }
        // Wake at the motion rate while a stick is deflected; otherwise for events (or to check commands).
        let wait = if *mouse_on && mapping.moving() { MOTION_EVERY } else { Duration::from_millis(100) };
        let timeout = rustix::event::Timespec { tv_sec: 0, tv_nsec: i64::from(wait.subsec_nanos()) };
        let mut fds = [rustix::event::PollFd::new(&real, rustix::event::PollFlags::IN)];
        rustix::event::poll(&mut fds, Some(&timeout))?;
        match real.fetch_events() {
            Ok(events) => {
                for e in events {
                    let (kind, code, value) = (e.event_type().0, e.code(), e.value());
                    held.update(kind, code, value);
                    if *mouse_on {
                        mapping.event(kind, code, value, &mut out);
                    } else {
                        to_pad.push(e);
                    }
                }
            }
            Err(e) if e.kind() == io::ErrorKind::WouldBlock => {}
            Err(e) => return Err(e),
        }
        let now = Instant::now();
        if *mouse_on && now.duration_since(last) >= MOTION_EVERY {
            mapping.tick(now.duration_since(last), &mut out);
        }
        if now.duration_since(last) >= MOTION_EVERY {
            last = now;
        }
        to_mouse.extend(out.drain(..).map(|(k, c, v)| InputEvent::new(k, c, v)));
        if !to_pad.is_empty() {
            virtual_pad.emit(&to_pad)?;
            to_pad.clear();
        }
        if !to_mouse.is_empty() {
            mouse.emit(&to_mouse)?;
            to_mouse.clear();
        }
    }
}

/// The configured controller, or the first device with gamepad buttons and two sticks (not our own).
fn find(wanted: Option<&str>) -> io::Result<RawDevice> {
    if let Some(path) = wanted.filter(|w| w.starts_with('/')) {
        return RawDevice::open(path);
    }
    let gamepad = |d: &RawDevice| {
        d.name() != Some(VIRTUAL_PAD)
            && d.supported_keys().is_some_and(|k| k.contains(KeyCode::BTN_SOUTH))
            && d.supported_absolute_axes().is_some_and(|a| a.contains(AbsoluteAxisCode::ABS_RX))
    };
    evdev::raw_stream::enumerate()
        .map(|(_, d)| d)
        .find(|d| match wanted {
            Some(name) => d.name().is_some_and(|n| n.contains(name)) && d.name() != Some(VIRTUAL_PAD),
            None => gamepad(d),
        })
        .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "no controller"))
}

const VIRTUAL_PAD: &str = "sysprog pad (Xbox 360)";

/// A pad with the real one's buttons and axes, identified as a wired Xbox 360 controller so SDL, Wine
/// and Steam map it without configuration.
fn virtual_pad(real: &RawDevice, axes: &[(AbsoluteAxisCode, AbsInfo)]) -> io::Result<VirtualDevice> {
    let mut builder = VirtualDevice::builder()?.name(VIRTUAL_PAD).input_id(InputId::new(BusType::BUS_USB, 0x045e, 0x028e, 0x0110));
    if let Some(keys) = real.supported_keys() {
        builder = builder.with_keys(keys)?;
    }
    for &(code, info) in axes {
        builder = builder.with_absolute_axis(&UinputAbsSetup::new(code, info))?;
    }
    builder.build()
}

fn virtual_mouse() -> io::Result<VirtualDevice> {
    let keys: AttributeSet<KeyCode> = [
        KeyCode::BTN_LEFT,
        KeyCode::BTN_RIGHT,
        KeyCode::BTN_MIDDLE,
        KeyCode::KEY_UP,
        KeyCode::KEY_DOWN,
        KeyCode::KEY_LEFT,
        KeyCode::KEY_RIGHT,
    ]
    .into_iter()
    .collect();
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
    VirtualDevice::builder()?.name("sysprog pad mouse").with_keys(&keys)?.with_relative_axes(&rel)?.build()
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::indexing_slicing)]
mod tests {
    use super::*;

    fn mouse() -> PadMouse {
        PadMouse::new(Ranges { stick: Range { min: -32768, max: 32767 }, trigger: Range { min: 0, max: 1023 } })
    }

    fn sum(out: &[Out], code: RelativeAxisCode) -> i32 {
        out.iter().filter(|(k, c, _)| *k == EV_REL && *c == code.0).map(|(_, _, v)| v).sum()
    }

    #[test]
    fn a_resting_stick_does_not_move_the_pointer() {
        let mut m = mouse();
        let mut out = Vec::new();
        m.event(EV_ABS, AbsoluteAxisCode::ABS_X.0, 3000, &mut out);
        assert!(!m.moving());
        m.tick(Duration::from_secs(1), &mut out);
        assert!(out.is_empty());
    }

    #[test]
    fn full_deflection_moves_at_max_speed() {
        let mut m = mouse();
        let mut out = Vec::new();
        m.event(EV_ABS, AbsoluteAxisCode::ABS_X.0, 32767, &mut out);
        for _ in 0..125 {
            m.tick(MOTION_EVERY, &mut out);
        }
        let moved = sum(&out, RelativeAxisCode::REL_X);
        assert!((moved - MAX_SPEED as i32).abs() <= 2, "moved {moved}");
        assert_eq!(sum(&out, RelativeAxisCode::REL_Y), 0);
    }

    #[test]
    fn half_deflection_is_much_slower_than_full() {
        let mut m = mouse();
        let mut out = Vec::new();
        m.event(EV_ABS, AbsoluteAxisCode::ABS_Y.0, 16384, &mut out);
        m.tick(Duration::from_secs(1), &mut out);
        let moved = sum(&out, RelativeAxisCode::REL_Y);
        assert!(moved > 0 && moved < MAX_SPEED as i32 / 3, "moved {moved}");
    }

    #[test]
    fn right_stick_down_scrolls_down() {
        let mut m = mouse();
        let mut out = Vec::new();
        m.event(EV_ABS, AbsoluteAxisCode::ABS_RY.0, 32767, &mut out);
        m.tick(Duration::from_secs(1), &mut out);
        assert_eq!(sum(&out, RelativeAxisCode::REL_WHEEL_HI_RES), -(MAX_SCROLL * HI_RES_NOTCH) as i32);
    }

    #[test]
    fn buttons_and_trigger_click() {
        let mut m = mouse();
        let mut out = Vec::new();
        m.event(EV_KEY, KeyCode::BTN_SOUTH.0, 1, &mut out);
        m.event(EV_KEY, KeyCode::BTN_EAST.0, 1, &mut out);
        assert_eq!(out[0], (EV_KEY, KeyCode::BTN_LEFT.0, 1));
        assert_eq!(out[2], (EV_KEY, KeyCode::BTN_RIGHT.0, 1));
        out.clear();
        m.event(EV_ABS, AbsoluteAxisCode::ABS_RZ.0, 900, &mut out);
        m.event(EV_ABS, AbsoluteAxisCode::ABS_RZ.0, 950, &mut out);
        assert_eq!(out, vec![(EV_KEY, KeyCode::BTN_LEFT.0, 1), (EV_SYN, 0, 0)], "one press, not one per report");
    }

    #[test]
    fn dpad_sends_arrows() {
        let mut m = mouse();
        let mut out = Vec::new();
        m.event(EV_ABS, AbsoluteAxisCode::ABS_HAT0Y.0, -1, &mut out);
        assert!(out.contains(&(EV_KEY, KeyCode::KEY_UP.0, 1)));
        out.clear();
        m.event(EV_ABS, AbsoluteAxisCode::ABS_HAT0Y.0, 0, &mut out);
        assert!(out.contains(&(EV_KEY, KeyCode::KEY_UP.0, 0)));
    }
}
