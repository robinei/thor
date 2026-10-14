//! The game role and what is in front. At most one game runs; it is "the game" however it started:
//! from shelf, or from Steam's own client (a `steam_app_<id>` window of a game in the library is adopted,
//! and its play ends when its last window closes). Activating a game in shelf starts it, resumes it if it
//! is the one running, or, with another running, asks once and then quits that one and starts this.
//! What is in front (shelf, the game, another app) decides where the pad goes, what Home does and what
//! the settings page edits.

use std::sync::Arc;

use clamshell_library::Launch;

use crate::apps::SHELF_APP_ID;
use crate::windows::{GAME_WORKSPACE, Window};
use crate::{Effect, Event, State};

/// What is in front on the main screen.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Front {
    Shelf,
    Game,
    /// Another app (a launcher instance, or anything else).
    App,
}

/// A game Steam started, followed by its windows: the game, when it was first seen, its Steam id.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Adopted {
    game: Arc<str>,
    start: u64,
    appid: u32,
}

fn steam_id(w: &Window) -> Option<u32> {
    w.app_id.strip_prefix("steam_app_").and_then(|n| n.parse().ok())
}

impl State {
    /// What is in front, from the compositor's focused window; without one, the game if one runs, else
    /// shelf.
    pub fn front(&self) -> Front {
        match self.windows.iter().find(|w| w.focused) {
            Some(w) if w.app_id == SHELF_APP_ID => Front::Shelf,
            Some(w) if w.of_game() => Front::Game,
            Some(_) => Front::App,
            None if self.running.is_some() => Front::Game,
            None => Front::Shelf,
        }
    }

    /// Shelf's action on its selected game: start it, resume it (the running one), or with another
    /// running, ask (the first time) and then quit that one and start this once it has ended.
    pub(crate) fn play(&mut self, game: Arc<str>) -> Vec<Effect> {
        match &self.running {
            None => self.launch_effects(game),
            Some(running) if *running == game => vec![Effect::FocusWorkspace(GAME_WORKSPACE.into())],
            Some(_) if self.switch_to.as_ref() == Some(&game) => {
                self.switch_to = None;
                self.queued = Some(game);
                self.tap_game(true)
            }
            Some(_) => {
                self.switch_to = Some(game);
                vec![]
            }
        }
    }

    /// The game to start once the running one has ended, if one was asked for.
    pub(crate) fn queued_launch(&mut self) -> Vec<Effect> {
        match self.queued.take() {
            Some(game) => self.launch_effects(game),
            None => vec![],
        }
    }

