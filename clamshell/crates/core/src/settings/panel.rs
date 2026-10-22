//! The settings panel's rules. The panel is about an item chosen by what is in front. It edits one
//! level: All, the item's runner (Desktop for an app, Steam for a Steam game), or the item itself.
//! Changes are applied at once when they change the settings in force. The panel also opens the
//! environment editor.

use crate::State;
use crate::bottom::editor::{Editor, Purpose};
use crate::effect::{Effect, Save};
use crate::id::SettingsId;
use crate::session::program::Program;
use crate::session::running::Front;
use crate::settings::{Setting, Values};

/// What the settings panel edits: the global settings (All), the runner of the item in context, or the
/// item itself. The item can be a game, an app or a film, but not a browsed file or a folder, nor an
/// item of a runner without item settings. Content edits the runner's level when the item has no level
/// of its own.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum Scope {
    Global,
    Runner,
    #[default]
    Content,
}

/// Where a settings tile's value comes from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TileOrigin {
    /// The game's own choice.
    Own,
    /// The global value, which the game follows because it has no choice of its own.
    Inherited,
    /// The global settings are being edited, or the setting is global only.
    Global,
}

impl State {
    /// The settings in force: those of the program in force (see [`crate::session::program`]), or else
    /// the global ones.
    pub fn in_force(&self) -> Values {
        self.session.program.as_ref().map_or(self.settings.global, |id| self.settings_of(id))
    }

    /// The level that entry `id` takes its settings from before its own, which is its runner's, if it
    /// has one.
    fn runner_of(&self, id: &str) -> Option<&str> {
        self.entry(id).and_then(|g| g.runner.as_ref()).map(|r| &*r.key)
    }

    /// The settings entry or level `id` runs with: the global ones, then its runner's, then its own.
    pub fn settings_of(&self, id: &str) -> Values {
        self.settings.of_in(id, self.runner_of(id).as_slice())
    }

    /// What entry or level `id` does while hidden ([`Setting::Background`]). If it is set to Pauses
    /// games, the default, it is paused if it is a game (see [`State::is_games`]) and runs otherwise.
    pub fn background_of(&self, id: &str) -> crate::settings::InBackground {
        use crate::settings::InBackground;
        match self.settings_of(id).choice::<InBackground>() {
            InBackground::PausesGames if self.is_games(id) => InBackground::Pauses,
            InBackground::PausesGames => InBackground::Runs,
            b => b,
        }
    }

    /// Whether entry `id` is a game, rather than an app, a game launcher, a film, a file or a group. For
    /// a runner's level `id`, whether all of the runner's entries are games. Games are paused when hidden
    /// by default, as consoles do. A game that minds, such as one played online, can be set to run.
    pub fn is_games(&self, id: &str) -> bool {
        let game = |g: &crate::Entry| !g.app && g.browsed.is_none() && g.mpv_file().is_none() && !g.is_group();
        if let Some(g) = self.entry(id) {
            return game(g);
        }
        let mut of_runner = self.shelf.library().entries.iter().filter(|g| g.runner.as_ref().is_some_and(|r| &*r.key == id)).peekable();
        of_runner.peek().is_some() && of_runner.all(game)
    }

    /// The environment entry `id` gets: the global variables, then its runner's, then its own.
    pub fn env_of(&self, id: &str) -> Vec<(String, String)> {
        self.settings.env_in(id, self.runner_of(id).as_slice())
    }

