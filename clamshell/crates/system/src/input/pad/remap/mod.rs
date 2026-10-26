//! The keymap in force, applied to the controller on the way to the game, on the Game route only.
//!
//! Rather than rewriting events one by one, the controller's state is mapped to what every output should
//! be now. The state is the router's snapshot and the gyro's rate. The outputs are the game's pad
//! (buttons, D-pad, sticks, triggers), the keys and mouse buttons held on the virtual keyboard and mouse,
//! wheel steps and pointer motion. Only what changed since the last time is sent. Letting go is then the
//! same computation with nothing held, so leaving the game, losing the controller or changing the keymap
//! mid-press never leaves anything down. Chords press their modifiers before their key and let them go
//! after it.
//!
//! An input without a binding does what it does on the pad. Controller events this doesn't know, such as
//! other buttons and axes, pass through as they are.
//!
//! A binding to a layer of the keymap turns the layer on while the binding is on, held or toggled. The
//! bindings come from the layers that are on, newest first, then Base. The binding that turned a layer on
//! stays as it was until it goes off, whatever the layer makes of its input ([`Activation`]). A Once
//! layer turned on by a tap stays armed until the first input it maps has acted. A toggled Once layer
//! works the same way, and its toggle goes off with it. Pressing its binding again turns it off
//! ([`Span`]). A hold to a layer is decided at once when an input that layer maps is pressed, so a quick
//! chord acts in the layer. A layer that stays on, armed or toggled, buzzes the controller as it comes on
//! and as it goes off ([`Buzz`]).
//!
//! The Thor's Back button ([`BACK_KEY`]) is an input like the others and does nothing in the game as on
//! the pad. So is its AYN button ([`AYN_KEY`]), which is Guide in the game as on the pad. A stick's and a
//! trigger's tuning (deadzone, full point, curve, inversion) applies to its values before they are
//! mapped, including when the input is passed on as on the pad.
//!
//! A button, D-pad direction or stick direction with a hold's or a double's binding goes through a small
//! state machine ([`Phase`]). Held past the hold's time, it is a hold. Let go sooner and pressed again
//! within [`DOUBLE_WINDOW`], it is a double. Otherwise it is a press, sent as a short tap ([`TAP`]) once
//! that is known. Without a hold or a double it is a press at once, held as long as the button. On top
//! of the actuation, a binding that toggles turns on at one actuation and off at the next, and one with
//! turbo goes on and off at its rate while on. Time moves these on as well as events, and
//! [`Remap::tick`] says when.

use std::time::{Duration, Instant};

use clamshell_state::keymap::{Actuation, Binding, Chord, Dir, Input, Keymap, Kind, Layer, Output, PadOut, Side};
use evdev::{AbsoluteAxisCode, KeyCode, RelativeAxisCode};

use super::motion::Rate;
use super::route::{Out, PadSnapshot, Ranges};
use super::rumble::Buzz;
use crate::input::keyboard::{MODIFIERS, chord_keys};
use crate::os::evdev::{Press, Raw};

mod actuate;
mod analog;

pub(crate) use super::codes::*;
use actuate::*;
use analog::*;

/// How often a stick on the pointer moves it.
const POINTER_EVERY: Duration = Duration::from_millis(8);
/// High-resolution wheel units per notch, by the kernel's convention.
const NOTCH: i32 = 120;
/// A binding to a layer that is on. It holds the input and actuation it belongs to, and the binding as it
/// was when it came on. That binding is kept while on, whatever the layers since make of the input. It
/// also holds how the layer is on, and the first input the layer maps that acted while it was on, which
/// ends a Once layer.
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
    /// Its binding is on, held or toggled.
    Held,
    /// A Once layer whose binding went off before it was used. It is on until the first input it maps
    /// acts.
    Armed,
    /// An armed layer whose binding was pressed again. The layer is off, and the binding's press is its
    /// own until it goes off.
    Cancelled,
}

/// What an input is doing now, read before its actuator follows. That is whether it is pressed (`on`),
/// whether it is pulled past its full pull's threshold (`full`), its hold's time if it has a hold's
/// binding, whether it has a double's binding, and which of its actuations' bindings toggle.
struct InputState {
    input: Input,
    on: bool,
    full: bool,
    hold: Option<Duration>,
    double: bool,
    toggles: [bool; Actuation::ALL.len()],
}

