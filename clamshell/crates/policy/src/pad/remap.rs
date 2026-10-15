//! The keymap in force, applied to the controller on the way to the game (the Game route only). Rather
//! than rewriting events one by one, the controller's state (the router's snapshot, and the gyro's rate)
//! is mapped to what every output should be now: the game's pad (buttons, D-pad, sticks, triggers), the
//! keys and mouse buttons held on the virtual keyboard and mouse, wheel steps and pointer motion. Only
//! what changed since the last time is sent. So letting go is the same computation with nothing held:
//! leaving the game, losing the controller or changing the keymap mid-press never leaves anything down.
//! Chords press their modifiers before their key and let them go after it.
//!
//! Without a binding an input does what it does on the pad; the controller's events this doesn't know
//! (other buttons and axes) pass through as they are. While the layer key (the Thor's Back button,
//! [`LAYER_KEYS`]) is held the keymap's Back layer applies, falling through to the main one; the key
//! itself never reaches the game. A stick's and a trigger's tuning (deadzone, full point, curve,
//! inversion) applies to its values before they are mapped, to the input as on the pad as well.
//!
//! A button (or a D-pad or stick direction) with a hold's or a double's binding goes through a little
//! state machine ([`Phase`]): held past the hold's time it is a hold; let go sooner and pressed again
//! within [`DOUBLE_WINDOW`] a double; otherwise a press, sent as a short tap ([`TAP`]) once that is known.
//! Without either it is a press at once, held as long as the button. On top of the actuation, a binding
//! that toggles turns on at one actuation and off at the next, and one with turbo goes on and off at its
//! rate while on. Time moves these on as well as events: [`Remap::tick`] says when.

use std::time::{Duration, Instant};

use clamshell_core::keymap::{Actuation, Binding, Button, Chord, Dir, Input, Keymap, Kind, MouseButton, Output, PadOut, Side};
use evdev::{AbsoluteAxisCode, KeyCode, RelativeAxisCode};

use super::motion::Rate;
use super::route::{Out, PadSnapshot, Ranges};
use crate::input::{Press, Raw};
use crate::keyboard::{MODIFIERS, chord_keys};

/// A stick or trigger counts as pushed (a direction, a button) past this.
pub const HALF: f32 = 0.5;
/// Pointer speed of a stick at full deflection, pixels per second, at speeds 0, 25, 50, 75 and 100
/// (between, in proportion); below full, as the binding's deadzone and curve make it.
const STICK_POINTER: [f32; 5] = [500.0, 900.0, 1400.0, 2100.0, 3000.0];
/// The gyro to the pointer: pixels per degree turned, at the same speeds.
const GYRO_POINTER: [f32; 5] = [6.0, 10.0, 15.0, 22.0, 32.0];
/// The gyro to a stick: degrees per second for full deflection, at the same speeds.
const GYRO_STICK: [f32; 5] = [240.0, 160.0, 100.0, 60.0, 35.0];
/// Which way tilting the device like a steering wheel (roll) turns, against rotating it (yaw), when a
/// binding turns by tilting: the Thor's sign is a device test (docs/thor-gameplan.md).
const ROLL_TURNS: f32 = 1.0;
/// How often a stick on the pointer moves it.
pub const POINTER_EVERY: Duration = Duration::from_millis(8);
/// High-resolution wheel units per notch (the kernel's convention).
const NOTCH: i32 = 120;
/// A second press within this of letting go makes a double.
pub const DOUBLE_WINDOW: Duration = Duration::from_millis(300);
/// A press decided only after letting go (the input has a hold or a double) is sent as a tap this long.
pub const TAP: Duration = Duration::from_millis(60);

/// Where a digital input's press is.
#[derive(Debug, Clone, Copy, PartialEq)]
enum Phase {
    Up,
    /// Down since then: not yet known to be a press, a hold or a double.
    Deciding(Instant),
    /// Let go then after a short press: a second press soon makes a double.
    Waiting(Instant),
    /// An actuation on: while held, or (a tap) until then.
    On { act: Actuation, until: Option<Instant> },
}

/// A binding's on and off: its actuation on now, its toggle's state, and since when it has been on (turbo's
/// beat counts from there).
#[derive(Debug, Clone, Copy, Default, PartialEq)]
struct Beat {
    raw: bool,
    latched: bool,
    since: Option<Instant>,
}

impl Beat {
    /// Follows the actuation (`raw`, now): a toggle flips at each start of one.
    fn follow(&mut self, raw: bool, toggle: bool, now: Instant) {
        let rising = raw && !self.raw;
        self.raw = raw;
        if toggle {
            if rising {
                self.latched = !self.latched;
                self.since = self.latched.then_some(now);
            }
        } else {
            self.latched = false;
            if rising {
                self.since = Some(now);
            }
        }
    }

    /// Whether the output is on: the actuation's (or the toggle's) state, beating at `turbo` a second.
    fn on(&self, b: &Binding, now: Instant) -> bool {
        let on = if b.toggle { self.latched } else { self.raw };
        match (on, b.turbo, self.since) {
            (false, ..) => false,
            (true, 0, _) | (true, _, None) => true,
            (true, rate, Some(since)) => (now.saturating_duration_since(since).as_nanos() / half_beat(rate)).is_multiple_of(2),
        }
    }

    /// When its turbo next turns on or off (none: no turbo, or off).
    fn next_beat(&self, b: &Binding, now: Instant) -> Option<Instant> {
        let on = if b.toggle { self.latched } else { self.raw };
        let since = self.since.filter(|_| on && b.turbo > 0)?;
        let half = half_beat(b.turbo);
        let n = now.saturating_duration_since(since).as_nanos() / half + 1;
        Some(since + Duration::from_nanos(u64::try_from(n * half).unwrap_or(u64::MAX)))
    }
}

/// Half a turbo beat at `rate` a second, in nanoseconds (on for one half, off for the other).
fn half_beat(rate: u8) -> u128 {
    500_000_000 / u128::from(rate.max(1))
}

/// A digital input's actuations: its press's phase, and each actuation's beat.
#[derive(Debug, Clone, Copy, PartialEq)]
struct Actuator {
    input: Input,
    phase: Phase,
    beats: [Beat; 3],
    /// When the phase moves on by itself (a hold's time, a double's window, a tap's end).
    due: Option<Instant>,
}

impl Actuator {
    fn new(input: Input) -> Actuator {
        Actuator { input, phase: Phase::Up, beats: [Beat::default(); 3], due: None }
    }

    /// The input on or off at `now`, with a hold's binding (its time) and a double's or not: the phase
    /// moves on.
    fn step(&mut self, on: bool, hold: Option<Duration>, double: bool, now: Instant) {
        // (A press during a tap starts a new one: twice round at most.)
        for _ in 0..2 {
            let next = match self.phase {
                Phase::Up if on && hold.is_none() && !double => Phase::On { act: Actuation::Press, until: None },
                Phase::Up if on => Phase::Deciding(now),
                Phase::Up => Phase::Up,
                Phase::Deciding(at) if on => match hold {
                    Some(t) if now >= at + t => Phase::On { act: Actuation::Hold, until: None },
                    Some(_) => Phase::Deciding(at),
                    None if double && now < at + DOUBLE_WINDOW => Phase::Deciding(at),
                    // (Held past the window with no hold: a press after all.)
                    None => Phase::On { act: Actuation::Press, until: None },
                },
                Phase::Deciding(_) if double => Phase::Waiting(now),
                Phase::Deciding(_) => Phase::On { act: Actuation::Press, until: Some(now + TAP) },
                Phase::Waiting(_) if on => Phase::On { act: Actuation::Double, until: None },
                Phase::Waiting(at) if now >= at + DOUBLE_WINDOW => Phase::On { act: Actuation::Press, until: Some(now + TAP) },
                Phase::Waiting(at) => Phase::Waiting(at),
                Phase::On { until: None, .. } if !on => Phase::Up,
                Phase::On { until: Some(_), .. } if on => {
                    self.phase = Phase::Up;
                    continue;
                }
                Phase::On { until: Some(t), .. } if now >= t => Phase::Up,
                p @ Phase::On { .. } => p,
            };
            self.phase = next;
            break;
        }
        self.due = match self.phase {
            Phase::Deciding(at) => Some(hold.map_or(at + DOUBLE_WINDOW, |t| at + t)),
            Phase::Waiting(at) => Some(at + DOUBLE_WINDOW),
            Phase::On { until, .. } => until,
            Phase::Up => None,
        };
    }

    /// Whether `act` is on now (as the phase says).
    fn actuated(&self, act: Actuation) -> bool {
        matches!(self.phase, Phase::On { act: a, .. } if a == act)
    }

    fn beat(&self, act: Actuation) -> Beat {
        self.beats.get(act as usize).copied().unwrap_or_default()
    }
}

/// The layer key's codes: the Thor's Back button, under the screen. (KEY_BACK is assumed until the
/// device test, docs/thor-gameplan.md.)
pub const LAYER_KEYS: [KeyCode; 1] = [KeyCode::KEY_BACK];

/// Whether a code is the layer key's.
pub fn is_layer_key(code: KeyCode) -> bool {
    LAYER_KEYS.contains(&code)
}

/// The controller's codes for a button (the back buttons go by several names).
pub fn button_codes(b: Button) -> &'static [KeyCode] {
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
        // BTN_TRIGGER_HAPPY1-4 (xpad's paddles), BTN_GRIPL/R/L2/R2 (newer kernels' back buttons).
        Button::L4 => &[KeyCode::BTN_TRIGGER_HAPPY1, KeyCode(0x224)],
        Button::R4 => &[KeyCode::BTN_TRIGGER_HAPPY2, KeyCode(0x225)],
        Button::L5 => &[KeyCode::BTN_TRIGGER_HAPPY3, KeyCode(0x226)],
        Button::R5 => &[KeyCode::BTN_TRIGGER_HAPPY4, KeyCode(0x227)],
    }
}

/// The button a controller's code is, if it is one.
pub fn button_of(code: KeyCode) -> Option<Button> {
    Button::ALL.into_iter().find(|b| button_codes(*b).contains(&code))
}

