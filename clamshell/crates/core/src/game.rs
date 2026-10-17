//! Running games and what is in front. Games run side by side, each on a workspace of its own: one shelf
//! started has a scope and workspace named alike ([`Instance::Game`]); one Steam started (a
//! `steam_app_<id>` window of a library game) gets [`Instance::Steam`], its windows moved there from
//! where they land. "The game" is the running game most recently in front: its settings are in force,
//! the settings page and the pad follow it. A game Steam starts while another runs becomes the game; the
//! other keeps running in the background.
//!
//! In shelf, activating a running game shows it; with another running, shelf offers to keep that one
//! running or close it, then starts this one. What is in front (shelf, a game, another app) decides where
//! the pad goes, what Home does and what the settings page edits.

use std::sync::Arc;

use clamshell_library::Launch;

use crate::State;
use crate::effect::{Effect, Save, WindowCommand};
use crate::id::{Id, ShelfId};
use crate::instance::Instance;
use crate::windows::Window;

/// What is in front on the main screen.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Front {
    Shelf,
    Game,
    /// Another app (a dock instance, or anything else).
    App,
}

/// Who started a running game.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Origin {
    /// Shelf, with Steam's id if Steam runs it (its windows are found by it). The launcher thread says
    /// when it has ended.
    Shelf { appid: Option<u32> },
    /// Steam, by its id: its play ends, and is recorded here, when its last window closes.
    Steam(u32),
}

/// A game that runs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RunningGame {
    pub id: Arc<str>,
    /// Its workspace (and, if shelf started it, its scope).
    pub instance: Instance,
    pub origin: Origin,
    /// When it started (epoch seconds).
    pub start: u64,
}

impl RunningGame {
    /// Steam's id, if Steam runs it.
    pub fn appid(&self) -> Option<u32> {
        match self.origin {
            Origin::Shelf { appid } => appid,
            Origin::Steam(appid) => Some(appid),
        }
    }
}

/// A launch asked for and not started yet (the launcher thread says when it has, or that it couldn't).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Launching {
    pub game: Arc<str>,
    pub instance: Instance,
    /// Steam's id, if Steam runs it.
    pub appid: Option<u32>,
}

/// A game to start once the one closed for it has ended.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Queued {
    pub closing: Arc<str>,
    pub start: Arc<str>,
}

impl State {
    /// Shelf's action on its selected game: show it if it runs; start it if nothing else does; with
    /// another game running, the choice (keep it running or close it) is offered instead, and the focus
    /// goes to it.
    pub(crate) fn play(&mut self, game: Arc<str>) -> Vec<Effect> {
        if let Some(g) = self.session.running_game(&game) {
            return vec![Effect::Window(WindowCommand::Focus(g.instance.name()))];
        }
        if self.session.current.is_some() {
            self.pointer.focus = Some(Id::Shelf(ShelfId::StartKeep));
            self.pointer.show_focus = true;
            return vec![];
        }
        self.launch(game)
    }

    /// The choice in shelf, with another game running: start the selected one and keep the other
    /// running, or close the other first (the selected one starts once it has ended).
    pub(crate) fn start_beside(&mut self, close: bool) -> Vec<Effect> {
        let Some(game) = self.shelf.selected().cloned() else {
            return vec![];
        };
        let Some(current) = self.session.current.clone().filter(|_| close) else {
            return self.launch(game);
        };
        self.session.queued = Some(Queued { closing: current.clone(), start: game });
        self.quit_game(&current)
    }

    /// Closing a game: its windows asked to close, its scope (ours) ended if it doesn't go.
    pub(crate) fn quit_game(&self, id: &str) -> Vec<Effect> {
        let session = &self.session;
        let Some(g) = session.running_game(id) else {
            return vec![];
        };
        let (windows, units, steam) = self.game_parts(g);
        vec![Effect::QuitGame { windows, units, steam }]
    }

    /// The dock's Force kill on a game: its scopes, or (Steam's) the processes under Steam's launcher for
    /// it, killed at once.
    pub(crate) fn kill_game(&self, id: &str) -> Vec<Effect> {
        let Some(g) = self.session.running_game(id) else {
            return vec![];
        };
        let (_, units, steam) = self.game_parts(g);
        vec![Effect::KillGame { units, steam }]
    }

    /// A running game's windows, its scopes (one shelf started), and its Steam app id when it has no scope
    /// of ours (Steam's: it runs in Steam's).
    fn game_parts(&self, g: &RunningGame) -> (Vec<i64>, Vec<Instance>, Option<u32>) {
        let session = &self.session;
        let windows = session.windows.iter().filter(|w| session.game_of(w).is_some_and(|o| o.id == g.id)).map(|w| w.id).collect();
        let units: Vec<Instance> = session.units.iter().filter(|u| **u == g.instance).cloned().collect();
        let steam = if units.is_empty() { g.appid() } else { None };
        (windows, units, steam)
    }