/// What the bindings make of the inputs, gathered before the pad's axes are worked out. That is the
/// outputs other than the pad's axes, and the pad's sticks as offsets from the middle, its triggers as
/// 0-1 and its D-pad as up, down, left and right.
#[derive(Default)]
struct Gathered {
    w: Wanted,
    sticks: [(f32, f32); 2],
    triggers: [f32; 2],
    dpad: [bool; 4],
}

impl Gathered {
    /// Adds `input`'s binding to `output`, an output that is on or off, while `on`.
    fn digital(&mut self, input: Input, on: bool, output: &Output, snap: &PadSnapshot) {
        if !on {
            return;
        }
        match output {
            Output::Pad(PadOut::Button(b)) => match (b.is_back(), input) {
                // A back button bound to itself sends the code it came with.
                (true, Input::Button(own)) if own == *b => self.w.pad_keys.extend(button_codes(*b).iter().filter(|&&c| snap.held(c))),
                (true, _) => {}
                (false, _) => self.w.pad_keys.extend(pad_code(*b)),
            },
            Output::Pad(PadOut::Dpad(d)) => {
                if let Some(slot) = self.dpad.get_mut(*d as usize) {
                    *slot = true;
                }
            }
            Output::Pad(PadOut::Trigger(s)) => {
                if let Some(t) = self.triggers.get_mut(*s as usize) {
                    *t = 1.0;
                }
            }
            Output::Key(chord) => chord_down(&mut self.w.keys, chord),
            Output::Mouse(m) => self.w.keys.push(mouse_code(*m)),
            Output::Wheel(d) => self.w.wheels.push((input, *d)),
            _ => {}
        }
    }
}

/// What every output should be now. The default is the pad at rest: nothing pressed, nothing moved.
#[derive(Debug, Clone, Default, PartialEq)]
struct Wanted {
    /// The game's pad: the buttons down, and every axis' value.
    pad_keys: Vec<KeyCode>,
    pad_axes: Vec<(AbsoluteAxisCode, i32)>,
    /// Keys and mouse buttons down on the virtual keyboard and mouse.
    keys: Vec<KeyCode>,
    /// The inputs whose wheel step is due. Each press steps once.
    wheels: Vec<(Input, Dir)>,
    /// Sticks moving the pointer, with their deflection and speed.
    pointer: Vec<((f32, f32), u8)>,
}

/// The remapping, and what it last sent.
pub(crate) struct Remap {
    keymap: Keymap,
    ranges: Ranges,
    sent: Wanted,
    /// The gyro's current rate in degrees per second, and the sub-pixel pointer motion not yet sent.
    gyro: Rate,
    carry: (f32, f32),
    /// When a stick last moved the pointer.
    last_pointer: Option<Instant>,
    /// The layers that are on, in the order they came on.
    activations: Vec<Activation>,
    /// The digital inputs' actuations, for those that have been pressed.
    actuators: Vec<Actuator>,
    /// A buzz due because a layer that stays on came on or went off. The next update sends it.
    buzz: Option<Buzz>,
}

