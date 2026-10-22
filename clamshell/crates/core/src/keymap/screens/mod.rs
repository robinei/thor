//! The keymaps' screens, shown over the settings tab on the second screen.
//!
//! - The picker lists, for a game, its own keymap or a row that makes one from the keymap in force. Then
//!   come Default, the named keymaps with how many games use each, and New.
//! - A keymap's menu has Edit, Rename, Duplicate and Delete; deleting a keymap in use asks first. A game's
//!   own keymap stays while another is chosen. Its menu has Edit, Load from, Save as and Delete. Load from
//!   copies Default or a named keymap into it and opens its edit screen, where Undo goes back. Save as
//!   copies it to a new named keymap, or over an existing one after asking.
//! - The edit screen shows the pad's inputs in one layer of the keymap ([`EditView`]). The layer button
//!   opens the list of layers: tap one to show it, add one, or long-press one to rename, clear or delete
//!   it. An input is chosen by touch or by pressing it on the pad, which listens while the screen is open.
//!   The chosen input is shown in a panel with its parameters and the ways to remap it that fit its kind
//!   ([`ways`]).
//!
//! A way to remap opens what it needs, which closes as soon as something is chosen; Back gives up. It is
//! one of: a control picked on the pad's drawing, by tapping it or pressing it on the pad; a key chord or
//! a stick's four keys typed on the keyboard; or a part of the mouse on its drawing. The pointer, Nothing
//! and no binding apply at once. No binding leaves the input as on the pad, or as the layers below. Names
//! are typed in the editor.
//!
//! This module also sends the keymap in force to the pad whenever it changes.

use crate::State;
use crate::bottom::editor::{Editor, KeymapName, Purpose};
use crate::bottom::keyboard::{Key, Stroke};
use crate::effect::{Effect, Save};
use crate::id::{KeymapId, Target};
use crate::keymap::{Actuation, Binding, Chord, Input, Keymap, Keymaps, Kind, Layer, Output, PadOut, Param, as_on_the_pad};
use crate::levels::slider::{Control, Slider};
use crate::nav::lists::RowPart;
use crate::settings::Setting;

mod layers;
mod params;
mod undo;

pub use undo::Undo;

/// Which of the keymaps' screens is open.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Screen {
    /// Default, the keymaps, and New.
    List,
    /// A keymap's long-press menu.
    Menu(u16),
    /// Asks whether to delete a keymap that games use.
    Delete(u16),
    /// Default and the named keymaps, to pick one to copy into this game's own keymap.
    Load(u16),
    /// A new named keymap and the named keymaps, to pick what to save this game's own keymap as.
    SaveAs(u16),
    /// Asks whether to save `own` over the named keymap `to`.
    Overwrite { own: u16, to: u16 },
    /// A keymap's inputs. The pad listens.
    Edit(u16),
    /// The edit screen while the control that `input` maps to is picked on the drawing, by tapping it or
    /// pressing it on the pad. The pad listens.
    Pick { id: u16, input: Input },
    /// The edit screen while what holds the gyro on is picked on the drawing, by tapping it or pressing it
    /// on the pad, or Always is chosen. The pad listens. `input` is the gyro.
    Holds { id: u16, input: Input },
    /// A part of the mouse for `input`, on its drawing.
    Mouse { id: u16, input: Input },
    /// A layer for `input`, from the keymap's layers.
    Layers { id: u16, input: Input },
    /// The keymap's layers, to show one on the edit screen or add one. A long press opens a layer's menu.
    LayerPicker(u16),
    /// A layer's menu: Once, rename, clear and delete. Base has only clear.
    LayerMenu { id: u16, layer: Layer },
    /// A key chord typed on the keyboard for `input`. For a stick's four keys, `keys` holds which one is
    /// next and the ones typed so far.
    Capture { id: u16, input: Input, keys: Option<(usize, [Chord; 4])> },
}

impl Screen {
    /// Whether the pad listens, so that its presses choose inputs instead of moving the focus.
    pub fn listens(self) -> bool {
        matches!(self, Screen::Edit(_) | Screen::Pick { .. } | Screen::Holds { .. })
    }

