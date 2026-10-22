//! Keymaps: remappings of the controller (a game's or all games' setting,
//! [`crate::settings::Setting::Keymap`], is a keymap's id; 0 is Default, no remapping). A keymap is named,
//! and shared by the games that choose it, or a game's own: unnamed, offered to that game alone (kept while
//! another is chosen, until deleted), made from the keymap in force there. Bindings move between the two by
//! copying: a named keymap (or Default) loaded into a game's own, a game's own saved as a new named keymap or
//! over one. Each maps some of the pad's inputs ([`Input`]) to something else ([`Output`]); an input without
//! a binding does what it does on the pad. What an input may map to depends on its kind ([`Kind`]): a button
//! to a pad button, a key chord, a mouse button or a wheel step; a stick to a stick, the mouse pointer, four
//! keys or the D-pad; a trigger to a trigger or, past a threshold, anything a button maps to; the gyro to the
//! mouse pointer or a stick, always or while a chosen button is held.
//!
//! A keymap has layers ([`Layer`]): Base, and any number added. A binding turns a layer on
//! ([`Output::Layer`]) while held, until pressed again, or for one input. An input with no binding in a
//! layer that is on does what the layers below it say, Base last.
//!
//! A button (or a D-pad or stick direction) may also act differently held long or pressed twice
//! ([`Actuation`]): each is a binding of its own. A binding to something on or off may also repeat while
//! on (turbo) or stay on from one actuation to the next (toggle).
//!
//! Kept as JSON, through serde (a file each in `keymaps/`: see clamshell-system's `store`).

use std::fmt::{self, Write};

use crate::keyboard::{Key, Named};
use crate::kit::names::names;

mod inputs;
mod outputs;
mod param;
pub mod screens;

pub use inputs::*;
pub use outputs::*;
pub use param::*;

/// Motion mapped to a stick or the pointer: its speed, 0 to 100 (50 is the stock speed).
pub const DEFAULT_SPEED: u8 = 50;
/// A trigger mapped to a button acts past this (percent of its travel, 1 to 100).
pub const DEFAULT_THRESHOLD: u8 = 50;
/// A trigger's full pull acts past this (percent of its travel) unless set otherwise.
pub const FULL_PULL: u8 = 95;
/// A press held this long is a hold (hundredths of a second, 20 to 100).
pub const DEFAULT_HOLD_TIME: u8 = 40;
/// The gyro: turning faster than this (degrees per second) gets the whole of its acceleration.
pub const DEFAULT_ACCEL_AT: u8 = 60;
/// The gyro: turning slower than this (tenths of a degree per second) is scaled down toward nothing
/// (tightening, against a hand's tremor).
pub const DEFAULT_TIGHTEN: u8 = 10;

/// A stick on the pointer: its deadzone and curve unless set otherwise (as the pointer moved before the
/// parameters were there: nothing within 15 % of the middle, then the distance from there squared).
pub const POINTER_DEADZONE: u8 = 15;
pub const POINTER_CURVE: u8 = 50;

/// An input's binding: what it maps to, and how. A stick's or trigger's tuning (deadzone, where it is
/// full, curve, inversion) applies to its values before they are mapped, whatever they map to; a binding
/// to the input itself ([`as_on_the_pad`]) with tuning is the input as on the pad, tuned.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
pub struct Binding {
    pub output: Output,
    /// Motion to the pointer or a stick: 0 to 100 (see [`DEFAULT_SPEED`]).
    pub speed: u8,
    /// A trigger to a button: percent of its travel.
    pub threshold: u8,
    /// A stick or trigger: percent of its travel that counts as rest, and where it counts as full
    /// (the range between stretched to the whole).
    pub deadzone: u8,
    pub full: u8,
    /// A stick: 0 is linear, more is finer near the middle (up to its distance cubed).
    pub curve: u8,
    /// A stick or the gyro's motion: across and up/down turned round.
    pub invert_x: bool,
    pub invert_y: bool,
    /// The gyro acts only while this is held (a button, or a trigger past half); always without one.
    pub hold: Option<Input>,
    /// Something on or off: pressed again and again while on, this many times a second (0: not).
    pub turbo: u8,
    /// Something on or off: one actuation turns it on, the next off.
    pub toggle: bool,
    /// A hold's binding: how long a press must last (hundredths of a second).
    pub hold_time: u8,
    /// The gyro: how much faster turns go further (0: as slow ones; 100: three times at full).
    pub accel: u8,
    /// The gyro: the turning speed (degrees per second) that gets all of the acceleration.
    pub accel_at: u8,
    /// The gyro: tightening, tenths of a degree per second (see [`DEFAULT_TIGHTEN`]).
    pub tighten: u8,
    /// The gyro to a stick: where its output starts (percent; past a game's own stick deadzone).
    pub stick_min: u8,
    /// The gyro: tilting the device like a steering wheel (roll) turns as well as rotating it (yaw).
    pub twist: bool,
    /// The gyro: off while its hold is held, instead of on (to lift and re-centre, a ratchet).
    pub ratchet: bool,
}

