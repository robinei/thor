//! The keymap in force, applied to the controller on the way to the game (the Game route only). Rather
//! than rewriting events one by one, the controller's state (the router's snapshot, and the gyro's rate)
//! is mapped to what every output should be now: the game's pad (buttons, D-pad, sticks, triggers), the
//! keys and mouse buttons held on the virtual keyboard and mouse, wheel steps and pointer motion. Only
//! what changed since the last time is sent. So letting go is the same computation with nothing held:
//! leaving the game, losing the controller or changing the keymap mid-press never leaves anything down.
//! Chords press their modifiers before their key and let them go after it.
//!
//! Without a binding an input does what it does on the pad; the controller's events this doesn't know
//! (other buttons and axes) pass through as they are. A binding to a layer of the keymap turns it on
//! while it is on (held, or toggled): the bindings come from the layers on, the newest first, then Base;
//! the binding that turned a layer on stays as it was until it goes off, whatever the layer makes of its
//! input ([`Activation`]). A Once layer turned on by a tap stays armed until the first input it maps has
//! acted (a toggled one too, its toggle going off with it); its binding pressed again turns it off
//! ([`Span`]). A hold to a layer is decided at once when an input that layer maps is pressed, so a quick
//! chord acts in the layer. A layer that stays on (armed, toggled) buzzes the controller as it comes on
//! and as it goes off ([`Buzz`]). The Thor's Back button ([`BACK_KEY`]) is an input like the others (nothing
//! in the game as on the pad). A stick's and a trigger's tuning (deadzone, full point, curve,
//! inversion) applies to its values before they are mapped, to the input as on the pad as well.
//!
//! A button (or a D-pad or stick direction) with a hold's or a double's binding goes through a little
//! state machine ([`Phase`]): held past the hold's time it is a hold; let go sooner and pressed again
//! within [`DOUBLE_WINDOW`] a double; otherwise a press, sent as a short tap ([`TAP`]) once that is known.
//! Without either it is a press at once, held as long as the button. On top of the actuation, a binding
//! that toggles turns on at one actuation and off at the next, and one with turbo goes on and off at its
//! rate while on. Time moves these on as well as events: [`Remap::tick`] says when.

use std::time::{Duration, Instant};

use clamshell_core::keymap::{Actuation, Binding, Button, Chord, Dir, Input, Keymap, Kind, Layer, MouseButton, Output, PadOut, Side};
use evdev::{AbsoluteAxisCode, KeyCode, RelativeAxisCode};

use super::motion::Rate;
use super::route::{Out, PadSnapshot, Ranges};
use super::rumble::Buzz;
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
    On {
        act: Actuation,
        until: Option<Instant>,
    },
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
    beats: [Beat; 4],
    /// When the phase moves on by itself (a hold's time, a double's window, a tap's end).
    due: Option<Instant>,
}

impl Actuator {
    fn new(input: Input) -> Actuator {
        Actuator { input, phase: Phase::Up, beats: [Beat::default(); 4], due: None }
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

/// The Thor's Back button, under the screen (KEY_BACK is assumed until the device test,
/// docs/thor-gameplan.md): an input of keymaps, never on the game's pad itself.
pub const BACK_KEY: KeyCode = KeyCode::KEY_BACK;

/// Whether a code is the Back button's.
pub fn is_back_key(code: KeyCode) -> bool {
    code == BACK_KEY
}

/// A binding to a layer that is on: whose (an input's actuation), the binding as it was when it came on
/// (kept while on, whatever the layers since make of that input), how it is on, and the first input the
/// layer maps that acted while it was (a Once layer's end).
#[derive(Debug, Clone, Copy, PartialEq)]
struct Activation {
    input: Input,
    act: Actuation,
    binding: Binding,
    span: Span,
    used_by: Option<Input>,
}

/// How a layer is on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Span {
    /// Its binding is on (held, or toggled).
    Held,
    /// A Once layer whose binding went off before it was used: on until the first input it maps acts.
    Armed,
    /// An armed layer whose binding was pressed again: off, the binding's own until it goes off.
    Cancelled,
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
        Button::Back => &[BACK_KEY],
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
        return button_of(code).is_some() || Dir::ALL.into_iter().any(|d| dpad_code(d) == code);
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
    /// The layers on, in the order they came on.
    activations: Vec<Activation>,
    /// The digital inputs' actuations (those that have been pressed).
    actuators: Vec<Actuator>,
    /// A buzz due (a layer that stays on came on or went off), for the next update to send.
    buzz: Option<Buzz>,
}

impl Remap {
    pub fn new(ranges: Ranges) -> Remap {
        Remap {
            keymap: Keymap::default(),
            ranges,
            sent: Wanted::default(),
            gyro: Rate::default(),
            carry: (0.0, 0.0),
            last_pointer: None,
            activations: Vec::new(),
            actuators: Vec::new(),
            buzz: None,
        }
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
        self.activations.clear();
        self.buzz = None;
    }

