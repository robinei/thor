//! The keymaps' screens on the second screen, over the settings tab: the picker (for a game: its own
//! keymap, or the row that makes one from the keymap in force; then Default, then the named keymaps
//! with how many games use each, and New), a keymap's menu (Edit, Rename, Duplicate, Delete, deleting one
//! in use after a confirmation; a game's own, which stays while another is chosen: Edit, Load from (Default
//! or a named keymap copied into it, then its edit screen, where Undo goes back), Save as (a copy, as a new
//! named keymap or over one, which asks first), Delete), and the edit screen: the pad's inputs in one layer of the
//! keymap ([`EditView`]; the layer button opens the list of layers: show one, add one, a long press for
//! rename, clear and delete); an input chosen by touch or by pressing it on the pad, which listens
//! meanwhile, is selected into a panel with the ways to remap it that fit its kind ([`ways`]) and its
//! parameters. A way to remap opens what it needs, which closes as soon as something is chosen (Back gives
//! up): a control picked on the pad's drawing (tapped, or pressed on the pad), a key chord or a stick's
//! four keys typed on the keyboard, a part of the mouse on its drawing; the pointer, nothing and "as
//! before" apply at once. Names are typed in the editor. Also the keymap in force, sent to the pad
//! whenever it changes.

use crate::State;
use crate::editor::{Editor, KeymapName, Purpose};
use crate::effect::{Effect, Save};
use crate::id::{KeymapId, Target};
use crate::keyboard::{Key, Stroke};
use crate::keymap::{Actuation, Binding, Chord, Input, Keymap, Keymaps, Kind, Layer, Output, PadOut, Param, as_on_the_pad};
use crate::scroll::RowPart;
use crate::settings::Setting;
use crate::slider::{Control, Slider};

mod layers;
mod params;
mod undo;

pub use undo::Undo;

/// Which of the keymaps' screens is open.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Screen {
    /// Default and the keymaps; New.
    List,
    /// A keymap's long-press menu.
    Menu(u16),
    /// Deleting a keymap games use: are you sure.
    Delete(u16),
    /// Default and the named keymaps, one to copy into this (a game's own).
    Load(u16),
    /// A new named keymap and the named keymaps, to save this (a game's own) as.
    SaveAs(u16),
    /// Saving `own` over the named keymap `to`: are you sure.
    Overwrite { own: u16, to: u16 },
    /// A keymap's inputs: the pad listens.
    Edit(u16),
    /// The edit screen while the control `input` maps to is picked on the drawing, tapped or pressed on
    /// the pad (the pad listens).
    Pick { id: u16, input: Input },
    /// The edit screen while what holds the gyro on is picked on the drawing (tapped or pressed on the
    /// pad, which listens), or Always (`input`: the gyro).
    Holds { id: u16, input: Input },
    /// A part of the mouse for `input`, on its drawing.
    Mouse { id: u16, input: Input },
    /// A layer for `input`, from the keymap's.
    Layers { id: u16, input: Input },
    /// The keymap's layers, to show one on the edit screen (or add one; a long press: its menu).
    LayerPicker(u16),
    /// A layer's menu: rename, clear, delete (Base: clear).
    LayerMenu { id: u16, layer: Layer },
    /// A key chord typed on the keyboard for `input`; for a stick's four keys, which one and those so far.
    Capture { id: u16, input: Input, keys: Option<(usize, [Chord; 4])> },
}

impl Screen {
    /// Whether the pad listens: its presses choose inputs instead of moving the focus.
    pub fn listens(self) -> bool {
        matches!(self, Screen::Edit(_) | Screen::Pick { .. } | Screen::Holds { .. })
    }

    /// The keymap it is about, if one.
    pub fn keymap(self) -> Option<u16> {
        match self {
            Screen::List => None,
            Screen::Menu(id) | Screen::Delete(id) | Screen::Load(id) | Screen::SaveAs(id) | Screen::Edit(id) => Some(id),
            Screen::Overwrite { own, .. } => Some(own),
            Screen::Pick { id, .. } | Screen::Holds { id, .. } | Screen::Mouse { id, .. } | Screen::Capture { id, .. } => Some(id),
            Screen::Layers { id, .. } | Screen::LayerMenu { id, .. } | Screen::LayerPicker(id) => Some(id),
        }
    }
}