impl Binding {
    /// A binding to `output` with the parameters' general defaults (no deadzone, linear): where those
    /// are written. See [`Binding::of`] for an input's.
    pub fn to(output: Output) -> Binding {
        Binding {
            output,
            speed: DEFAULT_SPEED,
            threshold: DEFAULT_THRESHOLD,
            deadzone: 0,
            full: 100,
            curve: 0,
            invert_x: false,
            invert_y: false,
            hold: None,
            turbo: 0,
            toggle: false,
            hold_time: DEFAULT_HOLD_TIME,
            accel: 0,
            accel_at: DEFAULT_ACCEL_AT,
            tighten: DEFAULT_TIGHTEN,
            stick_min: 0,
            twist: false,
            ratchet: false,
        }
    }

    /// `input`'s binding to `output` with the defaults for that pair ([`Param::default_for`]: a stick on
    /// the pointer has a deadzone and a curve).
    pub fn of(input: Input, output: Output) -> Binding {
        let mut b = Binding::to(output);
        for p in Param::ALL {
            p.set(&mut b, p.default_for(input, output));
        }
        b
    }

    /// Whether any parameter of `input`'s differs from its default for this output.
    pub fn tuned(&self, input: Input) -> bool {
        params(input, self.output).any(|p| p.get(self) != p.default_for(input, self.output)) || self.hold.is_some()
    }

    /// Whether it changes a stick's or trigger's values (a deadzone, a full point short of the end, a
    /// curve, an inversion).
    pub fn shapes(&self) -> bool {
        self.deadzone > 0 || self.full < 100 || self.curve > 0 || self.invert_x || self.invert_y
    }

    /// The same binding to another output: each parameter at its default for the old output goes to its
    /// default for the new one; the ones set stay.
    pub fn retarget(&self, input: Input, output: Output) -> Binding {
        let mut b = Binding { output, ..*self };
        for p in Param::ALL {
            if p.get(self) == p.default_for(input, self.output) {
                p.set(&mut b, p.default_for(input, output));
            }
        }
        b
    }
}

/// How an input is used: pressed (as usual; a trigger pulled), held long, pressed twice quickly (a button,
/// a D-pad or stick direction), or pulled all the way (a trigger). Each may have a binding of its own;
/// with a hold's or a double's, a short press acts only once it is known not to be one of those (on
/// letting go, or after the window for a second press). A trigger's full pull acts past its own
/// threshold beside the pull's binding (analog, or a button past its threshold): two stages.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum Actuation {
    #[default]
    Press,
    Hold,
    Double,
    FullPull,
}

names!(Actuation {
    Press: "press", "Press";
    Hold: "hold", "Hold";
    Double: "double", "Double";
    FullPull: "full", "Full pull";
});

impl Actuation {
    /// Its label for an input of `kind` (a trigger's press is its pull).
    pub fn label_for(self, kind: Kind) -> &'static str {
        match (self, kind) {
            (Actuation::Press, Kind::Trigger) => "Pull",
            (a, _) => a.label(),
        }
    }

    /// Whether an input of `kind` has it: every input is pressed; buttons and directions are held or
    /// pressed twice; a trigger is pulled all the way.
    pub fn fits(self, kind: Kind) -> bool {
        match self {
            Actuation::Press => true,
            Actuation::Hold | Actuation::Double => kind == Kind::Digital,
            Actuation::FullPull => kind == Kind::Trigger,
        }
    }

    /// The actuations an input of `kind` has, in the order shown.
    pub fn of(kind: Kind) -> impl Iterator<Item = Actuation> {
        Actuation::ALL.into_iter().filter(move |a| a.fits(kind))
    }
}

