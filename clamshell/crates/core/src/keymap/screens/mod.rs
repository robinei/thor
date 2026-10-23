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

use std::sync::Arc;

use crate::bottom::editor::{Editor, Editors, KeymapName, Purpose};
use crate::bottom::keyboard::{Key, Keyboard, Stroke};
use crate::effect::{Effect, Save};
use crate::event::KeymapEvent;
use crate::id::{KeymapId, Target};
use crate::keymap::{Actuation, Binding, Chord, Input, Keymap, Keymaps, Kind, Layer, Output, PadOut, Param, as_on_the_pad};
use crate::levels::slider::{Control, Slider};
use crate::nav::lists::RowPart;
use crate::settings::Settings;
use crate::state::out::Out;
use crate::{Event, Tab};

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

/// The keymaps and their screens: every keymap, the screen open over the settings tab, and the edit
/// screen's view and its Undo.
#[derive(Debug, Clone, Default)]
pub struct Keymapping {
    keymaps: Keymaps,
    open: Option<Screen>,
    edit: EditView,
    undo: Undo,
}

/// What the keymaps' screens read from elsewhere: the game the settings panel edits, if any, and the
/// keymap chosen for its scope.
#[derive(Debug, Clone, Default)]
pub(crate) struct KeymapCx {
    pub(crate) entry: Option<Arc<str>>,
    pub(crate) chosen: u16,
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
    pub all_entries: bool,
    pub entries: usize,
}

impl Users {
    pub fn any(self) -> bool {
        self.all_entries || self.entries > 0
    }
}

/// A stroke of the keyboard as a chord.
pub(crate) fn chord(stroke: Stroke) -> Chord {
    let key = match stroke.key {
        Key::Mod(_) | Key::Caps => None,
        key => Some(key),
    };
    Chord { key, shift: stroke.shift, ctrl: stroke.ctrl, alt: stroke.alt, logo: stroke.logo }
}

/// Opens the editor for a keymap's name.
fn name_keymap(editors: &mut Editors, action: KeymapName, text: String) {
    editors.open(Editor { purpose: Purpose::KeymapName(action), text });
}

impl Keymapping {
    pub(crate) fn new(keymaps: Keymaps) -> Keymapping {
        Keymapping { keymaps, ..Keymapping::default() }
    }

    pub fn keymaps(&self) -> &Keymaps {
        &self.keymaps
    }

    /// The keymaps' screen open over the settings tab, if one is.
    pub fn screen(&self) -> Option<Screen> {
        self.open
    }

    /// The edit screen's layer, selection and confirmation.
    pub fn edit(&self) -> EditView {
        self.edit
    }

    /// The keymaps' screen that is open, while the settings tab is the one shown (`tab`).
    pub(crate) fn screen_in(&self, tab: Tab) -> Option<Screen> {
        self.open.filter(|_| tab == Tab::Settings)
    }

    /// Whether the pad listens for inputs: the edit screen, or a screen waiting for a pad button, is in
    /// view, with no editor over it.
    pub(crate) fn listening(&self, tab: Tab, editor_open: bool) -> bool {
        !editor_open && self.screen_in(tab).is_some_and(Screen::listens)
    }

    /// Whether the keyboard's keys make a chord now, because the capture screen is in view, with no editor
    /// over it.
    pub(crate) fn capturing(&self, tab: Tab, editor_open: bool) -> bool {
        matches!(self.screen_in(tab), Some(Screen::Capture { .. })) && !editor_open
    }