/// The ways to remap an input of `kind`, as the panel offers them: picking a control on the drawing,
/// typing keys, the mouse (its parts, or for motion the pointer), as before (`Default`), nothing. The
/// gyro's "as before" is off.
pub fn ways(kind: Kind) -> &'static [Target] {
    match kind {
        Kind::Digital | Kind::Trigger => &[Target::Pick, Target::Key, Target::MouseParts, Target::Layers, Target::Default, Target::Nothing],
        Kind::Stick => &[Target::Pick, Target::Keys, Target::Pointer, Target::Default, Target::Nothing],
        Kind::Gyro => &[Target::Pick, Target::Pointer, Target::Default],
    }
}

/// The way an input mapped to `output` was remapped (none: it isn't; `Default`).
pub fn way_of(output: Option<Output>) -> Target {
    match output {
        None => Target::Default,
        Some(Output::Nothing) => Target::Nothing,
        Some(Output::Pad(_) | Output::Stick(_) | Output::Trigger(_) | Output::Dpad) => Target::Pick,
        Some(Output::Key(_)) => Target::Key,
        Some(Output::Keys(_)) => Target::Keys,
        Some(Output::Mouse(_) | Output::Wheel(_)) => Target::MouseParts,
        Some(Output::Pointer) => Target::Pointer,
        Some(Output::Layer(_)) => Target::Layers,
    }
}

/// Whether `input` can hold the gyro on (a button or a trigger the Thor has).
pub fn holds(input: Input) -> bool {
    crate::keymap::gyro_holds().any(|h| h == input) && !matches!(input, Input::Button(b) if b.is_back())
}

/// What picking `tapped` on the pad's drawing (or pressing it) maps `source` to, if that fits it: a
/// button or the D-pad's direction for a button; a trigger to a trigger; a stick (or a direction of it) to
/// that stick and the D-pad to the D-pad for a stick or the gyro.
pub fn picked(source: Input, tapped: Input) -> Option<Output> {
    let kind = source.kind();
    let out = match (kind, tapped) {
        (Kind::Stick | Kind::Gyro, Input::Stick(s) | Input::StickDir(s, _)) => Output::Stick(s),
        (Kind::Stick, Input::Dpad(_)) => Output::Dpad,
        (Kind::Trigger, Input::Trigger(s)) => Output::Trigger(s),
        (_, t) => Output::Pad(PadOut::of(t)?),
    };
    out.fits(kind).then_some(out)
}

/// The edit screen's own state, kept while its input's screens are open: the layer shown and edited, the
/// input selected (shown in the panel) and which of its actuations, whether "No changes" waits for its
/// confirmation, and the slider of one of its parameters while a finger is on it.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct EditView {
    pub layer: Layer,
    pub selected: Option<Input>,
    pub act: Actuation,
    pub clearing: bool,
    pub slider: Option<Slider>,
}

/// A row of the picker.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PickerRow<'a> {
    /// The game's own keymap; none: the row that makes it.
    Own(Option<&'a Keymap>),
    /// Saving a game's own: as a new named keymap.
    New,
    Default,
    Named(&'a Keymap),
}

/// Who uses a keymap: all games (it is their default), and how many games of their own.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Users {
    pub all_games: bool,
    pub games: usize,
}

impl Users {
    pub fn any(self) -> bool {
        self.all_games || self.games > 0
    }
}

impl State {
    pub fn keymaps(&self) -> &Keymaps {
        &self.keymaps
    }

    /// The keymaps' screen open over the settings tab, if one is.
    pub fn keymap_screen(&self) -> Option<Screen> {
        self.bottom.keymaps
    }

    /// The edit screen's layer, selection and confirmation.
    pub fn keymap_edit(&self) -> EditView {
        self.bottom.keymap_edit
    }

    /// What the pad's Back button does on ClamShell's screens, as the target it activates: in the editor,
    /// Cancel; on the keymaps' screens, back one (picking a control: give up). (Elsewhere nothing has a
    /// back of its own. On the edit screen while the pad listens, Back comes as an input to remap.)
    pub fn pad_back(&self) -> Option<crate::Id> {
        if self.bottom.editor().is_some() {
            return Some(crate::Id::Editor(crate::EditorId::Cancel));
        }
        if (self.dock.menu.is_some() || self.dock.moving.is_some()) && self.bottom.tab == crate::Tab::Dock {
            return Some(crate::Id::Dock(crate::DockId::Back));
        }
        self.keymaps_shown().map(|_| crate::Id::Keymap(KeymapId::Back))
    }