/// How the panel and the labels tell an input's binding: none, its own output only tuned, or mapped to
/// something else.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Change {
    None,
    Tuned,
    Mapped,
}

/// What `binding` (of `input`) changes.
pub fn change(input: Input, binding: Option<&Binding>) -> Change {
    match binding {
        None => Change::None,
        Some(b) if Some(b.output) != as_on_the_pad(input) => Change::Mapped,
        Some(b) if b.tuned(input) => Change::Tuned,
        Some(_) => Change::None,
    }
}

/// What may hold the gyro on: the buttons and the triggers.
pub fn gyro_holds() -> impl Iterator<Item = Input> {
    Button::ALL.into_iter().map(Input::Button).chain(Side::ALL.into_iter().map(Input::Trigger))
}

/// A keymap's layer, by its number in the keymap: Base (0), whose inputs without a binding do what
/// they do on the pad, and the layers added to it, each on while a binding to it is
/// ([`Output::Layer`]), its inputs without a binding doing what the layers on before it say (Base
/// last).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, PartialOrd, Ord, serde::Serialize, serde::Deserialize)]
pub struct Layer(pub u8);

impl Layer {
    pub const BASE: Layer = Layer(0);
}

/// One layer's bindings, an actuation's each in [`Input::all`]'s order, one per input at most.
#[derive(Debug, Clone, PartialEq, Eq, Default, serde::Serialize, serde::Deserialize)]
struct Bindings {
    press: Vec<(Input, Binding)>,
    hold: Vec<(Input, Binding)>,
    double: Vec<(Input, Binding)>,
    full: Vec<(Input, Binding)>,
}

impl Bindings {
    fn of(&self, act: Actuation) -> &Vec<(Input, Binding)> {
        match act {
            Actuation::Press => &self.press,
            Actuation::Hold => &self.hold,
            Actuation::Double => &self.double,
            Actuation::FullPull => &self.full,
        }
    }

    fn of_mut(&mut self, act: Actuation) -> &mut Vec<(Input, Binding)> {
        match act {
            Actuation::Press => &mut self.press,
            Actuation::Hold => &mut self.hold,
            Actuation::Double => &mut self.double,
            Actuation::FullPull => &mut self.full,
        }
    }

    fn is_empty(&self) -> bool {
        Actuation::ALL.into_iter().all(|a| self.of(a).is_empty())
    }
}

/// A layer of a keymap: its number, its name, its bindings, and whether it is Once (see
/// [`Keymap::layer_once`]).
#[derive(Debug, Clone, PartialEq, Eq, Default, serde::Serialize, serde::Deserialize)]
struct LayerOf {
    id: Layer,
    name: String,
    bindings: Bindings,
    once: bool,
}

/// A set of bindings in layers ([`Layer`]: Base, and those added), each actuation's ([`Actuation`]): named,
/// or a game's own.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Keymap {
    /// Its number, kept across renames (0 is Default, which no keymap has).
    pub id: u16,
    /// Empty for a game's own.
    pub name: String,
    /// The game whose own it is (by id); none: a named one, which any game may choose.
    pub owner: Option<std::sync::Arc<str>>,
    /// Base first, then the others in the order added.
    layers: Vec<LayerOf>,
}

impl Default for Keymap {
    fn default() -> Keymap {
        Keymap::new(0, "")
    }
}

/// An empty set of bindings, for a layer that isn't there.
static NO_BINDINGS: Bindings = Bindings { press: Vec::new(), hold: Vec::new(), double: Vec::new(), full: Vec::new() };

impl Keymap {
    pub fn new(id: u16, name: &str) -> Keymap {
        Keymap {
            id,
            name: name.to_owned(),
            owner: None,
            layers: vec![LayerOf { id: Layer::BASE, name: "Base".into(), bindings: Bindings::default(), once: false }],
        }
    }

    /// Base's bindings (a press's).
    pub fn bindings(&self) -> &[(Input, Binding)] {
        self.layer(Layer::BASE)
    }

    /// The layers, Base first: their numbers and names.
    pub fn layers(&self) -> impl DoubleEndedIterator<Item = (Layer, &str)> {
        self.layers.iter().map(|l| (l.id, l.name.as_str()))
    }

    /// A layer's name (none: not there).
    pub fn layer_name(&self, layer: Layer) -> Option<&str> {
        self.layers.iter().find(|l| l.id == layer).map(|l| l.name.as_str())
    }

