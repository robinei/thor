//! Keymaps: remappings of the controller. The keymap setting of a game or of all games
//! ([`crate::settings::Setting::Keymap`]) is a keymap's id; 0 is Default, which remaps nothing.
//!
//! A keymap is either named and shared by the games that choose it, or a game's own. A game's own keymap
//! is unnamed and offered to that game alone. It is made from the keymap in force for the game, and it is
//! kept while the game uses another one, until it is deleted. Bindings move between the two kinds by
//! copying: a named keymap (or Default) can be loaded into a game's own, and a game's own can be saved as a
//! new named keymap or over an existing one.
//!
//! A keymap maps some of the pad's inputs ([`Input`]) to something else ([`Output`]). An input without a
//! binding does what it does on the pad. What an input may map to depends on its kind ([`Kind`]):
//!
//! - a button to a pad button, a key chord, a mouse button or a wheel step;
//! - a stick to a stick, the mouse pointer, four keys or the D-pad;
//! - a trigger to a trigger, or, past a threshold, anything a button maps to;
//! - the gyro to the mouse pointer or a stick, either always or while a chosen button is held.
//!
//! A keymap has layers ([`Layer`]): Base, and any number of added ones. A binding can turn a layer on
//! ([`Output::Layer`]) while it is held, until it is pressed again, or for one input. An input with no
//! binding in a layer that is on does what the layers below it say, with Base last.
//!
//! A button, D-pad direction or stick direction may also act differently when held long or pressed twice
//! ([`Actuation`]). Each of these is a binding of its own. A binding to something that is on or off may
//! also repeat while on (turbo) or stay on from one actuation to the next (toggle).
//!
//! Keymaps are kept as JSON through serde, one file each in `keymaps/` (see clamshell-system's `store`).

use std::fmt::{self, Write};

use crate::bottom::keyboard::{Key, Named};
use crate::kit::names::names;
use crate::kit::replace;

mod inputs;
mod outputs;
mod param;
pub mod screens;

pub use inputs::*;
pub use outputs::*;
pub use param::*;

/// The speed of motion mapped to a stick or the pointer, 0 to 100. 50 is the stock speed.
pub const DEFAULT_SPEED: u8 = 50;
/// A trigger mapped to a button acts past this, in percent of its travel (1 to 100).
pub const DEFAULT_THRESHOLD: u8 = 50;
/// A trigger's full pull acts past this, in percent of its travel, unless set otherwise.
pub const FULL_PULL: u8 = 95;
/// A press held this long is a hold, in hundredths of a second (20 to 100).
pub const DEFAULT_HOLD_TIME: u8 = 40;
/// The gyro turning speed, in degrees per second, above which a turn gets all of the acceleration.
pub const DEFAULT_ACCEL_AT: u8 = 60;
/// The gyro turning speed, in tenths of a degree per second, below which motion is scaled down toward
/// nothing. This tightening counters a hand's tremor.
pub const DEFAULT_TIGHTEN: u8 = 10;

/// The default deadzone and curve for a stick mapped to the pointer. They match how the pointer moved
/// before these parameters existed: nothing within 15 % of the middle, then the distance from there
/// squared.
pub const POINTER_DEADZONE: u8 = 15;
pub const POINTER_CURVE: u8 = 50;

