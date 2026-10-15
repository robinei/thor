//! The keymaps' screens on the second screen, over the settings tab: the picker (Default, then the
//! keymaps with how many games use each, and New), a keymap's menu (Edit, Rename, Duplicate, Delete,
//! deleting one in use after a confirmation), the edit screen ("map from": the pad's inputs, chosen by
//! touch or by pressing them on the pad, which listens meanwhile), what an input maps to ("map to": the
//! targets that fit its kind, with speed, threshold and the gyro's hold), and a key chord or a stick's
//! four keys typed on the keyboard. Names are typed in the editor. Also the keymap in force, sent to the
//! pad whenever it changes.

use crate::State;
use crate::editor::{Editor, KeymapName, Purpose};
use crate::effect::{Effect, Save};
use crate::id::{KeymapId, Target};
use crate::items::ItemKey;
use crate::keyboard::{Key, Stroke};
use crate::keymap::{Binding, Chord, Input, Keymap, Keymaps, Kind, Output, PadOut};
use crate::scroll::{RowPart, Rows};
use crate::settings::Setting;

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
            Screen::MapTo { id, .. } | Screen::PressPad { id, .. } | Screen::Capture { id, .. } => Some(id),
        }
    }
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
                self.bottom.keymaps = Some(Screen::Edit(id));
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
                self.bottom.keymaps = Some(Screen::Edit(k));
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
            (KeymapId::Input(input), Screen::Edit(k)) if long => self.bind(k, input, None),
            (KeymapId::Input(input), Screen::Edit(k)) => {
                self.bottom.keymaps = Some(Screen::MapTo { id: k, input });
                vec![]
            }
            (KeymapId::Target(target), Screen::MapTo { id: k, input }) => self.map_to(k, input, target),
            (KeymapId::Speed(speed), Screen::MapTo { id: k, input }) => self.adjust(k, input, |b| b.speed = speed),
            (KeymapId::Threshold(t), Screen::MapTo { id: k, input }) => self.adjust(k, input, |b| b.threshold = t),
            (KeymapId::Hold(hold), Screen::MapTo { id: k, input }) => self.adjust(k, input, |b| b.hold = hold),
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
        self.bottom.keymaps = match self.bottom.keymaps {
            None | Some(Screen::List) => None,
            Some(Screen::Menu(_) | Screen::Delete(_) | Screen::Edit(_)) => Some(Screen::List),
            Some(Screen::MapTo { id, .. }) => Some(Screen::Edit(id)),
            Some(Screen::PressPad { id, input } | Screen::Capture { id, input, .. }) => {
                self.keyboard.take_modifiers();
                Some(Screen::MapTo { id, input })
            }
        };
    }

    /// An input pressed on the pad while it listens: on the edit screen, its "map to" opens; waiting
    /// for a pad button, it is the one.
    pub(crate) fn listened(&mut self, input: Input) -> Vec<Effect> {
        if self.pad.route() != crate::PadRoute::Listen {
            return vec![];
        }
        match self.bottom.keymaps {
            Some(Screen::Edit(id)) => {
                self.bottom.keymaps = Some(Screen::MapTo { id, input });
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
                self.bottom.keymaps = Some(Screen::Edit(id));
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

    /// `input` of keymap `id` bound to `binding` (none: as on the pad); saved if it changed.
    fn bind(&mut self, id: u16, input: Input, binding: Option<Binding>) -> Vec<Effect> {
        let changed = self.keymaps.get_mut(id).is_some_and(|k| k.set(input, binding));
        if changed { vec![self.save_keymaps()] } else { vec![] }
    }

    /// Changes `input`'s binding, if it has one.
    fn adjust(&mut self, id: u16, input: Input, change: impl FnOnce(&mut Binding)) -> Vec<Effect> {
        let Some(mut binding) = self.keymaps.get(id).and_then(|k| k.get(input)).copied() else {
            return vec![];
        };
        change(&mut binding);
        self.bind(id, input, Some(binding))
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

    /// `input` maps to `output` (its speed, threshold and hold kept): back to the edit screen, unless
    /// the "map to" screen has more to set for it (a speed, a threshold, the gyro's hold).
    fn choose(&mut self, id: u16, input: Input, output: Output) -> Vec<Effect> {
        if !output.fits(input.kind()) {
            return vec![];
        }
        let kept = self.keymaps.get(id).and_then(|k| k.get(input)).copied();
        let binding = Binding { output, ..kept.unwrap_or(Binding::to(output)) };
        let effects = self.bind(id, input, Some(binding));
        let more = match input.kind() {
            Kind::Gyro => output != Output::Nothing,
            Kind::Trigger => output.digital(),
            Kind::Stick => output == Output::Pointer,
            Kind::Digital => false,
        };
        self.bottom.keymaps = Some(if more { Screen::MapTo { id, input } } else { Screen::Edit(id) });
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
        // A pressed on the pad: its "map to"; a key chord typed: Ctrl, then x.
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
        keymap(&mut s, KeymapId::Input(Input::Stick(Side::Right)));
        keymap(&mut s, KeymapId::Target(Target::Pointer));
        assert_eq!(s.keymap_screen(), Some(Screen::MapTo { id: 2, input: Input::Stick(Side::Right) }), "its speed next");
        keymap(&mut s, KeymapId::Speed(5));
        assert_eq!(s.keymaps().get(2).unwrap().get(Input::Stick(Side::Right)).map(|b| (b.output, b.speed)), Some((Output::Pointer, 5)));
        // The left stick to four keys, in turn.
        keymap(&mut s, KeymapId::Back);
        keymap(&mut s, KeymapId::Input(Input::Stick(Side::Left)));
        keymap(&mut s, KeymapId::Target(Target::Keys));
        type_text(&mut s, "wasd");
        let w = |c| Chord::of(Key::Char(c));
        assert_eq!(s.keymaps().get(2).unwrap().get(Input::Stick(Side::Left)).map(|b| b.output), Some(Output::Keys([w('w'), w('a'), w('s'), w('d')])));
        // The gyro to the pointer while LT is held; Shift alone on LB (Done with only a modifier).
        keymap(&mut s, KeymapId::Input(Input::Gyro));
        keymap(&mut s, KeymapId::Target(Target::Pointer));
        keymap(&mut s, KeymapId::Hold(Some(Input::Trigger(Side::Left))));
        assert_eq!(s.keymaps().get(2).unwrap().get(Input::Gyro).and_then(|b| b.hold), Some(Input::Trigger(Side::Left)));
        keymap(&mut s, KeymapId::Back);
        keymap(&mut s, KeymapId::Input(Input::Button(Button::LB)));
        keymap(&mut s, KeymapId::Target(Target::Key));
        key(&mut s, Key::Mod(Modifier::Shift));
        keymap(&mut s, KeymapId::Done);
        assert_eq!(s.keymaps().get(2).unwrap().get(Input::Button(Button::LB)).map(|b| b.output), Some(Output::Key(Chord { shift: true, ..Chord::default() })));
        assert!(!s.keyboard().shift.on(), "used up");
        // B to a pad button pressed on the pad (Y); the trigger to a mouse button past 75 %.
        keymap(&mut s, KeymapId::Input(Input::Button(Button::B)));
        keymap(&mut s, KeymapId::Target(Target::PressPad));
        assert_eq!(s.pad().route(), PadRoute::Listen);
        s.apply(Event::Listened(Input::Button(Button::Y)));
        assert_eq!(s.keymaps().get(2).unwrap().get(Input::Button(Button::B)).map(|b| b.output), Some(Output::Pad(PadOut::Button(Button::Y))));
        keymap(&mut s, KeymapId::Input(Input::Trigger(Side::Right)));
        keymap(&mut s, KeymapId::Target(Target::Mouse(MouseButton::Left)));
        keymap(&mut s, KeymapId::Threshold(75));
        assert_eq!(s.keymaps().get(2).unwrap().get(Input::Trigger(Side::Right)).map(|b| b.threshold), Some(75));
        // A long press on an input: as on the pad again. Home leaves the edit screen.
        keymap(&mut s, KeymapId::Back);
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
        keymap(&mut s, KeymapId::Input(Input::Button(Button::A)));
        assert_eq!(keymap(&mut s, KeymapId::Target(Target::Pointer)), vec![], "a button can't move the pointer");
        keymap(&mut s, KeymapId::Target(Target::Wheel(Dir::Down)));
        assert_eq!(s.keymaps().get(1).unwrap().get(Input::Button(Button::A)).map(|b| b.output), Some(Output::Wheel(Dir::Down)));
        keymap(&mut s, KeymapId::Input(Input::Button(Button::A)));
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
}