    /// After the windows changed: a Steam game's window with no game running makes that game the game;
    /// the adopted game's last window gone ends its play.
    pub(crate) fn follow_steam_game(&mut self) -> Vec<Effect> {
        if let Some(adopted) = self.adopted.clone() {
            if self.windows.iter().any(|w| steam_id(w) == Some(adopted.appid)) {
                return vec![];
            }
            self.adopted = None;
            let duration = self.unix_secs.saturating_sub(adopted.start);
            let (game, start) = (adopted.game, adopted.start);
            let mut effects = vec![Effect::RecordPlay { game: game.clone(), start, duration }];
            effects.extend(self.apply_event(Event::Played { game, start, duration }).1);
            return effects;
        }
        if self.running.is_some() {
            return vec![];
        }
        let found = self.windows.iter().filter_map(steam_id).find_map(|appid| {
            self.library.games.iter().find(|g| g.launch == Launch::Steam { appid }).map(|g| (g.id.clone(), appid))
        });
        let Some((game, appid)) = found else {
            return vec![];
        };
        self.adopted = Some(Adopted { game: game.clone(), start: self.unix_secs, appid });
        let mut effects = vec![Effect::Apply { game: Some(game.clone()), settings: self.settings.of(&game) }];
        effects.extend(self.apply_event(Event::Started(game)).1);
        effects
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    use crate::tests::state;
    use crate::{Id, Library};

    fn win(app_id: &str, workspace: &str, focused: bool) -> Window {
        Window { id: 1, app_id: app_id.into(), title: String::new(), focused, unit: None, workspace: workspace.into() }
    }

    #[test]
    fn a_running_game_resumes_and_another_asks_before_switching() {
        let mut s = state();
        s.apply(Event::Started("zelda-lttp".into()));
        s.apply(Event::Activate(Id::Game("zelda-lttp".into())));
        assert_eq!(s.apply(Event::Activate(Id::Game("zelda-lttp".into()))).1, vec![Effect::FocusWorkspace(GAME_WORKSPACE.into())], "resume");
        s.apply(Event::Activate(Id::Game("ff6".into())));
        assert_eq!(s.apply(Event::Activate(Id::Game("ff6".into()))).1, vec![], "asks first");
        assert_eq!(s.switch_to.as_deref(), Some("ff6"));
        let (_, fx) = s.apply(Event::Activate(Id::Game("ff6".into())));
        assert!(matches!(fx.as_slice(), [Effect::QuitGame { .. }]), "{fx:?}");
        let (_, fx) = s.apply(Event::Played { game: "zelda-lttp".into(), start: 0, duration: 5 });
        assert!(fx.iter().any(|e| matches!(e, Effect::Launch { game, .. } if &**game == "ff6")), "then starts it: {fx:?}");
    }

    #[test]
    fn moving_the_selection_drops_the_question() {
        let mut s = state();
        s.apply(Event::Started("zelda-lttp".into()));
        s.apply(Event::Activate(Id::Game("ff6".into())));
        s.apply(Event::Activate(Id::Game("ff6".into())));
        s.apply(Event::ListStep(1));
        assert_eq!(s.switch_to, None);
    }

    #[test]
    fn a_game_steam_started_is_the_game_until_its_window_closes() {
        let mut library = clamshell_library::demo_library();
        if let Some(g) = library.games.iter_mut().find(|g| &*g.id == "ff6") {
            g.launch = Launch::Steam { appid: 620 };
        }
        let mut s = State::new(library);
        s.apply(Event::Tick { unix_secs: 1000 });
        let (_, fx) = s.apply(Event::Windows(vec![win("steam_app_620", GAME_WORKSPACE, true)]));
        assert_eq!(s.running.as_deref(), Some("ff6"));
        assert!(matches!(fx.first(), Some(Effect::Apply { game: Some(g), .. }) if &**g == "ff6"));
        assert_eq!(s.front(), Front::Game);
        s.apply(Event::Tick { unix_secs: 1600 });
        let (_, fx) = s.apply(Event::Windows(vec![win(SHELF_APP_ID, "clamshell-shelf", true)]));
        assert_eq!(fx.first(), Some(&Effect::RecordPlay { game: "ff6".into(), start: 1000, duration: 600 }));
        assert_eq!(s.running, None);
        // A Steam game the library doesn't have isn't adopted.
        s.apply(Event::Windows(vec![win("steam_app_1", GAME_WORKSPACE, true)]));
        assert_eq!(s.running, None);
        let _ = Library::default();
    }

    #[test]
    fn the_settings_follow_the_front() {
        let mut s = state();
        s.apply(Event::Started("zelda-lttp".into()));
        s.apply(Event::Activate(Id::Game("ff6".into())));
        assert_eq!(s.context().map(|g| &*g.id), Some("zelda-lttp"), "the game in front");
        s.apply(Event::Windows(vec![win(SHELF_APP_ID, "clamshell-shelf", true)]));
        assert_eq!(s.context().map(|g| &*g.id), Some("ff6"), "shelf in front: its selection");
        s.apply(Event::Windows(vec![win("foot", "clamshell-app-terminal-1-0", true)]));
        assert_eq!(s.context(), None, "an app in front: all games");
    }
}