    /// `ended` has ended: the game queued to start once it had, if any.
    fn queued_launch(&mut self, ended: &str) -> Vec<Effect> {
        match self.session.queued.take() {
            Some(Queued { closing, start }) if &*closing == ended => self.launch(start),
            other => {
                self.session.queued = other;
                vec![]
            }
        }
    }

    /// Launching `game` on a workspace of its own (unless it runs already, or is starting).
    pub(crate) fn launch(&mut self, game: Arc<str>) -> Vec<Effect> {
        let session = &self.session;
        if session.running_game(&game).is_some() || session.launching.iter().any(|l| l.game == game) {
            return vec![];
        }
        let Some(g) = self.shelf.game(&game) else {
            return vec![];
        };
        let (launch, file) = (g.launch.clone(), g.file.clone());
        let appid = match launch {
            Launch::Steam { appid } => Some(appid),
            _ => None,
        };
        let (secs, n) = self.session.new_name(self.unix_secs);
        let instance = Instance::Game { secs, n };
        let settings = self.settings.of(&game);
        let env = self.settings.env_of(&game);
        self.session.launching.push(Launching { game: game.clone(), instance: instance.clone(), appid });
        // Shown once its first window appears.
        self.session.pending_show = Some(instance.clone());
        vec![Effect::Save(Save::Resume(self.resume_text())), Effect::Launch { game, instance, settings, env, launch, file }]
    }

    /// A game started (shelf's launch, now running; or Steam's, already listed): it is the game, and
    /// the second screen lights up again for it to darken.
    pub(crate) fn game_started(&mut self, game: Arc<str>) {
        let session = &mut self.session;
        if !session.games.iter().any(|g| g.id == game) {
            let (instance, appid) = match session.launching.iter().position(|l| l.game == game) {
                Some(i) => {
                    let l = session.launching.remove(i);
                    (l.instance, l.appid)
                }
                // Not asked for here (only in tests: Started follows our own launches): the landing
                // workspace.
                None => (Instance::Landing, None),
            };
            session.games.push(RunningGame { id: game.clone(), instance, origin: Origin::Shelf { appid }, start: self.unix_secs });
        }
        session.current = Some(game);
        self.bottom.woken = false;
    }

    /// A game ended after `duration` seconds from `start`: its play counted, and it no longer runs.
    pub(crate) fn game_played(&mut self, game: &str, start: u64, duration: u64) -> Vec<Effect> {
        if self.shelf.add_play(game, start, duration) {
            self.saves.resume.touch(self.unix_secs);
        }
        self.game_gone(game)
    }

    /// A game no longer runs (it ended, or its launch failed). If it was the game, the most recently
    /// shown of the others is now (its settings applied), or none is and the global settings apply
    /// again.
    pub(crate) fn game_gone(&mut self, game: &str) -> Vec<Effect> {
        let session = &mut self.session;
        session.games.retain(|g| &*g.id != game);
        if session.current.as_deref() != Some(game) {
            return self.queued_launch(game);
        }
        let next = session.recent.iter().find_map(|ws| session.games.iter().find(|g| g.instance.is(ws))).or_else(|| session.games.last()).map(|g| g.id.clone());
        session.current = next.clone();
        let apply = match next {
            Some(id) => Effect::Apply { settings: self.settings.of(&id), game: Some(id) },
            None => Effect::Apply { game: None, settings: self.settings.global },
        };
        let mut effects = vec![apply];
        effects.extend(self.queued_launch(game));
        effects
    }