/// An input's binding: what it maps to, and how. A stick's or trigger's tuning (deadzone, full point,
/// curve, inversion) applies to its values before they are mapped, whatever they map to. A binding of an
/// input to itself ([`as_on_the_pad`]) with tuning gives the input as it is on the pad, tuned.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
pub struct Binding {
    pub output: Output,
    /// The speed of motion to the pointer or a stick, 0 to 100 (see [`DEFAULT_SPEED`]).
    pub speed: u8,
    /// For a trigger mapped to a button: where it acts, in percent of its travel.
    pub threshold: u8,
    /// For a stick or trigger: the percent of its travel that counts as rest, and the point where it
    /// counts as full. The range between them is stretched to the whole.
    pub deadzone: u8,
    pub full: u8,
    /// For a stick: 0 is linear, and higher values are finer near the middle, up to the distance cubed.
    pub curve: u8,
    /// For a stick or the gyro's motion: reverses the horizontal and the vertical axis.
    pub invert_x: bool,
    pub invert_y: bool,
    /// The gyro acts only while this is held: a button, or a trigger past half. Without one it always
    /// acts.
    pub hold: Option<Input>,
    /// For something on or off: how many times a second it is pressed again while on. 0 is no turbo.
    pub turbo: u8,
    /// For something on or off: one actuation turns it on and the next turns it off.
    pub toggle: bool,
    /// For a hold's binding: how long a press must last, in hundredths of a second.
    pub hold_time: u8,
    /// The gyro's acceleration: how much further fast turns go. At 0 they go as far as slow ones; at 100
    /// they go three times as far at full acceleration.
    pub accel: u8,
    /// The gyro turning speed, in degrees per second, that gets all of the acceleration.
    pub accel_at: u8,
    /// The gyro's tightening, in tenths of a degree per second (see [`DEFAULT_TIGHTEN`]).
    pub tighten: u8,
    /// For the gyro mapped to a stick: where its output starts, in percent. This gets it past a game's own
    /// stick deadzone.
    pub stick_min: u8,
    /// The gyro: tilting the device like a steering wheel (roll) turns as well as rotating it (yaw).
    pub twist: bool,
    /// For the gyro: holding its hold turns it off instead of on, so the device can be lifted and
    /// re-centred like a ratchet.
    pub ratchet: bool,
}

impl Binding {
    /// A binding to `output` with the general defaults for the parameters, such as no deadzone and a
    /// linear curve. This is where those defaults are set. See [`Binding::of`] for an input's defaults.
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

    /// `input`'s binding to `output` with the defaults for that pair ([`Param::default_for`]). For
    /// example, a stick on the pointer has a deadzone and a curve.
    pub fn of(input: Input, output: Output) -> Binding {
        let mut b = Binding::to(output);
        for p in Param::ALL {
            p.set(&mut b, p.default_for(input, output));
        }
        b
    }

    /// Whether any of `input`'s parameters differs from its default for this output, or a hold is set.
    pub fn tuned(&self, input: Input) -> bool {
        params(input, self.output).any(|p| p.get(self) != p.default_for(input, self.output)) || self.hold.is_some()
    }

    /// Whether it changes a stick's or trigger's values with a deadzone, a full point short of the end, a
    /// curve or an inversion.
    pub fn shapes(&self) -> bool {
        self.deadzone > 0 || self.full < 100 || self.curve > 0 || self.invert_x || self.invert_y
    }

    /// The same binding to another output. Each parameter at its default for the old output gets its
    /// default for the new one; parameters that were set keep their values.
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

/// How an input is used: pressed (for a trigger, pulled), held long, pressed twice quickly, or pulled all
/// the way. Holds and doubles are for buttons and D-pad or stick directions; a full pull is for a trigger.
///
/// Each may have a binding of its own. When there is a hold or double binding, a short press acts only
/// once it is known not to be one of those: on letting go, or after the window for a second press. A
/// trigger's full pull acts past its own threshold alongside the pull's binding, whether that is analog
/// or a button past its threshold, which gives the trigger two stages.
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
    /// Its label for an input of `kind`. A trigger's press is labelled as its pull.
    pub fn label_for(self, kind: Kind) -> &'static str {
        match (self, kind) {
            (Actuation::Press, Kind::Trigger) => "Pull",
            (a, _) => a.label(),
        }
    }

    /// Whether an input of `kind` has it. Every input is pressed; buttons and directions can also be held
    /// or pressed twice, and a trigger can be pulled all the way.
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

/// How the panel and the labels show an input's binding: no change, its own output tuned, or mapped to
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

/// A keymap's layer, by its number in the keymap. In Base (0), inputs without a binding do what they do
/// on the pad. Each added layer is on while a binding to it is ([`Output::Layer`]), and its inputs without
/// a binding do what the layers that came on before it say, with Base last.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, PartialOrd, Ord, serde::Serialize, serde::Deserialize)]
pub struct Layer(pub u8);

impl Layer {
    pub const BASE: Layer = Layer(0);
}

/// One layer's bindings, in a list per actuation. Each list is in [`Input::all`]'s order and has at most
/// one binding per input.
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