    /// The picker's rows. Usually they are, for `entry`, its own keymap or the row that makes one, then
    /// Default and the named keymaps. When loading into a game's own keymap they are Default and the named
    /// keymaps; when saving it, a new keymap and the named keymaps.
    fn rows<'a>(&'a self, entry: Option<&str>) -> impl Iterator<Item = PickerRow<'a>> + use<'a> {
        let (loading, saving) = match self.open {
            Some(Screen::Load(_)) => (true, false),
            Some(Screen::SaveAs(_)) => (false, true),
            _ => (false, false),
        };
        let own = entry.filter(|_| !loading && !saving).map(|g| PickerRow::Own(self.keymaps.own_of(g)));
        let new = saving.then_some(PickerRow::New);
        let default = (!saving).then_some(PickerRow::Default);
        own.into_iter().chain(new).chain(default).chain(self.keymaps.named().map(PickerRow::Named))
    }

    pub(crate) fn rows_len(&self, entry: Option<&str>) -> usize {
        self.rows(entry).count()
    }

    /// Row `i` of the picker, with the settings panel editing `entry`.
    pub(crate) fn row(&self, i: usize, entry: Option<&str>) -> Option<PickerRow<'_>> {
        self.rows(entry).nth(i)
    }

    /// Whether keymap `id` is a game's own.
    pub fn is_own(&self, id: u16) -> bool {
        self.keymaps.get(id).is_some_and(|k| k.owner.is_some())
    }

    /// Whether `entry`, the one the settings panel edits, has no keymap of its own and so can make one.
    pub(crate) fn can_make_own(&self, entry: Option<&str>) -> bool {
        entry.is_some_and(|g| self.keymaps.own_of(g).is_none())
    }

    /// A keymap's name. It is "Own keymap" for a game's own, and "Default" for none or for one that is
    /// gone.
    pub fn name(&self, id: u16) -> &str {
        match self.keymaps.get(id) {
            Some(k) if k.owner.is_some() => "Own keymap",
            Some(k) => k.name.as_str(),
            None => "Default",
        }
    }

    /// Keymap `id`, the one in force. It is Default (id 0, empty) if there is none.
    pub(crate) fn in_force(&self, id: u16) -> Keymap {
        self.keymaps.get(id).cloned().unwrap_or_else(|| Keymap::new(0, "Default"))
    }

    /// Keymap `id` as it is sent, to tell when it changed: its id and version (see
    /// [`Keymaps::version`]), or (0, 0) for Default, which has no stored keymap.
    pub(crate) fn version_of(&self, id: u16) -> (u16, u64) {
        match self.keymaps.get(id) {
            Some(k) => (k.id, self.keymaps.version(k.id)),
            None => (0, 0),
        }
    }

    /// For runners that come with one of ClamShell's keymaps (see [`crate::Library::runner_keymaps`]) and
    /// have no keymap chosen and none of their own: makes that keymap their own (see
    /// [`Keymaps::make_own_from`]), chooses it, and saves.
    pub(crate) fn adopt_runner_keymaps(&mut self, library: &crate::Library, prefs: &mut crate::Prefs, out: &mut Out) {
        let wanting: Vec<(Arc<str>, Keymap)> = library
            .runner_keymaps
            .iter()
            .filter(|(key, _)| prefs.settings().overrides_of(key).get(crate::settings::Setting::Keymap).is_none() && self.keymaps.own_of(key).is_none())
            .filter_map(|(key, name)| Some((key.clone(), crate::keymap::preset(name)?)))
            .collect();
        if wanting.is_empty() {
            return;
        }
        for (key, preset) in &wanting {
            if let Some(id) = self.keymaps.make_own_from(preset, key) {
                prefs.choose_keymap(Some(key), id);
            }
        }
        out.effects.push(self.save_effect());
        out.effects.push(prefs.save_effect());
    }

    /// The keymap tile opens the picker.
    pub(crate) fn open_picker(&mut self) {
        self.open = Some(Screen::List);
    }

    /// A row of the picker. A tap chooses it for the scope being edited and closes the picker, a long
    /// press opens the keymap's menu, and its Edit button opens its edit screen. The game's row, when the
    /// game has no keymap of its own, makes one.
    pub(crate) fn on_row(&mut self, i: usize, part: RowPart, long: bool, cx: &KeymapCx, editors: &mut Editors, out: &mut Out) {
        let entry = cx.entry.as_deref();
        match self.open {
            Some(Screen::Load(own)) => {
                match self.row(i, entry) {
                    Some(PickerRow::Default) if !long => self.load_into(own, 0, out),
                    Some(PickerRow::Named(k)) if !long => self.load_into(own, k.id, out),
                    _ => {}
                }
                return;
            }
            Some(Screen::SaveAs(own)) => {
                match self.row(i, entry) {
                    Some(PickerRow::New) if !long => name_keymap(editors, KeymapName::SaveAs(own), String::new()),
                    Some(PickerRow::Named(k)) if !long => self.open = Some(Screen::Overwrite { own, to: k.id }),
                    _ => {}
                }
                return;
            }
            _ => {}
        }
        let (id, keymap) = match self.row(i, entry) {
            None | Some(PickerRow::New) => return,
            Some(PickerRow::Own(None)) => return self.make_own(cx.chosen, cx, out),
            Some(PickerRow::Default) => (0, false),
            Some(PickerRow::Own(Some(k)) | PickerRow::Named(k)) => (k.id, true),
        };
        match (part, long) {
            (RowPart::Edit, _) if keymap => self.open_edit(id),
            (_, true) if keymap => self.open = Some(Screen::Menu(id)),
            (_, false) => {
                self.open = None;
                out.events.push(Event::Keymap(KeymapEvent::Choose(id)));
            }
            _ => {}
        }
    }

    /// Copies keymap `from` into `own`, a game's own keymap, and opens its edit screen, where Undo puts it
    /// back as it was.
    fn load_into(&mut self, own: u16, from: u16, out: &mut Out) {
        self.open_edit(own);
        if self.keymaps.copy_into(own, from) {
            out.effects.push(self.save());
        }
    }

    /// Gives the game the panel edits, if it has none, its own keymap as a copy of keymap `from`. Its edit
    /// screen opens, and State chooses it for the game and saves the keymaps.
    fn make_own(&mut self, from: u16, cx: &KeymapCx, out: &mut Out) {
        let Some(entry) = cx.entry.as_ref().filter(|g| self.keymaps.own_of(g).is_none()) else {
            return;
        };
        let Some(own) = self.keymaps.make_own(from, entry) else {
            return;
        };
        self.open_edit(own);
        out.events.push(Event::Keymap(KeymapEvent::MadeOwn(own)));
    }

    /// The keymaps' screens' buttons, but Back (see [`Keymapping::back`]).
    pub(crate) fn on(&mut self, id: KeymapId, long: bool, cx: &KeymapCx, settings: &Settings, editors: &mut Editors, out: &mut Out) {
        let Some(screen) = self.open else {
            return;
        };
        debug_assert!(screen.keymap().is_none_or(|k| self.keymaps.get(k).is_some()), "the keymap of the screen open is there: {screen:?}");
        match screen {
            Screen::List if id == KeymapId::New => name_keymap(editors, KeymapName::New, String::new()),
            Screen::List => {}
            Screen::Menu(k) => self.on_menu(id, k, settings, editors, out),
            Screen::Overwrite { own, to } if id == KeymapId::ConfirmSave => {
                self.open = Some(Screen::List);
                if self.keymaps.copy_into(to, own) {
                    out.effects.push(self.save());
                }
            }
            Screen::Delete(k) if id == KeymapId::ConfirmDelete => self.delete(k, out),
            Screen::Edit(k) => self.on_edit_screen(id, k, long, cx, out),
            Screen::LayerPicker(k) => self.on_layer_picker(id, k, long, editors),
            Screen::LayerMenu { id: k, layer } => self.on_layer_menu(id, k, layer, editors, out),
            Screen::Layers { id: k, input } => match id {
                KeymapId::ToLayer(layer) => self.choose(k, input, Output::Layer(layer), out),
                KeymapId::NewLayer => {
                    let act = self.edit.act;
                    name_keymap(editors, KeymapName::NewLayer(k, Some((input, act))), String::new());
                }
                _ => {}
            },
            Screen::Pick { id: k, input } => match id {
                KeymapId::Input(tapped) => {
                    if let Some(output) = picked(input, tapped) {
                        self.choose(k, input, output, out);
                    }
                }
                KeymapId::Target(target) => self.map_to(k, input, target, out),
                _ => {}
            },
            Screen::Mouse { id: k, input } => {
                if let KeymapId::Target(target) = id {
                    self.map_to(k, input, target, out);
                }
            }
            Screen::Holds { id: k, input } => match id {
                KeymapId::Hold(hold) => self.set_hold(k, input, hold, out),
                KeymapId::Input(tapped) if holds(tapped) => self.set_hold(k, input, Some(tapped), out),
                _ => {}
            },
            // The modifiers held make the chord, and only State has the keyboard.
            Screen::Capture { .. } if id == KeymapId::Done => out.events.push(Event::Keymap(KeymapEvent::Capture)),
            Screen::Load(_) | Screen::SaveAs(_) | Screen::Overwrite { .. } | Screen::Delete(_) | Screen::Capture { .. } => {}
        }
    }

    /// A keymap's menu (a long press on its row).
    fn on_menu(&mut self, id: KeymapId, k: u16, settings: &Settings, editors: &mut Editors, out: &mut Out) {
        match id {
            KeymapId::Edit => self.open_edit(k),
            KeymapId::Rename => name_keymap(editors, KeymapName::Rename(k), self.name(k).to_owned()),
            KeymapId::Duplicate => name_keymap(editors, KeymapName::Duplicate(k), self.keymaps.unique_name(self.name(k))),
            KeymapId::LoadFrom if self.is_own(k) => self.open = Some(Screen::Load(k)),
            KeymapId::SaveAs if self.is_own(k) => self.open = Some(Screen::SaveAs(k)),
            KeymapId::Delete if settings.keymap_users(k).any() => self.open = Some(Screen::Delete(k)),
            KeymapId::Delete => self.delete(k, out),
            _ => {}
        }
    }

    /// The edit screen's buttons: selecting an input (a long press removes its binding), its actuation,
    /// its target and parameters, the layer shown, Undo, Make own, and the gyro's calibration and hold.
    fn on_edit_screen(&mut self, id: KeymapId, k: u16, long: bool, cx: &KeymapCx, out: &mut Out) {
        match id {
            KeymapId::MakeOwn => self.make_own(k, cx, out),
            KeymapId::Input(input) if long => {
                self.edit.selected = Some(input);
                self.bind(k, input, None, out);
            }
            KeymapId::Input(input) => self.select(k, input, out),
            KeymapId::Actuation(act) if self.edit.selected.is_some_and(|i| act.fits(i.kind())) => self.edit.act = act,
            KeymapId::Undo => self.undo(k, out),
            KeymapId::LayerPicker => self.open = Some(Screen::LayerPicker(k)),
            KeymapId::Layer(layer) => {
                self.edit.layer = layer;
                self.edit.clearing = false;
            }
            KeymapId::Target(target) => {
                if let Some(input) = self.edit.selected {
                    self.map_to(k, input, target, out);
                }
            }
            KeymapId::Toggle(param) => {
                if let Some((_, input, mut b)) = self.param_binding(param) {
                    let on = param.get(&b) > 0;
                    param.set(&mut b, u8::from(!on));
                    let changed = self.set_tuned(k, input, b);
                    self.saved_if(changed, out);
                }
            }
            KeymapId::Calibrate => out.events.push(Event::CalibrateGyro),
            KeymapId::Holds => {
                let input = self.edit.selected.filter(|i| *i == Input::Gyro);
                if let Some(input) = input.filter(|i| self.param_base(k, *i).is_some_and(|b| b.output != Output::Nothing)) {
                    self.open = Some(Screen::Holds { id: k, input });
                }
            }
            _ => {}
        }
    }

    /// The layer picker: shows a layer on the edit screen, opens its menu on a long press, or adds a new
    /// one.
    fn on_layer_picker(&mut self, id: KeymapId, k: u16, long: bool, editors: &mut Editors) {
        match id {
            KeymapId::EditLayer(layer) => self.open_layer_menu(k, layer),
            KeymapId::Layer(layer) if long => self.open_layer_menu(k, layer),
            KeymapId::Layer(layer) => {
                self.edit.layer = layer;
                self.open = Some(Screen::Edit(k));
            }
            KeymapId::NewLayer => name_keymap(editors, KeymapName::NewLayer(k, None), String::new()),
            _ => {}
        }
    }

    /// A layer's menu: rename, Once, clear (after confirmation) and delete.
    fn on_layer_menu(&mut self, id: KeymapId, k: u16, layer: Layer, editors: &mut Editors, out: &mut Out) {
        match id {
            KeymapId::RenameLayer => {
                let name = self.keymaps.get(k).and_then(|km| km.layer_name(layer)).unwrap_or("").to_owned();
                name_keymap(editors, KeymapName::RenameLayer(k, layer), name);
            }
            KeymapId::LayerOnce => {
                let changed = self.keymaps.edit(k, |km| {
                    let once = !km.layer_once(layer);
                    km.set_layer_once(layer, once)
                });
                self.saved_if(changed, out);
            }
            KeymapId::DeleteLayer if layer != Layer::BASE => {
                self.open = Some(Screen::LayerPicker(k));
                if self.edit.layer == layer {
                    self.edit.layer = Layer::BASE;
                }
                let changed = self.keymaps.edit(k, |km| km.remove_layer(layer));
                self.saved_if(changed, out);
            }
            KeymapId::Clear => self.edit.clearing = true,
            KeymapId::ConfirmClear => {
                self.edit.clearing = false;
                self.open = Some(Screen::LayerPicker(k));
                let changed = self.keymaps.edit(k, |km| km.clear(layer));
                self.saved_if(changed, out);
            }
            _ => {}
        }
    }

    /// Goes back one screen: from a keymap's screens to the picker, from an input's screens to its
    /// keymap's, and from the picker to the settings. Leaving the capture screen drops the `keyboard`'s
    /// modifiers held.
    pub(crate) fn back(&mut self, keyboard: &mut Keyboard) {
        // A layer clear waiting for confirmation is cancelled instead.
        if matches!(self.open, Some(Screen::LayerMenu { .. })) && self.edit.clearing {
            self.edit.clearing = false;
            return;
        }
        let capture = matches!(self.open, Some(Screen::Capture { .. }));
        self.open = match self.open {
            None | Some(Screen::List) => None,
            Some(Screen::Menu(_) | Screen::Delete(_) | Screen::Edit(_)) => Some(Screen::List),
            Some(Screen::Load(id) | Screen::SaveAs(id)) => Some(Screen::Menu(id)),
            Some(Screen::Overwrite { own, .. }) => Some(Screen::SaveAs(own)),
            Some(Screen::Pick { id, .. } | Screen::Holds { id, .. } | Screen::Mouse { id, .. }) => Some(Screen::Edit(id)),
            Some(Screen::Layers { id, .. } | Screen::LayerPicker(id)) => Some(Screen::Edit(id)),
            Some(Screen::LayerMenu { id, .. }) => Some(Screen::LayerPicker(id)),
            Some(Screen::Capture { id, .. }) => Some(Screen::Edit(id)),
        };
        if capture {
            keyboard.take_modifiers();
        }
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
    fn select(&mut self, id: u16, input: Input, out: &mut Out) {
        let view = &mut self.edit;
        view.clearing = false;
        if view.selected != Some(input) {
            view.selected = Some(input);
            view.act = Actuation::Press;
            return;
        }
        if let way @ (Target::Pick | Target::Key | Target::Keys | Target::MouseParts) = way_of(self.binding_shown(id, input).map(|b| b.output)) {
            self.map_to(id, input, way, out);
        }
    }

    /// An input pressed on the pad while it `listens`. On the edit screen it is selected as a touch would
    /// select it. While a control or the gyro's hold is being picked, it is the one picked, if it fits.
    pub(crate) fn listened(&mut self, input: Input, listens: bool, out: &mut Out) {
        if !listens {
            return;
        }
        match self.open {
            Some(Screen::Edit(id)) => self.select(id, input, out),
            Some(Screen::Pick { id, input: to }) => {
                if let Some(output) = picked(to, input) {
                    self.choose(id, to, output, out);
                }
            }
            Some(Screen::Holds { id, input: gyro }) if holds(input) => self.set_hold(id, gyro, Some(input), out),
            _ => {}
        }
    }

    /// Sets what holds the gyro on, with none meaning always on, and goes back to the edit screen.
    fn set_hold(&mut self, id: u16, gyro: Input, hold: Option<Input>, out: &mut Out) {
        self.open = Some(Screen::Edit(id));
        let Some(mut b) = self.param_base(id, gyro) else {
            return;
        };
        b.hold = hold;
        let changed = self.set_tuned(id, gyro, b);
        self.saved_if(changed, out);
    }

    /// The editor saved a name: for a new keymap, which then opens on its edit screen, a renamed keymap, a
    /// copy, or a new or renamed layer.
    pub(crate) fn named(&mut self, action: KeymapName, name: &str, out: &mut Out) {
        if name.trim().is_empty() {
            return;
        }
        match action {
            KeymapName::New => {
                if let Some(id) = self.keymaps.add(name) {
                    self.open_edit(id);
                }
            }
            KeymapName::Rename(id) => {
                self.keymaps.rename(id, name);
                self.open = Some(Screen::List);
            }
            KeymapName::Duplicate(id) | KeymapName::SaveAs(id) => {
                self.keymaps.duplicate(id, name);
                self.open = Some(Screen::List);
            }
            KeymapName::NewLayer(id, for_input) => {
                let mut layer = None;
                self.keymaps.edit(id, |k| {
                    layer = k.add_layer(name);
                    layer.is_some()
                });
                self.open = Some(Screen::Edit(id));
                match (layer, for_input) {
                    // Made from the list of layers for an input: the input maps to the new layer, in the
                    // layer shown.
                    (Some(l), Some((input, act))) => {
                        self.edit.act = act;
                        self.choose(id, input, Output::Layer(l), out);
                        out.effects.push(self.save());
                        return;
                    }
                    // Made with the + button: the new layer is shown.
                    (Some(l), None) => self.edit.layer = l,
                    (None, _) => {}
                }
            }
            KeymapName::RenameLayer(id, layer) => {
                self.keymaps.edit(id, |k| k.layer_name(layer).is_some_and(|n| n != name.trim()) && k.rename_layer(layer, name));
                self.open = Some(Screen::LayerPicker(id));
            }
        }
        out.effects.push(self.save());
    }

    /// Deletes keymap `id`. State then puts the games that used it, and all games if it was theirs, back
    /// on Default.
    fn delete(&mut self, id: u16, out: &mut Out) {
        self.keymaps.remove(id);
        self.open = Some(Screen::List);
        out.effects.push(self.save());
        out.events.push(Event::Keymap(KeymapEvent::Deleted(id)));
    }

    /// The menu of keymap `id`'s `layer` (see [`Screen::LayerMenu`]).
    fn open_layer_menu(&mut self, id: u16, layer: Layer) {
        self.edit.clearing = false;
        self.open = Some(Screen::LayerMenu { id, layer });
    }

    /// Binds `input` of keymap `id` to `binding` in the layer and actuation shown, and saves the keymaps
    /// if that changed them. `None` removes the binding: the input then acts as on the pad, or as the
    /// layers below, and a hold or double does nothing.
    fn bind(&mut self, id: u16, input: Input, binding: Option<Binding>, out: &mut Out) {
        let EditView { layer, act, .. } = self.edit;
        debug_assert!(self.keymaps.get(id).is_some(), "keymap {id}, being edited, is there");
        let changed = self.keymaps.edit(id, |k| k.set_of(layer, input, act, binding));
        self.saved_if(changed, out);
    }

    /// `input`'s binding in the layer and actuation shown.
    fn binding_shown(&self, id: u16, input: Input) -> Option<Binding> {
        let EditView { layer, act, .. } = self.edit;
        self.keymaps.get(id).and_then(|k| k.get_of(layer, input, act)).copied()
    }

    /// A way to remap `input`, from an icon in the panel: what it needs opens, or it applies at once. It
    /// can also be an output chosen directly, from the mouse's parts or the control pipe.
    fn map_to(&mut self, id: u16, input: Input, target: Target, out: &mut Out) {
        let screen = match target {
            Target::Pick => Screen::Pick { id, input },
            Target::Key => Screen::Capture { id, input, keys: None },
            Target::Keys => Screen::Capture { id, input, keys: Some((0, [Chord::default(); 4])) },
            Target::MouseParts => Screen::Mouse { id, input },
            Target::Layers => Screen::Layers { id, input },
            Target::Default => {
                self.open = Some(Screen::Edit(id));
                return self.bind(id, input, None, out);
            }
            Target::To(output) => return self.choose(id, input, output, out),
        };
        // Only the ways that fit the input open: a stick has no key chord of its own, and the gyro has no
        // mouse buttons.
        if ways(input.kind()).contains(&target) {
            self.open = Some(screen);
        }
    }

    /// Maps `input` to `output`, keeping its parameters and hold, and goes back to the edit screen, whose
    /// panel shows the parameters.
    fn choose(&mut self, id: u16, input: Input, output: Output, out: &mut Out) {
        let full_pull = self.edit.act == Actuation::FullPull;
        // For a full pull, a picked trigger means that trigger pulled all the way, as a button.
        let output = match output {
            Output::Trigger(s) if full_pull => Output::Pad(PadOut::Trigger(s)),
            o => o,
        };
        if !output.fits(input.kind()) {
            return;
        }
        let kept = self.binding_shown(id, input);
        let fresh = || Binding { threshold: if full_pull { crate::keymap::FULL_PULL } else { crate::keymap::DEFAULT_THRESHOLD }, ..Binding::of(input, output) };
        let binding = kept.map_or_else(fresh, |k| k.retarget(input, output));
        self.bind(id, input, Some(binding), out);
        self.open = Some(Screen::Edit(id));
    }

    /// Capture was pressed: the `keyboard`'s modifiers armed or held are the chord, on the capture screen.
    pub(crate) fn capture(&mut self, keyboard: &mut Keyboard, out: &mut Out) {
        if matches!(self.open, Some(Screen::Capture { .. })) {
            let chord = keyboard.take_modifiers();
            self.captured(chord, out);
        }
    }

    /// A key typed while a chord is wanted. It is the chord for the input, or the next of a stick's four
    /// keys; the fourth completes them.
    pub(crate) fn captured(&mut self, chord: Chord, out: &mut Out) {
        let Some(Screen::Capture { id, input, keys }) = self.open else {
            return;
        };
        if chord.is_empty() {
            return;
        }
        match keys {
            None => self.choose(id, input, Output::Key(chord), out),
            Some((n, mut so_far)) => {
                if let Some(slot) = so_far.get_mut(n) {
                    *slot = chord;
                }
                if n + 1 < so_far.len() {
                    self.open = Some(Screen::Capture { id, input, keys: Some((n + 1, so_far)) });
                    return;
                }
                self.choose(id, input, Output::Keys(so_far), out);
            }
        }
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests;