/// The D-pad as buttons (controllers without a hat).
fn dpad_code(d: Dir) -> KeyCode {
    match d {
        Dir::Up => KeyCode::BTN_DPAD_UP,
        Dir::Down => KeyCode::BTN_DPAD_DOWN,
        Dir::Left => KeyCode::BTN_DPAD_LEFT,
        Dir::Right => KeyCode::BTN_DPAD_RIGHT,
    }
}

pub fn stick_axes(s: Side) -> (AbsoluteAxisCode, AbsoluteAxisCode) {
    match s {
        Side::Left => (AbsoluteAxisCode::ABS_X, AbsoluteAxisCode::ABS_Y),
        Side::Right => (AbsoluteAxisCode::ABS_RX, AbsoluteAxisCode::ABS_RY),
    }
}

pub fn trigger_axis(s: Side) -> AbsoluteAxisCode {
    match s {
        Side::Left => AbsoluteAxisCode::ABS_Z,
        Side::Right => AbsoluteAxisCode::ABS_RZ,
    }
}

const HAT: [AbsoluteAxisCode; 2] = [AbsoluteAxisCode::ABS_HAT0X, AbsoluteAxisCode::ABS_HAT0Y];

fn mouse_code(m: MouseButton) -> KeyCode {
    match m {
        MouseButton::Left => KeyCode::BTN_LEFT,
        MouseButton::Right => KeyCode::BTN_RIGHT,
        MouseButton::Middle => KeyCode::BTN_MIDDLE,
        MouseButton::Back => KeyCode::BTN_SIDE,
        MouseButton::Forward => KeyCode::BTN_EXTRA,
    }
}

/// The mouse buttons a keymap may press (the virtual mouse has them).
pub fn mouse_codes() -> [KeyCode; 5] {
    MouseButton::ALL.map(mouse_code)
}

/// Whether the remapping handles this event (else it passes through).
pub fn known(raw: Raw) -> bool {
    if let Some((code, _)) = raw.as_key() {
        return button_of(code).is_some() || is_layer_key(code) || Dir::ALL.into_iter().any(|d| dpad_code(d) == code);
    }
    raw.as_abs().is_some_and(|(axis, _)| {
        HAT.contains(&axis) || Side::ALL.into_iter().any(|s| trigger_axis(s) == axis || stick_axes(s).0 == axis || stick_axes(s).1 == axis)
    })
}

/// A speed (0 to 100) along a table of five speeds, at 0, 25, 50, 75 and 100 (between them, in
/// proportion).
fn step(table: &[f32; 5], speed: u8) -> f32 {
    let at = f32::from(speed.min(100)) / 25.0;
    let i = (at.floor() as usize).min(3);
    let (Some(a), Some(b)) = (table.get(i), table.get(i + 1)) else {
        return 1.0;
    };
    a + (b - a) * (at - i as f32)
}

/// A value from 0 to 1 with its deadzone and full point (percent): 0 up to the deadzone, 1 from the
/// full point, stretched between.
fn range(v: f32, deadzone: u8, full: u8) -> f32 {
    let (dz, full) = (f32::from(deadzone) / 100.0, f32::from(full.max(deadzone.saturating_add(1))) / 100.0);
    ((v - dz) / (full - dz).max(0.01)).clamp(0.0, 1.0)
}

/// A stick's position (each axis -1 to 1, y down) tuned by its binding: inverted, its distance from the
/// middle through the deadzone, the full point and the curve (the direction kept).
pub fn tune_stick((x, y): (f32, f32), b: &Binding) -> (f32, f32) {
    let (x, y) = (if b.invert_x { -x } else { x }, if b.invert_y { -y } else { y });
    if b.deadzone == 0 && b.full >= 100 && b.curve == 0 {
        return (x, y);
    }
    let len = x.hypot(y);
    if len <= 0.0 {
        return (0.0, 0.0);
    }
    let tuned = range(len.min(1.0), b.deadzone, b.full).powf(1.0 + 2.0 * f32::from(b.curve) / 100.0);
    (x / len * tuned, y / len * tuned)
}

/// What every output should be now.
#[derive(Debug, Clone, Default, PartialEq)]
struct Wanted {
    /// The game's pad: buttons down, and every axis' value.
    pad_keys: Vec<KeyCode>,
    pad_axes: Vec<(AbsoluteAxisCode, i32)>,
    /// Keys and mouse buttons down on the virtual keyboard and mouse.
    keys: Vec<KeyCode>,
    /// The inputs whose wheel step is due (each press steps once).
    wheels: Vec<(Input, Dir)>,
    /// Sticks moving the pointer: deflection and speed step.
    pointer: Vec<((f32, f32), u8)>,
}

/// The remapping, and what it last sent.
pub struct Remap {
    keymap: Keymap,
    ranges: Ranges,
    sent: Wanted,
    /// The gyro's rate now (degrees per second), and the pointer's motion not yet sent (sub-pixel).
    gyro: Rate,
    carry: (f32, f32),
    /// When a stick last moved the pointer.
    last_pointer: Option<Instant>,
    /// The digital inputs' actuations (those that have been pressed).
    actuators: Vec<Actuator>,
}

impl Remap {
    pub fn new(ranges: Ranges) -> Remap {
        Remap { keymap: Keymap::default(), ranges, sent: Wanted::default(), gyro: Rate::default(), carry: (0.0, 0.0), last_pointer: None, actuators: Vec::new() }
    }

    /// The pad's state as it was when nothing had been sent (a fresh pad, or after letting go).
    fn rest(&self) -> Wanted {
        Wanted::default()
    }

    /// A new keymap: what it makes of the controller now replaces what the old one held.
    pub fn set_keymap(&mut self, keymap: Keymap, snap: &PadSnapshot, live: bool, out: &mut Out) {
        self.keymap = keymap;
        if live {
            self.update(snap, Instant::now(), out);
        }
    }

    /// Lets go of everything sent (leaving the game, the controller gone): the keyboard's and mouse's
    /// keys up, the pad's buttons up and axes at rest.
    pub fn release(&mut self, out: &mut Out) {
        let rest = Wanted { pad_axes: self.sent.pad_axes.iter().map(|&(a, _)| (a, self.axis_rest(a))).collect(), ..self.rest() };
        self.send(rest, out);
        self.sent = self.rest();
        self.gyro = Rate::default();
        self.carry = (0.0, 0.0);
        self.last_pointer = None;
        self.actuators.clear();
    }

    fn axis_rest(&self, axis: AbsoluteAxisCode) -> i32 {
        if Side::ALL.into_iter().any(|s| trigger_axis(s) == axis) { self.ranges.trigger.min } else { 0 }
    }

    /// Takes the outputs to be what the keymap makes of `snap` already (without sending anything).
    pub fn prime(&mut self, snap: &PadSnapshot) {
        let now = Instant::now();
        self.actuate(snap, now);
        self.sent = self.wanted(snap, now);
    }

    /// The controller's state changed (`snap` is it now, at `now`): the outputs follow.
    pub fn update(&mut self, snap: &PadSnapshot, now: Instant, out: &mut Out) {
        self.actuate(snap, now);
        let wanted = self.wanted(snap, now);
        self.send(wanted, out);
    }

    /// The digital inputs' actuations follow the controller at `now`.
    fn actuate(&mut self, snap: &PadSnapshot, now: Instant) {
        let inputs = Input::all().filter(|i| i.kind() == Kind::Digital || (i.kind() == Kind::Trigger && self.binding(*i, snap).is_some_and(|b| b.output.digital())));
        for input in inputs.collect::<Vec<_>>() {
            let on = self.pressed(input, snap);
            let back = Self::back(snap);
            let hold = self.keymap.lookup_of(input, Actuation::Hold, back).map(|b| Duration::from_millis(u64::from(b.hold_time) * 10));
            let double = self.keymap.lookup_of(input, Actuation::Double, back).is_some();
            let binding = |act| match act {
                Actuation::Press => self.binding(input, snap),
                act => self.keymap.lookup_of(input, act, back).copied(),
            };
            let toggles = Actuation::ALL.map(|a| binding(a).is_some_and(|b| b.toggle));
            let found = self.actuators.iter().position(|a| a.input == input);
            if found.is_none() && !on {
                continue;
            }
            let i = found.unwrap_or_else(|| {
                self.actuators.push(Actuator::new(input));
                self.actuators.len() - 1
            });
            let Some(a) = self.actuators.get_mut(i) else {
                continue;
            };
            a.step(on, hold, double, now);
            for (act, toggle) in Actuation::ALL.into_iter().zip(toggles) {
                let raw = a.actuated(act);
                if let Some(beat) = a.beats.get_mut(act as usize) {
                    beat.follow(raw, toggle, now);
                }
            }
        }
        // (Those at rest and holding nothing on are dropped.)
        self.actuators.retain(|a| a.phase != Phase::Up || a.beats.iter().any(|b| b.raw || b.latched));
    }

    /// Whether a digital input is down: a trigger past its press binding's threshold, the rest as
    /// [`Remap::digital`].
    fn pressed(&self, input: Input, snap: &PadSnapshot) -> bool {
        match (input, self.binding(input, snap)) {
            (Input::Trigger(s), Some(b)) => {
                let v = self.trigger(s, snap);
                v * 100.0 >= f32::from(b.threshold) && v > 0.0
            }
            _ => self.digital(input, snap),
        }
    }

    /// The gyro's rate now, at `dt` since its last sample: the pointer moves by it, a stick follows it.
    pub fn motion(&mut self, rate: Rate, dt: f32, snap: &PadSnapshot, out: &mut Out) {
        self.gyro = rate;
        if let Some(b) = self.gyro_binding(snap)
            && b.output == Output::Pointer
        {
            let px = step(&GYRO_POINTER, b.speed);
            let (x, y) = gyro_turn(&b, rate);
            let gain = gyro_gain(&b, (x, y));
            // Turning left (yaw) moves the pointer left; tilting the top back (pitch) moves it up.
            self.carry.0 -= x * gain * dt * px;
            self.carry.1 -= y * gain * dt * px;
            self.flush_pointer(out);
        }
        self.update(snap, Instant::now(), out);
    }