    fn axis_rest(&self, axis: AbsoluteAxisCode) -> i32 {
        if Side::ALL.into_iter().any(|s| trigger_axis(s) == axis) { self.ranges.trigger.min } else { 0 }
    }

    /// Takes the outputs to be what the keymap makes of `snap` already (without sending anything).
    pub fn prime(&mut self, snap: &PadSnapshot) {
        let now = Instant::now();
        self.actuate(snap, now);
        self.sent = self.wanted(snap, now);
        self.buzz = None;
    }

    /// The controller's state changed (`snap` is it now, at `now`): the outputs follow.
    pub fn update(&mut self, snap: &PadSnapshot, now: Instant, out: &mut Out) {
        self.actuate(snap, now);
        let wanted = self.wanted(snap, now);
        self.send(wanted, out);
        if let Some(buzz) = self.buzz.take() {
            out.buzz = Some(buzz);
        }
    }

    /// The digital inputs' actuations follow the controller at `now`.
    fn actuate(&mut self, snap: &PadSnapshot, now: Instant) {
        self.decide_layer_holds(snap, now);
        // What each input is doing now (read first; the actuators follow below).
        let mut states = Vec::new();
        for input in Input::all() {
            let digital_trigger = self.binding(input, snap).is_some_and(|b| b.output.digital());
            let full_pull = self.find(input, Actuation::FullPull).copied();
            if !self.watched(input, snap) {
                continue;
            }
            // A trigger's full pull: on past its own threshold, beside the pull.
            let full = match (input, full_pull) {
                (Input::Trigger(s), Some(b)) => self.trigger(s, snap) * 100.0 >= f32::from(b.threshold),
                _ => false,
            };
            let on = (input.kind() == Kind::Digital || digital_trigger) && self.pressed(input, snap);
            let hold = self.find(input, Actuation::Hold).map(|b| Duration::from_millis(u64::from(b.hold_time) * 10));
            let double = self.find(input, Actuation::Double).is_some();
            let binding = |act| self.binding_of(input, act, snap);
            let toggles = Actuation::ALL.map(|a| binding(a).is_some_and(|b| b.toggle));
            states.push((input, on, full, hold, double, toggles));
        }
        // The actuations that came on now (a Once layer's use).
        let mut risen = Vec::new();
        for (input, on, full, hold, double, toggles) in states {
            let found = self.actuators.iter().position(|a| a.input == input);
            if found.is_none() && !on && !full {
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
                let raw = if act == Actuation::FullPull { full } else { a.actuated(act) };
                if let Some(beat) = a.beats.get_mut(act as usize) {
                    if raw && !beat.raw {
                        risen.push((input, act));
                    }
                    beat.follow(raw, toggle, now);
                }
            }
        }
        // (Those at rest and holding nothing on are dropped.)
        self.actuators.retain(|a| a.phase != Phase::Up || a.beats.iter().any(|b| b.raw || b.latched));
        self.activate(snap, now, &risen);
    }

