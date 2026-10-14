//! Running games and what is in front. Games run side by side, each on a workspace of its own: one shelf
//! started has a scope and workspace named alike (`clamshell-game-<secs>-<n>`); one Steam started (a
//! `steam_app_<id>` window of a library game) gets `clamshell-game-steam-<id>`, its windows moved there
//! from where they land. "The game" is the running game most recently in front: its settings are in
//! force, the settings page and the pad follow it. A game Steam starts while another runs becomes the
//! game; the other keeps running in the background.
//!
//! In shelf, activating a running game shows it; with another running, shelf offers to keep that one
//! running or close it, then starts this one. What is in front (shelf, a game, another app) decides where
//! the pad goes, what Home does and what the settings page edits.

use std::sync::Arc;

use clamshell_library::Launch;

use crate::apps::SHELF_APP_ID;
use crate::windows::Window;
use crate::{Effect, Event, State};

/// What is in front on the main screen.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Front {
    Shelf,
    Game,
    /// Another app (a dock instance, or anything else).
    App,
}

/// A game that runs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RunningGame {
    pub id: Arc<str>,
    /// Its workspace (and, if shelf started it, its scope's name).
    pub workspace: String,
    /// Steam's id, if Steam runs it: its windows are found by it.
    pub appid: Option<u32>,
    /// When it started (epoch seconds).
    pub start: u64,
    /// Steam started it (not shelf): its play ends, and is recorded here, when its last window closes.
    adopted: bool,
}

/// A launch asked for and not started yet: the game, its workspace, its Steam id.
pub(crate) type Launching = (Arc<str>, String, Option<u32>);

impl State {
    /// What is in front, from the compositor's focused window; without one, the game if one runs, else
    /// shelf.
    pub fn front(&self) -> Front {
        match self.windows.iter().find(|w| w.focused) {
            Some(w) if w.app_id == SHELF_APP_ID => Front::Shelf,
            Some(w) if w.of_game() || self.game_of(w).is_some() => Front::Game,
            Some(_) => Front::App,
            None if self.running.is_some() => Front::Game,
            None => Front::Shelf,
        }
    }

    /// The running game a window belongs to: on its workspace, or Steam's window of it.
    pub fn game_of(&self, w: &Window) -> Option<&RunningGame> {
        self.games.iter().find(|g| g.workspace == w.workspace || (g.appid.is_some() && g.appid == w.steam_id()))
    }

    pub fn running_game(&self, id: &str) -> Option<&RunningGame> {
        self.games.iter().find(|g| &*g.id == id)
    }

    /// Shelf's action on its selected game: show it if it runs; start it if nothing else does; with
    /// another game running, the choice (keep it running or close it) is offered instead, and the focus
    /// goes to it.
    pub(crate) fn play(&mut self, game: Arc<str>) -> Vec<Effect> {
        if let Some(g) = self.running_game(&game) {
            return vec![Effect::FocusWorkspace(g.workspace.clone())];
        }
        if self.running.is_some() {
            self.focus = Some(crate::Id::StartKeep);
            self.show_focus = true;
            return vec![];
        }
        self.launch_effects(game)
    }

    /// The choice in shelf, with another game running: start the selected one and keep the other
    /// running, or close the other first (the selected one starts once it has ended).
    pub(crate) fn start_beside(&mut self, close: bool) -> Vec<Effect> {
        let Some(game) = self.selected.clone() else {
            return vec![];
        };
        if !close || self.running.is_none() {
            return self.launch_effects(game);
        }
        self.queued = Some(game);
        let current = self.running.clone();
        current.map_or_else(Vec::new, |id| self.quit_game(&id))
    }

    /// Closing a game: its windows asked to close, its scope (ours) ended if it doesn't go.
    pub(crate) fn quit_game(&self, id: &str) -> Vec<Effect> {
        let Some(g) = self.running_game(id) else {
            return vec![];
        };
        let windows = self.windows.iter().filter(|w| self.game_of(w).is_some_and(|o| o.id == g.id)).map(|w| w.id).collect();
        let units = self.units.iter().filter(|u| **u == g.workspace).map(|u| format!("{u}.scope")).collect();
        vec![Effect::QuitGame { windows, units }]
    }

    /// The game to start once the one closed for it has ended, if any.
    pub(crate) fn queued_launch(&mut self) -> Vec<Effect> {
        match self.queued.take() {
            Some(game) => self.launch_effects(game),
            None => vec![],
        }
    }

    /// A game started (shelf's launch, now running): it is the game.
    pub(crate) fn game_started(&mut self, game: &Arc<str>) {
        // A game Steam started is already listed (with its own workspace).
        if self.games.iter().any(|g| g.id == *game) {
            return;
        }
        let (workspace, appid) = match self.launching.iter().position(|l| l.0 == *game) {
            Some(i) => {
                let (_, workspace, appid) = self.launching.remove(i);
                (workspace, appid)
            }
            // Not asked for here (a test, or a restart): the landing workspace.
            None => (crate::windows::GAME_WORKSPACE.into(), None),
        };
        self.games.push(RunningGame { id: game.clone(), workspace, appid, start: self.unix_secs, adopted: false });
    }