    /// Time passed: actuations move on (a hold's time, a double's window, a tap's end, turbo's beat), and
    /// sticks on the pointer move it; when to tick next, if anything waits for time.
    pub fn tick(&mut self, now: Instant, snap: &PadSnapshot, out: &mut Out) -> Option<Instant> {
        if !self.actuators.is_empty() {
            self.update(snap, now, out);
        }
        let timed = self.next_due(snap, now);
        let pointer = self.tick_pointer(now, out);
        [timed, pointer].into_iter().flatten().min()
    }

    /// When an actuation next moves on by itself, or a turbo beats.
    fn next_due(&self, snap: &PadSnapshot, now: Instant) -> Option<Instant> {
        let back = Self::back(snap);
        let mut next = None;
        for a in &self.actuators {
            let mut times = vec![a.due];
            for act in Actuation::ALL {
                let b = match act {
                    Actuation::Press => self.binding(a.input, snap),
                    act => self.keymap.lookup_of(a.input, act, back).copied(),
                };
                times.push(b.and_then(|b| a.beat(act).next_beat(&b, now)));
            }
            next = times.into_iter().flatten().chain(next).min();
        }
        next
    }

    /// Sticks on the pointer move it; when to tick next for that, while one is deflected.
    fn tick_pointer(&mut self, now: Instant, out: &mut Out) -> Option<Instant> {
        if self.sent.pointer.is_empty() {
            self.last_pointer = None;
            return None;
        }
        let last = *self.last_pointer.get_or_insert(now);
        let dt = now.duration_since(last);
        if dt >= POINTER_EVERY {
            let secs = dt.as_secs_f32();
            for &((x, y), speed) in &self.sent.pointer {
                let max = step(&STICK_POINTER, speed);
                self.carry.0 += x * max * secs;
                self.carry.1 += y * max * secs;
            }
            self.flush_pointer(out);
            self.last_pointer = Some(now);
        }
        Some(self.last_pointer.unwrap_or(now) + POINTER_EVERY)
    }

    /// Sends the whole pixels of the pointer's motion; keeps the rest.
    fn flush_pointer(&mut self, out: &mut Out) {
        let (dx, dy) = (self.carry.0.trunc(), self.carry.1.trunc());
        self.carry = (self.carry.0 - dx, self.carry.1 - dy);
        let start = out.mouse.len();
        for (axis, v) in [(RelativeAxisCode::REL_X, dx), (RelativeAxisCode::REL_Y, dy)] {
            if v != 0.0 {
                out.mouse.push(Raw::rel(axis, v as i32));
            }
        }
        if out.mouse.len() > start {
            out.mouse.push(Raw::REPORT);
        }
    }

    /// Whether the layer key is held (the Back layer applies).
    fn back(snap: &PadSnapshot) -> bool {
        LAYER_KEYS.iter().any(|&k| snap.held(k))
    }

    /// `input`'s binding in the layer in force, if it has one.
    fn get(&self, input: Input, snap: &PadSnapshot) -> Option<&Binding> {
        self.keymap.lookup(input, Self::back(snap))
    }

    /// The gyro's binding, if it acts now: always without a hold; with one, while it is held (or, a
    /// ratchet, while it isn't).
    fn gyro_binding(&self, snap: &PadSnapshot) -> Option<Binding> {
        let b = *self.get(Input::Gyro, snap)?;
        let on = b.hold.is_none_or(|h| self.digital(h, snap) != b.ratchet);
        on.then_some(b)
    }

    fn binding(&self, input: Input, snap: &PadSnapshot) -> Option<Binding> {
        if let Some(b) = self.get(input, snap) {
            return Some(*b);
        }
        // As on the pad.
        let output = match input {
            Input::Button(b) => Output::Pad(PadOut::Button(b)),
            Input::Dpad(d) => Output::Pad(PadOut::Dpad(d)),
            Input::Stick(s) => Output::Stick(s),
            Input::Trigger(s) => Output::Trigger(s),
            Input::StickDir(..) | Input::Gyro => return None,
        };
        Some(Binding::to(output))
    }

    /// The stick's middle, and half its travel.
    fn stick_middle(&self) -> (i32, f32) {
        let r = self.ranges.stick;
        ((r.min + r.max + 1) / 2, ((r.max - r.min) as f32 / 2.0).max(1.0))
    }

    /// A stick's position (each axis -1 to 1, y down), tuned by its binding.
    fn tuned_stick(&self, s: Side, snap: &PadSnapshot) -> (f32, f32) {
        let (ax, ay) = stick_axes(s);
        let at = |a| self.ranges.stick.centered(snap.value(a).unwrap_or(0));
        let xy = (at(ax), at(ay));
        match self.get(Input::Stick(s), snap) {
            Some(b) => tune_stick(xy, b),
            None => xy,
        }
    }

    /// A stick as raw units from its middle (y down), tuned, its bound directions taken out (they act
    /// as buttons instead).
    fn stick(&self, s: Side, snap: &PadSnapshot) -> (f32, f32) {
        let (mut x, mut y) = match self.get(Input::Stick(s), snap) {
            Some(b) if b.shapes() => {
                let half = self.stick_middle().1;
                let (x, y) = self.tuned_stick(s, snap);
                (x * half, y * half)
            }
            // (Not shaped: in raw units as they are.)
            _ => {
                let (ax, ay) = stick_axes(s);
                let mid = self.stick_middle().0;
                let at = |a| snap.value(a).map_or(0.0, |v| (v - mid) as f32);
                (at(ax), at(ay))
            }
        };
        let bound = |d| self.get(Input::StickDir(s, d), snap).is_some();
        if (x < 0.0 && bound(Dir::Left)) || (x > 0.0 && bound(Dir::Right)) {
            x = 0.0;
        }
        if (y < 0.0 && bound(Dir::Up)) || (y > 0.0 && bound(Dir::Down)) {
            y = 0.0;
        }
        (x, y)
    }

    /// A trigger's pull, 0 to 1, tuned by its binding (deadzone, full point).
    fn trigger(&self, s: Side, snap: &PadSnapshot) -> f32 {
        let v = self.ranges.trigger.unit(snap.value(trigger_axis(s)).unwrap_or(self.ranges.trigger.min));
        match self.get(Input::Trigger(s), snap) {
            Some(b) if b.deadzone > 0 || b.full < 100 => range(v, b.deadzone, b.full),
            _ => v,
        }
    }

    /// Whether a digital input is on (a trigger: past half).
    fn digital(&self, input: Input, snap: &PadSnapshot) -> bool {
        let pushed = |(x, y): (f32, f32), d| match d {
            Dir::Up => y <= -HALF,
            Dir::Down => y >= HALF,
            Dir::Left => x <= -HALF,
            Dir::Right => x >= HALF,
        };
        match input {
            Input::Button(b) => button_codes(b).iter().any(|&c| snap.held(c)),
            Input::Dpad(d) => {
                let hat = |a| snap.value(a).unwrap_or(0) as f32;
                snap.held(dpad_code(d)) || pushed((hat(AbsoluteAxisCode::ABS_HAT0X), hat(AbsoluteAxisCode::ABS_HAT0Y)), d)
            }
            Input::StickDir(s, d) => pushed(self.tuned_stick(s, snap), d),
            Input::Trigger(s) => self.trigger(s, snap) >= HALF,
            Input::Stick(_) | Input::Gyro => false,
        }
    }

    /// Whether `input`'s binding of `act` is on at `now` (as its actuation, toggle and turbo make it).
    fn on(&self, input: Input, act: Actuation, b: &Binding, now: Instant) -> bool {
        self.actuators.iter().find(|a| a.input == input).is_some_and(|a| a.beat(act).on(b, now))
    }