    /// A new layer named `name`; its number (none: no more room, 255 layers).
    pub fn add_layer(&mut self, name: &str) -> Option<Layer> {
        let id = Layer(self.layers.iter().map(|l| l.id.0).max().unwrap_or(0).checked_add(1)?);
        self.layers.push(LayerOf { id, name: name.trim().to_owned(), bindings: Bindings::default(), once: false });
        Some(id)
    }

    /// Whether a layer is Once: turned on by a tap (or a toggle), it stays on until the first input it
    /// maps has acted, then goes off; held, it is on while held, as any layer. (Base never is.)
    pub fn layer_once(&self, layer: Layer) -> bool {
        self.layers.iter().any(|l| l.id == layer && l.once)
    }

    /// Makes a layer (not Base) Once or not; whether it is there.
    pub fn set_layer_once(&mut self, layer: Layer, once: bool) -> bool {
        match self.layers.iter_mut().find(|l| l.id == layer && l.id != Layer::BASE) {
            Some(l) => {
                l.once = once;
                true
            }
            None => false,
        }
    }

    /// Renames a layer (Base keeps its name); whether it is there.
    pub fn rename_layer(&mut self, layer: Layer, name: &str) -> bool {
        match self.layers.iter_mut().find(|l| l.id == layer && l.id != Layer::BASE) {
            Some(l) => {
                l.name = name.trim().to_owned();
                true
            }
            None => false,
        }
    }

    /// Removes a layer (not Base) and the bindings to it; whether it was there.
    pub fn remove_layer(&mut self, layer: Layer) -> bool {
        let before = self.layers.len();
        self.layers.retain(|l| l.id != layer || l.id == Layer::BASE);
        if self.layers.len() == before {
            return false;
        }
        for l in &mut self.layers {
            for act in Actuation::ALL {
                l.bindings.of_mut(act).retain(|(_, b)| b.output != Output::Layer(layer));
            }
        }
        true
    }

    /// A layer's bindings (a press's).
    pub fn layer(&self, layer: Layer) -> &[(Input, Binding)] {
        self.layer_of(layer, Actuation::Press)
    }

    /// A layer's bindings of `act`.
    pub fn layer_of(&self, layer: Layer, act: Actuation) -> &[(Input, Binding)] {
        self.bindings_in(layer).of(act)
    }

    fn bindings_in(&self, layer: Layer) -> &Bindings {
        self.layers.iter().find(|l| l.id == layer).map_or(&NO_BINDINGS, |l| &l.bindings)
    }

    fn bindings_in_mut(&mut self, layer: Layer) -> Option<&mut Bindings> {
        self.layers.iter_mut().find(|l| l.id == layer).map(|l| &mut l.bindings)
    }

    /// Whether `layer` has no bindings at all.
    pub fn is_empty(&self, layer: Layer) -> bool {
        self.bindings_in(layer).is_empty()
    }

    /// `input`'s binding in Base.
    pub fn get(&self, input: Input) -> Option<&Binding> {
        self.get_in(Layer::BASE, input)
    }

    /// `input`'s binding in `layer` itself (none in another layer than Base: those on before it apply).
    pub fn get_in(&self, layer: Layer, input: Input) -> Option<&Binding> {
        self.get_of(layer, input, Actuation::Press)
    }

    /// `input`'s binding of `act` in `layer` itself.
    pub fn get_of(&self, layer: Layer, input: Input, act: Actuation) -> Option<&Binding> {
        self.layer_of(layer, act).iter().find(|(i, _)| *i == input).map(|(_, b)| b)
    }

    /// The binding in force for `input` with the layers `on` (in the order they came on; Base
    /// beneath them all): the newest's that has one.
    pub fn lookup(&self, input: Input, on: &[Layer]) -> Option<&Binding> {
        self.lookup_of(input, Actuation::Press, on)
    }

    /// [`Keymap::lookup`] for `act`.
    pub fn lookup_of(&self, input: Input, act: Actuation, on: &[Layer]) -> Option<&Binding> {
        on.iter().rev().find_map(|l| self.get_of(*l, input, act)).or_else(|| self.get_of(Layer::BASE, input, act))
    }

    /// Binds `input` in Base (none: back to what it does on the pad); see [`Keymap::set_in`].
    pub fn set(&mut self, input: Input, binding: Option<Binding>) -> bool {
        self.set_in(Layer::BASE, input, binding)
    }

