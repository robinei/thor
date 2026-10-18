//! The settings panel's rules: which game it is about (by what is in front) and which scope it edits,
//! changes applied at once when they change the settings in force, the environment editor typing on the
//! keyboard, and the dark second screen.

use crate::State;
use crate::editor::{Done, Editor, Purpose};
use crate::effect::{Effect, Save};
use crate::id::{EditorId, SettingsId};
use crate::keyboard::Stroke;
use crate::program::Program;
use crate::running::Front;
use crate::settings::{Setting, Values};

/// What the settings panel edits: the global settings, or the game in context's overrides.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum Scope {
    #[default]
    Game,
    Global,
}

/// Where a settings tile's value comes from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TileOrigin {
    /// The game's own choice.
    Own,
    /// The global value, which the game follows (it has no choice of its own).
    Inherited,
    /// The global settings are being edited, or the setting is global only.
    Global,
}

impl State {
    /// The settings in force: the program in force's (see [`crate::program`]), else the global ones.
    pub fn in_force(&self) -> Values {
        self.session.program.as_ref().map_or(self.settings.global, |id| self.settings_of(id))
    }

    /// The folders entry `id` is in (outermost first), whose settings it inherits.
    fn folders_of(&self, id: &str) -> Vec<std::sync::Arc<str>> {
        let library = self.shelf.library();
        let Some(i) = library.games.iter().position(|g| &*g.id == id) else {
            return Vec::new();
        };
        library.groups_of(i).iter().filter_map(|&g| library.games.get(g)).map(|g| g.id.clone()).collect()
    }

    /// The settings entry `id` runs with: the global ones, its folders' overrides, its own.
    pub fn settings_of(&self, id: &str) -> Values {
        let folders = self.folders_of(id);
        self.settings.of_in(id, &folders.iter().map(|f| &**f).collect::<Vec<_>>())
    }

    /// The environment entry `id` gets: the global variables, its folders', its own.
    pub fn env_of(&self, id: &str) -> Vec<(String, String)> {
        let folders = self.folders_of(id);
        self.settings.env_in(id, &folders.iter().map(|f| &**f).collect::<Vec<_>>())
    }