    /// What every output should be for the controller in `snap` at `now`.
    fn wanted(&self, snap: &PadSnapshot, now: Instant) -> Wanted {
        let mut w = Wanted::default();
        let mut sticks = [(0.0f32, 0.0f32); 2];
        let mut triggers = [0.0f32; 2];
        let mut dpad = [false; 4];
        let digital = |w: &mut Wanted, input: Input, on: bool, output: &Output, triggers: &mut [f32; 2], dpad: &mut [bool; 4]| {
            if !on {
                return;
            }
            match output {
                Output::Pad(PadOut::Button(b)) => match (b.is_back(), input) {
                    // A back button as itself: the code it came with.
                    (true, Input::Button(own)) if own == *b => w.pad_keys.extend(button_codes(*b).iter().filter(|&&c| snap.held(c))),
                    (true, _) => {}
                    (false, _) => w.pad_keys.extend(button_codes(*b).first()),
                },
                Output::Pad(PadOut::Dpad(d)) => {
                    if let Some(slot) = dpad.get_mut(*d as usize) {
                        *slot = true;
                    }
                }
                Output::Pad(PadOut::Trigger(s)) => {
                    if let Some(t) = triggers.get_mut(*s as usize) {
                        *t = 1.0;
                    }
                }
                Output::Key(chord) => chord_down(&mut w.keys, chord),
                Output::Mouse(m) => w.keys.push(mouse_code(*m)),
                Output::Wheel(d) => w.wheels.push((input, *d)),
                _ => {}
            }
        };
        for input in Input::all() {
            let Some(b) = self.binding(input, snap) else {
                continue;
            };
            match input {
                Input::Button(_) | Input::Dpad(_) | Input::StickDir(..) => {
                    let on = self.on(input, Actuation::Press, &b, now);
                    digital(&mut w, input, on, &b.output, &mut triggers, &mut dpad);
                    // Its hold and double, if it has them.
                    for act in [Actuation::Hold, Actuation::Double] {
                        if let Some(other) = self.keymap.lookup_of(input, act, Self::back(snap)) {
                            let on = self.on(input, act, other, now);
                            digital(&mut w, input, on, &other.output, &mut triggers, &mut dpad);
                        }
                    }
                }
                Input::Trigger(s) => {
                    let v = self.trigger(s, snap);
                    match b.output {
                        Output::Trigger(to) => {
                            if let Some(t) = triggers.get_mut(to as usize) {
                                *t = t.max(v);
                            }
                        }
                        ref o => digital(&mut w, input, self.on(input, Actuation::Press, &b, now), o, &mut triggers, &mut dpad),
                    }
                }
                Input::Stick(s) => {
                    let (x, y) = self.stick(s, snap);
                    let half = self.stick_middle().1;
                    match b.output {
                        Output::Stick(to) => {
                            if let Some(t) = sticks.get_mut(to as usize) {
                                *t = (t.0 + x, t.1 + y);
                            }
                        }
                        // (Tuned already: its deadzone and curve are the binding's.)
                        Output::Pointer if (x, y) != (0.0, 0.0) => w.pointer.push(((x / half, y / half), b.speed)),
                        Output::Dpad => {
                            for d in Dir::ALL {
                                let on = self.digital(Input::StickDir(s, d), snap);
                                digital(&mut w, input, on, &Output::Pad(PadOut::Dpad(d)), &mut triggers, &mut dpad);
                            }
                        }
                        Output::Keys(keys) => {
                            // Up, left, down, right.
                            for (d, chord) in [Dir::Up, Dir::Left, Dir::Down, Dir::Right].into_iter().zip(keys) {
                                let on = self.digital(Input::StickDir(s, d), snap);
                                digital(&mut w, input, on, &Output::Key(chord), &mut triggers, &mut dpad);
                            }
                        }
                        _ => {}
                    }
                }
                Input::Gyro => {}
            }
        }
        if let Some(b) = self.gyro_binding(snap)
            && let Output::Stick(to) = b.output
            && let Some(t) = sticks.get_mut(to as usize)
        {
            let full = step(&GYRO_STICK, b.speed);
            let half = self.stick_middle().1;
            let (x, y) = gyro_turn(&b, self.gyro);
            let gain = gyro_gain(&b, (x, y));
            let (vx, vy) = stick_from(-x * gain / full, -y * gain / full, b.stick_min);
            *t = (t.0 + vx * half, t.1 + vy * half);
        }
        // The pad's axes from the outputs: sticks, triggers, the D-pad.
        let stick = self.ranges.stick;
        let mid = self.stick_middle().0;
        let raw = |v: f32| (mid + v.round() as i32).clamp(stick.min, stick.max);
        for (s, (x, y)) in Side::ALL.into_iter().zip(sticks) {
            let (ax, ay) = stick_axes(s);

            // A stick as itself keeps its exact values.
            let plain = self.get(Input::Stick(s), snap).is_none_or(|b| b.output == Output::Stick(s) && !b.shapes());
            let own = plain && self.get(Input::Gyro, snap).is_none_or(|g| g.output != Output::Stick(s));
            let other = Side::ALL.into_iter().filter(|o| *o != s).any(|o| self.get(Input::Stick(o), snap).is_some_and(|b| b.output == Output::Stick(s)));
            let dirs = Dir::ALL.into_iter().any(|d| self.get(Input::StickDir(s, d), snap).is_some());
            // (Only the axes the pad has: the game's pad has the controller's.)
            for (axis, v) in [(ax, x), (ay, y)] {
                if let Some(value) = snap.value(axis) {
                    w.pad_axes.push((axis, if own && !other && !dirs { value } else { raw(v) }));
                }
            }
        }
        let trig = self.ranges.trigger;
        for (s, v) in Side::ALL.into_iter().zip(triggers) {
            let axis = trigger_axis(s);
            if snap.value(axis).is_none() {
                continue;
            }
            let plain = self.get(Input::Trigger(s), snap).is_none_or(|b| b.output == Output::Trigger(s) && !b.shapes());
            let own = plain && v == self.trigger(s, snap);
            let value = if own { snap.value(axis).unwrap_or(trig.min) } else { trig.min + ((trig.max - trig.min) as f32 * v.clamp(0.0, 1.0)).round() as i32 };
            w.pad_axes.push((axis, value));
        }
        let [up, down, left, right] = dpad;
        // The D-pad: a hat if the pad has one, else buttons.
        if snap.value(AbsoluteAxisCode::ABS_HAT0X).is_some() {
            w.pad_axes.push((AbsoluteAxisCode::ABS_HAT0X, i32::from(right) - i32::from(left)));
            w.pad_axes.push((AbsoluteAxisCode::ABS_HAT0Y, i32::from(down) - i32::from(up)));
        } else {
            for (d, on) in [(Dir::Up, up), (Dir::Down, down), (Dir::Left, left), (Dir::Right, right)] {
                if on {
                    w.pad_keys.push(dpad_code(d));
                }
            }
        }
        w.pad_keys.sort_by_key(|k| k.0);
        w.pad_keys.dedup();
        w.keys.dedup();
        w
    }

    /// Sends what differs between what was sent and `wanted`, and keeps it as sent.
    fn send(&mut self, wanted: Wanted, out: &mut Out) {
        // The game's pad.
        let start = out.pad.len();
        for &k in &self.sent.pad_keys {
            if !wanted.pad_keys.contains(&k) {
                out.pad.push(Raw::key(k, Press::Up));
            }
        }
        for &k in &wanted.pad_keys {
            if !self.sent.pad_keys.contains(&k) {
                out.pad.push(Raw::key(k, Press::Down));
            }
        }
        for &(axis, v) in &wanted.pad_axes {
            if self.sent.pad_axes.iter().find(|(a, _)| *a == axis).is_none_or(|(_, was)| *was != v) {
                out.pad.push(Raw::abs(axis, v));
            }
        }
        if out.pad.len() > start {
            out.pad.push(Raw::REPORT);
        }
        // The keyboard and mouse: keys up first, then modifiers up; modifiers down, then keys down.
        let start = out.mouse.len();
        let modifier = |k: &KeyCode| MODIFIERS.contains(k);
        let ups = self.sent.keys.iter().filter(|k| !wanted.keys.contains(k));
        let (mods_up, keys_up): (Vec<KeyCode>, Vec<KeyCode>) = ups.partition(|k| modifier(k));
        let downs = wanted.keys.iter().filter(|k| !self.sent.keys.contains(k));
        let (mods_down, keys_down): (Vec<KeyCode>, Vec<KeyCode>) = downs.partition(|k| modifier(k));
        for (keys, press) in [(keys_up, Press::Up), (mods_up, Press::Up), (mods_down, Press::Down), (keys_down, Press::Down)] {
            out.mouse.extend(keys.into_iter().map(|k| Raw::key(k, press)));
        }
        // A wheel step for each input that just came on.
        for &(input, d) in &wanted.wheels {
            if !self.sent.wheels.contains(&(input, d)) {
                let (axis, hi, v) = match d {
                    Dir::Up => (RelativeAxisCode::REL_WHEEL, RelativeAxisCode::REL_WHEEL_HI_RES, 1),
                    Dir::Down => (RelativeAxisCode::REL_WHEEL, RelativeAxisCode::REL_WHEEL_HI_RES, -1),
                    Dir::Left => (RelativeAxisCode::REL_HWHEEL, RelativeAxisCode::REL_HWHEEL_HI_RES, -1),
                    Dir::Right => (RelativeAxisCode::REL_HWHEEL, RelativeAxisCode::REL_HWHEEL_HI_RES, 1),
                };
                out.mouse.extend([Raw::rel(axis, v), Raw::rel(hi, v * NOTCH)]);
            }
        }
        if out.mouse.len() > start {
            out.mouse.push(Raw::REPORT);
        }
        self.sent = wanted;
    }
}

/// A chord's keys, held: its modifiers, then its key.
fn chord_down(keys: &mut Vec<KeyCode>, chord: &Chord) {
    let (mods, key) = chord_keys(chord);
    for k in mods.into_iter().chain(key) {
        if !keys.contains(&k) {
            keys.push(k);
        }
    }
}

/// The gyro's motion turned round across and up/down, as its binding says.
fn gyro_signs(b: &Binding) -> (f32, f32) {
    (if b.invert_x { -1.0 } else { 1.0 }, if b.invert_y { -1.0 } else { 1.0 })
}

/// The gyro's turning (degrees per second; across: yaw, plus roll when the binding turns by tilting; up
/// and down: pitch), inverted as the binding says, and tightened: slower than its tightening, scaled down
/// in proportion (half the tightening turns at a quarter), so a hand's tremor fades without a hard edge.
fn gyro_turn(b: &Binding, r: Rate) -> (f32, f32) {
    let (sx, sy) = gyro_signs(b);
    let across = if b.twist { r.yaw + ROLL_TURNS * r.roll } else { r.yaw };
    let (x, y) = (sx * across, sy * r.pitch);
    let tight = f32::from(b.tighten) / 10.0;
    let speed = x.hypot(y);
    let k = if tight > 0.0 && speed < tight { speed / tight } else { 1.0 };
    (x * k, y * k)
}

/// The gyro's acceleration: 1 for slow turns, up to 3 (at 100 %) for turns as fast as its "fastest at".
fn gyro_gain(b: &Binding, (x, y): (f32, f32)) -> f32 {
    let at = f32::from(b.accel_at.max(1));
    1.0 + 2.0 * f32::from(b.accel) / 100.0 * (x.hypot(y) / at).min(1.0)
}

/// A stick's deflection (each axis -1 to 1) kept within the circle, and starting at `min` percent: a
/// game's own stick deadzone doesn't swallow small motion.
fn stick_from(x: f32, y: f32, min: u8) -> (f32, f32) {
    let m = x.hypot(y);
    if m <= 0.0 {
        return (0.0, 0.0);
    }
    let min = f32::from(min) / 100.0;
    let to = min + (1.0 - min) * m.min(1.0);
    (x / m * to, y / m * to)
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::indexing_slicing)]
mod tests {
    use super::*;
    use crate::input::Range;
    use crate::pad::motion::Sample;
    use crate::pad::route::{Axis, PadRouter};
    use clamshell_core::PadRoute;
    use clamshell_core::keyboard::Key;