    /// The keymap it is about, if any.
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
/// typing keys, the mouse (its parts, or the pointer for motion), a layer, as before (`Default`), and
/// nothing. For the gyro, "as before" is off.
pub fn ways(kind: Kind) -> &'static [Target] {
    match kind {
        Kind::Digital | Kind::Trigger => &[Target::Pick, Target::Key, Target::MouseParts, Target::Layers, Target::Default, Target::To(Output::Nothing)],
        Kind::Stick => &[Target::Pick, Target::Keys, Target::To(Output::Pointer), Target::Default, Target::To(Output::Nothing)],
        Kind::Gyro => &[Target::Pick, Target::To(Output::Pointer), Target::Default],
    }
}

/// The way an input mapped to `output` was remapped. With no output it isn't remapped, which is
/// `Default`.
pub fn way_of(output: Option<Output>) -> Target {
    match output {
        None => Target::Default,
        Some(Output::Nothing) => Target::To(Output::Nothing),
        Some(Output::Pad(_) | Output::Stick(_) | Output::Trigger(_) | Output::Dpad) => Target::Pick,
        Some(Output::Key(_)) => Target::Key,
        Some(Output::Keys(_)) => Target::Keys,
        Some(Output::Mouse(_) | Output::Wheel(_)) => Target::MouseParts,
        Some(Output::Pointer) => Target::To(Output::Pointer),
        Some(Output::Layer(_)) => Target::Layers,
    }
}

/// Whether `input` can hold the gyro on: a button or a trigger that the Thor has.
pub fn holds(input: Input) -> bool {
    crate::keymap::gyro_holds().any(|h| h == input) && !matches!(input, Input::Button(b) if b.is_back())
}

/// What picking `tapped` on the pad's drawing, or pressing it, maps `source` to, if that fits `source`. A
/// button or trigger maps to the pad output picked: a button, a D-pad direction or a trigger, and a
/// trigger picked for a trigger maps to that trigger. A stick or the gyro maps to the stick picked, by the
/// stick or one of its directions, and a stick can also map to the D-pad.
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

/// The state of the keymaps' screens: the screen open over the settings tab, the edit screen's view, and
/// its Undo.
#[derive(Debug, Clone, Default)]
pub struct Screens {
    pub(crate) open: Option<Screen>,
    pub(crate) edit: EditView,
    pub(crate) undo: Undo,
}