    /// Whether an input's actuations are followed: a digital input, or a trigger mapped to something on
    /// or off (or with a full pull).
    fn watched(&self, input: Input, snap: &PadSnapshot) -> bool {
        match input.kind() {
            Kind::Digital => true,
            Kind::Trigger => self.binding(input, snap).is_some_and(|b| b.output.digital()) || self.find(input, Actuation::FullPull).is_some(),
            Kind::Stick | Kind::Gyro => false,
        }
    }

    /// A hold to a layer still deciding is decided now if an input that layer maps is pressed: a quick
    /// chord acts in the layer, as it would have once the hold's time was up.
    fn decide_layer_holds(&mut self, snap: &PadSnapshot, now: Instant) {
        let mut decided = Vec::new();
        for a in &self.actuators {
            if !matches!(a.phase, Phase::Deciding(_)) {
                continue;
            }
            let Some(b) = self.find(a.input, Actuation::Hold).filter(|b| matches!(b.output, Output::Layer(_))) else {
                continue;
            };
            let Output::Layer(layer) = b.output else {
                continue;
            };
            let new_press = |i: Input| {
                i != a.input
                    && Actuation::ALL.into_iter().any(|act| self.keymap.get_of(layer, i, act).is_some())
                    && self.watched(i, snap)
                    && self.pressed(i, snap)
                    && !self.actuators.iter().any(|x| x.input == i && x.phase != Phase::Up)
            };
            if Input::all().any(new_press) {
                decided.push((a.input, b.toggle));
            }
        }
        if decided.is_empty() {
            return;
        }
        for (input, toggle) in decided {
            if let Some(a) = self.actuators.iter_mut().find(|a| a.input == input) {
                a.phase = Phase::On { act: Actuation::Hold, until: None };
                a.due = None;
                if let Some(beat) = a.beats.get_mut(Actuation::Hold as usize) {
                    beat.follow(true, toggle, now);
                }
            }
        }
        self.activate(snap, now, &[]);
    }

    /// Layers on and off. A binding to one turns it on while its actuation (or its toggle) is; a Once
    /// layer turned on by a tap (its binding off before it was used) stays armed until the first input
    /// it maps has acted and let go, as does a toggled one (and its toggle goes off with it); its binding
    /// pressed again while armed turns it off. `risen`: the actuations that came on now.
    fn activate(&mut self, snap: &PadSnapshot, now: Instant, risen: &[(Input, Actuation)]) {
        // An input a Once layer maps acted: the newest such layer on is used.
        for &(input, act) in risen {
            if self.activations.iter().any(|x| x.input == input) {
                continue;
            }
            let keymap = &self.keymap;
            let maps = |x: &Activation| match x.binding.output {
                Output::Layer(l) => x.span != Span::Cancelled && keymap.get_of(l, input, act).is_some(),
                _ => false,
            };
            if let Some(x) = self.activations.iter_mut().rev().find(|x| maps(x))
                && x.used_by.is_none()
            {
                x.used_by = Some(input);
            }
        }
        // The bindings to layers, on or off now.
        let mut seen = Vec::new();
        for a in &self.actuators {
            for act in Actuation::ALL {
                let Some(b) = self.binding_of(a.input, act, snap).filter(|b| matches!(b.output, Output::Layer(_))) else {
                    continue;
                };
                seen.push((a.input, act, b, a.beat(act).on(&b, now)));
            }
        }
        // (Those whose input is at rest, its actuator gone: off.)
        for x in &self.activations {
            if !self.actuators.iter().any(|a| a.input == x.input) {
                seen.push((x.input, x.act, x.binding, false));
            }
        }
        for (input, act, b, on) in seen {
            let i = self.activations.iter().position(|x| (x.input, x.act) == (input, act));
            let once = |x: &Activation| matches!(x.binding.output, Output::Layer(l) if self.keymap.layer_once(l));
            match (i.and_then(|i| self.activations.get(i)).copied(), on) {
                (None, true) => {
                    self.activations.push(Activation { input, act, binding: b, span: Span::Held, used_by: None });
                    if b.toggle {
                        self.buzz = Some(Buzz::On);
                    }
                }
                (Some(x), false) if x.span == Span::Held => {
                    if once(&x) && x.used_by.is_none() && !x.binding.toggle {
                        self.set_span(input, act, Span::Armed);
                        self.buzz = Some(Buzz::On);
                    } else {
                        self.activations.retain(|y| (y.input, y.act) != (input, act));
                        if x.binding.toggle {
                            self.buzz = Some(Buzz::Off);
                        }
                    }
                }
                (Some(x), true) if x.span == Span::Armed => {
                    self.set_span(input, act, Span::Cancelled);
                    self.buzz = Some(Buzz::Off);
                }
                (Some(x), false) if x.span == Span::Cancelled => self.activations.retain(|y| (y.input, y.act) != (input, act)),
                _ => {}
            }
        }
        // A Once layer armed or toggled, used: off once the input that used it has let go (a toggle too).
        let busy = |actuators: &[Actuator], i: Input| actuators.iter().any(|a| a.input == i && (a.phase != Phase::Up || a.beats.iter().any(|b| b.raw)));
        let mut ended = Vec::new();
        for x in &self.activations {
            let once = matches!(x.binding.output, Output::Layer(l) if self.keymap.layer_once(l));
            let waits = x.span == Span::Armed || (x.span == Span::Held && x.binding.toggle);
            if let Some(user) = x.used_by
                && once
                && waits
                && !busy(&self.actuators, user)
            {
                ended.push((x.input, x.act));
            }
        }
        for (input, act) in ended {
            self.activations.retain(|y| (y.input, y.act) != (input, act));
            if let Some(beat) = self.actuators.iter_mut().find(|a| a.input == input).and_then(|a| a.beats.get_mut(act as usize)) {
                beat.latched = false;
            }
            self.buzz = Some(Buzz::Off);
        }
        // (A layer's binding at rest: off, but an armed one.)
        let actuators = &self.actuators;
        self.activations.retain(|x| x.span == Span::Armed || actuators.iter().any(|a| a.input == x.input));
    }

