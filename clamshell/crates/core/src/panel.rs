//! The settings panel's rules: which game it is about (by what is in front) and
//! which tab it edits, changes applied at once when they change the settings in force, the environment
//! editor typing on the keyboard, and the dark second screen.

use crate::editor::{Done, Editor, Purpose};
use crate::keyboard::Stroke;
use crate::settings::{Setting, Values};
use crate::{Effect, Game, Scope, State};

impl State {
    /// The settings in force: the running game's, else the global ones.
    pub fn current(&self) -> Values {
        self.current_game.as_ref().map_or(self.settings.global, |id| self.settings.of(id))
    }

    /// The game the settings panel is about, by what is in front: the game, shelf's selection, or none
    /// (another app: the panel edits all games).
    pub fn context(&self) -> Option<&Game> {
        let id = match self.front() {
            crate::Front::Game => self.current_game.as_ref(),
            crate::Front::Shelf => self.selected.as_ref(),
            crate::Front::App => None,
        }?;
        self.library.games.iter().find(|g| &g.id == id)
    }

    /// What the settings panel edits now: the game in context (if the scope is the game's), or none
    /// for the global settings.
    pub fn editing(&self) -> Option<&Game> {
        self.context().filter(|_| self.scope == Scope::Game)
    }

    /// The second screen is dark: a game runs that wants it so, and it hasn't been tapped since.
    pub fn bottom_dark(&self) -> bool {
        self.current_game.is_some() && self.current().get(Setting::BottomScreen) == 1 && !self.woken
    }

    /// Changes `setting` where the panel edits (the game's overrides, or the global settings): to
    /// `value`, or back to the default (none). Saved, and applied at once if the settings in force
    /// changed.
    pub(crate) fn change(&mut self, setting: Setting, value: Option<u16>) -> Vec<Effect> {
        let before = self.current();
        let game = self.editing().map(|g| g.id.clone());
        self.settings.set(game.as_ref(), setting, value);
        self.saved_and_applied(before)
    }

    pub(crate) fn saved_and_applied(&self, before: Values) -> Vec<Effect> {
        let mut effects = vec![Effect::SaveSettings(self.settings.text())];
        let now = self.current();
        if now != before {
            effects.push(Effect::Apply { game: self.current_game.clone(), settings: now });
        }
        effects
    }

    /// The value a tile shows and steps from: the game's effective one, or the global one.
    pub fn tile_value(&self, setting: Setting) -> u16 {
        match self.editing() {
            Some(g) if !setting.global_only() => self.settings.of(&g.id).get(setting),
            _ => self.settings.global.get(setting),
        }
    }

    /// Opens the editor on what the panel edits: the game's variables, or all games'.
    pub(crate) fn open_editor(&mut self) {
        let game = self.editing().map(|g| g.id.clone());
        let text = self.settings.env_text(game.as_deref()).to_owned();
        self.editor = Some(Editor { purpose: Purpose::Env { game }, text });
    }

    /// A key from the on-screen keyboard while the editor is open.
    pub(crate) fn edit(&mut self, stroke: Stroke) -> Vec<Effect> {
        match self.editor.as_mut().and_then(|e| e.key(stroke)) {
            Some(Done::Save) => self.save_editor(),
            Some(Done::Cancel) => {
                self.editor = None;
                vec![]
            }
            None => vec![],
        }
    }

    /// The editor's Save: the environment saved, or the network joined with the password.
    pub(crate) fn save_editor(&mut self) -> Vec<Effect> {
        let Some(editor) = self.editor.take() else {
            return vec![];
        };
        match editor.purpose {
            Purpose::Env { game } => {
                self.settings.set_env(game, &editor.text);
                vec![Effect::SaveSettings(self.settings.text())]
            }
            Purpose::Password { ssid } => self.join(ssid, Some(editor.text)),
        }
    }