/// The edit screen's own state, kept while the screens for its input are open. It holds the layer shown
/// and edited, the input selected into the panel and which of its actuations is shown, whether "No
/// changes" waits for confirmation, and the slider of one of the input's parameters while a finger is on
/// it.
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
    /// The game's own keymap, or none for the row that makes it.
    Own(Option<&'a Keymap>),
    /// When saving a game's own keymap: save it as a new named keymap.
    New,
    Default,
    Named(&'a Keymap),
}

/// Who uses a keymap: whether all games do, as their default, and how many games have chosen it as their
/// own.
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
        self.keymap_screens.open
    }

    /// The edit screen's layer, selection and confirmation.
    pub fn keymap_edit(&self) -> EditView {
        self.keymap_screens.edit
    }

    /// The keymaps' screen that is open, while the settings tab is shown.
    pub(crate) fn keymaps_shown(&self) -> Option<Screen> {
        self.keymap_screens.open.filter(|_| self.bottom.tab == crate::Tab::Settings)
    }

    /// Whether the pad listens for inputs: the edit screen, or a screen waiting for a pad button, is in
    /// view.
    pub(crate) fn listening(&self) -> bool {
        self.bottom.editor().is_none() && self.keymaps_shown().is_some_and(Screen::listens)
    }

    /// The picker's rows. Usually they are, for a game, its own keymap or the row that makes one, then
    /// Default and the named keymaps. When loading into a game's own keymap they are Default and the named
    /// keymaps; when saving it, a new keymap and the named keymaps.
    fn keymap_rows(&self) -> impl Iterator<Item = PickerRow<'_>> {
        let (loading, saving) = match self.keymap_screens.open {
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

    /// Whether the panel edits a game that has no keymap of its own and so can make one.
    pub fn keymap_can_make_own(&self) -> bool {
        self.editing().is_some_and(|g| self.keymaps.own_of(&g.id).is_none())
    }

    /// The keymap used by the settings panel's scope; 0 is Default.
    pub fn keymap_chosen(&self) -> u16 {
        self.tile_value(Setting::Keymap)
    }

    /// A keymap's name. It is "Own keymap" for a game's own, and "Default" for none or for one that is
    /// gone.
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

    /// The keymap in force: the running game's, or else all games'. It is Default (id 0, empty) if there
    /// is none.
    pub fn keymap_in_force(&self) -> Keymap {
        let id = self.in_force().get(Setting::Keymap);
        self.keymaps.get(id).cloned().unwrap_or_else(|| Keymap::new(0, "Default"))
    }

    /// The keymap in force, to send to the pad if it changed since it was last sent, or if `always`.
    pub(crate) fn follow_keymap(&mut self, always: bool) -> Option<Effect> {
        // Compared in place, since this runs after every event. Default, which has no stored keymap, is
        // id 0.
        let stored = self.keymaps.get(self.in_force().get(Setting::Keymap));
        let same = self.keymap_sent.as_ref().is_some_and(|sent| match stored {
            Some(k) => sent == k,
            None => sent.id == 0,
        });
        if same && !always {
            return None;
        }
        let now = self.keymap_in_force();
        self.keymap_sent = Some(now.clone());
        Some(Effect::Keymap(now))
    }

    /// The keymap tile opens the picker.
    pub(crate) fn open_keymaps(&mut self) {
        self.keymap_screens.open = Some(Screen::List);
    }

    /// A row of the picker. A tap chooses it for the scope being edited and closes the picker, a long
    /// press opens the keymap's menu, and its Edit button opens its edit screen. The game's row, when the
    /// game has no keymap of its own, makes one.
    pub(crate) fn on_keymap_row(&mut self, i: usize, part: RowPart, long: bool) -> Vec<Effect> {
        match self.keymap_screens.open {
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
                    Some(PickerRow::Named(k)) if !long => self.keymap_screens.open = Some(Screen::Overwrite { own, to: k.id }),
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
                self.keymap_screens.open = Some(Screen::Menu(id));
                vec![]
            }
            (_, false) => {
                self.keymap_screens.open = None;
                self.change(Setting::Keymap, Some(id))
            }
            _ => vec![],
        }
    }

    /// Copies keymap `from` into `own`, a game's own keymap, and opens its edit screen, where Undo puts it
    /// back as it was.
    fn load_into(&mut self, own: u16, from: u16) -> Vec<Effect> {
        self.open_edit(own);
        let mut effects = vec![];
        if self.keymaps.copy_into(own, from) {
            effects.push(self.save_keymaps());
            effects.extend(self.follow_keymap(false));
        }
        effects
    }

    /// Gives the game the panel edits, if it has none, its own keymap as a copy of keymap `from`. The new
    /// keymap is chosen for the game and its edit screen opens.
    fn make_own(&mut self, from: u16) -> Vec<Effect> {
        if !self.keymap_can_make_own() {
            return vec![];
        }
        let Some(game) = self.editing().map(|g| g.id.clone()) else {
            return vec![];
        };
        let Some(own) = self.keymaps.make_own(from, &game) else {
            return vec![];
        };
        let mut effects = self.change(Setting::Keymap, Some(own));
        self.open_edit(own);
        effects.push(self.save_keymaps());
        effects
    }

    /// The keymaps' screens' buttons.
    pub(crate) fn on_keymap(&mut self, id: KeymapId, long: bool) -> Vec<Effect> {
        let Some(screen) = self.keymap_screens.open else {
            return vec![];
        };
        debug_assert!(screen.keymap().is_none_or(|k| self.keymaps.get(k).is_some()), "the keymap of the screen open is there: {screen:?}");
        if id == KeymapId::Back {
            self.keymap_back();
            return vec![];
        }
        match screen {
            Screen::List if id == KeymapId::New => {
                self.name_keymap(KeymapName::New, String::new());
                vec![]
            }
            Screen::List => vec![],
            Screen::Menu(k) => self.on_keymap_menu(id, k),
            Screen::Overwrite { own, to } if id == KeymapId::ConfirmSave => {
                self.keymap_screens.open = Some(Screen::List);
                if !self.keymaps.copy_into(to, own) {
                    return vec![];
                }
                let mut effects = vec![self.save_keymaps()];
                effects.extend(self.follow_keymap(false));
                effects
            }
            Screen::Delete(k) if id == KeymapId::ConfirmDelete => self.delete_keymap(k),
            Screen::Edit(k) => self.on_edit_screen(id, k, long),
            Screen::LayerPicker(k) => self.on_layer_picker(id, k, long),
            Screen::LayerMenu { id: k, layer } => self.on_layer_menu(id, k, layer),
            Screen::Layers { id: k, input } => match id {
                KeymapId::ToLayer(layer) => self.choose(k, input, Output::Layer(layer)),
                KeymapId::NewLayer => {
                    let act = self.keymap_screens.edit.act;
                    self.name_keymap(KeymapName::NewLayer(k, Some((input, act))), String::new());
                    vec![]
                }
                _ => vec![],
            },
            Screen::Pick { id: k, input } => match id {
                KeymapId::Input(tapped) => picked(input, tapped).map(|output| self.choose(k, input, output)).unwrap_or_default(),
                KeymapId::Target(target) => self.map_to(k, input, target),
                _ => vec![],
            },
            Screen::Mouse { id: k, input } => match id {
                KeymapId::Target(target) => self.map_to(k, input, target),
                _ => vec![],
            },
            Screen::Holds { id: k, input } => match id {
                KeymapId::Hold(hold) => self.set_hold(k, input, hold),
                KeymapId::Input(tapped) if holds(tapped) => self.set_hold(k, input, Some(tapped)),
                _ => vec![],
            },
            Screen::Capture { .. } if id == KeymapId::Done => {
                let chord = self.keyboard.take_modifiers();
                self.captured(chord)
            }
            Screen::Load(_) | Screen::SaveAs(_) | Screen::Overwrite { .. } | Screen::Delete(_) | Screen::Capture { .. } => vec![],
        }
    }

    /// A keymap's menu (a long press on its row).
    fn on_keymap_menu(&mut self, id: KeymapId, k: u16) -> Vec<Effect> {
        match id {
            KeymapId::Edit => self.open_edit(k),
            KeymapId::Rename => {
                let name = self.keymap_name(k).to_owned();
                self.name_keymap(KeymapName::Rename(k), name);
            }
            KeymapId::Duplicate => {
                let name = self.keymaps.unique_name(self.keymap_name(k));
                self.name_keymap(KeymapName::Duplicate(k), name);
            }
            KeymapId::LoadFrom if self.keymap_is_own(k) => self.keymap_screens.open = Some(Screen::Load(k)),
            KeymapId::SaveAs if self.keymap_is_own(k) => self.keymap_screens.open = Some(Screen::SaveAs(k)),
            KeymapId::Delete if self.keymap_users(k).any() => self.keymap_screens.open = Some(Screen::Delete(k)),
            KeymapId::Delete => return self.delete_keymap(k),
            _ => {}
        }
        vec![]
    }

    /// The edit screen's buttons: selecting an input (a long press removes its binding), its actuation,
    /// its target and parameters, the layer shown, Undo, Make own, and the gyro's calibration and hold.
    fn on_edit_screen(&mut self, id: KeymapId, k: u16, long: bool) -> Vec<Effect> {
        match id {
            KeymapId::MakeOwn => self.make_own(k),
            KeymapId::Input(input) if long => {
                self.keymap_screens.edit.selected = Some(input);
                self.bind(k, input, None)
            }
            KeymapId::Input(input) => self.select(k, input),
            KeymapId::Actuation(act) => {
                let view = &mut self.keymap_screens.edit;
                if view.selected.is_some_and(|i| act.fits(i.kind())) {
                    view.act = act;
                }
                vec![]
            }
            KeymapId::Undo => self.undo(k),
            KeymapId::LayerPicker => {
                self.keymap_screens.open = Some(Screen::LayerPicker(k));
                vec![]
            }
            KeymapId::Layer(layer) => {
                self.keymap_screens.edit.layer = layer;
                self.keymap_screens.edit.clearing = false;
                vec![]
            }
            KeymapId::Target(target) => match self.keymap_screens.edit.selected {
                Some(input) => self.map_to(k, input, target),
                None => vec![],
            },
            KeymapId::Toggle(param) => match self.param_binding(param) {
                Some((_, input, mut b)) => {
                    let on = param.get(&b) > 0;
                    param.set(&mut b, u8::from(!on));
                    let changed = self.set_tuned(k, input, b);
                    self.saved_if(changed)
                }
                None => vec![],
            },
            KeymapId::Calibrate => {
                self.notify(crate::bottom::notice::NoticeKind::Gyro(None));
                vec![Effect::CalibrateGyro]
            }
            KeymapId::Holds => {
                let input = self.keymap_screens.edit.selected.filter(|i| *i == Input::Gyro);
                if let Some(input) = input.filter(|i| self.param_base(k, *i).is_some_and(|b| b.output != Output::Nothing)) {
                    self.keymap_screens.open = Some(Screen::Holds { id: k, input });
                }
                vec![]
            }
            _ => vec![],
        }
    }

    /// The layer picker: shows a layer on the edit screen, opens its menu on a long press, or adds a new
    /// one.
    fn on_layer_picker(&mut self, id: KeymapId, k: u16, long: bool) -> Vec<Effect> {
        match id {
            KeymapId::EditLayer(layer) => self.open_layer_menu(k, layer),
            KeymapId::Layer(layer) if long => self.open_layer_menu(k, layer),
            KeymapId::Layer(layer) => {
                self.keymap_screens.edit.layer = layer;
                self.keymap_screens.open = Some(Screen::Edit(k));
                vec![]
            }
            KeymapId::NewLayer => {
                self.name_keymap(KeymapName::NewLayer(k, None), String::new());
                vec![]
            }
            _ => vec![],
        }
    }

    /// A layer's menu: rename, Once, clear (after confirmation) and delete.
    fn on_layer_menu(&mut self, id: KeymapId, k: u16, layer: Layer) -> Vec<Effect> {
        match id {
            KeymapId::RenameLayer => {
                let name = self.keymaps.get(k).and_then(|km| km.layer_name(layer)).unwrap_or("").to_owned();
                self.name_keymap(KeymapName::RenameLayer(k, layer), name);
                vec![]
            }
            KeymapId::LayerOnce => {
                let changed = self.keymaps.get_mut(k).is_some_and(|km| {
                    let once = !km.layer_once(layer);
                    km.set_layer_once(layer, once)
                });
                self.saved_if(changed)
            }
            KeymapId::DeleteLayer if layer != Layer::BASE => {
                self.keymap_screens.open = Some(Screen::LayerPicker(k));
                if self.keymap_screens.edit.layer == layer {
                    self.keymap_screens.edit.layer = Layer::BASE;
                }
                let changed = self.keymaps.get_mut(k).is_some_and(|km| km.remove_layer(layer));
                self.saved_if(changed)
            }
            KeymapId::Clear => {
                self.keymap_screens.edit.clearing = true;
                vec![]
            }
            KeymapId::ConfirmClear => {
                self.keymap_screens.edit.clearing = false;
                self.keymap_screens.open = Some(Screen::LayerPicker(k));
                let changed = self.keymaps.get_mut(k).is_some_and(|km| km.clear(layer));
                self.saved_if(changed)
            }
            _ => vec![],
        }
    }

    /// Goes back one screen: from a keymap's screens to the picker, from an input's screens to its
    /// keymap's, and from the picker to the settings.
    pub(crate) fn keymap_back(&mut self) {
        // A layer clear waiting for confirmation is cancelled instead.
        if matches!(self.keymap_screens.open, Some(Screen::LayerMenu { .. })) && self.keymap_screens.edit.clearing {
            self.keymap_screens.edit.clearing = false;
            return;
        }
        self.keymap_screens.open = match self.keymap_screens.open {
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

    /// Whether the selected input has a hold or double binding in force in the layer shown. Its press then
    /// acts only once it is known not to be one of those.
    pub fn press_waits(&self, id: u16, input: Input) -> bool {
        let on = self.layers_shown();
        self.keymaps.get(id).is_some_and(|k| [Actuation::Hold, Actuation::Double].into_iter().any(|a| k.lookup_of(input, a, on.as_slice()).is_some()))
    }

    /// Selects an input chosen on the edit screen, by touch or by pressing it on the pad, into the panel.
    /// If it is already selected, the way it is mapped now opens again, if it has one; for a key, that is
    /// the keyboard.
    fn select(&mut self, id: u16, input: Input) -> Vec<Effect> {
        let view = &mut self.keymap_screens.edit;
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

    /// An input pressed on the pad while it listens. On the edit screen it is selected as a touch would
    /// select it. While a control or the gyro's hold is being picked, it is the one picked, if it fits.
    pub(crate) fn listened(&mut self, input: Input) -> Vec<Effect> {
        if self.pad.route() != crate::PadRoute::Listen {
            return vec![];
        }
        match self.keymap_screens.open {
            Some(Screen::Edit(id)) => self.select(id, input),
            Some(Screen::Pick { id, input: to }) => match picked(to, input) {
                Some(out) => self.choose(id, to, out),
                None => vec![],
            },
            Some(Screen::Holds { id, input: gyro }) if holds(input) => self.set_hold(id, gyro, Some(input)),
            _ => vec![],
        }
    }

    /// Sets what holds the gyro on, with none meaning always on, and goes back to the edit screen.
    fn set_hold(&mut self, id: u16, gyro: Input, hold: Option<Input>) -> Vec<Effect> {
        self.keymap_screens.open = Some(Screen::Edit(id));
        let Some(mut b) = self.param_base(id, gyro) else {
            return vec![];
        };
        b.hold = hold;
        let changed = self.set_tuned(id, gyro, b);
        self.saved_if(changed)
    }

    /// Opens the editor for a keymap's name.
    fn name_keymap(&mut self, action: KeymapName, text: String) {
        self.bottom.open_editor(Editor { purpose: Purpose::KeymapName(action), text });
    }

    /// The editor saved a name: for a new keymap, which then opens on its edit screen, a renamed keymap, a
    /// copy, or a new or renamed layer.
    pub(crate) fn named_keymap(&mut self, action: KeymapName, name: &str) -> Vec<Effect> {
        if name.trim().is_empty() {
            return vec![];
        }
        match action {
            KeymapName::New => {
                if let Some(id) = self.keymaps.add(name) {
                    self.open_edit(id);
                }
            }
            KeymapName::Rename(id) => {
                self.keymaps.rename(id, name);
                self.keymap_screens.open = Some(Screen::List);
            }
            KeymapName::Duplicate(id) | KeymapName::SaveAs(id) => {
                self.keymaps.duplicate(id, name);
                self.keymap_screens.open = Some(Screen::List);
            }
            KeymapName::NewLayer(id, for_input) => {
                let layer = self.keymaps.get_mut(id).and_then(|k| k.add_layer(name));
                self.keymap_screens.open = Some(Screen::Edit(id));
                match (layer, for_input) {
                    // Made from the list of layers for an input: the input maps to the new layer, in the
                    // layer shown.
                    (Some(l), Some((input, act))) => {
                        self.keymap_screens.edit.act = act;
                        let mut effects = self.choose(id, input, Output::Layer(l));
                        effects.push(self.save_keymaps());
                        return effects;
                    }
                    // Made with the + button: the new layer is shown.
                    (Some(l), None) => self.keymap_screens.edit.layer = l,
                    (None, _) => {}
                }
            }
            KeymapName::RenameLayer(id, layer) => {
                self.keymaps.get_mut(id).map(|k| k.rename_layer(layer, name));
                self.keymap_screens.open = Some(Screen::LayerPicker(id));
            }
        }
        vec![self.save_keymaps()]
    }

    /// Deletes keymap `id`. The games that used it, and all games if it was theirs, go back to Default.
    fn delete_keymap(&mut self, id: u16) -> Vec<Effect> {
        let before = self.in_force();
        self.keymaps.remove(id);
        if self.settings.global.get(Setting::Keymap) == id {
            self.settings.global.set(Setting::Keymap, 0);
        }
        let games: Vec<_> = self.settings.overrides.iter().filter(|(_, o)| o.get(Setting::Keymap) == Some(id)).map(|(g, _)| g.clone()).collect();
        let global = self.settings.global.get(Setting::Keymap);
        for game in games {
            // The game gets Default as its own choice, unless all games use Default anyway, in which case
            // its own choice is cleared.
            self.settings.set(Some(&game), Setting::Keymap, (global != 0).then_some(0));
        }
        self.keymap_screens.open = Some(Screen::List);
        let mut effects = vec![self.save_keymaps()];
        effects.extend(self.saved_and_applied(before));
        effects
    }

    /// The menu of keymap `id`'s `layer` (see [`Screen::LayerMenu`]).
    fn open_layer_menu(&mut self, id: u16, layer: Layer) -> Vec<Effect> {
        self.keymap_screens.edit.clearing = false;
        self.keymap_screens.open = Some(Screen::LayerMenu { id, layer });
        vec![]
    }

    /// Binds `input` of keymap `id` to `binding` in the layer and actuation shown, and saves the keymaps
    /// if that changed them. `None` removes the binding: the input then acts as on the pad, or as the
    /// layers below, and a hold or double does nothing.
    fn bind(&mut self, id: u16, input: Input, binding: Option<Binding>) -> Vec<Effect> {
        let EditView { layer, act, .. } = self.keymap_screens.edit;
        debug_assert!(self.keymaps.get(id).is_some(), "keymap {id}, being edited, is there");
        let changed = self.keymaps.get_mut(id).is_some_and(|k| k.set_of(layer, input, act, binding));
        self.saved_if(changed)
    }

    /// `input`'s binding in the layer and actuation shown.
    fn binding_shown(&self, id: u16, input: Input) -> Option<Binding> {
        let EditView { layer, act, .. } = self.keymap_screens.edit;
        self.keymaps.get(id).and_then(|k| k.get_of(layer, input, act)).copied()
    }

    /// A way to remap `input`, from an icon in the panel: what it needs opens, or it applies at once. It
    /// can also be an output chosen directly, from the mouse's parts or the control pipe.
    fn map_to(&mut self, id: u16, input: Input, target: Target) -> Vec<Effect> {
        let screen = match target {
            Target::Pick => Screen::Pick { id, input },
            Target::Key => Screen::Capture { id, input, keys: None },
            Target::Keys => Screen::Capture { id, input, keys: Some((0, [Chord::default(); 4])) },
            Target::MouseParts => Screen::Mouse { id, input },
            Target::Layers => Screen::Layers { id, input },
            Target::Default => {
                self.keymap_screens.open = Some(Screen::Edit(id));
                return self.bind(id, input, None);
            }
            _ => return self.chosen(id, input, target),
        };
        // Only the ways that fit the input open: a stick has no key chord of its own, and the gyro has no
        // mouse buttons.
        if ways(input.kind()).contains(&target) {
            self.keymap_screens.open = Some(screen);
        }
        vec![]
    }

    /// An output chosen for `input`, by the way it was asked for.
    fn chosen(&mut self, id: u16, input: Input, target: Target) -> Vec<Effect> {
        match target {
            Target::To(output) => self.choose(id, input, output),
            Target::Default | Target::Pick | Target::Key | Target::Keys | Target::MouseParts | Target::Layers => vec![],
        }
    }

    /// Maps `input` to `output`, keeping its parameters and hold, and goes back to the edit screen, whose
    /// panel shows the parameters.
    fn choose(&mut self, id: u16, input: Input, output: Output) -> Vec<Effect> {
        let full_pull = self.keymap_screens.edit.act == Actuation::FullPull;
        // For a full pull, a picked trigger means that trigger pulled all the way, as a button.
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
        self.keymap_screens.open = Some(Screen::Edit(id));
        effects
    }

    /// A key typed while a chord is wanted. It is the chord for the input, or the next of a stick's four
    /// keys; the fourth completes them.
    pub(crate) fn captured(&mut self, chord: Chord) -> Vec<Effect> {
        let Some(Screen::Capture { id, input, keys }) = self.keymap_screens.open else {
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
                    self.keymap_screens.open = Some(Screen::Capture { id, input, keys: Some((n + 1, so_far)) });
                    return vec![];
                }
                self.choose(id, input, Output::Keys(so_far))
            }
        }
    }

    /// Whether the keyboard's keys make a chord now, because the capture screen is open.
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