/// A set of bindings in layers ([`Layer`]), with bindings for each actuation ([`Actuation`]). It is either
/// named or a game's own.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Keymap {
    /// Its number, kept across renames. 0 is Default, which no keymap has.
    pub id: u16,
    /// Empty for a game's own.
    pub name: String,
    /// The id of the game whose own keymap this is. None for a named keymap, which any game may choose.
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

    /// Base's press bindings.
    pub fn bindings(&self) -> &[(Input, Binding)] {
        self.layer(Layer::BASE)
    }

    /// The layers, Base first: their numbers and names.
    pub fn layers(&self) -> impl DoubleEndedIterator<Item = (Layer, &str)> {
        self.layers.iter().map(|l| (l.id, l.name.as_str()))
    }

    /// A layer's name, or none if the layer isn't there.
    pub fn layer_name(&self, layer: Layer) -> Option<&str> {
        self.layers.iter().find(|l| l.id == layer).map(|l| l.name.as_str())
    }

    /// Adds a layer named `name` and returns its number, or none when there is no more room (255 layers).
    pub fn add_layer(&mut self, name: &str) -> Option<Layer> {
        let id = Layer(self.layers.iter().map(|l| l.id.0).max().unwrap_or(0).checked_add(1)?);
        self.layers.push(LayerOf { id, name: name.trim().to_owned(), bindings: Bindings::default(), once: false });
        Some(id)
    }

    /// Whether a layer is Once. A Once layer turned on by a tap or a toggle stays on until the first input
    /// it maps has acted, then goes off. While held, it is on like any other layer. Base is never Once.
    pub fn layer_once(&self, layer: Layer) -> bool {
        self.layers.iter().any(|l| l.id == layer && l.once)
    }

    /// Makes a layer other than Base Once or not. Returns whether the layer is there.
    pub fn set_layer_once(&mut self, layer: Layer, once: bool) -> bool {
        match self.layers.iter_mut().find(|l| l.id == layer && l.id != Layer::BASE) {
            Some(l) => {
                l.once = once;
                true
            }
            None => false,
        }
    }

    /// Renames a layer; Base keeps its name. Returns whether the layer is there.
    pub fn rename_layer(&mut self, layer: Layer, name: &str) -> bool {
        match self.layers.iter_mut().find(|l| l.id == layer && l.id != Layer::BASE) {
            Some(l) => {
                l.name = name.trim().to_owned();
                true
            }
            None => false,
        }
    }

    /// Removes a layer other than Base, and the bindings to it. Returns whether it was there.
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

    /// A layer's press bindings.
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

    /// `input`'s binding in `layer` itself. Where a layer other than Base has none, the layers on before
    /// it apply.
    pub fn get_in(&self, layer: Layer, input: Input) -> Option<&Binding> {
        self.get_of(layer, input, Actuation::Press)
    }

    /// `input`'s binding of `act` in `layer` itself.
    pub fn get_of(&self, layer: Layer, input: Input, act: Actuation) -> Option<&Binding> {
        self.layer_of(layer, act).iter().find(|(i, _)| *i == input).map(|(_, b)| b)
    }

    /// The binding in force for `input` with the layers `on`, given in the order they came on, and Base
    /// beneath them all. It is the binding of the newest layer that has one.
    pub fn lookup(&self, input: Input, on: &[Layer]) -> Option<&Binding> {
        self.lookup_of(input, Actuation::Press, on)
    }

    /// [`Keymap::lookup`] for `act`.
    pub fn lookup_of(&self, input: Input, act: Actuation, on: &[Layer]) -> Option<&Binding> {
        on.iter().rev().find_map(|l| self.get_of(*l, input, act)).or_else(|| self.get_of(Layer::BASE, input, act))
    }

    /// Binds `input` in Base; none puts it back to what it does on the pad. See [`Keymap::set_in`].
    pub fn set(&mut self, input: Input, binding: Option<Binding>) -> bool {
        self.set_in(Layer::BASE, input, binding)
    }

    /// Binds `input` in `layer`. None makes it act as on the pad in Base, or as the layers beneath in
    /// another layer. A binding that doesn't fit the input's kind is refused. Returns whether anything
    /// changed.
    pub fn set_in(&mut self, layer: Layer, input: Input, binding: Option<Binding>) -> bool {
        self.set_of(layer, input, Actuation::Press, binding)
    }

    /// Binds `input`'s `act` in `layer`. None removes a hold or double; for a press it works as in
    /// [`Keymap::set_in`]. A binding is refused where it doesn't fit: an output that doesn't fit the
    /// input's kind, or a hold or double on a stick. Returns whether anything changed.
    pub fn set_of(&mut self, layer: Layer, input: Input, act: Actuation, binding: Option<Binding>) -> bool {
        if binding.is_some_and(|b| !b.output.fits(input.kind())) || (binding.is_some() && !act.fits(input.kind())) {
            return false;
        }
        // A full pull acts as a button: it maps to something on or off, or to nothing.
        if act == Actuation::FullPull && binding.is_some_and(|b| !b.output.digital() && b.output != Output::Nothing) {
            return false;
        }
        // A binding to a layer that isn't there is refused.
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

    /// Drops every binding of `layer`. Returns whether it had any.
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
        // Back isn't on the pad that games see, so it does nothing there.
        Input::Button(Button::Back) => Some(Output::Nothing),
        Input::Button(b) => Some(Output::Pad(PadOut::Button(b))),
        Input::Dpad(d) => Some(Output::Pad(PadOut::Dpad(d))),
        Input::Stick(s) => Some(Output::Stick(s)),
        Input::Trigger(s) => Some(Output::Trigger(s)),
        Input::Gyro => Some(Output::Nothing),
        Input::StickDir(..) => None,
    }
}