    fn set_span(&mut self, input: Input, act: Actuation, span: Span) {
        if let Some(x) = self.activations.iter_mut().find(|x| (x.input, x.act) == (input, act)) {
            x.span = span;
        }
    }

    /// The layers on, in the order they came on.
    fn layers_on(&self) -> Vec<Layer> {
        self.activations
            .iter()
            .filter(|a| a.span != Span::Cancelled)
            .filter_map(|a| if let Output::Layer(l) = a.binding.output { Some(l) } else { None })
            .collect()
    }

    /// `input`'s binding of `act` in force: the one turning a layer on if it is, else the newest layer's
    /// on that has one, else Base's.
    fn find(&self, input: Input, act: Actuation) -> Option<&Binding> {
        if let Some(a) = self.activations.iter().find(|a| a.input == input && a.act == act) {
            return Some(&a.binding);
        }
        self.keymap.lookup_of(input, act, &self.layers_on())
    }

    /// [`Remap::find`], a press's as on the pad without one.
    fn binding_of(&self, input: Input, act: Actuation, snap: &PadSnapshot) -> Option<Binding> {
        match act {
            Actuation::Press => self.binding(input, snap),
            act => self.find(input, act).copied(),
        }
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
        let mut next = None;
        for a in &self.actuators {
            let mut times = vec![a.due];
            for act in Actuation::ALL {
                let b = self.binding_of(a.input, act, snap);
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

    /// `input`'s binding in the layer in force, if it has one.
    fn get(&self, input: Input, _: &PadSnapshot) -> Option<&Binding> {
        self.find(input, Actuation::Press)
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
                        if let Some(other) = self.find(input, act) {
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
                    // Its full pull, a second stage beside the pull.
                    if let Some(full) = self.find(input, Actuation::FullPull) {
                        let on = self.on(input, Actuation::FullPull, full, now);
                        digital(&mut w, input, on, &full.output, &mut triggers, &mut dpad);
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
mod tests;