    const STICK_MAX: i32 = 32767;

    fn router(keymap: Keymap, route: PadRoute) -> (PadRouter, Out) {
        let ranges = Ranges { stick: Range::STICK, trigger: Range { min: 0, max: 1023 } };
        let axis = |code| Axis { code, value: 0, rest: 0 };
        let axes = [
            AbsoluteAxisCode::ABS_X,
            AbsoluteAxisCode::ABS_Y,
            AbsoluteAxisCode::ABS_RX,
            AbsoluteAxisCode::ABS_RY,
            AbsoluteAxisCode::ABS_Z,
            AbsoluteAxisCode::ABS_RZ,
            AbsoluteAxisCode::ABS_HAT0X,
            AbsoluteAxisCode::ABS_HAT0Y,
        ]
        .map(axis)
        .to_vec();
        let mut out = Out::default();
        let r = PadRouter::new(ranges, PadSnapshot::new(axes), route, keymap, &mut out);
        (r, Out::default())
    }

    fn keymap(bindings: &[(Input, Binding)]) -> Keymap {
        let mut k = Keymap::new(1, "test");
        for (i, b) in bindings {
            assert!(k.set(*i, Some(*b)), "{i:?}");
        }
        k
    }

    fn to(output: Output) -> Binding {
        Binding::to(output)
    }

    fn key(code: KeyCode, press: Press) -> Raw {
        Raw::key(code, press)
    }

    fn press(r: &mut PadRouter, code: KeyCode, down: bool, out: &mut Out) {
        r.event(key(code, if down { Press::Down } else { Press::Up }), Instant::now(), out);
        r.event(Raw::REPORT, Instant::now(), out);
    }

    fn axis(r: &mut PadRouter, code: AbsoluteAxisCode, v: i32, out: &mut Out) {
        r.event(Raw::abs(code, v), Instant::now(), out);
        r.event(Raw::REPORT, Instant::now(), out);
    }

    /// The keys down after these events, as the kernel would keep them.
    fn held(events: &[Raw]) -> Vec<KeyCode> {
        let mut down = Vec::new();
        for (k, p) in events.iter().filter_map(|e| e.as_key()) {
            down.retain(|&d| d != k);
            if p.held() {
                down.push(k);
            }
        }
        down.sort_by_key(|k| k.0);
        down
    }

    /// An axis' value after these events (every axis starts at 0 here).
    fn last(events: &[Raw], code: AbsoluteAxisCode) -> Option<i32> {
        Some(events.iter().rev().find_map(|e| e.as_abs().filter(|(c, _)| *c == code).map(|(_, v)| v)).unwrap_or(0))
    }

    fn sum(events: &[Raw], code: RelativeAxisCode) -> i32 {
        events.iter().filter(|e| e.kind == evdev::EventType::RELATIVE && e.code == code.0).map(|e| e.value).sum()
    }

    fn chord(text: &str) -> Chord {
        Chord::parse(text).unwrap()
    }

    #[test]
    fn a_button_to_another_button() {
        let (mut r, mut out) = router(clamshell_core::keymap::nintendo(1), PadRoute::Game);
        press(&mut r, KeyCode::BTN_SOUTH, true, &mut out);
        assert_eq!(held(&out.pad), vec![KeyCode::BTN_EAST], "A is B");
        press(&mut r, KeyCode::BTN_TL, true, &mut out);
        assert_eq!(held(&out.pad), vec![KeyCode::BTN_EAST, KeyCode::BTN_TL], "LB as it is");
        press(&mut r, KeyCode::BTN_SOUTH, false, &mut out);
        assert_eq!(held(&out.pad), vec![KeyCode::BTN_TL]);
    }

    #[test]
    fn a_button_to_a_key_chord_held_while_held() {
        let k = keymap(&[(Input::Button(Button::A), to(Output::Key(chord("ctrl+x"))))]);
        let (mut r, mut out) = router(k, PadRoute::Game);
        press(&mut r, KeyCode::BTN_SOUTH, true, &mut out);
        let downs: Vec<Raw> = out.mouse.iter().copied().filter(|e| e.as_key().is_some()).collect();
        assert_eq!(downs, vec![key(KeyCode::KEY_LEFTCTRL, Press::Down), key(KeyCode::KEY_X, Press::Down)], "the modifier first");
        assert!(held(&out.pad).is_empty(), "nothing on the game's pad");
        out.mouse.clear();
        r.event(key(KeyCode::BTN_SOUTH, Press::Repeat), Instant::now(), &mut out);
        assert!(out.mouse.is_empty(), "held: no repeats from here (the compositor repeats)");
        press(&mut r, KeyCode::BTN_SOUTH, false, &mut out);
        let ups: Vec<Raw> = out.mouse.iter().copied().filter(|e| e.as_key().is_some()).collect();
        assert_eq!(ups, vec![key(KeyCode::KEY_X, Press::Up), key(KeyCode::KEY_LEFTCTRL, Press::Up)], "the modifier last");
    }

    #[test]
    fn two_chords_share_a_modifier() {
        let k = keymap(&[(Input::Button(Button::A), to(Output::Key(chord("ctrl+x")))), (Input::Button(Button::B), to(Output::Key(chord("ctrl+c"))))]);
        let (mut r, mut out) = router(k, PadRoute::Game);
        press(&mut r, KeyCode::BTN_SOUTH, true, &mut out);
        press(&mut r, KeyCode::BTN_EAST, true, &mut out);
        press(&mut r, KeyCode::BTN_SOUTH, false, &mut out);
        assert_eq!(held(&out.mouse), vec![KeyCode::KEY_LEFTCTRL, KeyCode::KEY_C], "Ctrl stays for B's chord");
        press(&mut r, KeyCode::BTN_EAST, false, &mut out);
        assert!(held(&out.mouse).is_empty());
    }

    #[test]
    fn buttons_to_the_mouse_and_the_wheel() {
        let k = keymap(&[
            (Input::Button(Button::RB), to(Output::Mouse(MouseButton::Right))),
            (Input::Dpad(Dir::Down), to(Output::Wheel(Dir::Down))),
            (Input::Button(Button::Y), to(Output::Nothing)),
        ]);
        let (mut r, mut out) = router(k, PadRoute::Game);
        press(&mut r, KeyCode::BTN_TR, true, &mut out);
        assert_eq!(held(&out.mouse), vec![KeyCode::BTN_RIGHT]);
        axis(&mut r, AbsoluteAxisCode::ABS_HAT0Y, 1, &mut out);
        axis(&mut r, AbsoluteAxisCode::ABS_HAT0X, 1, &mut out);
        assert_eq!(sum(&out.mouse, RelativeAxisCode::REL_WHEEL), -1, "one step a press");
        assert_eq!(last(&out.pad, AbsoluteAxisCode::ABS_HAT0Y), Some(0), "not on the game's D-pad");
        assert_eq!(last(&out.pad, AbsoluteAxisCode::ABS_HAT0X), Some(1), "right as it is");
        axis(&mut r, AbsoluteAxisCode::ABS_HAT0Y, 0, &mut out);
        axis(&mut r, AbsoluteAxisCode::ABS_HAT0Y, 1, &mut out);
        assert_eq!(sum(&out.mouse, RelativeAxisCode::REL_WHEEL), -2);
        assert_eq!(sum(&out.mouse, RelativeAxisCode::REL_WHEEL_HI_RES), -240);
        press(&mut r, KeyCode::BTN_NORTH, true, &mut out);
        assert!(!held(&out.pad).contains(&KeyCode::BTN_NORTH), "Y does nothing");
    }

    #[test]
    fn a_stick_to_the_pointer_to_keys_and_to_the_other_stick() {
        let k = keymap(&[
            (Input::Stick(Side::Right), to(Output::Pointer)),
            (Input::Stick(Side::Left), to(Output::Keys([chord("w"), chord("a"), chord("s"), chord("d")]))),
        ]);
        let (mut r, mut out) = router(k, PadRoute::Game);
        let t0 = Instant::now();
        axis(&mut r, AbsoluteAxisCode::ABS_RX, STICK_MAX, &mut out);
        assert_eq!(last(&out.pad, AbsoluteAxisCode::ABS_RX), Some(0), "the game's stick stays at rest");
        let mut t = t0;
        let next = r.tick(t, &mut out);
        assert!(next.is_some(), "ticks while deflected");
        for _ in 0..125 {
            t += POINTER_EVERY;
            r.tick(t, &mut out);
        }
        let moved = sum(&out.mouse, RelativeAxisCode::REL_X);
        assert!((moved - 1400).abs() < 30, "about the stock speed for a second: {moved}");
        axis(&mut r, AbsoluteAxisCode::ABS_RX, 0, &mut out);
        r.tick(t + POINTER_EVERY, &mut out);
        assert_eq!(r.tick(t + POINTER_EVERY * 2, &mut out), None, "at rest: no ticks");
        // The left stick: W while up past half, A with left.
        axis(&mut r, AbsoluteAxisCode::ABS_Y, -20000, &mut out);
        axis(&mut r, AbsoluteAxisCode::ABS_X, -20000, &mut out);
        assert_eq!(held(&out.mouse), vec![KeyCode::KEY_W, KeyCode::KEY_A]);
        axis(&mut r, AbsoluteAxisCode::ABS_Y, -5000, &mut out);
        assert_eq!(held(&out.mouse), vec![KeyCode::KEY_A]);
        // The left stick to the right one instead.
        r.set_keymap(keymap(&[(Input::Stick(Side::Left), to(Output::Stick(Side::Right)))]), &mut out);
        assert!(held(&out.mouse).is_empty(), "the keys let go with the old keymap");
        assert_eq!((last(&out.pad, AbsoluteAxisCode::ABS_RX), last(&out.pad, AbsoluteAxisCode::ABS_X)), (Some(-20000), Some(0)));
        // Its up direction bound: that half is a button now.
        r.set_keymap(keymap(&[(Input::StickDir(Side::Left, Dir::Left), to(Output::Pad(PadOut::Button(Button::LB))))]), &mut out);
        assert_eq!((last(&out.pad, AbsoluteAxisCode::ABS_X), last(&out.pad, AbsoluteAxisCode::ABS_RX)), (Some(0), Some(0)));
        assert_eq!(held(&out.pad), vec![KeyCode::BTN_TL]);
    }