    /// The program the settings panel is about, by what is in front: the entry whose window is in front,
    /// shelf's selection, or none (a window of no program: the panel edits the defaults).
    pub fn context(&self) -> Option<Program<'_>> {
        let id = match self.session.front() {
            Front::Program => {
                self.session.windows.iter().find(|w| w.focused && !w.down).and_then(|w| self.program_of(w)).or_else(|| self.session.program.clone())
            }
            Front::Shelf => self.shelf.selected().cloned(),
            Front::Other => None,
        }?;
        self.program(&id)
    }

    /// What the settings panel edits now: the program in context (if the scope is the program's), or
    /// none for the defaults.
    pub fn editing(&self) -> Option<Program<'_>> {
        self.context().filter(|_| self.bottom.scope == Scope::Game)
    }

    /// The second screen is dark: a program is in force that wants it so, and it hasn't been tapped
    /// since.
    pub fn bottom_dark(&self) -> bool {
        self.session.program.is_some() && self.in_force().get(Setting::BottomScreen) == 1 && !self.bottom.woken
    }

    /// The value a tile shows and steps from: the game's effective one, or the global one.
    pub fn tile_value(&self, setting: Setting) -> u16 {
        match self.editing() {
            Some(g) if !setting.global_only() => self.settings_of(&g.id).get(setting),
            _ => self.settings.global.get(setting),
        }
    }

    /// Where a tile's value comes from.
    pub fn tile_origin(&self, setting: Setting) -> TileOrigin {
        match self.editing() {
            Some(g) if !setting.global_only() => match self.settings.overrides_of(&g.id).get(setting) {
                Some(_) => TileOrigin::Own,
                None => TileOrigin::Inherited,
            },
            _ => TileOrigin::Global,
        }
    }

    /// A tap or long press in the settings tab.
    pub(crate) fn on_settings(&mut self, id: SettingsId, long: bool) -> Vec<Effect> {
        match id {
            SettingsId::Tile(Setting::Keymap) if !long => {
                self.open_keymaps();
                vec![]
            }
            SettingsId::Tile(setting) => {
                let value = if long { None } else { Some(setting.next(self.tile_value(setting))) };
                self.change(setting, value)
            }
            SettingsId::Env => {
                self.open_editor();
                vec![]
            }
            SettingsId::Scope(scope) => {
                self.bottom.scope = scope;
                vec![]
            }
            SettingsId::Reset => self.reset(),
        }
    }

    /// The editor's buttons.
    pub(crate) fn on_editor(&mut self, id: EditorId) -> Vec<Effect> {
        match id {
            EditorId::Save => self.save_editor(),
            EditorId::Cancel => self.cancel_editor(),
        }
    }

    /// Changes `setting` where the panel edits (the game's overrides, or the global settings): to
    /// `value`, or back to the default (none). Saved, and applied at once if the settings in force
    /// changed.
    pub(crate) fn change(&mut self, setting: Setting, value: Option<u16>) -> Vec<Effect> {
        let before = self.in_force();
        let game = self.editing().map(|g| g.id.clone());
        self.settings.set(game.as_ref(), setting, value);
        self.saved_and_applied(before)
    }

    pub(crate) fn saved_and_applied(&self, before: Values) -> Vec<Effect> {
        let mut effects = vec![Effect::Save(Save::Settings(self.settings.clone()))];
        let now = self.in_force();
        if now != before {
            effects.push(Effect::Apply { game: self.session.program.clone(), settings: now });
        }
        effects
    }

    /// Opens the editor on what the panel edits: the game's variables, or all games'.
    fn open_editor(&mut self) {
        let game = self.editing().map(|g| g.id.clone());
        let text = self.settings.env_text(game.as_deref()).to_owned();
        self.bottom.open_editor(Editor { purpose: Purpose::Env { game }, text });
    }

    /// A key from the on-screen keyboard while the editor is open.
    pub(crate) fn edit(&mut self, stroke: Stroke) -> Vec<Effect> {
        match self.bottom.editor_mut().and_then(|e| e.key(stroke)) {
            Some(Done::Save) => self.save_editor(),
            Some(Done::Cancel) => self.cancel_editor(),
            None => vec![],
        }
    }

    /// The editor's Cancel: closed (a file's name: back to the tab the keyboard came up from).
    fn cancel_editor(&mut self) -> Vec<Effect> {
        match self.bottom.close_editor().map(|e| e.purpose) {
            Some(Purpose::FileName(_)) => self.file_name_cancelled(),
            _ => vec![],
        }
    }

    /// The editor's Save: the environment saved, or the network joined with the password.
    fn save_editor(&mut self) -> Vec<Effect> {
        let Some(editor) = self.bottom.close_editor() else {
            return vec![];
        };
        match editor.purpose {
            Purpose::Env { game } => {
                self.settings.set_env(game, &editor.text);
                vec![Effect::Save(Save::Settings(self.settings.clone()))]
            }
            Purpose::Password { ssid } => self.join(ssid, Some(editor.text)),
            Purpose::KeymapName(action) => self.named_keymap(action, &editor.text),
            Purpose::FileName(purpose) => self.file_named(purpose, &editor.text),
        }
    }

    /// Drops the edited game's overrides and environment.
    fn reset(&mut self) -> Vec<Effect> {
        let before = self.in_force();
        if let Some(id) = self.editing().map(|g| g.id.clone()) {
            self.settings.reset(&id);
        }
        self.saved_and_applied(before)
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use crate::keyboard::{Key, Named, find};
    use crate::settings::{self, Setting};
    use crate::testkit::*;
    use crate::*;

    /// Taps a tile `n` times.
    fn taps(s: &mut State, setting: Setting, n: usize) -> Vec<Effect> {
        let mut fx = vec![];
        for _ in 0..n {
            fx = tap(s, Id::Settings(SettingsId::Tile(setting)));
        }
        fx
    }

    fn scope(s: &mut State, scope: Scope) {
        tap(s, Id::Settings(SettingsId::Scope(scope)));
    }

    fn state_with(settings: &settings::Settings) -> State {
        State::new(Init { settings: settings.clone(), ..init() })
    }

    #[test]
    fn an_entry_takes_its_folders_settings() {
        let mut settings = settings::Settings::default();
        settings.set(Some(&"tv:Cosmos".into()), Setting::FpsCap, Some(30));
        settings.set(Some(&"tv:Cosmos/Season 1".into()), Setting::Power, Some(0));
        settings.set_env(Some("tv:Cosmos".into()), "MPV=1");
        let mut s = state_with(&settings);
        let episode: std::sync::Arc<str> = "tv:Cosmos/Season 1/2. One Voice in the Cosmic Fugue.mkv".into();
        let fx = s.launch(episode.clone());
        let launch = fx.iter().find_map(|e| match e {
            Effect::Launch { settings, env, .. } => Some((settings.get(Setting::FpsCap), settings.get(Setting::Power), env.clone())),
            _ => None,
        });
        assert_eq!(launch, Some((30, 0, vec![("MPV".to_owned(), "1".to_owned())])), "the series', the season's");
        assert_eq!(s.settings_of("tv:Cosmos/Season 2").get(Setting::Power), 1, "not the other season's");
    }

    #[test]
    fn games_override_the_global_settings() {
        let mut s = state();
        scope(&mut s, Scope::Global);
        taps(&mut s, Setting::FpsCap, 4); // 30, 40, 45, 60
        scope(&mut s, Scope::Game);
        let fx = taps(&mut s, Setting::Power, 1); // Balanced -> Performance
        let Some(Effect::Save(Save::Settings(saved))) = fx.first() else {
            unreachable!("{fx:?}");
        };
        assert_eq!(saved, s.settings(), "saved whole");
        assert_eq!(saved.overrides_of("zelda-lttp").get(Setting::Power), Some(2));
        let zelda = s.settings().of("zelda-lttp");
        assert_eq!((zelda.get(Setting::Power), zelda.get(Setting::FpsCap)), (2, 60));
        assert_eq!(s.settings().of("ff6").get(Setting::Power), 1);
        assert_eq!((s.tile_origin(Setting::Power), s.tile_origin(Setting::FpsCap)), (TileOrigin::Own, TileOrigin::Inherited));
        let fresh = state_with(saved);
        assert_eq!(fresh.settings().of("zelda-lttp"), zelda);
        assert_eq!(fresh.settings().global, s.settings().global);
        // A long press drops one override; Reset all of them.
        taps(&mut s, Setting::Rumble, 1);
        long(&mut s, Id::Settings(SettingsId::Tile(Setting::Power)));
        assert_eq!(s.settings().overrides_of("zelda-lttp").get(Setting::Power), None);
        assert_eq!(s.settings().overrides_of("zelda-lttp").get(Setting::Rumble), Some(0));
        tap(&mut s, Id::Settings(SettingsId::Reset));
        assert!(s.settings().overrides.is_empty());
        // Global-only settings change the global value even on a game's tab.
        taps(&mut s, Setting::ChargeLimit, 1);
        assert_eq!(s.settings().global.get(Setting::ChargeLimit), 90);
        assert_eq!(s.tile_origin(Setting::ChargeLimit), TileOrigin::Global);
    }

    #[test]
    fn the_launch_carries_the_games_settings_and_environment() {
        let mut settings = settings::Settings::default();
        settings.set_env(None, "A=1 B=2");
        settings.set_env(Some("zelda-lttp".into()), "B=3 'C=x y'");
        let mut s = state_with(&settings);
        taps(&mut s, Setting::FpsCap, 2);
        let fx = tap(&mut s, Id::Shelf(ShelfId::Launch));
        let Some(Effect::Launch { settings, env, .. }) = fx.iter().find(|e| matches!(e, Effect::Launch { .. })) else { unreachable!("{fx:?}") };
        assert_eq!(settings.get(Setting::FpsCap), 40);
        let pairs: Vec<(&str, &str)> = env.iter().map(|(k, v)| (k.as_str(), v.as_str())).collect();
        assert_eq!(pairs, [("A", "1"), ("B", "2"), ("B", "3"), ("C", "x y")], "the game's last, so they win");
    }

    #[test]
    fn a_running_game_takes_changes_at_once() {
        let mut s = state();
        s.apply(GameEvent::Started("zelda-lttp".into()).into());
        tap(&mut s, game("ff6"));
        // The running game is the context, not the selection.
        let fx = taps(&mut s, Setting::FpsCap, 1);
        assert!(fx.iter().any(|e| matches!(e, Effect::Apply { game: Some(g), settings } if &**g == "zelda-lttp" && settings.get(Setting::FpsCap) == 30)));
        // A global change reaches it only where it doesn't override.
        scope(&mut s, Scope::Global);
        assert!(!taps(&mut s, Setting::FpsCap, 1).iter().any(|e| matches!(e, Effect::Apply { .. })), "its own cap stays: nothing to apply");
        let fx = taps(&mut s, Setting::Power, 1);
        assert!(fx.iter().any(|e| matches!(e, Effect::Apply { settings, .. } if settings.get(Setting::Power) == 2)));
        let fx = played(&mut s, "zelda-lttp");
        assert_eq!(fx, vec![Effect::Apply { game: None, settings: s.settings().global }, Effect::PadRoute(PadRoute::Ui)], "and the pad back to shelf");
    }

    #[test]
    fn a_game_can_start_the_pad_mouse_and_darken_the_second_screen() {
        let mut s = state();
        taps(&mut s, Setting::PadMouse, 1);
        taps(&mut s, Setting::BottomScreen, 1);
        let (_, fx) = s.apply(GameEvent::Started("zelda-lttp".into()).into());
        assert!(fx.contains(&Effect::PadRoute(PadRoute::Mouse)), "{fx:?}");
        assert!(s.bottom_dark());
        tap(&mut s, Id::Wake);
        assert!(!s.bottom_dark());
        let fx = played(&mut s, "zelda-lttp");
        assert!(fx.contains(&Effect::PadRoute(PadRoute::Ui)), "{fx:?}");
        assert!(!s.pad().mouse_on());
    }

    #[test]
    fn without_a_game_the_panel_edits_the_global_settings() {
        let mut empty = State::new(Init { library: Library::default(), ..init() });
        tap(&mut empty, Id::Tab(Tab::Settings));
        assert_eq!(empty.bottom().tab(), Tab::Settings, "always available");
        taps(&mut empty, Setting::FpsCap, 2);
        assert_eq!(empty.settings().global.get(Setting::FpsCap), 40);
    }

    #[test]
    fn the_environment_editor_takes_the_keyboard() {
        let mut s = state();
        tap(&mut s, Id::Tab(Tab::Settings));
        tap(&mut s, Id::Settings(SettingsId::Env));
        let key = |s: &mut State, k| tap(s, Id::Key(find(k).unwrap()));
        for c in ['d', 'x', 'v', 'k'] {
            assert_eq!(key(&mut s, Key::Char(c)), vec![], "typed into the editor, not sent");
        }
        key(&mut s, Key::Named(Named::Backspace));
        key(&mut s, Key::Char('='));
        key(&mut s, Key::Char('1'));
        let fx = key(&mut s, Key::Named(Named::Enter));
        assert!(matches!(fx.first(), Some(Effect::Save(Save::Settings(t))) if t.env_text(Some("zelda-lttp")) == "dxv=1"), "{fx:?}");
        assert_eq!(s.bottom().editor(), None);
        assert_eq!(s.settings().env_of("zelda-lttp"), vec![("dxv".to_owned(), "1".to_owned())]);
        let fresh = state_with(s.settings());
        assert_eq!(fresh.settings().env_of("zelda-lttp"), s.settings().env_of("zelda-lttp"));
    }

    #[test]
    fn a_text_field_doesnt_take_the_screen_from_an_open_editor() {
        let mut s = state();
        s.apply(Event::Windows(vec![foot()]));
        tap(&mut s, Id::Tab(Tab::Settings));
        tap(&mut s, Id::Settings(SettingsId::Env));
        assert!(s.bottom().editor().is_some());
        s.apply(Event::TextInput(true));
        assert!(s.bottom().editor().is_some() && s.bottom().tab() == Tab::Settings, "its typing would be lost");
    }

    #[test]
    fn an_editor_stays_with_its_tab() {
        let mut s = state();
        tap(&mut s, Id::Tab(Tab::Settings));
        tap(&mut s, Id::Settings(SettingsId::Env));
        let key = |s: &mut State, k| tap(s, Id::Key(find(k).unwrap()));
        key(&mut s, Key::Char('a'));
        // The keyboard tab: its keys go to the app, not the editor left in the settings.
        tap(&mut s, Id::Tab(Tab::Keyboard));
        assert_eq!(s.bottom().editor(), None);
        assert_eq!(key(&mut s, Key::Char('b')).len(), 1, "sent");
        tap(&mut s, Id::Tab(Tab::Settings));
        assert_eq!(s.bottom().editor().map(|e| e.text.as_str()), Some("a"), "as left");
    }
}