    /// The levels below All that the settings panel offers for the item in context, with their titles.
    /// Normally these are its runner's and its own (`All | SNES | Zelda`, `All | Desktop | Spotify`).
    /// There is only its runner's for a browsed file, a folder, or a runner's item without item settings
    /// (`All | Text`). There is only its own for an item without a runner, such as ClamShell's tools.
    pub fn setting_levels(&self) -> [Option<(Scope, Program<'_>)>; 2] {
        let Some((item, g)) = self.context().and_then(|item| Some((item.clone(), self.entry(&item.id)?))) else {
            return [None, None];
        };
        match &g.runner {
            Some(r) => {
                let runner = Some((Scope::Runner, Program { id: r.key.clone(), title: &r.name }));
                let own = (r.item_settings && g.browsed.is_none() && !g.is_group()).then_some((Scope::Content, item));
                [runner, own]
            }
            _ => [Some((Scope::Runner, item)), None],
        }
    }

    /// For runners that come with one of ClamShell's keymaps (see [`crate::Library::runner_keymaps`]) and
    /// have no keymap chosen and none of their own: makes that keymap their own, chooses it, and saves.
    pub(crate) fn follow_runner_keymaps(&mut self) -> Vec<Effect> {
        let library = self.shelf.library();
        let wanting: Vec<(std::sync::Arc<str>, crate::keymap::Keymap)> = library
            .runner_keymaps
            .iter()
            .filter(|(key, _)| self.settings.overrides_of(key).get(Setting::Keymap).is_none() && self.keymaps.own_of(key).is_none())
            .filter_map(|(key, name)| Some((key.clone(), crate::keymap::preset(name)?)))
            .collect();
        if wanting.is_empty() {
            return vec![];
        }
        for (key, preset) in &wanting {
            if let Some(id) = self.keymaps.make_own_from(preset, key) {
                self.settings.set(Some(key), Setting::Keymap, Some(id));
            }
        }
        vec![Effect::Save(Save::Keymaps(self.keymaps.clone())), Effect::Save(Save::Settings(self.settings.clone()))]
    }

    /// The item the settings panel is about, chosen by what is in front. It is the entry whose window is
    /// in front, or shelf's selection, where a browsed file that belongs to a library entry stands for
    /// that entry. It is none for a window of no program, and then the panel edits the defaults.
    pub fn context(&self) -> Option<Program<'_>> {
        let id = match self.session.front() {
            Front::Program => {
                self.session.windows.iter().find(|w| w.focused && !w.down).and_then(|w| self.program_of(w)).or_else(|| self.session.program.clone())
            }
            Front::Shelf => self.shelf.selected().cloned().map(|id| self.shelf.entry_of_file(&id).unwrap_or(id)),
            Front::Other => None,
        }?;
        self.program(&id)
    }

    /// What the settings panel edits now: the level of the chosen scope, or none for All. Content edits
    /// the runner's level when the item has none of its own.
    pub fn editing(&self) -> Option<Program<'_>> {
        if self.bottom.scope() == Scope::Global {
            return None;
        }
        let [runner, own] = self.setting_levels();
        let chosen = |l: &Option<(Scope, Program<'_>)>| l.as_ref().is_some_and(|(scope, _)| *scope == self.bottom.scope());
        match (chosen(&runner), own) {
            (true, _) => runner.map(|(_, p)| p),
            (false, Some((_, p))) => Some(p),
            (false, None) => runner.map(|(_, p)| p),
        }
    }

    /// Whether the settings panel edits a runner whose launches hand their app over to another program,
    /// or an item of such a runner. Android is one: Waydroid's session runs its apps. Anything that works
    /// through the launch's process, such as [`Setting::through_launch`] settings and the environment,
    /// has no effect there.
    pub fn handed_over(&self) -> bool {
        self.editing().is_some() && self.context().and_then(|item| self.entry(&item.id)).and_then(|g| g.runner.as_ref()).is_some_and(|r| r.handover)
    }

    /// The scope the settings panel shows as chosen: the one chosen, or the level that stands in for it.
    pub fn scope_shown(&self) -> Scope {
        match self.editing() {
            None => Scope::Global,
            Some(p) => self.setting_levels().into_iter().flatten().find(|(_, l)| l.id == p.id).map_or(Scope::Global, |(scope, _)| scope),
        }
    }