    #[test]
    fn a_stick_to_the_dpad() {
        let k = keymap(&[(Input::Stick(Side::Left), to(Output::Dpad))]);
        let (mut r, mut out) = router(k, PadRoute::Game);
        axis(&mut r, AbsoluteAxisCode::ABS_X, STICK_MAX, &mut out);
        assert_eq!((last(&out.pad, AbsoluteAxisCode::ABS_HAT0X), last(&out.pad, AbsoluteAxisCode::ABS_X)), (Some(1), Some(0)));
    }

    #[test]
    fn a_trigger_past_its_threshold_and_to_the_other_trigger() {
        let k = keymap(&[
            (Input::Trigger(Side::Left), Binding { threshold: 75, ..to(Output::Key(chord("shift"))) }),
            (Input::Trigger(Side::Right), to(Output::Trigger(Side::Left))),
        ]);
        let (mut r, mut out) = router(k, PadRoute::Game);
        axis(&mut r, AbsoluteAxisCode::ABS_Z, 600, &mut out);
        assert!(held(&out.mouse).is_empty(), "past half, short of 75 %");
        axis(&mut r, AbsoluteAxisCode::ABS_Z, 800, &mut out);
        assert_eq!(held(&out.mouse), vec![KeyCode::KEY_LEFTSHIFT]);
        assert_eq!(last(&out.pad, AbsoluteAxisCode::ABS_Z), Some(0), "not on the game's trigger");
        axis(&mut r, AbsoluteAxisCode::ABS_RZ, 512, &mut out);
        assert_eq!((last(&out.pad, AbsoluteAxisCode::ABS_Z), last(&out.pad, AbsoluteAxisCode::ABS_RZ)), (Some(512), Some(0)));
    }

    fn gyro(r: &mut PadRouter, ms: u64, yaw: f32, out: &mut Out) {
        r.motion(Sample { rate: super::super::motion::Rate { pitch: 0.0, yaw, roll: 0.0 }, at: Duration::from_millis(ms) }, out);
    }

    #[test]
    fn the_gyro_to_the_pointer_while_a_button_is_held() {
        let k = keymap(&[(Input::Gyro, Binding { hold: Some(Input::Trigger(Side::Left)), ..to(Output::Pointer) })]);
        let (mut r, mut out) = router(k, PadRoute::Game);
        for i in 0..=100 {
            gyro(&mut r, i * 4, 50.0, &mut out);
        }
        assert_eq!(sum(&out.mouse, RelativeAxisCode::REL_X), 0, "not held: nothing");
        axis(&mut r, AbsoluteAxisCode::ABS_Z, 1023, &mut out);
        assert_eq!(last(&out.pad, AbsoluteAxisCode::ABS_Z), Some(1023), "LT keeps its own mapping");
        for i in 101..=350 {
            gyro(&mut r, i * 4, -20.0, &mut out);
        }
        // A second at -20 °/s (turning right) at 15 px per degree: 300 px right.
        let moved = sum(&out.mouse, RelativeAxisCode::REL_X);
        assert!((moved - 300).abs() <= 2, "{moved}");
        // A sensor's offset at rest (0.5 °/s): tightened to a crawl, then learnt and taken off.
        let before = sum(&out.mouse, RelativeAxisCode::REL_X);
        for i in 351..=850 {
            gyro(&mut r, i * 4, 0.5, &mut out);
        }
        let learning = sum(&out.mouse, RelativeAxisCode::REL_X) - before;
        assert!(learning.abs() <= 5, "a few pixels while it learns: {learning}");
        let learnt = sum(&out.mouse, RelativeAxisCode::REL_X);
        for i in 851..=1100 {
            gyro(&mut r, i * 4, 0.5, &mut out);
        }
        assert_eq!(sum(&out.mouse, RelativeAxisCode::REL_X), learnt, "then nothing");
    }

    fn turning(r: &mut PadRouter, from: u64, ms: u64, yaw: f32, roll: f32, out: &mut Out) -> i32 {
        let before = sum(&out.mouse, RelativeAxisCode::REL_X);
        for i in from..=from + ms / 4 {
            r.motion(Sample { rate: super::super::motion::Rate { pitch: 0.0, yaw, roll }, at: Duration::from_millis(i * 4) }, out);
        }
        sum(&out.mouse, RelativeAxisCode::REL_X) - before
    }

    #[test]
    fn gyro_acceleration_tightening_and_tilting() {
        let pointer = |b: Binding| keymap(&[(Input::Gyro, Binding { ..b })]);
        // No acceleration: a second at 20 °/s and at 100 °/s go 15 px per degree.
        let (mut r, mut out) = router(pointer(to(Output::Pointer)), PadRoute::Game);
        assert!((turning(&mut r, 0, 1000, -20.0, 0.0, &mut out) - 300).abs() <= 3);
        assert!((turning(&mut r, 251, 1000, -100.0, 0.0, &mut out) - 1500).abs() <= 15);
        // Full acceleration, fastest at 60 °/s: slow turns as before, 100 °/s three times as far.
        let (mut r, mut out) = router(pointer(Binding { accel: 100, accel_at: 60, ..to(Output::Pointer) }), PadRoute::Game);
        let slow = turning(&mut r, 0, 1000, -6.0, 0.0, &mut out);
        assert!((slow - 108).abs() <= 3, "6 °/s: a tenth of the way to 60, so x1.2: {slow}");
        let fast = turning(&mut r, 251, 1000, -100.0, 0.0, &mut out);
        assert!((fast - 4500).abs() <= 45, "{fast}");
        // Tightening 2 °/s: 1 °/s goes at half (a quarter of the way), 4 °/s in full.
        let (mut r, mut out) = router(pointer(Binding { tighten: 20, ..to(Output::Pointer) }), PadRoute::Game);
        let tight = turning(&mut r, 0, 1000, -1.0, 0.0, &mut out);
        assert!((tight - 7).abs() <= 1, "{tight}");
        // Tilting turns too, with the binding's switch; not without.
        let (mut r, mut out) = router(pointer(to(Output::Pointer)), PadRoute::Game);
        assert_eq!(turning(&mut r, 0, 500, 0.0, -20.0, &mut out), 0);
        let (mut r, mut out) = router(pointer(Binding { twist: true, ..to(Output::Pointer) }), PadRoute::Game);
        assert!((turning(&mut r, 0, 1000, 0.0, -20.0, &mut out) - 300).abs() <= 3);
    }

    #[test]
    fn the_gyro_off_while_held_and_a_stick_from_its_minimum() {
        // A ratchet: on, but off while LB is held.
        let k = keymap(&[(Input::Gyro, Binding { hold: Some(Input::Button(Button::LB)), ratchet: true, ..to(Output::Pointer) })]);
        let (mut r, mut out) = router(k, PadRoute::Game);
        assert!(turning(&mut r, 0, 500, -20.0, 0.0, &mut out) > 140);
        press(&mut r, KeyCode::BTN_TL, true, &mut out);
        assert_eq!(turning(&mut r, 126, 500, -20.0, 0.0, &mut out), 0, "held: off");
        press(&mut r, KeyCode::BTN_TL, false, &mut out);
        assert!(turning(&mut r, 252, 500, -20.0, 0.0, &mut out) > 140);
        // A stick from 30 %: a slow turn starts there, full stays full.
        let k = keymap(&[(Input::Gyro, Binding { stick_min: 30, ..to(Output::Stick(Side::Right)) })]);
        let (mut r, mut out) = router(k, PadRoute::Game);
        gyro(&mut r, 0, -5.0, &mut out);
        let v = last(&out.pad, AbsoluteAxisCode::ABS_RX).unwrap();
        assert!((v - STICK_MAX * 335 / 1000).abs() < 400, "30 % + 70 % of 5 % of full: {v}");
        gyro(&mut r, 4, -500.0, &mut out);
        assert_eq!(last(&out.pad, AbsoluteAxisCode::ABS_RX), Some(STICK_MAX));
    }

    #[test]
    fn the_gyro_to_a_stick() {
        let k = keymap(&[(Input::Gyro, Binding { speed: 50, ..to(Output::Stick(Side::Right)) })]);
        let (mut r, mut out) = router(k, PadRoute::Game);
        gyro(&mut r, 0, -50.0, &mut out);
        let half = last(&out.pad, AbsoluteAxisCode::ABS_RX).unwrap();
        assert!((half - STICK_MAX / 2).abs() < 400, "50 of 100 °/s: half way: {half}");
        gyro(&mut r, 4, -500.0, &mut out);
        assert_eq!(last(&out.pad, AbsoluteAxisCode::ABS_RX), Some(STICK_MAX), "clamped");
        gyro(&mut r, 8, 0.0, &mut out);
        assert_eq!(last(&out.pad, AbsoluteAxisCode::ABS_RX), Some(0));
    }