    /// Binds `input` in `layer` (none: as on the pad in Base, as the layers beneath in another). A
    /// binding that doesn't fit the input's kind is refused; whether it changed.
    pub fn set_in(&mut self, layer: Layer, input: Input, binding: Option<Binding>) -> bool {
        self.set_of(layer, input, Actuation::Press, binding)
    }

    /// Binds `input`'s `act` in `layer` (none: no hold or double; for a press, as [`Keymap::set_in`]).
    /// Refused where it doesn't fit (a binding to the input's kind, a hold or double to a stick); whether
    /// it changed.
    pub fn set_of(&mut self, layer: Layer, input: Input, act: Actuation, binding: Option<Binding>) -> bool {
        if binding.is_some_and(|b| !b.output.fits(input.kind())) || (binding.is_some() && !act.fits(input.kind())) {
            return false;
        }
        // (A full pull acts as a button: something on or off, or nothing.)
        if act == Actuation::FullPull && binding.is_some_and(|b| !b.output.digital() && b.output != Output::Nothing) {
            return false;
        }
        // (A layer to a layer not there is refused.)
        if let Some(Output::Layer(to)) = binding.map(|b| b.output)
            && self.layer_name(to).is_none()
        {
            return false;
        }
        let Some(bindings) = self.bindings_in_mut(layer) else {
            return false;
        };
        let list = bindings.of_mut(act);
        let before = list.clone();
        list.retain(|(i, _)| *i != input);
        if let Some(b) = binding {
            list.push((input, b));
            list.sort_by_key(|(i, _)| *i);
        }
        *list != before
    }

    /// Drops every binding of `layer`; whether it had any.
    pub fn clear(&mut self, layer: Layer) -> bool {
        let had = !self.is_empty(layer);
        if let Some(b) = self.bindings_in_mut(layer) {
            *b = Bindings::default();
        }
        had
    }
}

/// The output that does what `input` does on the pad: a button, D-pad direction, stick or trigger maps to
/// itself, and the gyro to nothing. Used to give an input its plain behaviour back in a layer above one
/// that remaps it. A stick's direction has none, since it is part of its stick.
pub fn as_on_the_pad(input: Input) -> Option<Output> {
    match input {
        // (Back isn't on the pad games see: it does nothing there.)
        Input::Button(Button::Back) => Some(Output::Nothing),
        Input::Button(b) => Some(Output::Pad(PadOut::Button(b))),
        Input::Dpad(d) => Some(Output::Pad(PadOut::Dpad(d))),
        Input::Stick(s) => Some(Output::Stick(s)),
        Input::Trigger(s) => Some(Output::Trigger(s)),
        Input::Gyro => Some(Output::Nothing),
        Input::StickDir(..) => None,
    }
}

/// The ready-made keymap there is before any are saved: A and B, X and Y swapped.
pub fn nintendo(id: u16) -> Keymap {
    let mut k = Keymap::new(id, "Nintendo layout");
    for (from, to) in [(Button::A, Button::B), (Button::B, Button::A), (Button::X, Button::Y), (Button::Y, Button::X)] {
        k.set(Input::Button(from), Some(Binding::to(Output::Pad(PadOut::Button(to)))));
    }
    k
}

/// Every keymap, by id.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Keymaps {
    list: Vec<Keymap>,
}

impl Keymaps {
    /// These keymaps, as saved.
    pub fn from_list(list: Vec<Keymap>) -> Keymaps {
        Keymaps { list }
    }

    /// What there is before any were saved: the Nintendo layout.
    pub fn initial() -> Keymaps {
        Keymaps { list: vec![nintendo(1)] }
    }

    pub fn list(&self) -> &[Keymap] {
        &self.list
    }

    pub fn get(&self, id: u16) -> Option<&Keymap> {
        self.list.iter().find(|k| k.id == id)
    }

    pub fn get_mut(&mut self, id: u16) -> Option<&mut Keymap> {
        self.list.iter_mut().find(|k| k.id == id)
    }

    /// The named keymaps (those any game may choose).
    pub fn named(&self) -> impl Iterator<Item = &Keymap> {
        self.list.iter().filter(|k| k.owner.is_none())
    }

    /// `game`'s own keymap, if it has one.
    pub fn own_of(&self, game: &str) -> Option<&Keymap> {
        self.list.iter().find(|k| k.owner.as_deref() == Some(game))
    }

