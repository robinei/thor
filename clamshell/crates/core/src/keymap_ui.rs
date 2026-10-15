//! The keymaps' screens on the second screen, over the settings tab: the picker (Default, then the
//! keymaps with how many games use each, and New), a keymap's menu (Edit, Rename, Duplicate, Delete,
//! deleting one in use after a confirmation), the edit screen ("map from": the pad's inputs in one layer
//! of the keymap, main or Back held ([`EditView`]); an input chosen by touch or by pressing it on the pad,
//! which listens meanwhile, is selected into a panel with Change and Reset, and chosen again, Change;
//! the pad's Back button shows the other layer; "No changes" clears the layer shown, once confirmed),
//! what an input maps to ("map to": the targets that fit its kind, with speed, threshold and the gyro's
//! hold), and a key chord or a stick's four keys typed on the keyboard. Names are typed in the editor.
//! Also the keymap in force, sent to the pad whenever it changes.

use crate::State;
use crate::editor::{Editor, KeymapName, Purpose};
use crate::effect::{Effect, Save};
use crate::id::{KeymapId, Target};
use crate::items::ItemKey;
use crate::keyboard::{Key, Stroke};
use crate::keymap::{Binding, Chord, Input, Keymap, Keymaps, Layer, Output, PadOut, Param, as_on_the_pad};
use crate::scroll::{RowPart, Rows};
use crate::settings::Setting;
use crate::slider::{Control, Slider};

/// Which of the keymaps' screens is open.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Screen {
    /// Default and the keymaps; New.
    List,
    /// A keymap's long-press menu.
    Menu(u16),
    /// Deleting a keymap games use: are you sure.
    Delete(u16),
    /// A keymap's inputs ("map from"): the pad listens.
    Edit(u16),
    /// What one input maps to.
    MapTo { id: u16, input: Input },
    /// What holds the gyro on (`input`: the gyro).
    Holds { id: u16, input: Input },
    /// Waiting for a press on the pad: the pad button `input` maps to (the pad listens).
    PressPad { id: u16, input: Input },
    /// A key chord typed on the keyboard for `input`; for a stick's four keys, which one and those so far.
    Capture { id: u16, input: Input, keys: Option<(usize, [Chord; 4])> },
}

impl Screen {
    /// Whether the pad listens: its presses choose inputs instead of moving the focus.
    pub fn listens(self) -> bool {
        matches!(self, Screen::Edit(_) | Screen::PressPad { .. })
    }

    /// The keymap it is about, if one.
    pub fn keymap(self) -> Option<u16> {
        match self {
            Screen::List => None,
            Screen::Menu(id) | Screen::Delete(id) | Screen::Edit(id) => Some(id),
            Screen::MapTo { id, .. } | Screen::Holds { id, .. } | Screen::PressPad { id, .. } | Screen::Capture { id, .. } => Some(id),
        }
    }
}

/// The edit screen's own state, kept while its input's screens are open: the layer shown and edited, the
/// input selected (shown in the panel), whether "No changes" waits for its confirmation, and the slider
/// of one of the selected input's parameters while a finger is on it.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct EditView {
    pub layer: Layer,
    pub selected: Option<Input>,
    pub clearing: bool,
    pub slider: Option<Slider>,
}

/// The picker's rows: Default, then the keymaps (found again by id).
impl Rows for Keymaps {
    fn len(&self) -> usize {
        self.list().len() + 1
    }

    fn key(&self, i: usize) -> Option<ItemKey> {
        let id = if i == 0 { 0 } else { self.list().get(i - 1)?.id };
        Some(ItemKey::Number(u32::from(id)))
    }

    fn find(&self, key: &ItemKey) -> Option<usize> {
        match key {
            ItemKey::Number(0) => Some(0),
            ItemKey::Number(id) => self.list().iter().position(|k| u32::from(k.id) == *id).map(|i| i + 1),
            ItemKey::Name(_) => None,
        }
    }
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

    /// What the pad's Back button does on ClamShell's screens, as the target it activates: on the edit
    /// screen while the pad listens, the other layer; in the editor, Cancel; on the keymaps' other
    /// screens, back one. (Elsewhere nothing has a back of its own.)
    pub fn pad_back(&self) -> Option<crate::Id> {
        if self.bottom.editor.is_some() {
            return Some(crate::Id::Editor(crate::EditorId::Cancel));
        }
        match self.bottom.keymaps? {
            Screen::Edit(_) if self.listening() => Some(crate::Id::Keymap(KeymapId::Layer(self.bottom.keymap_edit.layer.other()))),
            _ => Some(crate::Id::Keymap(KeymapId::Back)),
        }
    }

    /// Whether the pad listens for inputs (the edit screen, or waiting for a pad button, in view).
    pub(crate) fn listening(&self) -> bool {
        self.bottom.tab == crate::Tab::Settings && self.bottom.editor.is_none() && self.bottom.keymaps.is_some_and(Screen::listens)
    }

    /// The picker's row `i`: Default (0), or a keymap.
    pub fn keymap_row(&self, i: usize) -> Option<Option<&Keymap>> {
        match i {
            0 => Some(None),
            i => self.keymaps.list().get(i - 1).map(Some),
        }
    }