/// The ready-made keymap there is before any are saved: A and B swapped, and X and Y swapped.
pub fn nintendo(id: u16) -> Keymap {
    let mut k = Keymap::new(id, "Nintendo layout");
    for (from, to) in [(Button::A, Button::B), (Button::B, Button::A), (Button::X, Button::Y), (Button::Y, Button::X)] {
        k.set(Input::Button(from), Some(Binding::to(Output::Pad(PadOut::Button(to)))));
    }
    k
}

/// Every keymap, by id. Each change counts, and a keymap's version is the count at its last change, so
/// that a keymap can be compared with one sent earlier by its id and version alone.
#[derive(Debug, Clone, Default)]
pub struct Keymaps {
    list: Vec<Keymap>,
    /// The changes so far.
    changes: u64,
    /// The keymaps changed since they were read, with the count at their last change.
    versions: Vec<(u16, u64)>,
}

/// Equal when the keymaps are; their versions don't count.
impl PartialEq for Keymaps {
    fn eq(&self, other: &Keymaps) -> bool {
        self.list == other.list
    }
}

impl Eq for Keymaps {}

impl Keymaps {
    /// These keymaps, as saved.
    pub fn from_list(list: Vec<Keymap>) -> Keymaps {
        Keymaps { list, ..Keymaps::default() }
    }

    /// The keymaps there are before any were saved: the Nintendo layout.
    pub fn initial() -> Keymaps {
        Keymaps::from_list(vec![nintendo(1)])
    }

    pub fn list(&self) -> &[Keymap] {
        &self.list
    }

    pub fn get(&self, id: u16) -> Option<&Keymap> {
        self.list.iter().find(|k| k.id == id)
    }

    /// Keymap `id` to change. It counts as changed whether or not it is.
    pub fn get_mut(&mut self, id: u16) -> Option<&mut Keymap> {
        self.changed(id);
        self.list.iter_mut().find(|k| k.id == id)
    }

    /// Changes keymap `id` with `f`, which returns whether it changed anything. Returns that, and false if
    /// the keymap isn't there.
    pub fn edit(&mut self, id: u16, f: impl FnOnce(&mut Keymap) -> bool) -> bool {
        let changed = self.list.iter_mut().find(|k| k.id == id).is_some_and(f);
        if changed {
            self.changed(id);
        }
        changed
    }

    /// Puts `keymap` in place of the keymap with its id, if there is one.
    pub fn replace(&mut self, keymap: Keymap) {
        self.edit(keymap.id, |k| replace(k, keymap));
    }

    /// Keymap `id`'s version: the count of changes at its last change, or 0 if it hasn't changed since the
    /// keymaps were read.
    pub fn version(&self, id: u16) -> u64 {
        self.versions.iter().find(|(k, _)| *k == id).map_or(0, |(_, v)| *v)
    }

    /// Counts a change to keymap `id`.
    fn changed(&mut self, id: u16) {
        self.changes += 1;
        let version = self.changes;
        match self.versions.iter_mut().find(|(k, _)| *k == id) {
            Some((_, v)) => *v = version,
            None => self.versions.push((id, version)),
        }
    }