    /// The id for a new keymap: one past the highest, so an id is never taken twice while the highest
    /// stays. None once the ids have run out.
    fn next_id(&self) -> Option<u16> {
        self.list.iter().map(|k| k.id).max().unwrap_or(0).checked_add(1)
    }

    /// `game`'s own keymap made, a copy of keymap `from` (Default, or one gone: empty); its id.
    pub fn make_own(&mut self, from: u16, game: &std::sync::Arc<str>) -> Option<u16> {
        let id = self.next_id()?;
        let mut own = Keymap::new(id, "");
        own.owner = Some(game.clone());
        if let Some(from) = self.get(from) {
            own.layers = from.layers.clone();
        }
        self.list.push(own);
        Some(id)
    }

    /// `owner`'s own keymap made with `preset`'s bindings (see [`preset`]); its id.
    pub fn make_own_from(&mut self, preset: &Keymap, owner: &std::sync::Arc<str>) -> Option<u16> {
        let id = self.make_own(0, owner)?;
        if let Some(own) = self.get_mut(id) {
            own.layers = preset.layers.clone();
        }
        Some(id)
    }

    /// Keymap `to`'s bindings and layers replaced by a copy of keymap `from`'s (Default, or one gone:
    /// none); whether `to` is there.
    pub fn copy_into(&mut self, to: u16, from: u16) -> bool {
        let layers = self.get(from).map_or_else(|| Keymap::new(0, "").layers, |k| k.layers.clone());
        self.get_mut(to).map(|k| k.layers = layers).is_some()
    }

    /// A name no keymap has: `name`, else with a number after it. As its file is named (see
    /// clamshell-system's `store`): a `/` made `-`, cut at 64 characters.
    pub fn unique_name(&self, name: &str) -> String {
        let name: String = name.trim().replace('/', "-").chars().take(64).collect();
        let name = name.trim();
        let taken = |n: &str| self.list.iter().any(|k| k.name == n);
        if !taken(name) {
            return name.to_owned();
        }
        (2..).map(|i| format!("{name} {i}")).find(|n| !taken(n)).unwrap_or_default()
    }

    /// A new, empty keymap named `name` (made unique); its id.
    pub fn add(&mut self, name: &str) -> Option<u16> {
        let id = self.next_id()?;
        let name = self.unique_name(name);
        self.list.push(Keymap::new(id, &name));
        Some(id)
    }

    /// A copy of keymap `id` named `name` (made unique); the copy's id.
    pub fn duplicate(&mut self, id: u16, name: &str) -> Option<u16> {
        let layers = self.get(id).map(|k| k.layers.clone())?;
        let copy = self.add(name)?;
        if let Some(k) = self.get_mut(copy) {
            k.layers = layers;
        }
        Some(copy)
    }

    /// Renames keymap `id` (another's name is made unique); whether it was there.
    pub fn rename(&mut self, id: u16, name: &str) -> bool {
        if self.get(id).is_some_and(|k| k.name == name.trim()) {
            return true;
        }
        let name = self.unique_name(name);
        self.get_mut(id).map(|k| k.name = name).is_some()
    }

    pub fn remove(&mut self, id: u16) -> bool {
        let before = self.list.len();
        self.list.retain(|k| k.id != id);
        self.list.len() != before
    }
}

/// ClamShell's own keymaps for its runners, by name: `viewer` (Text, PDF, Images: programs that read the
/// keyboard, not the pad): the D-pad the arrow keys, A Space (on a page), B `q` (quit), the shoulders Page
/// Up and Page Down.
pub fn preset(name: &str) -> Option<Keymap> {
    let key = |k: Key| Binding::to(Output::Key(Chord::of(k)));
    match name {
        "viewer" => {
            let mut k = Keymap::new(0, "");
            for (dir, named) in [(Dir::Up, Named::Up), (Dir::Down, Named::Down), (Dir::Left, Named::Left), (Dir::Right, Named::Right)] {
                k.set(Input::Dpad(dir), Some(key(Key::Named(named))));
            }
            k.set(Input::Button(Button::A), Some(key(Key::Char(' '))));
            k.set(Input::Button(Button::B), Some(key(Key::Char('q'))));
            k.set(Input::Button(Button::LB), Some(key(Key::Named(Named::PageUp))));
            k.set(Input::Button(Button::RB), Some(key(Key::Named(Named::PageDown))));
            Some(k)
        }
        _ => None,
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests;