    /// After the windows changed: a game shown becomes the game (its settings applied); a Steam game's
    /// window of a library game not running yet starts its play (it becomes the game; any other keeps
    /// running); Steam's windows go to their game's workspace; a game Steam started whose last window
    /// closed has ended (its play recorded here).
    pub(crate) fn follow_games(&mut self) -> Vec<Effect> {
        let mut effects = Vec::new();
        let windows = &self.session.windows;
        // Steam's games whose windows are all gone.
        let ended: Vec<(Arc<str>, u64)> = self
            .session
            .games
            .iter()
            .filter(|g| matches!(g.origin, Origin::Steam(appid) if !windows.iter().any(|w| w.steam_id() == Some(appid))))
            .map(|g| (g.id.clone(), g.start))
            .collect();
        for (game, start) in ended {
            let duration = self.unix_secs.saturating_sub(start);
            effects.push(Effect::RecordPlay { game: game.clone(), start, duration });
            effects.extend(self.game_played(&game, start, duration));
        }
        // A new Steam game.
        let session = &self.session;
        let new = session.windows.iter().filter_map(Window::steam_id).find_map(|appid| {
            let running = session.games.iter().any(|g| g.appid() == Some(appid)) || session.launching.iter().any(|l| l.appid == Some(appid));
            let game = self.shelf.library().games.iter().find(|g| g.launch == Launch::Steam { appid });
            game.filter(|_| !running).map(|g| (g.id.clone(), appid))
        });
        if let Some((game, appid)) = new {
            let instance = Instance::Steam(appid);
            let start = self.unix_secs;
            self.session.games.push(RunningGame { id: game.clone(), instance: instance.clone(), origin: Origin::Steam(appid), start });
            self.session.pending_show = Some(instance);
            effects.push(Effect::Apply { game: Some(game.clone()), settings: self.settings.of(&game) });
            self.game_started(game);
        }
        // Steam's windows to their game's workspace.
        let session = &self.session;
        for g in &session.games {
            let Some(appid) = g.appid() else {
                continue;
            };
            let stray: Vec<i64> = session.windows.iter().filter(|w| w.steam_id() == Some(appid) && !g.instance.is(&w.workspace)).map(|w| w.id).collect();
            if !stray.is_empty() {
                effects.push(Effect::Window(WindowCommand::Move { windows: stray, to: g.instance.clone() }));
            }
        }
        // The game shown is the game.
        let shown = session.windows.iter().find(|w| w.focused).and_then(|w| session.game_of(w)).map(|g| g.id.clone());
        if let Some(id) = shown
            && session.current.as_ref() != Some(&id)
        {
            self.session.current = Some(id.clone());
            effects.push(Effect::Apply { game: Some(id.clone()), settings: self.settings.of(&id) });
        }
        effects
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use crate::testkit::*;
    use crate::*;

    fn launches(fx: &[Effect], id: &str) -> bool {
        fx.iter().any(|e| matches!(e, Effect::Launch { game, .. } if &**game == id))
    }

    fn steam_ff6() -> State {
        let mut library = clamshell_library::demo_library();
        if let Some(g) = library.games.iter_mut().find(|g| &*g.id == "ff6") {
            g.launch = Launch::Steam { appid: 620 };
        }
        State::new(Init { library, ..init() })
    }

    #[test]
    fn a_game_runs_and_its_play_counts() {
        let mut s = state();
        let first = s.shelf().selected().cloned().unwrap();
        s.apply(GameEvent::Started(first.clone()).into());
        let fx = tap(&mut s, Id::Shelf(ShelfId::Launch));
        assert!(!fx.iter().any(|e| matches!(e, Effect::Launch { .. })), "no second launch while one runs: {fx:?}");
        s.apply(GameEvent::Played { game: first.clone(), start: 1000, duration: 600 }.into());
        assert_eq!(s.current_game(), None);
        let g = s.game(&first).unwrap();
        assert_eq!((g.plays.count, g.plays.seconds), (1, 600));
        assert!(s.shelf().library().collections.iter().any(|c| c.name == "Recent"));
    }

    #[test]
    fn a_second_game_is_offered_beside_or_instead_of_the_first() {
        let mut s = state();
        s.apply(Event::Tick { unix_secs: 100 });
        let zelda = launch(&mut s, "zelda-lttp");
        assert_eq!(zelda, Instance::Game { secs: 100, n: 0 });
        assert_eq!(tap(&mut s, game("zelda-lttp")), vec![focus(&zelda)], "running: shown");
        tap(&mut s, game("ff6"));
        assert_eq!(tap(&mut s, game("ff6")), vec![], "another runs: the choice");
        assert_eq!(s.pointer().focus(), Some(&Id::Shelf(ShelfId::StartKeep)));
        // Keep it running: both run, the new one is the game.
        assert!(launches(&tap(&mut s, Id::Shelf(ShelfId::StartKeep)), "ff6"));
        s.apply(GameEvent::Started("ff6".into()).into());
        assert_eq!((s.session().games().len(), s.current_game().map(|g| &**g)), (2, Some("ff6")));
        // ff6 ends: zelda is the game again, its settings applied.
        let fx = played(&mut s, "ff6");
        assert_eq!(s.current_game().map(|g| &**g), Some("zelda-lttp"));
        assert!(matches!(fx.first(), Some(Effect::Apply { game: Some(g), .. }) if &**g == "zelda-lttp"));
        // Close it: zelda quits, then ff6 starts.
        tap(&mut s, game("ff6"));
        let fx = tap(&mut s, Id::Shelf(ShelfId::StartClose));
        assert!(matches!(fx.as_slice(), [Effect::QuitGame { .. }]), "{fx:?}");
        assert!(launches(&played(&mut s, "zelda-lttp"), "ff6"));
    }

    #[test]
    fn a_queued_start_waits_for_the_game_closed_for_it() {
        let mut s = state();
        launch(&mut s, "zelda-lttp");
        tap(&mut s, game("ff6"));
        tap(&mut s, Id::Shelf(ShelfId::StartKeep));
        s.apply(GameEvent::Started("ff6".into()).into());
        // ff6 is the game; close it for a third.
        let third = s.shelf().library().games.iter().map(|g| g.id.clone()).find(|id| &**id != "ff6" && &**id != "zelda-lttp").unwrap();
        tap(&mut s, game(&third));
        tap(&mut s, Id::Shelf(ShelfId::StartClose));
        assert!(!launches(&played(&mut s, "zelda-lttp"), &third), "another game ending");
        assert!(!launches(&s.apply(GameEvent::LaunchFailed("zelda-lttp".into()).into()).1, &third), "another game failing");
        assert!(launches(&played(&mut s, "ff6"), &third), "the one closed for it");
    }

    #[test]
    fn the_game_is_the_one_last_shown() {
        let mut s = state();
        let zelda = launch(&mut s, "zelda-lttp").name();
        tap(&mut s, game("ff6"));
        tap(&mut s, Id::Shelf(ShelfId::StartKeep));
        s.apply(GameEvent::Started("ff6".into()).into());
        let ff6 = s.session().running_game("ff6").unwrap().instance.name();
        let (_, fx) = s.apply(Event::Windows(vec![win(1, "zelda3", &zelda, true), win(2, "ff6", &ff6, false)]));
        assert_eq!(s.current_game().map(|g| &**g), Some("zelda-lttp"));
        assert!(fx.iter().any(|e| matches!(e, Effect::Apply { game: Some(g), .. } if &**g == "zelda-lttp")), "{fx:?}");
        assert_eq!(s.session().front(), Front::Game);
    }

    #[test]
    fn a_game_steam_started_is_the_game_and_the_other_keeps_running() {
        let mut s = steam_ff6();
        s.apply(Event::Tick { unix_secs: 1000 });
        let zelda = launch(&mut s, "zelda-lttp").name();
        let landing = Instance::Landing.name();
        let (_, fx) = s.apply(Event::Windows(vec![win(1, "zelda3", &zelda, false), win(7, "steam_app_620", &landing, true)]));
        assert_eq!(s.current_game().map(|g| &**g), Some("ff6"), "the newest is the game");
        assert_eq!(s.session().games().len(), 2, "the other keeps running");
        assert!(fx.contains(&Effect::Window(WindowCommand::Move { windows: vec![7], to: Instance::Steam(620) })), "{fx:?}");
        s.apply(Event::Tick { unix_secs: 1600 });
        let (_, fx) = s.apply(Event::Windows(vec![win(1, "zelda3", &zelda, true)]));
        assert_eq!(fx.first(), Some(&Effect::RecordPlay { game: "ff6".into(), start: 1000, duration: 600 }));
        assert_eq!(s.current_game().map(|g| &**g), Some("zelda-lttp"));
        // A Steam game the library doesn't have isn't one.
        s.apply(Event::Windows(vec![win(1, "zelda3", &zelda, false), win(9, "steam_app_1", &landing, true)]));
        assert_eq!(s.session().games().len(), 1);
    }

    #[test]
    fn a_steam_game_shelf_started_ends_by_the_launcher_not_its_windows() {
        let mut s = steam_ff6();
        let ff6 = launch(&mut s, "ff6");
        assert_eq!(s.session().running_game("ff6").map(|g| g.origin), Some(Origin::Shelf { appid: Some(620) }));
        let landing = Instance::Landing.name();
        let fx = s.apply(Event::Windows(vec![win(7, "steam_app_620", &landing, true)])).1;
        assert!(fx.contains(&Effect::Window(WindowCommand::Move { windows: vec![7], to: ff6 })), "its windows go to its workspace: {fx:?}");
        let fx = s.apply(Event::Windows(vec![])).1;
        assert!(!fx.iter().any(|e| matches!(e, Effect::RecordPlay { .. })), "{fx:?}");
        assert_eq!(s.current_game().map(|g| &**g), Some("ff6"));
    }

    #[test]
    fn the_settings_follow_the_front() {
        let mut s = state();
        s.apply(GameEvent::Started("zelda-lttp".into()).into());
        tap(&mut s, game("ff6"));
        assert_eq!(s.context().map(|g| &*g.id), Some("zelda-lttp"), "the game in front");
        s.apply(Event::Windows(vec![win(1, apps::SHELF_APP_ID, &Instance::Shelf.name(), true)]));
        assert_eq!(s.context().map(|g| &*g.id), Some("ff6"), "shelf in front: its selection");
        s.apply(Event::Windows(vec![win(2, "foot", "clamshell-app-terminal-1-0", true)]));
        assert_eq!(s.context(), None, "an app in front: all games");
    }
}