    /// The keymaps' screen open, while the settings (its tab) are shown.
    fn keymaps_shown(&self) -> Option<Screen> {
        self.bottom.keymaps.filter(|_| self.bottom.tab == crate::Tab::Settings)
    }

    /// Whether the pad listens for inputs (the edit screen, or waiting for a pad button, in view).
    pub(crate) fn listening(&self) -> bool {
        self.bottom.editor().is_none() && self.keymaps_shown().is_some_and(Screen::listens)
    }

    /// The picker's rows: for a game, its own keymap (or the row making one), then Default and the named
    /// keymaps. Loading into a game's own: Default and the named keymaps; saving it: a new one and the named
    /// keymaps.
    fn keymap_rows(&self) -> impl Iterator<Item = PickerRow<'_>> {
        let (loading, saving) = match self.bottom.keymaps {
            Some(Screen::Load(_)) => (true, false),
            Some(Screen::SaveAs(_)) => (false, true),
            _ => (false, false),
        };
        let own = self.editing().filter(|_| !loading && !saving).map(|g| PickerRow::Own(self.keymaps.own_of(&g.id)));
        let new = saving.then_some(PickerRow::New);
        let default = (!saving).then_some(PickerRow::Default);
        own.into_iter().chain(new).chain(default).chain(self.keymaps.named().map(PickerRow::Named))
    }

    pub(crate) fn keymap_rows_len(&self) -> usize {
        self.keymap_rows().count()
    }

    pub fn keymap_row(&self, i: usize) -> Option<PickerRow<'_>> {
        self.keymap_rows().nth(i)
    }

    /// Whether keymap `id` is a game's own.
    pub fn keymap_is_own(&self, id: u16) -> bool {
        self.keymaps.get(id).is_some_and(|k| k.owner.is_some())
    }

    /// Whether the panel edits a game without a keymap of its own (which it can make).
    pub fn keymap_can_make_own(&self) -> bool {
        self.editing().is_some_and(|g| self.keymaps.own_of(&g.id).is_none())
    }

    /// The keymap the settings panel's scope uses (0: Default).
    pub fn keymap_chosen(&self) -> u16 {
        self.tile_value(Setting::Keymap)
    }

    /// A keymap's name, "Default" for none (or one that is gone).
    pub fn keymap_name(&self, id: u16) -> &str {
        match self.keymaps.get(id) {
            Some(k) if k.owner.is_some() => "Own keymap",
            Some(k) => k.name.as_str(),
            None => "Default",
        }
    }

    /// Who uses keymap `id`.
    pub fn keymap_users(&self, id: u16) -> Users {
        let all_games = self.settings.global.get(Setting::Keymap) == id;
        let games = self.settings.overrides.values().filter(|o| o.get(Setting::Keymap) == Some(id)).count();
        Users { all_games, games }
    }

    /// The keymap in force (the running game's, else all games'); Default (id 0, empty) if none.
    pub fn keymap_in_force(&self) -> Keymap {
        let id = self.in_force().get(Setting::Keymap);
        self.keymaps.get(id).cloned().unwrap_or_else(|| Keymap::new(0, "Default"))
    }

    /// The keymap in force for the pad, if it changed since last sent (or `always`).
    pub(crate) fn follow_keymap(&mut self, always: bool) -> Option<Effect> {
        let now = self.keymap_in_force();
        if !always && self.keymap_sent.as_ref() == Some(&now) {
            return None;
        }
        self.keymap_sent = Some(now.clone());
        Some(Effect::Keymap(now))
    }

    /// The keymap tile: the picker opens.
    pub(crate) fn open_keymaps(&mut self) {
        self.bottom.keymaps = Some(Screen::List);
    }

    /// A row of the picker: a tap chooses it for the scope being edited (and closes the picker), a long
    /// press opens a keymap's menu; its Edit button its edit screen. The game's row without its own keymap
    /// makes it.
    pub(crate) fn on_keymap_row(&mut self, i: usize, part: RowPart, long: bool) -> Vec<Effect> {
        match self.bottom.keymaps {
            Some(Screen::Load(own)) => {
                return match self.keymap_row(i) {
                    Some(PickerRow::Default) if !long => self.load_into(own, 0),
                    Some(PickerRow::Named(k)) if !long => self.load_into(own, k.id),
                    _ => vec![],
                };
            }
            Some(Screen::SaveAs(own)) => {
                match self.keymap_row(i) {
                    Some(PickerRow::New) if !long => self.name_keymap(KeymapName::SaveAs(own), String::new()),
                    Some(PickerRow::Named(k)) if !long => self.bottom.keymaps = Some(Screen::Overwrite { own, to: k.id }),
                    _ => {}
                }
                return vec![];
            }
            _ => {}
        }
        let (id, keymap) = match self.keymap_row(i) {
            None | Some(PickerRow::New) => return vec![],
            Some(PickerRow::Own(None)) => return self.make_own(self.keymap_chosen()),
            Some(PickerRow::Default) => (0, false),
            Some(PickerRow::Own(Some(k)) | PickerRow::Named(k)) => (k.id, true),
        };
        match (part, long) {
            (RowPart::Edit, _) if keymap => {
                self.open_edit(id);
                vec![]
            }
            (_, true) if keymap => {
                self.bottom.keymaps = Some(Screen::Menu(id));
                vec![]
            }
            (_, false) => {
                self.bottom.keymaps = None;
                self.change(Setting::Keymap, Some(id))
            }
            _ => vec![],
        }
    }

    /// Keymap `from` copied into `own` (a game's own); its edit screen opens, where Undo goes back to it
    /// as it was.
    fn load_into(&mut self, own: u16, from: u16) -> Vec<Effect> {
        self.open_edit(own);
        let mut effects = vec![];
        if self.keymaps.copy_into(own, from) {
            effects.push(self.save_keymaps());
            effects.extend(self.follow_keymap(false));
        }
        effects
    }

    /// The game the panel edits (without one) gets its own keymap, a copy of keymap `from`, chosen there;
    /// its edit screen opens.
    fn make_own(&mut self, from: u16) -> Vec<Effect> {
        if !self.keymap_can_make_own() {
            return vec![];
        }
        let Some(game) = self.editing().map(|g| g.id.clone()) else {
            return vec![];
        };
        let own = self.keymaps.make_own(from, &game);
        let mut effects = self.change(Setting::Keymap, Some(own));
        self.open_edit(own);
        effects.push(self.save_keymaps());
        effects
    }

    /// The keymaps' screens' buttons.
    pub(crate) fn on_keymap(&mut self, id: KeymapId, long: bool) -> Vec<Effect> {
        let Some(screen) = self.bottom.keymaps else {
            return vec![];
        };
        match (id, screen) {
            (KeymapId::Back, _) => {
                self.keymap_back();
                vec![]
            }
            (KeymapId::New, Screen::List) => {
                self.name_keymap(KeymapName::New, String::new());
                vec![]
            }
            (KeymapId::Edit, Screen::Menu(k)) => {
                self.open_edit(k);
                vec![]
            }
            (KeymapId::Rename, Screen::Menu(k)) => {
                let name = self.keymap_name(k).to_owned();
                self.name_keymap(KeymapName::Rename(k), name);
                vec![]
            }
            (KeymapId::Duplicate, Screen::Menu(k)) => {
                let name = self.keymaps.unique_name(self.keymap_name(k));
                self.name_keymap(KeymapName::Duplicate(k), name);
                vec![]
            }
            (KeymapId::LoadFrom, Screen::Menu(k)) if self.keymap_is_own(k) => {
                self.bottom.keymaps = Some(Screen::Load(k));
                vec![]
            }
            (KeymapId::SaveAs, Screen::Menu(k)) if self.keymap_is_own(k) => {
                self.bottom.keymaps = Some(Screen::SaveAs(k));
                vec![]
            }
            (KeymapId::ConfirmSave, Screen::Overwrite { own, to }) => {
                self.bottom.keymaps = Some(Screen::List);
                if !self.keymaps.copy_into(to, own) {
                    return vec![];
                }
                let mut effects = vec![self.save_keymaps()];
                effects.extend(self.follow_keymap(false));
                effects
            }
            (KeymapId::MakeOwn, Screen::Edit(k)) => self.make_own(k),
            (KeymapId::Delete, Screen::Menu(k)) if self.keymap_users(k).any() => {
                self.bottom.keymaps = Some(Screen::Delete(k));
                vec![]
            }
            (KeymapId::Delete, Screen::Menu(k)) | (KeymapId::ConfirmDelete, Screen::Delete(k)) => self.delete_keymap(k),
            (KeymapId::Input(input), Screen::Edit(k)) if long => {
                self.bottom.keymap_edit.selected = Some(input);
                self.bind(k, input, None)
            }
            (KeymapId::Input(input), Screen::Edit(k)) => self.select(k, input),
            (KeymapId::Input(tapped), Screen::Pick { id: k, input }) => match picked(input, tapped) {
                Some(output) => self.choose(k, input, output),
                None => vec![],
            },
            (KeymapId::Actuation(act), Screen::Edit(_)) => {
                let view = &mut self.bottom.keymap_edit;
                if view.selected.is_some_and(|i| act.fits(i.kind())) {
                    view.act = act;
                }
                vec![]
            }
            (KeymapId::Undo, Screen::Edit(k)) => self.undo(k),
            (KeymapId::EditLayer(layer), Screen::LayerPicker(k)) => {
                self.bottom.keymap_edit.clearing = false;
                self.bottom.keymaps = Some(Screen::LayerMenu { id: k, layer });
                vec![]
            }
            (KeymapId::LayerPicker, Screen::Edit(k)) => {
                self.bottom.keymaps = Some(Screen::LayerPicker(k));
                vec![]
            }
            (KeymapId::Layer(layer), Screen::LayerPicker(k)) if long => {
                self.bottom.keymap_edit.clearing = false;
                self.bottom.keymaps = Some(Screen::LayerMenu { id: k, layer });
                vec![]
            }
            (KeymapId::Layer(layer), Screen::LayerPicker(k)) => {
                self.bottom.keymap_edit.layer = layer;
                self.bottom.keymaps = Some(Screen::Edit(k));
                vec![]
            }
            (KeymapId::NewLayer, Screen::LayerPicker(k)) => {
                self.name_keymap(KeymapName::NewLayer(k, None), String::new());
                vec![]
            }
            (KeymapId::Layer(layer), Screen::Edit(_)) => {
                self.bottom.keymap_edit.layer = layer;
                self.bottom.keymap_edit.clearing = false;
                vec![]
            }
            (KeymapId::RenameLayer, Screen::LayerMenu { id: k, layer }) => {
                let name = self.keymaps.get(k).and_then(|km| km.layer_name(layer)).unwrap_or("").to_owned();
                self.name_keymap(KeymapName::RenameLayer(k, layer), name);
                vec![]
            }
            (KeymapId::LayerOnce, Screen::LayerMenu { id: k, layer }) => {
                let changed = self.keymaps.get_mut(k).is_some_and(|km| {
                    let once = !km.layer_once(layer);
                    km.set_layer_once(layer, once)
                });
                if changed { vec![self.save_keymaps()] } else { vec![] }
            }
            (KeymapId::DeleteLayer, Screen::LayerMenu { id: k, layer }) if layer != Layer::BASE => {
                self.bottom.keymaps = Some(Screen::LayerPicker(k));
                if self.bottom.keymap_edit.layer == layer {
                    self.bottom.keymap_edit.layer = Layer::BASE;
                }
                let changed = self.keymaps.get_mut(k).is_some_and(|km| km.remove_layer(layer));
                if changed { vec![self.save_keymaps()] } else { vec![] }
            }
            (KeymapId::ToLayer(layer), Screen::Layers { id: k, input }) => self.choose(k, input, Output::Layer(layer)),
            (KeymapId::NewLayer, Screen::Layers { id: k, input }) => {
                let act = self.bottom.keymap_edit.act;
                self.name_keymap(KeymapName::NewLayer(k, Some((input, act))), String::new());
                vec![]
            }
            (KeymapId::Clear, Screen::LayerMenu { .. }) => {
                self.bottom.keymap_edit.clearing = true;
                vec![]
            }
            (KeymapId::ConfirmClear, Screen::LayerMenu { id: k, layer }) => {
                self.bottom.keymap_edit.clearing = false;
                self.bottom.keymaps = Some(Screen::LayerPicker(k));
                let changed = self.keymaps.get_mut(k).is_some_and(|k| k.clear(layer));
                if changed { vec![self.save_keymaps()] } else { vec![] }
            }
            (KeymapId::Target(target), Screen::Edit(k)) => match self.bottom.keymap_edit.selected {
                Some(input) => self.map_to(k, input, target),
                None => vec![],
            },
            (KeymapId::Target(target), Screen::Pick { id: k, input } | Screen::Mouse { id: k, input }) => self.map_to(k, input, target),
            (KeymapId::Toggle(param), Screen::Edit(k)) => match self.param_binding(param) {
                Some((_, input, mut b)) => {
                    let on = param.get(&b) > 0;
                    param.set(&mut b, u8::from(!on));
                    let changed = self.set_tuned(k, input, b);
                    if changed { vec![self.save_keymaps()] } else { vec![] }
                }
                None => vec![],
            },
            (KeymapId::Calibrate, Screen::Edit(_)) => {
                self.notify(crate::hardware::NoticeKind::Gyro(None));
                vec![Effect::CalibrateGyro]
            }
            (KeymapId::Holds, Screen::Edit(k)) => {
                let input = self.bottom.keymap_edit.selected.filter(|i| *i == Input::Gyro);
                if let Some(input) = input.filter(|i| self.param_base(k, *i).is_some_and(|b| b.output != Output::Nothing)) {
                    self.bottom.keymaps = Some(Screen::Holds { id: k, input });
                }
                vec![]
            }
            (KeymapId::Hold(hold), Screen::Holds { id: k, input }) => self.set_hold(k, input, hold),
            (KeymapId::Input(tapped), Screen::Holds { id: k, input }) if holds(tapped) => self.set_hold(k, input, Some(tapped)),
            (KeymapId::Done, Screen::Capture { .. }) => {
                let chord = self.keyboard.take_modifiers();
                self.captured(chord)
            }
            _ => vec![],
        }
    }

    /// Back one screen: from a keymap's screens to the picker, from an input's to its keymap's, from
    /// the picker to the settings.
    pub(crate) fn keymap_back(&mut self) {
        // Clearing a layer waiting for its confirmation: not after all.
        if matches!(self.bottom.keymaps, Some(Screen::LayerMenu { .. })) && self.bottom.keymap_edit.clearing {
            self.bottom.keymap_edit.clearing = false;
            return;
        }
        self.bottom.keymaps = match self.bottom.keymaps {
            None | Some(Screen::List) => None,
            Some(Screen::Menu(_) | Screen::Delete(_) | Screen::Edit(_)) => Some(Screen::List),
            Some(Screen::Load(id) | Screen::SaveAs(id)) => Some(Screen::Menu(id)),
            Some(Screen::Overwrite { own, .. }) => Some(Screen::SaveAs(own)),
            Some(Screen::Pick { id, .. } | Screen::Holds { id, .. } | Screen::Mouse { id, .. }) => Some(Screen::Edit(id)),
            Some(Screen::Layers { id, .. } | Screen::LayerPicker(id)) => Some(Screen::Edit(id)),
            Some(Screen::LayerMenu { id, .. }) => Some(Screen::LayerPicker(id)),
            Some(Screen::Capture { id, .. }) => {
                self.keyboard.take_modifiers();
                Some(Screen::Edit(id))
            }
        };
    }

    /// Whether the selected input has a hold's or a double's binding in force in the layer shown (its press
    /// then acts only once it is known not to be one).
    pub fn press_waits(&self, id: u16, input: Input) -> bool {
        let on = self.layers_shown();
        self.keymaps.get(id).is_some_and(|k| [Actuation::Hold, Actuation::Double].into_iter().any(|a| k.lookup_of(input, a, on.as_slice()).is_some()))
    }

    /// An input chosen on the edit screen (touched, or pressed on the pad): selected into the panel; the
    /// one selected already, the way it is mapped now opens again (a key: the keyboard), if it has one.
    fn select(&mut self, id: u16, input: Input) -> Vec<Effect> {
        let view = &mut self.bottom.keymap_edit;
        view.clearing = false;
        if view.selected != Some(input) {
            view.selected = Some(input);
            view.act = Actuation::Press;
            return vec![];
        }
        match way_of(self.binding_shown(id, input).map(|b| b.output)) {
            way @ (Target::Pick | Target::Key | Target::Keys | Target::MouseParts) => self.map_to(id, input, way),
            _ => vec![],
        }
    }

    /// An input pressed on the pad while it listens: on the edit screen, it is selected as a touch
    /// selects it; while a control is picked, it is the one (if it fits).
    pub(crate) fn listened(&mut self, input: Input) -> Vec<Effect> {
        if self.pad.route() != crate::PadRoute::Listen {
            return vec![];
        }
        match self.bottom.keymaps {
            Some(Screen::Edit(id)) => self.select(id, input),
            Some(Screen::Pick { id, input: to }) => match picked(to, input) {
                Some(out) => self.choose(id, to, out),
                None => vec![],
            },
            Some(Screen::Holds { id, input: gyro }) if holds(input) => self.set_hold(id, gyro, Some(input)),
            _ => vec![],
        }
    }

    /// What holds the gyro on, chosen (none: always on): back to the edit screen.
    fn set_hold(&mut self, id: u16, gyro: Input, hold: Option<Input>) -> Vec<Effect> {
        self.bottom.keymaps = Some(Screen::Edit(id));
        let Some(mut b) = self.param_base(id, gyro) else {
            return vec![];
        };
        b.hold = hold;
        if self.set_tuned(id, gyro, b) { vec![self.save_keymaps()] } else { vec![] }
    }

    /// Opens the editor for a keymap's name.
    fn name_keymap(&mut self, action: KeymapName, text: String) {
        self.bottom.open_editor(Editor { purpose: Purpose::KeymapName(action), text });
    }

    /// The editor saved a keymap's name: a new keymap (then its edit screen), renamed, or a copy.
    pub(crate) fn named_keymap(&mut self, action: KeymapName, name: &str) -> Vec<Effect> {
        if name.trim().is_empty() {
            return vec![];
        }
        match action {
            KeymapName::New => {
                let id = self.keymaps.add(name);
                self.open_edit(id);
            }
            KeymapName::Rename(id) => {
                self.keymaps.rename(id, name);
                self.bottom.keymaps = Some(Screen::List);
            }
            KeymapName::Duplicate(id) => {
                self.keymaps.duplicate(id, name);
                self.bottom.keymaps = Some(Screen::List);
            }
            KeymapName::SaveAs(id) => {
                self.keymaps.duplicate(id, name);
                self.bottom.keymaps = Some(Screen::List);
            }
            KeymapName::NewLayer(id, for_input) => {
                let layer = self.keymaps.get_mut(id).and_then(|k| k.add_layer(name));
                self.bottom.keymaps = Some(Screen::Edit(id));
                match (layer, for_input) {
                    // Made from the layers' list: the input maps to it (in the layer shown).
                    (Some(l), Some((input, act))) => {
                        self.bottom.keymap_edit.act = act;
                        let mut effects = self.choose(id, input, Output::Layer(l));
                        effects.push(self.save_keymaps());
                        return effects;
                    }
                    // From the +: shown.
                    (Some(l), None) => self.bottom.keymap_edit.layer = l,
                    (None, _) => {}
                }
            }
            KeymapName::RenameLayer(id, layer) => {
                self.keymaps.get_mut(id).map(|k| k.rename_layer(layer, name));
                self.bottom.keymaps = Some(Screen::LayerPicker(id));
            }
        }
        vec![self.save_keymaps()]
    }

    /// Deletes keymap `id`: the games that used it (and all games, if theirs) are back on Default.
    fn delete_keymap(&mut self, id: u16) -> Vec<Effect> {
        let before = self.in_force();
        self.keymaps.remove(id);
        if self.settings.global.get(Setting::Keymap) == id {
            self.settings.global.set(Setting::Keymap, 0);
        }
        let games: Vec<_> = self.settings.overrides.iter().filter(|(_, o)| o.get(Setting::Keymap) == Some(id)).map(|(g, _)| g.clone()).collect();
        let global = self.settings.global.get(Setting::Keymap);
        for game in games {
            // Default: their own choice of it, unless all games' is Default anyway.
            self.settings.set(Some(&game), Setting::Keymap, (global != 0).then_some(0));
        }
        self.bottom.keymaps = Some(Screen::List);
        let mut effects = vec![self.save_keymaps()];
        effects.extend(self.saved_and_applied(before));
        effects
    }

    /// `input` of keymap `id` bound to `binding` in the layer and actuation shown (none: as on the pad, or
    /// as the main layer; no hold or double); saved if it changed.
    fn bind(&mut self, id: u16, input: Input, binding: Option<Binding>) -> Vec<Effect> {
        let EditView { layer, act, .. } = self.bottom.keymap_edit;
        let changed = self.keymaps.get_mut(id).is_some_and(|k| k.set_of(layer, input, act, binding));
        if changed { vec![self.save_keymaps()] } else { vec![] }
    }

    /// `input`'s binding in the layer and actuation shown.
    fn binding_shown(&self, id: u16, input: Input) -> Option<Binding> {
        let EditView { layer, act, .. } = self.bottom.keymap_edit;
        self.keymaps.get(id).and_then(|k| k.get_of(layer, input, act)).copied()
    }

    /// A way to remap `input` (a panel's icon): what it needs opens, or it applies at once; or an output
    /// chosen (the mouse's parts, the control pipe).
    fn map_to(&mut self, id: u16, input: Input, target: Target) -> Vec<Effect> {
        let screen = match target {
            Target::Pick => Screen::Pick { id, input },
            Target::Key => Screen::Capture { id, input, keys: None },
            Target::Keys => Screen::Capture { id, input, keys: Some((0, [Chord::default(); 4])) },
            Target::MouseParts => Screen::Mouse { id, input },
            Target::Layers => Screen::Layers { id, input },
            Target::Default => {
                self.bottom.keymaps = Some(Screen::Edit(id));
                return self.bind(id, input, None);
            }
            _ => return self.chosen(id, input, target),
        };
        // (Only the ways that fit it: a stick has no key chord of its own, the gyro no mouse buttons.)
        if ways(input.kind()).contains(&target) {
            self.bottom.keymaps = Some(screen);
        }
        vec![]
    }

    /// An output chosen for `input`, by the way it was asked for.
    fn chosen(&mut self, id: u16, input: Input, target: Target) -> Vec<Effect> {
        let output = match target {
            Target::Default | Target::Pick | Target::Key | Target::Keys | Target::MouseParts | Target::Layers => return vec![],
            Target::Nothing => Output::Nothing,
            Target::Pad(p) => Output::Pad(p),
            Target::Mouse(m) => Output::Mouse(m),
            Target::Wheel(d) => Output::Wheel(d),
            Target::Stick(s) => Output::Stick(s),
            Target::Pointer => Output::Pointer,
            Target::Dpad => Output::Dpad,
            Target::Trigger(s) => Output::Trigger(s),
        };
        self.choose(id, input, output)
    }

    /// `input` maps to `output` (its parameters and hold kept): back to the edit screen, whose panel
    /// has its parameters.
    fn choose(&mut self, id: u16, input: Input, output: Output) -> Vec<Effect> {
        let full_pull = self.bottom.keymap_edit.act == Actuation::FullPull;
        // (A full pull to a trigger picked: that trigger pulled all the way, as a button.)
        let output = match output {
            Output::Trigger(s) if full_pull => Output::Pad(PadOut::Trigger(s)),
            o => o,
        };
        if !output.fits(input.kind()) {
            return vec![];
        }
        let kept = self.binding_shown(id, input);
        let fresh = || Binding { threshold: if full_pull { crate::keymap::FULL_PULL } else { crate::keymap::DEFAULT_THRESHOLD }, ..Binding::of(input, output) };
        let binding = kept.map_or_else(fresh, |k| k.retarget(input, output));
        let effects = self.bind(id, input, Some(binding));
        self.bottom.keymaps = Some(Screen::Edit(id));
        effects
    }

    /// A key typed while a chord is wanted: the chord for the input, or the next of a stick's four keys
    /// (the fourth completes them).
    pub(crate) fn captured(&mut self, chord: Chord) -> Vec<Effect> {
        let Some(Screen::Capture { id, input, keys }) = self.bottom.keymaps else {
            return vec![];
        };
        if chord.is_empty() {
            return vec![];
        }
        match keys {
            None => self.choose(id, input, Output::Key(chord)),
            Some((n, mut so_far)) => {
                if let Some(slot) = so_far.get_mut(n) {
                    *slot = chord;
                }
                if n + 1 < so_far.len() {
                    self.bottom.keymaps = Some(Screen::Capture { id, input, keys: Some((n + 1, so_far)) });
                    return vec![];
                }
                self.choose(id, input, Output::Keys(so_far))
            }
        }
    }

    /// Whether the keyboard's keys make a chord now (the capture screen is open).
    pub(crate) fn capturing(&self) -> bool {
        matches!(self.keymaps_shown(), Some(Screen::Capture { .. })) && self.bottom.editor().is_none()
    }

    /// A stroke of the keyboard as a chord.
    pub(crate) fn chord(stroke: Stroke) -> Chord {
        let key = match stroke.key {
            Key::Mod(_) | Key::Caps => None,
            key => Some(key),
        };
        Chord { key, shift: stroke.shift, ctrl: stroke.ctrl, alt: stroke.alt, logo: stroke.logo }
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests;