impl Remap {
    pub(crate) fn new(ranges: Ranges) -> Remap {
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

    /// Puts a new keymap in force. What it makes of the controller now replaces what the old one held.
    pub(crate) fn set_keymap(&mut self, keymap: Keymap, snap: &PadSnapshot, live: bool, out: &mut Out) {
        self.keymap = keymap;
        if live {
            self.update(snap, Instant::now(), out);
        }
    }

    /// Lets go of everything sent, on leaving the game or when the controller is gone. The keyboard's and
    /// mouse's keys go up, and the pad's buttons go up and its axes to rest.
    pub(crate) fn release(&mut self, out: &mut Out) {
        let rest = Wanted { pad_axes: self.sent.pad_axes.iter().map(|&(a, _)| (a, self.axis_rest(a))).collect(), ..Wanted::default() };
        self.send(rest, out);
        self.sent = Wanted::default();
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

    /// Takes the outputs to be what the keymap makes of `snap` already, without sending anything.
    pub(crate) fn prime(&mut self, snap: &PadSnapshot) {
        let now = Instant::now();
        self.actuate(snap, now);
        self.sent = self.wanted(snap, now);
        self.buzz = None;
    }

    /// The controller's state changed to `snap` at `now`, and the outputs follow.
    pub(crate) fn update(&mut self, snap: &PadSnapshot, now: Instant, out: &mut Out) {
        self.actuate(snap, now);
        let wanted = self.wanted(snap, now);
        self.send(wanted, out);
        if let Some(buzz) = self.buzz.take() {
            out.buzz = Some(buzz);
        }
    }

    /// Makes the digital inputs' actuations follow the controller at `now`.
    fn actuate(&mut self, snap: &PadSnapshot, now: Instant) {
        self.decide_layer_holds(snap, now);
        // Read what each input is doing now first. The actuators follow below.
        let mut states = Vec::new();
        for input in Input::all() {
            let digital_trigger = self.binding(input).is_some_and(|b| b.output.digital());
            let full_pull = self.find(input, Actuation::FullPull).copied();
            if !self.watched(input) {
                continue;
            }
            // A trigger's full pull is on past its own threshold, alongside the pull.
            let full = match (input, full_pull) {
                (Input::Trigger(s), Some(b)) => self.trigger(s, snap) * 100.0 >= f32::from(b.threshold),
                _ => false,
            };
            let on = (input.kind() == Kind::Digital || digital_trigger) && self.pressed(input, snap);
            let hold = self.find(input, Actuation::Hold).map(|b| Duration::from_millis(u64::from(b.hold_time) * 10));
            let double = self.find(input, Actuation::Double).is_some();
            let binding = |act| self.binding_of(input, act);
            let toggles = Actuation::ALL.map(|a| binding(a).is_some_and(|b| b.toggle));
            states.push(InputState { input, on, full, hold, double, toggles });
        }
        // The actuations that came on now, which may use a Once layer.
        let mut risen = Vec::new();
        for InputState { input, on, full, hold, double, toggles } in states {
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
        // Actuators at rest that hold nothing on are dropped.
        self.actuators.retain(|a| a.phase != Phase::Up || a.beats.iter().any(|b| b.raw || b.latched));
        self.activate(now, &risen);
    }

    /// Whether an input's actuations are followed. They are for a digital input, and for a trigger mapped
    /// to something on or off or with a full pull.
    fn watched(&self, input: Input) -> bool {
        match input.kind() {
            Kind::Digital => true,
            Kind::Trigger => self.binding(input).is_some_and(|b| b.output.digital()) || self.find(input, Actuation::FullPull).is_some(),
            Kind::Stick | Kind::Gyro => false,
        }
    }

    /// A hold to a layer that is still deciding is decided now if an input that layer maps is pressed. So a
    /// quick chord acts in the layer, as it would have once the hold's time was up.
    fn decide_layer_holds(&mut self, snap: &PadSnapshot, now: Instant) {
        let mut decided = Vec::new();
        for a in &self.actuators {
            if !matches!(a.phase, Phase::Deciding(_)) {
                continue;
            }
            let Some((b, layer)) = self.find(a.input, Actuation::Hold).and_then(|b| match b.output {
                Output::Layer(layer) => Some((b, layer)),
                _ => None,
            }) else {
                continue;
            };
            let new_press = |i: Input| {
                i != a.input
                    && Actuation::ALL.into_iter().any(|act| self.keymap.get_of(layer, i, act).is_some())
                    && self.watched(i)
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
        self.activate(now, &[]);
    }

    /// Turns layers on and off. A binding to a layer turns it on while its actuation, or its toggle, is
    /// on. A Once layer turned on by a tap, meaning its binding went off before it was used, stays armed
    /// until the first input it maps has acted and been let go. A toggled Once layer does the same, and
    /// its toggle goes off with it. Pressing its binding again while armed turns it off. `risen` holds the
    /// actuations that came on now.
    fn activate(&mut self, now: Instant, risen: &[(Input, Actuation)]) {
        // When an input that a Once layer maps acts, the newest such layer that is on is used.
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
        // The bindings to layers, and whether each is on now.
        let mut seen = Vec::new();
        for a in &self.actuators {
            for act in Actuation::ALL {
                let Some(b) = self.binding_of(a.input, act).filter(|b| matches!(b.output, Output::Layer(_))) else {
                    continue;
                };
                seen.push((a.input, act, b, a.beat(act).on(&b, now)));
            }
        }
        // A binding whose input is at rest, with its actuator gone, is off.
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
        // A Once layer that is armed or toggled and has been used goes off once the input that used it has
        // let go. A toggle goes off too.
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
        // A layer whose binding is at rest goes off, unless it is armed.
        let actuators = &self.actuators;
        self.activations.retain(|x| x.span == Span::Armed || actuators.iter().any(|a| a.input == x.input));
    }

    fn set_span(&mut self, input: Input, act: Actuation, span: Span) {
        if let Some(x) = self.activations.iter_mut().find(|x| (x.input, x.act) == (input, act)) {
            x.span = span;
        }
    }

    /// The layers that are on, in the order they came on.
    fn layers_on(&self) -> Vec<Layer> {
        self.activations
            .iter()
            .filter(|a| a.span != Span::Cancelled)
            .filter_map(|a| if let Output::Layer(l) = a.binding.output { Some(l) } else { None })
            .collect()
    }

    /// `input`'s binding for `act` in force. If the input is turning a layer on, that binding. Otherwise
    /// the binding from the newest layer that is on and has one, or else Base's.
    fn find(&self, input: Input, act: Actuation) -> Option<&Binding> {
        if let Some(a) = self.activations.iter().find(|a| a.input == input && a.act == act) {
            return Some(&a.binding);
        }
        self.keymap.lookup_of(input, act, &self.layers_on())
    }

    /// Like [`Remap::find`], but a press without a binding acts as on the pad.
    fn binding_of(&self, input: Input, act: Actuation) -> Option<Binding> {
        match act {
            Actuation::Press => self.binding(input),
            act => self.find(input, act).copied(),
        }
    }

    /// Whether a digital input is down. A trigger is down past its press binding's threshold, and the
    /// rest are as [`Remap::digital`] says.
    fn pressed(&self, input: Input, snap: &PadSnapshot) -> bool {
        match (input, self.binding(input)) {
            (Input::Trigger(s), Some(b)) => {
                let v = self.trigger(s, snap);
                v * 100.0 >= f32::from(b.threshold) && v > 0.0
            }
            _ => self.digital(input, snap),
        }
    }

    /// Takes the gyro's current rate, `dt` after its last sample. The pointer moves by it, or a stick
    /// follows it.
    pub(crate) fn motion(&mut self, rate: Rate, dt: f32, snap: &PadSnapshot, out: &mut Out) {
        self.gyro = rate;
        if let Some(b) = self.gyro_binding(snap)
            && b.output == Output::Pointer
        {
            let px = step(&GYRO_POINTER, b.speed);
            let (x, y) = gyro_turn(&b, rate);
            let gain = gyro_gain(&b, (x, y));
            // Turning left (yaw) moves the pointer left, and tilting the top back (pitch) moves it up.
            self.carry.0 -= x * gain * dt * px;
            self.carry.1 -= y * gain * dt * px;
            self.flush_pointer(out);
        }
        self.update(snap, Instant::now(), out);
    }

    /// Handles time passing. Actuations move on at a hold's time, a double's window, a tap's end or
    /// turbo's beat, and sticks on the pointer move it. Returns when to tick next, if anything waits for
    /// time.
    pub(crate) fn tick(&mut self, now: Instant, snap: &PadSnapshot, out: &mut Out) -> Option<Instant> {
        if !self.actuators.is_empty() {
            self.update(snap, now, out);
        }
        let timed = self.next_due(now);
        let pointer = self.tick_pointer(now, out);
        [timed, pointer].into_iter().flatten().min()
    }

    /// When an actuation next moves on by itself, or a turbo beats.
    fn next_due(&self, now: Instant) -> Option<Instant> {
        let mut next = None;
        for a in &self.actuators {
            let mut times = vec![a.due];
            for act in Actuation::ALL {
                let b = self.binding_of(a.input, act);
                times.push(b.and_then(|b| a.beat(act).next_beat(&b, now)));
            }
            next = times.into_iter().flatten().chain(next).min();
        }
        next
    }

    /// Moves the pointer for sticks bound to it. Returns when to tick next for that, while one is
    /// deflected.
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

    /// Sends the whole pixels of the pointer's motion and keeps the rest.
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
    fn get(&self, input: Input) -> Option<&Binding> {
        self.find(input, Actuation::Press)
    }

    /// The gyro's binding, if it acts now. Without a hold input it always acts. With one, it acts while
    /// the hold input is held, or for a ratchet while it isn't.
    fn gyro_binding(&self, snap: &PadSnapshot) -> Option<Binding> {
        let b = *self.get(Input::Gyro)?;
        let on = b.hold.is_none_or(|h| self.digital(h, snap) != b.ratchet);
        on.then_some(b)
    }

    /// `input`'s press binding in the layer in force, or else its own output as on the pad.
    fn binding(&self, input: Input) -> Option<Binding> {
        if let Some(b) = self.get(input) {
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

    /// The stick's middle and half its travel.
    fn stick_middle(&self) -> (i32, f32) {
        let r = self.ranges.stick;
        ((r.min + r.max + 1) / 2, ((r.max - r.min) as f32 / 2.0).max(1.0))
    }

    /// A stick's position, each axis -1 to 1 with y down, tuned by its binding.
    fn tuned_stick(&self, s: Side, snap: &PadSnapshot) -> (f32, f32) {
        let (ax, ay) = stick_axes(s);
        let at = |a| self.ranges.stick.centered(snap.value(a).unwrap_or(0));
        let xy = (at(ax), at(ay));
        match self.get(Input::Stick(s)) {
            Some(b) => tune_stick(xy, b),
            None => xy,
        }
    }

    /// A stick in raw units from its middle, with y down, tuned. Directions that have their own binding
    /// are taken out, since they act as buttons instead.
    fn stick(&self, s: Side, snap: &PadSnapshot) -> (f32, f32) {
        let (mut x, mut y) = match self.get(Input::Stick(s)) {
            Some(b) if b.shapes() => {
                let half = self.stick_middle().1;
                let (x, y) = self.tuned_stick(s, snap);
                (x * half, y * half)
            }
            // Not shaped, so in raw units as they are.
            _ => {
                let (ax, ay) = stick_axes(s);
                let mid = self.stick_middle().0;
                let at = |a| snap.value(a).map_or(0.0, |v| (v - mid) as f32);
                (at(ax), at(ay))
            }
        };
        let bound = |d| self.get(Input::StickDir(s, d)).is_some();
        if (x < 0.0 && bound(Dir::Left)) || (x > 0.0 && bound(Dir::Right)) {
            x = 0.0;
        }
        if (y < 0.0 && bound(Dir::Up)) || (y > 0.0 && bound(Dir::Down)) {
            y = 0.0;
        }
        (x, y)
    }

    /// A trigger's pull from 0 to 1, tuned by its binding's deadzone and full point.
    fn trigger(&self, s: Side, snap: &PadSnapshot) -> f32 {
        let v = self.ranges.trigger.unit(snap.value(trigger_axis(s)).unwrap_or(self.ranges.trigger.min));
        match self.get(Input::Trigger(s)) {
            Some(b) if b.deadzone > 0 || b.full < 100 => range(v, b.deadzone, b.full),
            _ => v,
        }
    }

    /// Whether a digital input is on. A trigger is on past half.
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

    /// Whether `input`'s binding for `act` is on at `now`, as its actuation, toggle and turbo make it.
    fn on(&self, input: Input, act: Actuation, b: &Binding, now: Instant) -> bool {
        self.actuators.iter().find(|a| a.input == input).is_some_and(|a| a.beat(act).on(b, now))
    }

    /// What every output should be for the controller in `snap` at `now`.
    fn wanted(&self, snap: &PadSnapshot, now: Instant) -> Wanted {
        let mut g = self.gather(snap, now);
        self.gyro_to_stick(snap, &mut g.sticks);
        self.pad_axes(g, snap)
    }

    /// What the bindings make of each input now.
    fn gather(&self, snap: &PadSnapshot, now: Instant) -> Gathered {
        let mut g = Gathered::default();
        for input in Input::all() {
            let Some(b) = self.binding(input) else {
                continue;
            };
            match input {
                Input::Button(_) | Input::Dpad(_) | Input::StickDir(..) => {
                    g.digital(input, self.on(input, Actuation::Press, &b, now), &b.output, snap);
                    // Its hold and double bindings, if it has them.
                    for act in [Actuation::Hold, Actuation::Double] {
                        if let Some(other) = self.find(input, act) {
                            g.digital(input, self.on(input, act, other, now), &other.output, snap);
                        }
                    }
                }
                Input::Trigger(s) => {
                    let v = self.trigger(s, snap);
                    match b.output {
                        Output::Trigger(to) => {
                            if let Some(t) = g.triggers.get_mut(to as usize) {
                                *t = t.max(v);
                            }
                        }
                        ref o => g.digital(input, self.on(input, Actuation::Press, &b, now), o, snap),
                    }
                    // Its full pull, a second stage alongside the pull.
                    if let Some(full) = self.find(input, Actuation::FullPull) {
                        g.digital(input, self.on(input, Actuation::FullPull, full, now), &full.output, snap);
                    }
                }
                Input::Stick(s) => {
                    let (x, y) = self.stick(s, snap);
                    let half = self.stick_middle().1;
                    match b.output {
                        Output::Stick(to) => {
                            if let Some(t) = g.sticks.get_mut(to as usize) {
                                *t = (t.0 + x, t.1 + y);
                            }
                        }
                        // Already tuned, with the binding's deadzone and curve.
                        Output::Pointer if (x, y) != (0.0, 0.0) => g.w.pointer.push(((x / half, y / half), b.speed)),
                        Output::Dpad => {
                            for d in Dir::ALL {
                                g.digital(input, self.digital(Input::StickDir(s, d), snap), &Output::Pad(PadOut::Dpad(d)), snap);
                            }
                        }
                        Output::Keys(keys) => {
                            // The keys are in the order up, left, down, right.
                            for (d, chord) in [Dir::Up, Dir::Left, Dir::Down, Dir::Right].into_iter().zip(keys) {
                                g.digital(input, self.digital(Input::StickDir(s, d), snap), &Output::Key(chord), snap);
                            }
                        }
                        _ => {}
                    }
                }
                Input::Gyro => {}
            }
        }
        g
    }

    /// Adds the gyro's turn to the stick it is bound to, while it acts.
    fn gyro_to_stick(&self, snap: &PadSnapshot, sticks: &mut [(f32, f32); 2]) {
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
    }

    /// The pad's axes from what was gathered: its sticks, triggers and D-pad. A stick or trigger as
    /// itself keeps the controller's exact values.
    fn pad_axes(&self, g: Gathered, snap: &PadSnapshot) -> Wanted {
        let Gathered { mut w, sticks, triggers, dpad } = g;
        let stick = self.ranges.stick;
        let mid = self.stick_middle().0;
        let raw = |v: f32| (mid + v.round() as i32).clamp(stick.min, stick.max);
        for (s, (x, y)) in Side::ALL.into_iter().zip(sticks) {
            let (ax, ay) = stick_axes(s);
            let plain = self.get(Input::Stick(s)).is_none_or(|b| b.output == Output::Stick(s) && !b.shapes());
            let own = plain && self.get(Input::Gyro).is_none_or(|g| g.output != Output::Stick(s));
            let other = Side::ALL.into_iter().filter(|o| *o != s).any(|o| self.get(Input::Stick(o)).is_some_and(|b| b.output == Output::Stick(s)));
            let dirs = Dir::ALL.into_iter().any(|d| self.get(Input::StickDir(s, d)).is_some());
            // Only the axes the pad has. The game's pad has the controller's axes.
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
            let plain = self.get(Input::Trigger(s)).is_none_or(|b| b.output == Output::Trigger(s) && !b.shapes());
            let own = plain && v == self.trigger(s, snap);
            let value = if own { snap.value(axis).unwrap_or(trig.min) } else { trig.min + ((trig.max - trig.min) as f32 * v.clamp(0.0, 1.0)).round() as i32 };
            w.pad_axes.push((axis, value));
        }
        let [up, down, left, right] = dpad;
        // The D-pad goes on the game's pad's hat. Like an Xbox 360 pad it has one, whatever the controller
        // has.
        w.pad_axes.push((AbsoluteAxisCode::ABS_HAT0X, i32::from(right) - i32::from(left)));
        w.pad_axes.push((AbsoluteAxisCode::ABS_HAT0Y, i32::from(down) - i32::from(up)));
        w.pad_keys.sort_by_key(|k| k.0);
        w.pad_keys.dedup();
        // Keys keep their order, so a chord's modifiers go down before its key.
        let mut keys = Vec::with_capacity(w.keys.len());
        for k in w.keys.drain(..) {
            if !keys.contains(&k) {
                keys.push(k);
            }
        }
        w.keys = keys;
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
        // The keyboard and mouse: keys up first, then modifiers up, then modifiers down, then keys down.
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

/// Adds a chord's keys as held: its modifiers, then its key.
fn chord_down(keys: &mut Vec<KeyCode>, chord: &Chord) {
    let (mods, key) = chord_keys(chord);
    for k in mods.into_iter().chain(key) {
        if !keys.contains(&k) {
            keys.push(k);
        }
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::indexing_slicing)]
mod tests;