    /// Drops the edited game's overrides and environment.
    pub(crate) fn reset(&mut self) -> Vec<Effect> {
        let before = self.current();
        if let Some(id) = self.editing().map(|g| g.id.clone()) {
            self.settings.reset(&id);
        }
        self.saved_and_applied(before)
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use crate::keyboard::{Key, Named};
    use crate::settings::{self, Setting};
    use crate::tests::state;
    use crate::*;

    /// Taps a tile `n` times.
    fn tap(s: &mut State, setting: Setting, n: usize) -> Vec<Effect> {
        let mut fx = vec![];
        for _ in 0..n {
            fx = s.apply(Event::Activate(Id::Tile(setting))).1;
        }
        fx
    }

    #[test]
    fn games_override_the_global_settings() {
        let mut s = state();
        s.apply(Event::Activate(Id::Scope(Scope::Global)));
        tap(&mut s, Setting::FpsCap, 4); // 30, 40, 45, 60
        s.apply(Event::Activate(Id::Scope(Scope::Game)));
        let fx = tap(&mut s, Setting::Power, 1); // Balanced -> Performance
        let saved = match fx.first() {
            Some(Effect::SaveSettings(t)) => t.clone(),
            other => format!("{other:?}"),
        };
        assert!(saved.starts_with("*\tpower=1\tfps=60\t"), "{saved}");
        assert!(saved.ends_with("\nzelda-lttp\tpower=2\n"), "{saved}");
        let zelda = s.settings.of("zelda-lttp");
        assert_eq!((zelda.get(Setting::Power), zelda.get(Setting::FpsCap)), (2, 60));
        assert_eq!(s.settings.of("ff6").get(Setting::Power), 1);
        let mut fresh = state();
        fresh.settings = settings::Settings::load(&saved);
        assert_eq!(fresh.settings.of("zelda-lttp"), zelda);
        assert_eq!(fresh.settings.global, s.settings.global);
        // A long press drops one override; Reset all of them.
        tap(&mut s, Setting::Buttons, 1);
        s.apply(Event::LongPress(Id::Tile(Setting::Power)));
        assert_eq!(s.settings.overrides_of("zelda-lttp").get(Setting::Power), None);
        assert_eq!(s.settings.overrides_of("zelda-lttp").get(Setting::Buttons), Some(1));
        s.apply(Event::Activate(Id::Reset));
        assert!(s.settings.overrides.is_empty());
        // Global-only settings change the global value even on a game's tab.
        tap(&mut s, Setting::ChargeLimit, 1);
        assert_eq!(s.settings.global.get(Setting::ChargeLimit), 90);
    }

    #[test]
    fn the_launch_carries_the_games_settings_and_environment() {
        let mut s = state();
        tap(&mut s, Setting::FpsCap, 2);
        s.settings.global_env = "A=1 B=2".into();
        s.settings.env.insert("zelda-lttp".into(), "B=3 'C=x y'".into());
        let (_, fx) = s.apply(Event::Activate(Id::Launch));
        let Some(Effect::Launch { settings, env, .. }) = fx.last() else {
            unreachable!("{fx:?}")
        };
        assert_eq!(settings.get(Setting::FpsCap), 40);
        let pairs: Vec<(&str, &str)> = env.iter().map(|(k, v)| (k.as_str(), v.as_str())).collect();
        assert_eq!(pairs, [("A", "1"), ("B", "2"), ("B", "3"), ("C", "x y")], "the game's last, so they win");
    }

    #[test]
    fn a_running_game_takes_changes_at_once() {
        let mut s = state();
        s.apply(Event::Started("zelda-lttp".into()));
        s.apply(Event::Activate(Id::Game("ff6".into())));
        // The running game is the context, not the selection.
        let fx = tap(&mut s, Setting::FpsCap, 1);
        assert!(matches!(fx.last(), Some(Effect::Apply { game: Some(g), settings }) if &**g == "zelda-lttp" && settings.get(Setting::FpsCap) == 30));
        // A global change reaches it only where it doesn't override.
        s.apply(Event::Activate(Id::Scope(Scope::Global)));
        assert_eq!(tap(&mut s, Setting::FpsCap, 1).len(), 1, "its own cap stays: nothing to apply");
        let fx = tap(&mut s, Setting::Power, 1);
        assert!(matches!(fx.last(), Some(Effect::Apply { settings, .. }) if settings.get(Setting::Power) == 2));
        let (_, fx) = s.apply(Event::Played { game: "zelda-lttp".into(), start: 0, duration: 1 });
        assert_eq!(fx, vec![Effect::Apply { game: None, settings: s.settings.global }, Effect::PadRoute(crate::PadRoute::Ui)], "and the pad back to shelf");
    }

    #[test]
    fn a_game_can_start_the_pad_mouse_and_darken_the_second_screen() {
        let mut s = state();
        tap(&mut s, Setting::PadMouse, 1);
        tap(&mut s, Setting::BottomScreen, 1);
        let (_, fx) = s.apply(Event::Started("zelda-lttp".into()));
        assert_eq!(fx, vec![Effect::PadRoute(crate::PadRoute::Mouse)]);
        assert!(s.bottom_dark());
        s.apply(Event::Activate(Id::Wake));
        assert!(!s.bottom_dark());
        let (_, fx) = s.apply(Event::Played { game: "zelda-lttp".into(), start: 0, duration: 1 });
        assert!(fx.contains(&Effect::PadRoute(crate::PadRoute::Ui)), "{fx:?}");
        assert!(!s.pad_mouse);
    }

    #[test]
    fn without_a_game_the_panel_edits_the_global_settings() {
        let mut empty = State::new(Library::default());
        empty.apply(Event::Activate(Id::Tab(Tab::Settings)));
        assert_eq!(empty.tab, Tab::Settings, "always available");
        tap(&mut empty, Setting::FpsCap, 2);
        assert_eq!(empty.settings.global.get(Setting::FpsCap), 40);
    }

    #[test]
    fn the_environment_editor_takes_the_keyboard() {
        use keyboard::find;
        let mut s = state();
        s.apply(Event::Activate(Id::Env));
        let key = |k| Event::Activate(Id::Key(find(k).unwrap()));
        for c in ['d', 'x', 'v', 'k'] {
            assert_eq!(s.apply(key(Key::Char(c))).1, vec![], "typed into the editor, not sent");
        }
        s.apply(key(Key::Named(Named::Backspace)));
        s.apply(key(Key::Char('=')));
        s.apply(key(Key::Char('1')));
        let (_, fx) = s.apply(key(Key::Named(Named::Enter)));
        assert!(matches!(fx.first(), Some(Effect::SaveSettings(t)) if t.ends_with("zelda-lttp\tenv=dxv=1\n")), "{fx:?}");
        assert_eq!(s.editor, None);
        assert_eq!(s.settings.env_of("zelda-lttp"), vec![("dxv".to_owned(), "1".to_owned())]);
        let mut fresh = state();
        fresh.settings = settings::Settings::load(&s.settings.text());
        assert_eq!(fresh.settings.env_of("zelda-lttp"), s.settings.env_of("zelda-lttp"));
    }
}