    /// The keymap the settings panel's scope uses (0: Default).
    pub fn keymap_chosen(&self) -> u16 {
        self.tile_value(Setting::Keymap)
    }

    /// A keymap's name, "Default" for none (or one that is gone).
    pub fn keymap_name(&self, id: u16) -> &str {
        self.keymaps.get(id).map_or("Default", |k| k.name.as_str())
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

    fn save_keymaps(&self) -> Effect {
        Effect::Save(Save::Keymaps(self.keymaps.text()))
    }

    /// The keymap tile: the picker opens.
    pub(crate) fn open_keymaps(&mut self) {
        self.bottom.keymaps = Some(Screen::List);
    }

    /// A row of the picker: a tap chooses it for the scope being edited (and closes the picker), a long
    /// press opens a keymap's menu; its Edit button its edit screen.
    pub(crate) fn on_keymap_row(&mut self, i: usize, part: RowPart, long: bool) -> Vec<Effect> {
        let Some(row) = self.keymap_row(i) else {
            return vec![];
        };
        let id = row.map_or(0, |k| k.id);
        match (part, long, row) {
            (RowPart::Edit, _, Some(_)) => {
                self.open_edit(id);
                vec![]
            }
            (_, true, Some(_)) => {
                self.bottom.keymaps = Some(Screen::Menu(id));
                vec![]
            }
            (_, false, _) => {
                self.bottom.keymaps = None;
                self.change(Setting::Keymap, Some(id))
            }
            _ => vec![],
        }
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
            (KeymapId::Delete, Screen::Menu(k)) if self.keymap_users(k).any() => {
                self.bottom.keymaps = Some(Screen::Delete(k));
                vec![]
            }
            (KeymapId::Delete, Screen::Menu(k)) | (KeymapId::ConfirmDelete, Screen::Delete(k)) => self.delete_keymap(k),
            (KeymapId::Input(input), Screen::Edit(k)) if long => {
                self.bottom.keymap_edit.selected = Some(input);
                self.bind(k, input, None)
            }
            (KeymapId::Input(input), Screen::Edit(k)) => {
                self.select(k, input);
                vec![]
            }
            (KeymapId::Change, Screen::Edit(k)) => {
                if let Some(input) = self.bottom.keymap_edit.selected {
                    self.bottom.keymaps = Some(Screen::MapTo { id: k, input });
                }
                vec![]
            }
            (KeymapId::Reset, Screen::Edit(k)) => match self.bottom.keymap_edit.selected {
                Some(input) => self.bind(k, input, None),
                None => vec![],
            },
            (KeymapId::Layer(layer), Screen::Edit(_)) => {
                self.bottom.keymap_edit.layer = layer;
                self.bottom.keymap_edit.clearing = false;
                vec![]
            }
            (KeymapId::Clear, Screen::Edit(_)) => {
                self.bottom.keymap_edit.clearing = true;
                vec![]
            }
            (KeymapId::ConfirmClear, Screen::Edit(k)) => {
                self.bottom.keymap_edit.clearing = false;
                let layer = self.bottom.keymap_edit.layer;
                let changed = self.keymaps.get_mut(k).is_some_and(|k| k.clear(layer));
                if changed { vec![self.save_keymaps()] } else { vec![] }
            }
            (KeymapId::Target(target), Screen::MapTo { id: k, input }) => self.map_to(k, input, target),
            (KeymapId::Toggle(param), Screen::Edit(k)) => match self.param_binding(param) {
                Some((_, input, mut b)) => {
                    let on = param.get(&b) > 0;
                    param.set(&mut b, u8::from(!on));
                    let changed = self.set_tuned(k, input, b);
                    if changed { vec![self.save_keymaps()] } else { vec![] }
                }
                None => vec![],
            },
            (KeymapId::Holds, Screen::Edit(k)) => {
                let input = self.bottom.keymap_edit.selected.filter(|i| *i == Input::Gyro);
                if let Some(input) = input.filter(|i| self.param_base(k, *i).is_some_and(|b| b.output != Output::Nothing)) {
                    self.bottom.keymaps = Some(Screen::Holds { id: k, input });
                }
                vec![]
            }
            (KeymapId::Hold(hold), Screen::Holds { id: k, input }) => {
                self.bottom.keymaps = Some(Screen::Edit(k));
                let Some(mut b) = self.param_base(k, input) else {
                    return vec![];
                };
                b.hold = hold;
                if self.set_tuned(k, input, b) { vec![self.save_keymaps()] } else { vec![] }
            }
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
        // "No changes" waiting for its confirmation: not after all.
        if matches!(self.bottom.keymaps, Some(Screen::Edit(_))) && self.bottom.keymap_edit.clearing {
            self.bottom.keymap_edit.clearing = false;
            return;
        }
        self.bottom.keymaps = match self.bottom.keymaps {
            None | Some(Screen::List) => None,
            Some(Screen::Menu(_) | Screen::Delete(_) | Screen::Edit(_)) => Some(Screen::List),
            Some(Screen::MapTo { id, .. } | Screen::Holds { id, .. }) => Some(Screen::Edit(id)),
            Some(Screen::PressPad { id, input } | Screen::Capture { id, input, .. }) => {
                self.keyboard.take_modifiers();
                Some(Screen::MapTo { id, input })
            }
        };
    }

    /// The edit screen of keymap `id`, as first opened: the main layer, nothing selected.
    fn open_edit(&mut self, id: u16) {
        self.bottom.keymaps = Some(Screen::Edit(id));
        self.bottom.keymap_edit = EditView::default();
    }

    /// An input chosen on the edit screen (touched, or pressed on the pad): selected into the panel; the
    /// one selected already, its "map to" opens.
    fn select(&mut self, id: u16, input: Input) {
        let view = &mut self.bottom.keymap_edit;
        view.clearing = false;
        if view.selected == Some(input) {
            self.bottom.keymaps = Some(Screen::MapTo { id, input });
        } else {
            view.selected = Some(input);
        }
    }

    /// An input pressed on the pad while it listens: on the edit screen, it is selected as a touch
    /// selects it (pressed again, its "map to" opens); waiting for a pad button, it is the one.
    pub(crate) fn listened(&mut self, input: Input) -> Vec<Effect> {
        if self.pad.route() != crate::PadRoute::Listen {
            return vec![];
        }
        match self.bottom.keymaps {
            Some(Screen::Edit(id)) => {
                self.select(id, input);
                vec![]
            }
            Some(Screen::PressPad { id, input: to }) => match PadOut::of(input) {
                Some(out) => self.choose(id, to, Output::Pad(out)),
                None => vec![],
            },
            _ => vec![],
        }
    }

    /// Opens the editor for a keymap's name.
    fn name_keymap(&mut self, action: KeymapName, text: String) {
        self.bottom.editor = Some(Editor { purpose: Purpose::KeymapName(action), text });
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

    /// `input` of keymap `id` bound to `binding` in the layer shown (none: as on the pad, or as the main
    /// layer); saved if it changed.
    fn bind(&mut self, id: u16, input: Input, binding: Option<Binding>) -> Vec<Effect> {
        let layer = self.bottom.keymap_edit.layer;
        let changed = self.keymaps.get_mut(id).is_some_and(|k| k.set_in(layer, input, binding));
        if changed { vec![self.save_keymaps()] } else { vec![] }
    }

    /// `input`'s binding in the layer shown.
    fn binding_shown(&self, id: u16, input: Input) -> Option<Binding> {
        self.keymaps.get(id).and_then(|k| k.get_in(self.bottom.keymap_edit.layer, input)).copied()
    }

    /// A target tapped on the "map to" screen.
    fn map_to(&mut self, id: u16, input: Input, target: Target) -> Vec<Effect> {
        let output = match target {
            Target::Default => {
                let effects = self.bind(id, input, None);
                self.bottom.keymaps = Some(Screen::Edit(id));
                return effects;
            }
            Target::PressPad => {
                self.bottom.keymaps = Some(Screen::PressPad { id, input });
                return vec![];
            }
            Target::Key => {
                self.bottom.keymaps = Some(Screen::Capture { id, input, keys: None });
                return vec![];
            }
            Target::Keys => {
                self.bottom.keymaps = Some(Screen::Capture { id, input, keys: Some((0, [Chord::default(); 4])) });
                return vec![];
            }
            Target::Own => match as_on_the_pad(input) {
                Some(output) => output,
                None => return vec![],
            },
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
        if !output.fits(input.kind()) {
            return vec![];
        }
        let kept = self.binding_shown(id, input);
        let binding = kept.map_or(Binding::of(input, output), |k| k.retarget(input, output));
        let effects = self.bind(id, input, Some(binding));
        self.bottom.keymaps = Some(Screen::Edit(id));
        effects
    }

    /// A finger held (or slid) on a parameter's row of the panel: its slider, from its value now.
    pub(crate) fn param_slide_start(&mut self, param: Param, x: f32, width: f32) {
        let Some((_, _, b)) = self.param_binding(param) else {
            return;
        };
        self.bottom.keymap_edit.slider = Some(Slider::new(Control::Param(param), param.get(&b), x, width));
    }

    /// What the selected input's parameters start from in the layer shown: its binding there; in the
    /// Back layer without one, the main layer's; else the input as on the pad.
    pub fn param_base(&self, id: u16, input: Input) -> Option<Binding> {
        let k = self.keymaps.get(id);
        let layer = self.bottom.keymap_edit.layer;
        k.and_then(|k| k.lookup(input, layer == Layer::Back)).copied().or_else(|| as_on_the_pad(input).map(|o| Binding::of(input, o)))
    }

    /// The selected input and what its parameters start from, if `param` applies to it (on the edit
    /// screen).
    fn param_binding(&self, param: Param) -> Option<(u16, Input, Binding)> {
        let Some(Screen::Edit(id)) = self.bottom.keymaps else {
            return None;
        };
        let input = self.bottom.keymap_edit.selected?;
        let b = self.param_base(id, input)?;
        param.applies(input, b.output).then_some((id, input, b))
    }

    /// `input`'s parameters changed in the layer shown: kept, unless in the main layer they are all as
    /// on the pad again (then it has no binding). Whether it changed.
    fn set_tuned(&mut self, id: u16, input: Input, b: Binding) -> bool {
        let layer = self.bottom.keymap_edit.layer;
        let plain = layer == Layer::Main && as_on_the_pad(input).map(|o| Binding::of(input, o)) == Some(b);
        self.keymaps.get_mut(id).is_some_and(|k| k.set_in(layer, input, (!plain).then_some(b)))
    }

    /// The parameter's slider moved: the binding follows at once (the pad gets the keymap as it changes),
    /// saved when the finger lifts. Whether it changed.
    pub(crate) fn param_slide_to(&mut self, x: f32) -> bool {
        let Some(slider) = self.bottom.keymap_edit.slider else {
            return false;
        };
        let Control::Param(param) = slider.control else {
            return false;
        };
        let Some((id, input, mut b)) = self.param_binding(param) else {
            return false;
        };
        param.set(&mut b, slider.value_at(x));
        let value = param.get(&b);
        if value == slider.value {
            return false;
        }
        self.bottom.keymap_edit.slider = Some(Slider { value, moved: true, ..slider });
        self.set_tuned(id, input, b)
    }

    /// The finger lifted: the keymaps saved, if the slider changed anything.
    pub(crate) fn param_slide_end(&mut self) -> Vec<Effect> {
        match self.bottom.keymap_edit.slider.take() {
            Some(s) if s.moved => vec![self.save_keymaps()],
            _ => vec![],
        }
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
        matches!(self.bottom.keymaps, Some(Screen::Capture { .. })) && self.bottom.editor.is_none()
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
mod tests {
    use super::*;
    use crate::keyboard::{Modifier, find};
    use crate::keymap::{Button, Dir, MouseButton, Side};
    use crate::slider::Control;
    use crate::testkit::*;
    use crate::*;

    fn key(s: &mut State, k: Key) -> Vec<Effect> {
        tap(s, Id::Key(find(k).unwrap()))
    }

    fn type_text(s: &mut State, text: &str) {
        for c in text.chars() {
            key(s, Key::Char(c));
        }
    }

    fn keymap(s: &mut State, id: KeymapId) -> Vec<Effect> {
        tap(s, Id::Keymap(id))
    }

    /// The selected input's parameter slid to `value` (a finger held on its row, moved, lifted).
    fn slide(s: &mut State, param: Param, value: u8) -> Vec<Effect> {
        s.apply(Slide::Start { control: Control::Param(param), x: 300.0, width: 620.0 }.into());
        let slider = s.keymap_edit().slider.unwrap();
        assert_eq!(slider.start, param.get(&s.param_base(s.keymap_screen().and_then(Screen::keymap).unwrap(), s.keymap_edit().selected.unwrap()).unwrap()));
        let x = slider.x + (f32::from(value) - f32::from(slider.start)) / 100.0 * slider.length;
        let mut fx = s.apply(Slide::To { x }.into()).1;
        fx.extend(s.apply(Slide::End.into()).1);
        fx
    }

    /// An input's "map to" from the edit screen: selected, then chosen again.
    fn open(s: &mut State, input: Input) {
        keymap(s, KeymapId::Input(input));
        if matches!(s.keymap_screen(), Some(Screen::Edit(_))) {
            keymap(s, KeymapId::Input(input));
        }
    }

    fn row(i: u16) -> Id {
        ListId::Keymaps.row(i)
    }

    fn picker(s: &mut State) {
        tap(s, Id::Tab(Tab::Settings));
        tap(s, Id::Settings(SettingsId::Tile(Setting::Keymap)));
    }

    fn sent(fx: &[Effect]) -> Option<&Keymap> {
        fx.iter().find_map(|e| if let Effect::Keymap(k) = e { Some(k) } else { None })
    }

    #[test]
    fn the_picker_chooses_a_keymap_for_the_scope_edited() {
        let mut s = State::new(Init { keymaps: Keymaps::initial(), ..init() });
        picker(&mut s);
        assert_eq!(s.keymap_screen(), Some(Screen::List));
        assert_eq!(s.list_len(ListId::Keymaps), 2, "Default, Nintendo layout");
        tap(&mut s, row(1));
        assert_eq!(s.keymap_screen(), None, "chosen: the picker closes");
        assert_eq!(s.settings().overrides_of("zelda-lttp").get(Setting::Keymap), Some(1));
        assert_eq!(s.keymap_users(1), Users { all_games: false, games: 1 });
        // Running, it reaches the pad (once).
        let fx = s.apply(GameEvent::Started("zelda-lttp".into()).into()).1;
        assert_eq!(sent(&fx).map(|k| k.name.as_str()), Some("Nintendo layout"));
        assert_eq!(sent(&s.apply(Event::Tick { unix_secs: 5 }).1), None);
        // Default for it, and the pad gets an empty keymap.
        picker(&mut s);
        let fx = tap(&mut s, row(0));
        assert_eq!(sent(&fx).map(|k| (k.id, k.bindings().len())), Some((0, 0)));
        assert_eq!(s.settings().overrides_of("zelda-lttp").get(Setting::Keymap), Some(0));
    }

    #[test]
    fn a_new_keymap_is_named_edited_and_mapped() {
        let mut s = State::new(Init { keymaps: Keymaps::initial(), ..init() });
        picker(&mut s);
        keymap(&mut s, KeymapId::New);
        assert!(matches!(s.bottom().editor(), Some(Editor { purpose: Purpose::KeymapName(KeymapName::New), .. })));
        type_text(&mut s, "fps");
        let fx = tap(&mut s, Id::Editor(EditorId::Save));
        assert!(matches!(fx.first(), Some(Effect::Save(Save::Keymaps(t))) if t.ends_with("keymap 2 fps\n")), "{fx:?}");
        assert_eq!(s.keymap_screen(), Some(Screen::Edit(2)));
        assert!(s.listening(), "the pad listens on the edit screen");
        assert_eq!(s.pad().route(), PadRoute::Listen);
        // A pressed on the pad: selected; pressed again, its "map to"; a key chord typed: Ctrl, then x.
        s.apply(Event::Listened(Input::Button(Button::A)));
        assert_eq!((s.keymap_screen(), s.keymap_edit().selected), (Some(Screen::Edit(2)), Some(Input::Button(Button::A))));
        s.apply(Event::Listened(Input::Button(Button::A)));
        assert_eq!(s.keymap_screen(), Some(Screen::MapTo { id: 2, input: Input::Button(Button::A) }));
        assert_eq!(s.pad().route(), PadRoute::Ui, "the pad moves the focus there");
        keymap(&mut s, KeymapId::Target(Target::Key));
        key(&mut s, Key::Mod(Modifier::Ctrl));
        let fx = key(&mut s, Key::Char('x'));
        assert!(fx.iter().all(|e| !matches!(e, Effect::Key(_))), "not typed into an app");
        assert_eq!(s.keymap_screen(), Some(Screen::Edit(2)), "done: back to the inputs");
        let a = s.keymaps().get(2).unwrap().get(Input::Button(Button::A)).copied();
        assert_eq!(a.map(|b| b.output), Some(Output::Key(Chord { ctrl: true, ..Chord::of(Key::Char('x')) })));
        // The right stick to the pointer, faster.
        open(&mut s, Input::Stick(Side::Right));
        keymap(&mut s, KeymapId::Target(Target::Pointer));

        assert_eq!(s.keymap_screen(), Some(Screen::Edit(2)), "chosen: its parameters are in the panel");
        slide(&mut s, Param::Speed, 80);
        assert_eq!(s.keymaps().get(2).unwrap().get(Input::Stick(Side::Right)).map(|b| (b.output, b.speed)), Some((Output::Pointer, 80)));
        // The left stick to four keys, in turn.
        open(&mut s, Input::Stick(Side::Left));
        keymap(&mut s, KeymapId::Target(Target::Keys));
        type_text(&mut s, "wasd");
        let w = |c| Chord::of(Key::Char(c));
        assert_eq!(s.keymaps().get(2).unwrap().get(Input::Stick(Side::Left)).map(|b| b.output), Some(Output::Keys([w('w'), w('a'), w('s'), w('d')])));
        // The gyro to the pointer while LT is held; Shift alone on LB (Done with only a modifier).
        open(&mut s, Input::Gyro);
        keymap(&mut s, KeymapId::Target(Target::Pointer));
        keymap(&mut s, KeymapId::Holds);
        assert_eq!(s.keymap_screen(), Some(Screen::Holds { id: 2, input: Input::Gyro }));
        keymap(&mut s, KeymapId::Hold(Some(Input::Trigger(Side::Left))));
        assert_eq!(s.keymaps().get(2).unwrap().get(Input::Gyro).and_then(|b| b.hold), Some(Input::Trigger(Side::Left)));
        assert_eq!(s.keymap_screen(), Some(Screen::Edit(2)));
        open(&mut s, Input::Button(Button::LB));
        keymap(&mut s, KeymapId::Target(Target::Key));
        key(&mut s, Key::Mod(Modifier::Shift));
        keymap(&mut s, KeymapId::Done);
        assert_eq!(s.keymaps().get(2).unwrap().get(Input::Button(Button::LB)).map(|b| b.output), Some(Output::Key(Chord { shift: true, ..Chord::default() })));
        assert!(!s.keyboard().shift.on(), "used up");
        // B to a pad button pressed on the pad (Y); the trigger to a mouse button past 75 %.
        open(&mut s, Input::Button(Button::B));
        keymap(&mut s, KeymapId::Target(Target::PressPad));
        assert_eq!(s.pad().route(), PadRoute::Listen);
        s.apply(Event::Listened(Input::Button(Button::Y)));
        assert_eq!(s.keymaps().get(2).unwrap().get(Input::Button(Button::B)).map(|b| b.output), Some(Output::Pad(PadOut::Button(Button::Y))));
        open(&mut s, Input::Trigger(Side::Right));
        keymap(&mut s, KeymapId::Target(Target::Mouse(MouseButton::Left)));
        slide(&mut s, Param::Threshold, 75);
        assert_eq!(s.keymaps().get(2).unwrap().get(Input::Trigger(Side::Right)).map(|b| b.threshold), Some(75));
        // A long press on an input: as on the pad again. Home leaves the edit screen.
        long(&mut s, Id::Keymap(KeymapId::Input(Input::Button(Button::B))));
        assert_eq!(s.keymaps().get(2).unwrap().get(Input::Button(Button::B)), None);
        s.apply(Event::Home);
        assert_eq!(s.keymap_screen(), Some(Screen::List));
        assert_eq!(s.pad().route(), PadRoute::Ui);
        // The saved text has it all.
        let fresh = Keymaps::load(&s.keymaps().text());
        assert_eq!(&fresh, s.keymaps());
    }

    #[test]
    fn targets_offered_fit_the_input() {
        let mut s = State::new(Init { keymaps: Keymaps::initial(), ..init() });
        picker(&mut s);
        tap(&mut s, row(1));
        picker(&mut s);
        tap(&mut s, Id::Row { list: ListId::Keymaps, i: 1, part: RowPart::Edit });
        open(&mut s, Input::Button(Button::A));
        assert_eq!(keymap(&mut s, KeymapId::Target(Target::Pointer)), vec![], "a button can't move the pointer");
        keymap(&mut s, KeymapId::Target(Target::Wheel(Dir::Down)));
        assert_eq!(s.keymaps().get(1).unwrap().get(Input::Button(Button::A)).map(|b| b.output), Some(Output::Wheel(Dir::Down)));
        open(&mut s, Input::Button(Button::A));
        keymap(&mut s, KeymapId::Target(Target::Default));
        assert_eq!(s.keymaps().get(1).unwrap().get(Input::Button(Button::A)), None);
    }

    #[test]
    fn rename_duplicate_and_delete_with_games_using_it() {
        let mut s = State::new(Init { keymaps: Keymaps::initial(), ..init() });
        picker(&mut s);
        tap(&mut s, row(1));
        tap(&mut s, Id::Settings(SettingsId::Scope(Scope::Global)));
        picker(&mut s);
        tap(&mut s, row(1));
        assert_eq!(s.keymap_users(1), Users { all_games: true, games: 1 });
        // Rename.
        picker(&mut s);
        long(&mut s, row(1));
        assert_eq!(s.keymap_screen(), Some(Screen::Menu(1)));
        keymap(&mut s, KeymapId::Rename);
        key(&mut s, Key::Char('1'));
        tap(&mut s, Id::Editor(EditorId::Save));
        assert_eq!(s.keymap_name(1), "Nintendo layout1");
        // Duplicate.
        long(&mut s, row(1));
        keymap(&mut s, KeymapId::Duplicate);
        tap(&mut s, Id::Editor(EditorId::Save));
        assert_eq!(s.keymaps().list().len(), 2);
        assert_eq!(s.keymaps().get(2).map(|k| k.bindings().len()), Some(4));
        // Not used: deleted at once.
        long(&mut s, row(2));
        keymap(&mut s, KeymapId::Delete);
        assert_eq!((s.keymaps().list().len(), s.keymap_screen()), (1, Some(Screen::List)));
        // In use: asked first; then everyone is on Default.
        long(&mut s, row(1));
        keymap(&mut s, KeymapId::Delete);
        assert_eq!(s.keymap_screen(), Some(Screen::Delete(1)));
        let fx = keymap(&mut s, KeymapId::ConfirmDelete);
        assert!(fx.iter().any(|e| matches!(e, Effect::Save(Save::Settings(_)))), "{fx:?}");
        assert!(s.keymaps().list().is_empty());
        assert_eq!(s.settings().global.get(Setting::Keymap), 0);
        assert_eq!(s.settings().overrides_of("zelda-lttp").get(Setting::Keymap), None, "Default, as all games");
        // The Default row has no menu.
        long(&mut s, row(0));
        assert_eq!(s.keymap_screen(), Some(Screen::List));
        keymap(&mut s, KeymapId::Back);
        assert_eq!(s.keymap_screen(), None);
    }

    #[test]
    fn the_picker_closes_with_its_tab_and_listens_only_in_view() {
        let mut s = State::new(Init { keymaps: Keymaps::initial(), ..init() });
        picker(&mut s);
        tap(&mut s, Id::Row { list: ListId::Keymaps, i: 1, part: RowPart::Edit });
        assert_eq!(s.pad().route(), PadRoute::Listen);
        let fx = tap(&mut s, Id::Tab(Tab::Dock));
        assert_eq!(fx, vec![Effect::PadRoute(PadRoute::Ui)]);
        assert_eq!(s.keymap_screen(), None);
        // A game in front keeps the pad.
        picker(&mut s);
        tap(&mut s, Id::Row { list: ListId::Keymaps, i: 1, part: RowPart::Edit });
        s.apply(GameEvent::Started("zelda-lttp".into()).into());
        assert_eq!(s.pad().route(), PadRoute::Game);
        s.apply(Event::Listened(Input::Button(Button::A)));
        assert_eq!(s.keymap_screen(), Some(Screen::Edit(1)), "not listening: nothing");
    }

    /// Keymap 1's edit screen, the pad listening.
    fn edit_screen() -> State {
        let mut s = State::new(Init { keymaps: Keymaps::initial(), ..init() });
        picker(&mut s);
        tap(&mut s, Id::Row { list: ListId::Keymaps, i: 1, part: RowPart::Edit });
        assert_eq!(s.pad().route(), PadRoute::Listen);
        s
    }

    #[test]
    fn the_panel_selects_changes_and_resets() {
        let mut s = edit_screen();
        let a = Input::Button(Button::A);
        assert_eq!(s.keymap_edit(), EditView::default(), "the main layer, nothing selected");
        assert_eq!(keymap(&mut s, KeymapId::Change), vec![], "nothing to change");
        keymap(&mut s, KeymapId::Input(a));
        assert_eq!((s.keymap_screen(), s.keymap_edit().selected), (Some(Screen::Edit(1)), Some(a)), "a touch selects");
        s.apply(Event::Listened(Input::Button(Button::B)));
        assert_eq!(s.keymap_edit().selected, Some(Input::Button(Button::B)), "so does a press");
        s.apply(Event::Listened(a));
        keymap(&mut s, KeymapId::Change);
        assert_eq!(s.keymap_screen(), Some(Screen::MapTo { id: 1, input: a }));
        keymap(&mut s, KeymapId::Target(Target::Mouse(MouseButton::Left)));
        assert_eq!((s.keymap_screen(), s.keymap_edit().selected), (Some(Screen::Edit(1)), Some(a)), "back with it still selected");
        assert_eq!(s.keymaps().get(1).unwrap().get(a).map(|b| b.output), Some(Output::Mouse(MouseButton::Left)));
        let fx = keymap(&mut s, KeymapId::Reset);
        assert!(matches!(fx.as_slice(), [Effect::Save(Save::Keymaps(_))]), "{fx:?}");
        assert_eq!(s.keymaps().get(1).unwrap().get(a), None, "as on the pad");
        assert_eq!(keymap(&mut s, KeymapId::Reset), vec![], "already");
    }

    #[test]
    fn the_back_layer_is_shown_edited_and_falls_through() {
        let mut s = edit_screen();
        let a = Input::Button(Button::A);
        // The pad's Back button flips the layer while the pad listens (the UI activates this).
        assert_eq!(s.pad_back(), Some(Id::Keymap(KeymapId::Layer(Layer::Back))));
        keymap(&mut s, KeymapId::Layer(Layer::Back));
        assert_eq!(s.keymap_edit().layer, Layer::Back);
        assert_eq!(s.pad_back(), Some(Id::Keymap(KeymapId::Layer(Layer::Main))));
        // A in the Back layer: a key; main keeps the Nintendo layout's B.
        open(&mut s, a);
        keymap(&mut s, KeymapId::Target(Target::Key));
        key(&mut s, Key::Char('y'));
        let k = s.keymaps().get(1).unwrap();
        assert_eq!(k.get_in(Layer::Back, a).map(|b| b.output), Some(Output::Key(Chord::of(Key::Char('y')))));
        assert_eq!(k.get(a).map(|b| b.output), Some(Output::Pad(PadOut::Button(Button::B))));
        assert!(s.keymaps().text().contains("layer back\na = key y\n"), "{}", s.keymaps().text());
        // X as on the pad, over main's Y; the gyro off in it.
        let x = Input::Button(Button::X);
        open(&mut s, x);
        keymap(&mut s, KeymapId::Target(Target::Own));
        open(&mut s, Input::Gyro);
        keymap(&mut s, KeymapId::Target(Target::Own));
        let k = s.keymaps().get(1).unwrap();
        assert_eq!(k.get_in(Layer::Back, x).map(|b| b.output), Some(Output::Pad(PadOut::Button(Button::X))));
        assert_eq!(k.get_in(Layer::Back, Input::Gyro).map(|b| b.output), Some(Output::Nothing));
        // Reset in the Back layer: as the main layer again.
        keymap(&mut s, KeymapId::Input(x));
        keymap(&mut s, KeymapId::Reset);
        assert_eq!(s.keymaps().get(1).unwrap().get_in(Layer::Back, x), None);
        assert_eq!(s.keymaps().get(1).unwrap().get(x).map(|b| b.output), Some(Output::Pad(PadOut::Button(Button::Y))));
        // The keymap sent to the pad has both layers.
        keymap(&mut s, KeymapId::Back);
        tap(&mut s, row(1));
        let fx = s.apply(GameEvent::Started("zelda-lttp".into()).into()).1;
        assert_eq!(sent(&fx).map(|k| (k.bindings().len(), k.layer(Layer::Back).len())), Some((4, 2)), "{fx:?}");
        // Off the edit screen the pad's Back goes back.
        let mut s = edit_screen();
        open(&mut s, a);
        assert_eq!(s.pad_back(), Some(Id::Keymap(KeymapId::Back)), "from \"map to\": back");
    }

    #[test]
    fn no_changes_clears_the_layer_shown_once_confirmed() {
        let mut s = edit_screen();
        keymap(&mut s, KeymapId::Layer(Layer::Back));
        open(&mut s, Input::Button(Button::LB));
        keymap(&mut s, KeymapId::Target(Target::Nothing));
        keymap(&mut s, KeymapId::Clear);
        assert!(s.keymap_edit().clearing, "asked first");
        // Back (or Home) instead: not cleared, still on the edit screen.
        keymap(&mut s, KeymapId::Back);
        assert_eq!((s.keymap_screen(), s.keymap_edit().clearing), (Some(Screen::Edit(1)), false));
        keymap(&mut s, KeymapId::Clear);
        let fx = keymap(&mut s, KeymapId::ConfirmClear);
        assert!(matches!(fx.as_slice(), [Effect::Save(Save::Keymaps(_))]), "{fx:?}");
        let k = s.keymaps().get(1).unwrap();
        assert!(k.layer(Layer::Back).is_empty());
        assert_eq!(k.bindings().len(), 4, "the main layer kept");
        assert!(!s.keymap_edit().clearing);
        // The main layer cleared too: Default's behaviour, the keymap kept.
        keymap(&mut s, KeymapId::Layer(Layer::Main));
        keymap(&mut s, KeymapId::Clear);
        keymap(&mut s, KeymapId::ConfirmClear);
        assert!(s.keymaps().get(1).unwrap().bindings().is_empty());
        // A fresh edit screen starts on the main layer.
        keymap(&mut s, KeymapId::Layer(Layer::Back));
        keymap(&mut s, KeymapId::Back);
        tap(&mut s, Id::Row { list: ListId::Keymaps, i: 1, part: RowPart::Edit });
        assert_eq!(s.keymap_edit(), EditView::default());
    }

    #[test]
    fn parameters_slide_and_switch_whatever_the_mapping() {
        let mut s = edit_screen();
        let ls = Input::Stick(Side::Left);
        keymap(&mut s, KeymapId::Input(ls));
        // As on the pad: tuned all the same; the slider changes it as it goes, saved on lifting.
        s.apply(Slide::Start { control: Control::Param(Param::Deadzone), x: 300.0, width: 620.0 }.into());
        let slider = s.keymap_edit().slider.unwrap();
        assert_eq!((slider.start, slider.control), (0, Control::Param(Param::Deadzone)));
        let (_, fx) = s.apply(Slide::To { x: 300.0 + 0.12 * slider.length }.into());
        assert!(fx.iter().all(|e| !matches!(e, Effect::Save(_))), "not saved while sliding: {fx:?}");
        let k = s.keymaps().get(1).unwrap().get(ls).copied();
        assert_eq!(k.map(|b| (b.output, b.deadzone)), Some((Output::Stick(Side::Left), 12)));
        assert_eq!(s.keymap_edit().slider.map(|s| s.value), Some(12));
        let (_, fx) = s.apply(Slide::End.into());
        assert!(matches!(fx.as_slice(), [Effect::Save(Save::Keymaps(_))]), "{fx:?}");
        assert_eq!(s.keymap_edit().slider, None);
        // Kept within its range; a switch flips.
        slide(&mut s, Param::Deadzone, 90);
        assert_eq!(s.keymaps().get(1).unwrap().get(ls).map(|b| b.deadzone), Some(50));
        keymap(&mut s, KeymapId::Toggle(Param::InvertY));
        assert_eq!(s.keymaps().get(1).unwrap().get(ls).map(|b| b.invert_y), Some(true));
        // Back to the defaults: no binding left in the main layer.
        keymap(&mut s, KeymapId::Toggle(Param::InvertY));
        slide(&mut s, Param::Deadzone, 0);
        assert_eq!(s.keymaps().get(1).unwrap().get(ls), None);
        // In the Back layer it starts from the main layer's and is the Back layer's own.
        slide(&mut s, Param::Full, 80);
        keymap(&mut s, KeymapId::Layer(Layer::Back));
        slide(&mut s, Param::Curve, 30);
        let k = s.keymaps().get(1).unwrap();
        assert_eq!(k.get_in(Layer::Back, ls).map(|b| (b.full, b.curve)), Some((80, 30)));
        assert_eq!(k.get(ls).map(|b| (b.full, b.curve)), Some((80, 0)));
        // A parameter the input hasn't: no slider. Buttons have none.
        assert_eq!(slide_start(&mut s, Param::Threshold), None);
        keymap(&mut s, KeymapId::Input(Input::Button(Button::A)));
        assert_eq!(slide_start(&mut s, Param::Speed), None);
        // The bar's sliders are unchanged by it.
        s.apply(Slide::Start { control: Control::Volume, x: 300.0, width: 620.0 }.into());
        assert!(s.levels().slider().is_some() && s.keymap_edit().slider.is_none());
        s.apply(Slide::End.into());
    }

    fn slide_start(s: &mut State, param: Param) -> Option<Slider> {
        s.apply(Slide::Start { control: Control::Param(param), x: 300.0, width: 620.0 }.into());
        let slider = s.keymap_edit().slider;
        s.apply(Slide::End.into());
        slider
    }
}