    /// A game ended: no longer running. If it was the game, the most recently shown of the others is
    /// now (its settings applied), or none is.
    pub(crate) fn game_gone(&mut self, game: &str) -> Vec<Effect> {
        self.games.retain(|g| &*g.id != game);
        if self.running.as_deref() != Some(game) {
            return self.queued_launch();
        }
        let next = self
            .recent
            .iter()
            .find_map(|ws| self.games.iter().find(|g| g.workspace == *ws))
            .or_else(|| self.games.last())
            .map(|g| g.id.clone());
        match next {
            Some(id) => {
                self.running = Some(id.clone());
                let mut effects = vec![Effect::Apply { game: Some(id.clone()), settings: self.settings.of(&id) }];
                effects.extend(self.queued_launch());
                effects
            }
            None => {
                self.running = None;
                self.game_ended()
            }
        }
    }

    /// After the windows changed: a game shown becomes the game (its settings applied); a Steam game's
    /// window of a library game not running yet starts its play (it becomes the game; any other keeps
    /// running); Steam's windows go to their game's workspace; an adopted game whose last window closed
    /// has ended.
    pub(crate) fn follow_games(&mut self) -> Vec<Effect> {
        let mut effects = Vec::new();
        // Adopted games whose windows are all gone.
        let ended: Vec<RunningGame> = self
            .games
            .iter()
            .filter(|g| g.adopted && !self.windows.iter().any(|w| g.appid.is_some() && w.steam_id() == g.appid))
            .cloned()
            .collect();
        for g in ended {
            let duration = self.unix_secs.saturating_sub(g.start);
            effects.push(Effect::RecordPlay { game: g.id.clone(), start: g.start, duration });
            effects.extend(self.apply_event(Event::Played { game: g.id, start: g.start, duration }).1);
        }
        // A new Steam game.
        let new = self.windows.iter().filter_map(Window::steam_id).find_map(|appid| {
            let running = self.games.iter().any(|g| g.appid == Some(appid)) || self.launching.iter().any(|l| l.2 == Some(appid));
            let game = self.library.games.iter().find(|g| g.launch == Launch::Steam { appid });
            game.filter(|_| !running).map(|g| (g.id.clone(), appid))
        });
        if let Some((game, appid)) = new {
            let workspace = format!("clamshell-game-steam-{appid}");
            self.games.push(RunningGame { id: game.clone(), workspace: workspace.clone(), appid: Some(appid), start: self.unix_secs, adopted: true });
            self.pending_show = Some(workspace);
            effects.push(Effect::Apply { game: Some(game.clone()), settings: self.settings.of(&game) });
            effects.extend(self.apply_event(Event::Started(game)).1);
        }
        // Steam's windows to their game's workspace.
        for g in &self.games {
            let stray: Vec<i64> = self.windows.iter().filter(|w| g.appid.is_some() && w.steam_id() == g.appid && w.workspace != g.workspace).map(|w| w.id).collect();
            if !stray.is_empty() {
                effects.push(Effect::MoveWindows { windows: stray, workspace: g.workspace.clone() });
            }
        }
        // The game shown is the game.
        let shown = self.windows.iter().find(|w| w.focused).and_then(|w| self.game_of(w)).map(|g| g.id.clone());
        if let Some(id) = shown
            && self.running.as_ref() != Some(&id)
        {
            self.running = Some(id.clone());
            effects.push(Effect::Apply { game: Some(id.clone()), settings: self.settings.of(&id) });
        }
        effects
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    use crate::tests::state;
    use crate::windows::GAME_WORKSPACE;
    use crate::{Id, Library};

    fn win(id: i64, app_id: &str, workspace: &str, focused: bool) -> Window {
        Window { id, app_id: app_id.into(), title: String::new(), focused, unit: None, workspace: workspace.into() }
    }

    /// Launches `game` from shelf and has it start; returns its workspace.
    fn launch(s: &mut State, game: &str) -> String {
        // Select it (if it isn't), then play it: the launch is in one of the two.
        let mut fx = s.apply(Event::Activate(Id::Game(game.into()))).1;
        fx.extend(s.apply(Event::Activate(Id::Game(game.into()))).1);
        let workspace = fx.iter().find_map(|e| match e {
            Effect::Launch { workspace, .. } => Some(workspace.clone()),
            _ => None,
        });
        s.apply(Event::Started(game.into()));
        workspace.unwrap()
    }

    #[test]
    fn a_second_game_is_offered_beside_or_instead_of_the_first() {
        let mut s = state();
        s.apply(Event::Tick { unix_secs: 100 });
        let zelda = launch(&mut s, "zelda-lttp");
        assert!(zelda.starts_with("clamshell-game-100-"));
        assert_eq!(s.apply(Event::Activate(Id::Game("zelda-lttp".into()))).1, vec![Effect::FocusWorkspace(zelda.clone())], "running: shown");
        s.apply(Event::Activate(Id::Game("ff6".into())));
        assert_eq!(s.apply(Event::Activate(Id::Game("ff6".into()))).1, vec![], "another runs: the choice");
        assert_eq!(s.focus, Some(Id::StartKeep));
        // Keep it running: both run, the new one is the game.
        let (_, fx) = s.apply(Event::Activate(Id::StartKeep));
        assert!(fx.iter().any(|e| matches!(e, Effect::Launch { game, .. } if &**game == "ff6")));
        s.apply(Event::Started("ff6".into()));
        assert_eq!((s.games.len(), s.running.as_deref()), (2, Some("ff6")));
        // ff6 ends: zelda is the game again, its settings applied.
        let (_, fx) = s.apply(Event::Played { game: "ff6".into(), start: 100, duration: 10 });
        assert_eq!(s.running.as_deref(), Some("zelda-lttp"));
        assert!(matches!(fx.first(), Some(Effect::Apply { game: Some(g), .. }) if &**g == "zelda-lttp"));
        // Close it: zelda quits, then ff6 starts.
        s.apply(Event::Activate(Id::Game("ff6".into())));
        let (_, fx) = s.apply(Event::Activate(Id::StartClose));
        assert!(matches!(fx.as_slice(), [Effect::QuitGame { .. }]), "{fx:?}");
        let (_, fx) = s.apply(Event::Played { game: "zelda-lttp".into(), start: 100, duration: 20 });
        assert!(fx.iter().any(|e| matches!(e, Effect::Launch { game, .. } if &**game == "ff6")), "{fx:?}");
    }

    #[test]
    fn the_game_is_the_one_last_shown() {
        let mut s = state();
        let zelda = launch(&mut s, "zelda-lttp");
        s.apply(Event::Activate(Id::Game("ff6".into())));
        s.apply(Event::Activate(Id::StartKeep));
        s.apply(Event::Started("ff6".into()));
        let ff6 = s.running_game("ff6").unwrap().workspace.clone();
        let (_, fx) = s.apply(Event::Windows(vec![win(1, "zelda3", &zelda, true), win(2, "ff6", &ff6, false)]));
        assert_eq!(s.running.as_deref(), Some("zelda-lttp"));
        assert!(fx.iter().any(|e| matches!(e, Effect::Apply { game: Some(g), .. } if &**g == "zelda-lttp")), "{fx:?}");
        assert_eq!(s.front(), Front::Game);
    }

    #[test]
    fn a_game_steam_started_is_the_game_and_the_other_keeps_running() {
        let mut library = clamshell_library::demo_library();
        if let Some(g) = library.games.iter_mut().find(|g| &*g.id == "ff6") {
            g.launch = Launch::Steam { appid: 620 };
        }
        let mut s = State::new(library);
        s.apply(Event::Tick { unix_secs: 1000 });
        let zelda = launch(&mut s, "zelda-lttp");
        let (_, fx) = s.apply(Event::Windows(vec![win(1, "zelda3", &zelda, false), win(7, "steam_app_620", GAME_WORKSPACE, true)]));
        assert_eq!(s.running.as_deref(), Some("ff6"), "the newest is the game");
        assert_eq!(s.games.len(), 2, "the other keeps running");
        assert!(fx.contains(&Effect::MoveWindows { windows: vec![7], workspace: "clamshell-game-steam-620".into() }), "{fx:?}");
        s.apply(Event::Tick { unix_secs: 1600 });
        let (_, fx) = s.apply(Event::Windows(vec![win(1, "zelda3", &zelda, true)]));
        assert_eq!(fx.first(), Some(&Effect::RecordPlay { game: "ff6".into(), start: 1000, duration: 600 }));
        assert_eq!(s.running.as_deref(), Some("zelda-lttp"));
        // A Steam game the library doesn't have isn't one.
        s.apply(Event::Windows(vec![win(1, "zelda3", &zelda, false), win(9, "steam_app_1", GAME_WORKSPACE, true)]));
        assert_eq!(s.games.len(), 1);
        let _ = Library::default();
    }

    #[test]
    fn the_settings_follow_the_front() {
        let mut s = state();
        s.apply(Event::Started("zelda-lttp".into()));
        s.apply(Event::Activate(Id::Game("ff6".into())));
        assert_eq!(s.context().map(|g| &*g.id), Some("zelda-lttp"), "the game in front");
        s.apply(Event::Windows(vec![win(1, SHELF_APP_ID, "clamshell-shelf", true)]));
        assert_eq!(s.context().map(|g| &*g.id), Some("ff6"), "shelf in front: its selection");
        s.apply(Event::Windows(vec![win(2, "foot", "clamshell-app-terminal-1-0", true)]));
        assert_eq!(s.context(), None, "an app in front: all games");
    }
}