    /// The named keymaps (those any game may choose).
    pub fn named(&self) -> impl Iterator<Item = &Keymap> {
        self.list.iter().filter(|k| k.owner.is_none())
    }

    /// `entry`'s own keymap, if it has one.
    pub fn own_of(&self, entry: &str) -> Option<&Keymap> {
        self.list.iter().find(|k| k.owner.as_deref() == Some(entry))
    }

    /// The id for a new keymap: one past the highest, so an id is never taken twice while the highest
    /// stays. None once the ids have run out.
    fn next_id(&self) -> Option<u16> {
        self.list.iter().map(|k| k.id).max().unwrap_or(0).checked_add(1)
    }

    /// Makes `entry`'s own keymap as a copy of keymap `from` and returns its id. If `from` is Default or no
    /// longer exists, the new keymap is empty.
    pub fn make_own(&mut self, from: u16, entry: &std::sync::Arc<str>) -> Option<u16> {
        let id = self.next_id()?;
        let mut own = Keymap::new(id, "");
        own.owner = Some(entry.clone());
        if let Some(from) = self.get(from) {
            own.layers = from.layers.clone();
        }
        self.list.push(own);
        self.changed(id);
        Some(id)
    }

    /// Makes `owner`'s own keymap with `preset`'s bindings (see [`preset`]) and returns its id.
    pub fn make_own_from(&mut self, preset: &Keymap, owner: &std::sync::Arc<str>) -> Option<u16> {
        let id = self.make_own(0, owner)?;
        self.edit(id, |own| {
            own.layers = preset.layers.clone();
            true
        });
        Some(id)
    }

    /// Replaces keymap `to`'s bindings and layers with a copy of keymap `from`'s. If `from` is Default or
    /// no longer exists, `to` is left with no bindings. Returns whether `to` is there.
    pub fn copy_into(&mut self, to: u16, from: u16) -> bool {
        let layers = self.get(from).map_or_else(|| Keymap::new(0, "").layers, |k| k.layers.clone());
        if self.get(to).is_none() {
            return false;
        }
        self.edit(to, |k| replace(&mut k.layers, layers));
        true
    }

    /// A name no keymap has: `name`, or else `name` with a number after it. It is cleaned up the way the
    /// file name is (see clamshell-system's `store`): a `/` becomes `-`, and it is cut at 64 characters.
    pub fn unique_name(&self, name: &str) -> String {
        let name: String = name.trim().replace('/', "-").chars().take(64).collect();
        let name = name.trim();
        let taken = |n: &str| self.list.iter().any(|k| k.name == n);
        if !taken(name) {
            return name.to_owned();
        }
        (2..).map(|i| format!("{name} {i}")).find(|n| !taken(n)).unwrap_or_default()
    }

    /// Adds a new, empty keymap named `name`, made unique, and returns its id.
    pub fn add(&mut self, name: &str) -> Option<u16> {
        let id = self.next_id()?;
        let name = self.unique_name(name);
        self.list.push(Keymap::new(id, &name));
        self.changed(id);
        Some(id)
    }

    /// Copies keymap `id` under the name `name`, made unique, and returns the copy's id.
    pub fn duplicate(&mut self, id: u16, name: &str) -> Option<u16> {
        let layers = self.get(id).map(|k| k.layers.clone())?;
        let copy = self.add(name)?;
        self.edit(copy, |k| replace(&mut k.layers, layers));
        Some(copy)
    }

    /// Renames keymap `id`. A name another keymap has is made unique. Returns whether the keymap was there.
    pub fn rename(&mut self, id: u16, name: &str) -> bool {
        if self.get(id).is_some_and(|k| k.name == name.trim()) {
            return true;
        }
        let name = self.unique_name(name);
        if self.get(id).is_none() {
            return false;
        }
        self.edit(id, |k| replace(&mut k.name, name));
        true
    }

    pub fn remove(&mut self, id: u16) -> bool {
        let before = self.list.len();
        self.list.retain(|k| k.id != id);
        self.versions.retain(|(k, _)| *k != id);
        self.list.len() != before
    }
}

/// ClamShell's own keymaps for its runners, by name. `viewer` is for Text, PDF and Images, which run
/// programs that read the keyboard rather than the pad. It maps the D-pad to the arrow keys, A to Space
/// (forward a page), B to `q` (quit), and the shoulders to Page Up and Page Down.
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