    /// The value a tile shows and steps from: the game's effective value, or the global one.
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
                self.bottom.set_scope(scope);
                vec![]
            }
            SettingsId::Reset => self.reset(),
        }
    }

    /// Changes `setting` where the panel edits, in the game's overrides or the global settings. It is set
    /// to `value`, or back to the default for `None`. The change is saved, and applied at once if the
    /// settings in force changed.
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
            effects.push(Effect::Apply { entry: self.session.program.clone(), settings: now });
        }
        effects
    }

    /// Opens the editor on what the panel edits: the game's variables, or those for all games.
    fn open_editor(&mut self) {
        let game = self.editing().map(|g| g.id.clone());
        let text = self.settings.env_text(game.as_deref()).to_owned();
        self.bottom.editors_mut().open(Editor { purpose: Purpose::Env { entry: game }, text });
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
    use crate::bottom::keyboard::{Key, Named, find};
    use crate::settings::{self, Choice, PowerProfile, Setting};
    use crate::testkit::*;
    use crate::*;
    use clamshell_library::runner_key;

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
    fn an_entry_takes_its_runners_settings_not_its_folders() {
        let mut library = clamshell_library::demo::library();
        for g in library.entries.iter_mut().filter(|g| g.id.starts_with("tv:")) {
            g.runner = Some(clamshell_library::RunnerOf::new("video", "Video", true));
        }
        let mut settings = settings::Settings::default();
        settings.set(Some(&runner_key("video")), Setting::FpsCap, Some(30));
        settings.set(Some(&"tv:Cosmos".into()), Setting::Power, Some(PowerProfile::Battery.value()));
        settings.set_env(Some(runner_key("video")), "MPV=1");
        let mut s = State::new(Init { library, settings, ..init() });
        let episode: std::sync::Arc<str> = "tv:Cosmos/Season 1/2. One Voice in the Cosmic Fugue.mkv".into();
        let fx = s.launch(episode.clone());
        let launch = fx.iter().find_map(|e| match e {
            Effect::Launch { settings, env, .. } => Some((settings.get(Setting::FpsCap), settings.get(Setting::Power), env.clone())),
            _ => None,
        });
        assert_eq!(launch, Some((30, 1, vec![("MPV".to_owned(), "1".to_owned())])), "the runner's, not the series folder's");
    }

    #[test]
    fn the_panel_offers_the_items_levels() {
        let mut library = clamshell_library::demo::library();
        for g in library.entries.iter_mut().filter(|g| g.id.starts_with("tv:")) {
            g.runner = Some(clamshell_library::RunnerOf::new("video", "Video", true));
        }
        let mut s = State::new(Init { library, ..init() });
        let levels = |s: &State| s.setting_levels().into_iter().flatten().map(|(scope, l)| (scope, l.title.to_owned())).collect::<Vec<_>>();
        assert_eq!(levels(&s), [(Scope::Runner, "A Link to the Past".to_owned())], "a game with no runner: its own, as its runner");
        tap(&mut s, Id::Shelf(ShelfId::Collection(2)));
        assert_eq!(levels(&s), [(Scope::Runner, "Video".to_owned())], "a series: its runner's only");
        tap(&mut s, Id::Shelf(ShelfId::Launch));
        tap(&mut s, Id::Shelf(ShelfId::Launch));
        let title = "1. The Shores of the Cosmic Ocean".to_owned();
        assert_eq!(levels(&s), [(Scope::Runner, "Video".to_owned()), (Scope::Content, title.clone())]);
        assert_eq!(s.editing().map(|p| p.title), Some(title.as_str()), "its own by default");
        scope(&mut s, Scope::Runner);
        assert_eq!(s.editing().map(|p| p.id), Some(runner_key("video")));
        taps(&mut s, Setting::Power, 1);
        assert_eq!(s.settings().overrides_of("runner:video").get(Setting::Power), Some(2), "kept by the runner");
        assert_eq!(s.settings_of("tv:Cosmos/Season 2/1. Standing Up in the Milky Way.mkv").get(Setting::Power), 2, "every video's");
    }

    #[test]
    fn a_runner_can_leave_its_items_without_settings_of_their_own() {
        let mut library = clamshell_library::demo::library();
        for g in library.entries.iter_mut() {
            g.runner = Some(clamshell_library::RunnerOf::new("snes", "SNES", false));
        }
        let s = State::new(Init { library, ..init() });
        let levels = s.setting_levels().into_iter().flatten().map(|(scope, l)| (scope, l.title)).collect::<Vec<_>>();
        assert_eq!(levels, [(Scope::Runner, "SNES")]);
        assert_eq!(s.editing().map(|p| p.id), Some(runner_key("snes")), "the item's tab: its runner's");
    }

    #[test]
    fn a_runner_gets_its_keymap_once() {
        let mut library = clamshell_library::demo::library();
        library.runner_keymaps = vec![(runner_key("text"), "viewer".into())];
        let mut s = State::new(Init { library, ..init() });
        let fx = s.apply(Event::Start).1;
        assert!(fx.iter().any(|e| matches!(e, Effect::Save(Save::Keymaps(_)))), "{fx:?}");
        let own = s.keymaps().own_of("runner:text").map(|k| k.id);
        assert!(own.is_some());
        assert_eq!(s.settings().overrides_of("runner:text").get(Setting::Keymap), own);
        // With Default chosen for it instead, it is left that way, also when the library is read again.
        s.settings.set(Some(&runner_key("text")), Setting::Keymap, Some(0));
        let fx = s.apply(Event::Library(Box::new(s.shelf().library().clone()))).1;
        assert!(!fx.iter().any(|e| matches!(e, Effect::Save(Save::Keymaps(_)))));
    }

    #[test]
    fn games_override_the_global_settings() {
        let mut s = state();
        scope(&mut s, Scope::Global);
        taps(&mut s, Setting::FpsCap, 4); // 30, 40, 45, 60
        scope(&mut s, Scope::Content);
        let fx = taps(&mut s, Setting::Power, 1); // Balanced -> Performance
        let Some(Effect::Save(Save::Settings(saved))) = fx.first() else {
            unreachable!("{fx:?}");
        };
        assert_eq!(saved, s.settings(), "saved whole");
        assert_eq!(saved.overrides_of("zelda-lttp").get(Setting::Power), Some(2));
        let zelda = s.settings_of("zelda-lttp");
        assert_eq!((zelda.get(Setting::Power), zelda.get(Setting::FpsCap)), (2, 60));
        assert_eq!(s.settings_of("ff6").get(Setting::Power), 1);
        assert_eq!((s.tile_origin(Setting::Power), s.tile_origin(Setting::FpsCap)), (TileOrigin::Own, TileOrigin::Inherited));
        let fresh = state_with(saved);
        assert_eq!(fresh.settings_of("zelda-lttp"), zelda);
        assert_eq!(fresh.settings().global, s.settings().global);
        // A long press drops one override, and Reset drops all of them.
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
        crate::testkit::start(&mut s, "zelda-lttp");
        tap(&mut s, game("ff6"));
        // The running game is the context, not the selection.
        let fx = taps(&mut s, Setting::FpsCap, 1);
        assert!(fx.iter().any(|e| matches!(e, Effect::Apply { entry: Some(g), settings } if &**g == "zelda-lttp" && settings.get(Setting::FpsCap) == 30)));
        // A global change reaches it only where it doesn't override.
        scope(&mut s, Scope::Global);
        assert!(!taps(&mut s, Setting::FpsCap, 1).iter().any(|e| matches!(e, Effect::Apply { .. })), "its own cap stays: nothing to apply");
        let fx = taps(&mut s, Setting::Power, 1);
        assert!(fx.iter().any(|e| matches!(e, Effect::Apply { settings, .. } if settings.get(Setting::Power) == 2)));
        let fx: Vec<Effect> = end(&mut s, "zelda-lttp").1.into_iter().filter(|e| !matches!(e, Effect::RecordPlay { .. })).collect();
        assert_eq!(fx, vec![Effect::Apply { entry: None, settings: s.settings().global }, Effect::PadRoute(PadRoute::Ui)], "and the pad back to shelf");
    }

    #[test]
    fn a_game_can_start_the_pad_mouse_and_darken_the_second_screen() {
        let mut s = state();
        taps(&mut s, Setting::PadMouse, 1);
        taps(&mut s, Setting::BottomScreen, 2);
        let (_, fx) = crate::testkit::start(&mut s, "zelda-lttp");
        assert!(fx.contains(&Effect::PadRoute(PadRoute::Mouse)), "{fx:?}");
        assert!(s.bottom_dark());
        tap(&mut s, Id::Wake);
        assert!(!s.bottom_dark());
        let fx = end(&mut s, "zelda-lttp").1;
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
    fn the_editor_pastes_the_clipboards_text_as_a_line() {
        let mut s = state();
        tap(&mut s, Id::Tab(Tab::Settings));
        tap(&mut s, Id::Settings(SettingsId::Env));
        let key = |s: &mut State, k| tap(s, Id::Key(find(k).unwrap()));
        let ctrl_v = |s: &mut State| {
            key(s, Key::Mod(crate::bottom::keyboard::Modifier::Ctrl));
            key(s, Key::Char('v'))
        };
        ctrl_v(&mut s);
        assert_eq!(s.bottom().editor().map(|e| e.text.as_str()), Some(""), "nothing on the clipboard");
        s.apply(Event::Reading(Reading::Clipboard(Some("A=1\nB=2\n".into()))));
        ctrl_v(&mut s);
        assert_eq!(s.bottom().editor().map(|e| e.text.as_str()), Some("A=1 B=2"), "its line breaks spaces, its end trimmed");
        // A right click does the same.
        s.apply(Event::Paste);
        assert_eq!(s.bottom().editor().map(|e| e.text.as_str()), Some("A=1 B=2A=1 B=2"));
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
        assert_eq!(s.env_of("zelda-lttp"), vec![("dxv".to_owned(), "1".to_owned())]);
        let fresh = state_with(s.settings());
        assert_eq!(fresh.env_of("zelda-lttp"), s.env_of("zelda-lttp"));
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
        // An app in front, so the keyboard types into it.
        s.apply(Event::Windows(vec![foot()]));
        tap(&mut s, Id::Tab(Tab::Settings));
        tap(&mut s, Id::Settings(SettingsId::Env));
        let key = |s: &mut State, k| tap(s, Id::Key(find(k).unwrap()));
        key(&mut s, Key::Char('a'));
        // In the keyboard tab, keys go to the app, not to the editor left open in the settings.
        tap(&mut s, Id::Tab(Tab::Keyboard));
        assert_eq!(s.bottom().editor(), None);
        assert_eq!(key(&mut s, Key::Char('b')).len(), 1, "sent");
        tap(&mut s, Id::Tab(Tab::Settings));
        assert_eq!(s.bottom().editor().map(|e| e.text.as_str()), Some("a"), "as left");
    }
}