    #[test]
    fn only_the_game_route_is_remapped_and_nothing_stays_down() {
        let k = keymap(&[
            (Input::Button(Button::A), to(Output::Key(chord("ctrl+x")))),
            (Input::Button(Button::B), to(Output::Pad(PadOut::Button(Button::X)))),
            (Input::Button(Button::X), to(Output::Mouse(MouseButton::Left))),
            (Input::Stick(Side::Right), to(Output::Pointer)),
        ]);
        let (mut r, mut out) = router(k.clone(), PadRoute::Ui);
        press(&mut r, KeyCode::BTN_SOUTH, true, &mut out);
        assert!(held(&out.mouse).is_empty() && out.events.contains(&clamshell_core::Event::Pad(clamshell_core::PadInput::A(true))), "the screens' A");
        // Into the game with A held: its chord goes down; B and X too.
        r.set_route(PadRoute::Game, &mut out);
        press(&mut r, KeyCode::BTN_EAST, true, &mut out);
        press(&mut r, KeyCode::BTN_WEST, true, &mut out);
        assert_eq!(held(&out.mouse), vec![KeyCode::KEY_LEFTCTRL, KeyCode::KEY_X, KeyCode::BTN_LEFT]);
        assert_eq!(held(&out.pad), vec![KeyCode::BTN_WEST]);
        // Away from the game: everything let go, keys, mouse buttons and the pad.
        r.set_route(PadRoute::Mouse, &mut out);
        assert!(held(&out.mouse).is_empty(), "{:?}", held(&out.mouse));
        assert!(held(&out.pad).is_empty());
        // Back, then a new keymap mid-press: the old outputs go up, the new ones down.
        r.set_route(PadRoute::Game, &mut out);
        assert_eq!(held(&out.pad), vec![KeyCode::BTN_WEST]);
        r.set_keymap(keymap(&[(Input::Button(Button::A), to(Output::Key(Chord::of(Key::Char('z')))))]), &mut out);
        assert_eq!(held(&out.mouse), vec![KeyCode::KEY_Z]);
        assert_eq!(held(&out.pad), vec![KeyCode::BTN_EAST, KeyCode::BTN_WEST], "B and X as themselves now");
        r.set_keymap(Keymap::default(), &mut out);
        assert!(held(&out.mouse).is_empty());
        assert_eq!(held(&out.pad), vec![KeyCode::BTN_SOUTH, KeyCode::BTN_EAST, KeyCode::BTN_WEST]);
        // The controller lost: nothing held on the keyboard.
        r.set_keymap(k, &mut out);
        r.lose(&mut out);
        assert!(held(&out.mouse).is_empty());
    }

    #[test]
    fn the_default_keymap_passes_everything_through() {
        let (mut r, mut out) = router(Keymap::default(), PadRoute::Game);
        press(&mut r, KeyCode::BTN_SOUTH, true, &mut out);
        axis(&mut r, AbsoluteAxisCode::ABS_X, 12345, &mut out);
        axis(&mut r, AbsoluteAxisCode::ABS_RZ, 77, &mut out);
        axis(&mut r, AbsoluteAxisCode::ABS_HAT0X, -1, &mut out);
        r.event(key(KeyCode::BTN_C, Press::Down), Instant::now(), &mut out);
        r.event(Raw::REPORT, Instant::now(), &mut out);
        assert_eq!(held(&out.pad), vec![KeyCode::BTN_SOUTH, KeyCode::BTN_C], "an unknown button passes as it is");
        assert_eq!((last(&out.pad, AbsoluteAxisCode::ABS_X), last(&out.pad, AbsoluteAxisCode::ABS_RZ)), (Some(12345), Some(77)));
        assert_eq!(last(&out.pad, AbsoluteAxisCode::ABS_HAT0X), Some(-1));
        assert!(out.mouse.is_empty());
    }

    fn back_layer(k: &mut Keymap, input: Input, b: Binding) {
        assert!(k.set_in(clamshell_core::keymap::Layer::Back, input, Some(b)));
    }

    #[test]
    fn the_back_layer_while_back_is_held_and_nothing_stuck_letting_go_mid_press() {
        let mut k = keymap(&[(Input::Button(Button::A), to(Output::Key(chord("ctrl+x")))), (Input::Button(Button::B), to(Output::Key(chord("z"))))]);
        back_layer(&mut k, Input::Button(Button::A), to(Output::Key(chord("ctrl+y"))));
        let (mut r, mut out) = router(k, PadRoute::Game);
        press(&mut r, KeyCode::BTN_SOUTH, true, &mut out);
        assert_eq!(held(&out.mouse), vec![KeyCode::KEY_LEFTCTRL, KeyCode::KEY_X], "main");
        press(&mut r, KeyCode::BTN_SOUTH, false, &mut out);
        press(&mut r, KeyCode::KEY_BACK, true, &mut out);
        assert!(out.pad.is_empty() && held(&out.mouse).is_empty(), "Back alone sends nothing: {out:?}");
        press(&mut r, KeyCode::BTN_SOUTH, true, &mut out);
        press(&mut r, KeyCode::BTN_EAST, true, &mut out);
        assert_eq!(held(&out.mouse), vec![KeyCode::KEY_Y, KeyCode::KEY_LEFTCTRL, KeyCode::KEY_Z], "the Back layer's, B falling through");
        // Back let go while A is held: Ctrl+Y up, A is main's Ctrl+X now.
        press(&mut r, KeyCode::KEY_BACK, false, &mut out);
        assert_eq!(held(&out.mouse), vec![KeyCode::KEY_LEFTCTRL, KeyCode::KEY_Z, KeyCode::KEY_X]);
        press(&mut r, KeyCode::BTN_SOUTH, false, &mut out);
        press(&mut r, KeyCode::BTN_EAST, false, &mut out);
        assert!(held(&out.mouse).is_empty(), "nothing stuck");
        assert!(!out.pad.iter().any(|e| e.as_key().is_some_and(|(k, _)| k == KeyCode::KEY_BACK)), "the game never sees Back");
        // Back held when the pad leaves the game: everything let go.
        press(&mut r, KeyCode::KEY_BACK, true, &mut out);
        press(&mut r, KeyCode::BTN_SOUTH, true, &mut out);
        r.set_route(PadRoute::Ui, &mut out);
        assert!(held(&out.mouse).is_empty() && held(&out.pad).is_empty());
    }

    #[test]
    fn the_gyro_differs_by_layer() {
        let mut k = keymap(&[]);
        back_layer(&mut k, Input::Gyro, to(Output::Pointer));
        let (mut r, mut out) = router(k, PadRoute::Game);
        for i in 0..=50 {
            gyro(&mut r, i * 4, -20.0, &mut out);
        }
        assert_eq!(sum(&out.mouse, RelativeAxisCode::REL_X), 0, "off in the main layer");
        press(&mut r, KeyCode::KEY_BACK, true, &mut out);
        for i in 51..=300 {
            gyro(&mut r, i * 4, -20.0, &mut out);
        }
        assert!(sum(&out.mouse, RelativeAxisCode::REL_X) > 250, "{}", sum(&out.mouse, RelativeAxisCode::REL_X));
    }

    #[test]
    fn speeds_between_the_stock_steps() {
        assert_eq!(step(&STICK_POINTER, 50), 1400.0);
        assert_eq!(step(&STICK_POINTER, 100), 3000.0);
        assert_eq!(step(&STICK_POINTER, 0), 500.0);
        assert!((step(&STICK_POINTER, 60) - 1680.0).abs() < 0.01, "two fifths from 1400 to 2100");
    }

    #[test]
    fn a_stick_as_on_the_pad_tuned() {
        let own = to(Output::Stick(Side::Left));
        let k = keymap(&[(Input::Stick(Side::Left), Binding { deadzone: 20, full: 80, invert_x: true, ..own })]);
        let (mut r, mut out) = router(k, PadRoute::Game);
        axis(&mut r, AbsoluteAxisCode::ABS_X, STICK_MAX / 10, &mut out);
        assert_eq!(last(&out.pad, AbsoluteAxisCode::ABS_X), Some(0), "inside the deadzone");
        axis(&mut r, AbsoluteAxisCode::ABS_X, STICK_MAX * 8 / 10, &mut out);
        assert!(last(&out.pad, AbsoluteAxisCode::ABS_X).unwrap() <= -STICK_MAX + 2, "full at 80 %, and inverted: {:?}", last(&out.pad, AbsoluteAxisCode::ABS_X));
        axis(&mut r, AbsoluteAxisCode::ABS_X, STICK_MAX / 2, &mut out);
        let half = last(&out.pad, AbsoluteAxisCode::ABS_X).unwrap();
        assert!((half + STICK_MAX / 2).abs() < 400, "(50-20)/(80-20): half way: {half}");
        // The right stick untouched: its exact values.
        axis(&mut r, AbsoluteAxisCode::ABS_RX, 1234, &mut out);
        assert_eq!(last(&out.pad, AbsoluteAxisCode::ABS_RX), Some(1234));
    }

    #[test]
    fn a_stick_curve_and_tuning_before_mapping() {
        let k = keymap(&[(Input::Stick(Side::Left), Binding { curve: 100, ..to(Output::Stick(Side::Left)) })]);
        let (mut r, mut out) = router(k, PadRoute::Game);
        axis(&mut r, AbsoluteAxisCode::ABS_X, STICK_MAX / 2, &mut out);
        let v = last(&out.pad, AbsoluteAxisCode::ABS_X).unwrap();
        assert!((v - STICK_MAX / 8).abs() < 200, "cubed: an eighth: {v}");
        // A stick on keys with a deadzone of 50: its directions count past it.
        let k = keymap(&[(Input::Stick(Side::Left), Binding { deadzone: 50, ..to(Output::Keys([chord("w"), chord("a"), chord("s"), chord("d")])) })]);
        let (mut r, mut out) = router(k, PadRoute::Game);
        axis(&mut r, AbsoluteAxisCode::ABS_X, STICK_MAX * 7 / 10, &mut out);
        assert!(held(&out.mouse).is_empty(), "70 % is under half once past the deadzone");
        axis(&mut r, AbsoluteAxisCode::ABS_X, STICK_MAX * 8 / 10, &mut out);
        assert_eq!(held(&out.mouse), vec![KeyCode::KEY_D]);
    }

    #[test]
    fn a_trigger_tuned_as_on_the_pad_and_on_a_key() {
        let k = keymap(&[
            (Input::Trigger(Side::Left), Binding { deadzone: 10, full: 60, ..to(Output::Trigger(Side::Left)) }),
            (Input::Trigger(Side::Right), Binding { full: 50, threshold: 90, ..to(Output::Key(chord("x"))) }),
        ]);
        let (mut r, mut out) = router(k, PadRoute::Game);
        axis(&mut r, AbsoluteAxisCode::ABS_Z, 80, &mut out);
        assert_eq!(last(&out.pad, AbsoluteAxisCode::ABS_Z), Some(0), "inside its deadzone");
        axis(&mut r, AbsoluteAxisCode::ABS_Z, 700, &mut out);
        assert_eq!(last(&out.pad, AbsoluteAxisCode::ABS_Z), Some(1023), "past its full point");
        axis(&mut r, AbsoluteAxisCode::ABS_RZ, 470, &mut out);
        assert_eq!(held(&out.mouse), vec![KeyCode::KEY_X], "46 % is 92 % of the way to full at 50 %");
    }

    #[test]
    fn a_stick_on_the_pointer_moves_as_before_by_default_and_without_a_deadzone_at_0() {
        // How the pointer moved before the parameters: a radial deadzone of 15 %, then squared.
        let old = |v: f32| ((v - 0.15) / 0.85).max(0.0).powi(2) * step(&STICK_POINTER, DEFAULT_SPEED_FOR_TEST);
        let moved = |b: Binding, deflection: f32| {
            let (mut r, mut out) = router(keymap(&[(Input::Stick(Side::Right), b)]), PadRoute::Game);
            axis(&mut r, AbsoluteAxisCode::ABS_RX, (deflection * STICK_MAX as f32) as i32, &mut out);
            let mut t = Instant::now();
            r.tick(t, &mut out);
            for _ in 0..125 {
                t += POINTER_EVERY;
                r.tick(t, &mut out);
            }
            sum(&out.mouse, RelativeAxisCode::REL_X) as f32
        };
        let default = Binding::of(Input::Stick(Side::Right), Output::Pointer);
        assert_eq!((default.deadzone, default.curve), (15, 50), "the panel shows what applies");
        for v in [0.1, 0.3, 0.5, 0.8, 1.0] {
            let (now, before) = (moved(default, v), old(v));
            assert!((now - before).abs() <= before * 0.02 + 2.0, "{v}: {now} px, before {before} px");
        }
        let plain = Binding { deadzone: 0, curve: 0, ..default };
        let slight = moved(plain, 0.1);
        assert!((slight - 140.0).abs() < 5.0, "no deadzone, linear: a tenth of the speed: {slight}");
    }

    const DEFAULT_SPEED_FOR_TEST: u8 = clamshell_core::keymap::DEFAULT_SPEED;

    /// A button down or up at `t`.
    fn press_at(r: &mut PadRouter, code: KeyCode, down: bool, t: Instant, out: &mut Out) {
        r.event(key(code, if down { Press::Down } else { Press::Up }), t, out);
        r.event(Raw::REPORT, t, out);
    }

    fn ms(n: u64) -> Duration {
        Duration::from_millis(n)
    }

    /// A on X when pressed, plus `act`'s binding.
    fn with(act: Actuation, b: Binding) -> Keymap {
        let mut k = keymap(&[(Input::Button(Button::A), to(Output::Key(chord("x"))))]);
        assert!(k.set_of(clamshell_core::keymap::Layer::Main, Input::Button(Button::A), act, Some(b)));
        k
    }

    #[test]
    fn a_plain_press_acts_at_once_with_no_timers() {
        let (mut r, mut out) = router(keymap(&[(Input::Button(Button::A), to(Output::Key(chord("x"))))]), PadRoute::Game);
        let t0 = Instant::now();
        press_at(&mut r, KeyCode::BTN_SOUTH, true, t0, &mut out);
        assert_eq!(held(&out.mouse), vec![KeyCode::KEY_X], "on the press itself");
        assert_eq!(r.tick(t0, &mut out), None, "nothing waits for time");
    }

    #[test]
    fn a_hold_acts_at_its_time_and_a_short_press_taps_on_letting_go() {
        let k = with(Actuation::Hold, Binding { hold_time: 40, ..to(Output::Key(chord("esc"))) });
        let (mut r, mut out) = router(k, PadRoute::Game);
        let t0 = Instant::now();
        press_at(&mut r, KeyCode::BTN_SOUTH, true, t0, &mut out);
        assert!(held(&out.mouse).is_empty(), "undecided");
        assert_eq!(r.tick(t0, &mut out), Some(t0 + ms(400)), "wakes at the hold's time");
        r.tick(t0 + ms(399), &mut out);
        assert!(held(&out.mouse).is_empty());
        r.tick(t0 + ms(400), &mut out);
        assert_eq!(held(&out.mouse), vec![KeyCode::KEY_ESC], "a hold");
        press_at(&mut r, KeyCode::BTN_SOUTH, false, t0 + ms(700), &mut out);
        assert!(held(&out.mouse).is_empty() && !out.mouse.iter().any(|e| e.as_key().is_some_and(|(k, _)| k == KeyCode::KEY_X)), "never X");
        // Short: X tapped on letting go, for a moment.
        let t1 = t0 + ms(2000);
        press_at(&mut r, KeyCode::BTN_SOUTH, true, t1, &mut out);
        press_at(&mut r, KeyCode::BTN_SOUTH, false, t1 + ms(100), &mut out);
        assert_eq!(held(&out.mouse), vec![KeyCode::KEY_X], "the press, on letting go");
        assert_eq!(r.tick(t1 + ms(100), &mut out), Some(t1 + ms(100) + TAP));
        r.tick(t1 + ms(100) + TAP, &mut out);
        assert!(held(&out.mouse).is_empty(), "a tap");
        assert_eq!(r.tick(t1 + ms(200), &mut out), None);
    }

    #[test]
    fn a_double_and_a_single_press_with_a_double() {
        let k = with(Actuation::Double, to(Output::Key(chord("tab"))));
        let (mut r, mut out) = router(k, PadRoute::Game);
        let t0 = Instant::now();
        press_at(&mut r, KeyCode::BTN_SOUTH, true, t0, &mut out);
        press_at(&mut r, KeyCode::BTN_SOUTH, false, t0 + ms(50), &mut out);
        assert!(held(&out.mouse).is_empty(), "waiting for a second press");
        press_at(&mut r, KeyCode::BTN_SOUTH, true, t0 + ms(200), &mut out);
        assert_eq!(held(&out.mouse), vec![KeyCode::KEY_TAB], "a double, held while held");
        press_at(&mut r, KeyCode::BTN_SOUTH, false, t0 + ms(400), &mut out);
        assert!(held(&out.mouse).is_empty());
        // Once: X after the window, as a tap.
        let t1 = t0 + ms(2000);
        press_at(&mut r, KeyCode::BTN_SOUTH, true, t1, &mut out);
        press_at(&mut r, KeyCode::BTN_SOUTH, false, t1 + ms(50), &mut out);
        r.tick(t1 + ms(50) + DOUBLE_WINDOW, &mut out);
        assert_eq!(held(&out.mouse), vec![KeyCode::KEY_X]);
        r.tick(t1 + ms(50) + DOUBLE_WINDOW + TAP, &mut out);
        assert!(held(&out.mouse).is_empty());
        // Held past the window (no hold binding): a press, held.
        let t2 = t1 + ms(2000);
        press_at(&mut r, KeyCode::BTN_SOUTH, true, t2, &mut out);
        r.tick(t2 + DOUBLE_WINDOW, &mut out);
        assert_eq!(held(&out.mouse), vec![KeyCode::KEY_X]);
        press_at(&mut r, KeyCode::BTN_SOUTH, false, t2 + ms(900), &mut out);
        assert!(held(&out.mouse).is_empty());
    }

    #[test]
    fn toggle_and_turbo() {
        let k = keymap(&[
            (Input::Button(Button::A), Binding { toggle: true, ..to(Output::Key(chord("x"))) }),
            (Input::Button(Button::B), Binding { turbo: 10, ..to(Output::Key(chord("z"))) }),
        ]);
        let (mut r, mut out) = router(k, PadRoute::Game);
        let t0 = Instant::now();
        press_at(&mut r, KeyCode::BTN_SOUTH, true, t0, &mut out);
        press_at(&mut r, KeyCode::BTN_SOUTH, false, t0 + ms(50), &mut out);
        assert_eq!(held(&out.mouse), vec![KeyCode::KEY_X], "on after one press");
        press_at(&mut r, KeyCode::BTN_SOUTH, true, t0 + ms(500), &mut out);
        assert!(held(&out.mouse).is_empty(), "off at the next press");
        press_at(&mut r, KeyCode::BTN_SOUTH, false, t0 + ms(550), &mut out);
        assert!(held(&out.mouse).is_empty(), "and stays off");
        // Turbo at 10 a second: on 50 ms, off 50 ms, while held.
        let t1 = t0 + ms(1000);
        press_at(&mut r, KeyCode::BTN_EAST, true, t1, &mut out);
        assert_eq!(held(&out.mouse), vec![KeyCode::KEY_Z]);
        assert_eq!(r.tick(t1, &mut out), Some(t1 + ms(50)));
        r.tick(t1 + ms(50), &mut out);
        assert!(held(&out.mouse).is_empty());
        r.tick(t1 + ms(100), &mut out);
        assert_eq!(held(&out.mouse), vec![KeyCode::KEY_Z]);
        press_at(&mut r, KeyCode::BTN_EAST, false, t1 + ms(120), &mut out);
        assert!(held(&out.mouse).is_empty());
        assert_eq!(r.tick(t1 + ms(130), &mut out), None, "let go: no more beats");
    }

    #[test]
    fn a_parameter_slid_applies_at_once() {
        let own = to(Output::Stick(Side::Left));
        let (mut r, mut out) = router(Keymap::default(), PadRoute::Game);
        axis(&mut r, AbsoluteAxisCode::ABS_X, STICK_MAX / 2, &mut out);
        assert_eq!(last(&out.pad, AbsoluteAxisCode::ABS_X), Some(STICK_MAX / 2));
        // The slider sends the keymap as it moves (see core's keymap_ui): the stick held still follows.
        for dz in [10, 30, 50] {
            r.set_keymap(keymap(&[(Input::Stick(Side::Left), Binding { deadzone: dz, ..own })]), &mut out);
        }
        assert_eq!(last(&out.pad, AbsoluteAxisCode::ABS_X), Some(0), "half way is inside a deadzone of 50");
    }
}
