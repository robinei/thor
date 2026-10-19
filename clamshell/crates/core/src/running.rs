//! Running entries: what was launched from shelf or the dock, a game or an app alike. Each launch is a
//! scope of its own in the entry's slice, its windows on the entry's workspace (see [`Instance`]); an
//! entry may run more than once (another launch is another scope, another tab). One that Steam runs (a
//! `steam_app_<id>` window of a library entry) isn't in a scope of ours: it runs in Steam's, its windows
//! moved to its entry's workspace from where they land, and it has ended when its last window has.
//!
//! A file opened from the Files tab (a browsed entry) whose launch hands it to a program not ours and ends
//! (`xdg-open` to an app's own instance, D-Bus activation) runs on by its windows: a new window of no launch
//! of ours appearing within [`HANDOFF`] is moved to its workspace, and it has ended when that window has
//! closed (or when none came).
//!
//! In shelf, activating a running entry shows it, a stopped one starts. Quit asks an entry's windows to
//! close and, once none is left and it still runs, ends it: its slice stopped, or (Steam's) the processes
//! under Steam's launcher for it; Force kill ends it at once. What is in front (shelf, a running entry,
//! another window) decides where the pad goes, what Home does and what the settings page edits.

use std::sync::Arc;

use clamshell_library::Launch;

use crate::State;
use crate::effect::{Effect, Save, WindowCommand};
use crate::instance::Instance;
use crate::windows::Window;

/// What is in front on the main screen.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Front {
    Shelf,
    /// A window of a running entry.
    Program,
    /// Another window (none of ours).
    Other,
}

/// Who started a running entry.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Origin {
    /// ClamShell, with Steam's id if Steam runs it. The launcher thread says when each launch has ended.
    Ours { appid: Option<u32> },
    /// Steam, by its id: its play ends, and is recorded here, when its last window closes.
    Steam(u32),
    /// Ours, but what we started handed over to a program not ours and ended: it runs by its windows,
    /// taking a new stray window until `until` (epoch seconds), ending once it has had windows (`seen`)
    /// and has none, or none came.
    Handed { until: u64, seen: bool },
}

/// How long a launch that handed over waits for its window (seconds).
pub const HANDOFF: u64 = 15;

/// An entry that runs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Running {
    pub id: Arc<str>,
    /// Its key: its workspace's (and its launches' scopes' and slice's) names.
    pub key: String,
    pub origin: Origin,
    /// When it first started (epoch seconds).
    pub start: u64,
    /// Its launches that run (ours: each ends on its own).
    pub(crate) launches: u32,
    /// Found running by its scopes (started before ClamShell was): it runs while systemd lists them.
    pub(crate) adopted: bool,
    /// Its entry as it was when it started: the dock shows it by this if it leaves the library while it
    /// runs (its file renamed or deleted).
    pub entry: Option<Box<crate::Game>>,
}

impl Running {
    /// Steam's id, if Steam runs it.
    pub fn appid(&self) -> Option<u32> {
        match self.origin {
            Origin::Ours { appid } => appid,
            Origin::Steam(appid) => Some(appid),
            Origin::Handed { .. } => None,
        }
    }

    /// Its workspace's name.
    pub fn workspace(&self) -> String {
        Instance::workspace_of(&self.key)
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

impl State {
    /// Shelf's action on its selected entry, or a tap on a stopped dock entry: show it if it runs, else
    /// start it.
    /// A group isn't started: it is shown open in shelf.
    pub(crate) fn play(&mut self, game: Arc<str>) -> Vec<Effect> {
        // A browsed file that is a library entry too: as that entry.
        let game = self.shelf.entry_of_file(&game).unwrap_or(game);
        if self.session.running(&game).is_some() {
            return self.show_running(&game);
        }
        self.launch(game)
    }

    /// Running entry `id` shown: its workspace, if a window of it is there to see; else what is in front
    /// stays, and a notice says it has none (it runs without one, hasn't shown one yet, or lost its
    /// display: its dock menu's Quit ends it). Never an empty screen.
    pub(crate) fn show_running(&mut self, id: &str) -> Vec<Effect> {
        let session = &self.session;
        let Some(r) = session.running(id) else {
            return vec![];
        };
        if session.windows.iter().any(|w| session.running_of(w).is_some_and(|o| o.id == r.id)) {
            return vec![Effect::Window(WindowCommand::Focus(r.workspace()))];
        }
        let title = r.entry.as_ref().map_or_else(|| id.to_owned(), |g| g.title.clone());
        self.notify(crate::hardware::NoticeKind::NoWindow(title));
        vec![]
    }

    /// Launches `game` (again, if it runs: another tab), unless a launch of it is starting; one Steam runs
    /// is shown instead (Steam doesn't start a game twice).
    pub(crate) fn launch(&mut self, game: Arc<str>) -> Vec<Effect> {
        self.launch_with(game, None)
    }

    /// Launches `game` as [`State::launch`] does, by `command` if given (then a group too) instead of its own.
    pub(crate) fn launch_with(&mut self, game: Arc<str>, command: Option<Launch>) -> Vec<Effect> {
        if let Some(Launch::Tool(tool)) = self.shelf.game(&game).map(|g| &g.launch) {
            return vec![Effect::Tool(tool.clone())];
        }
        if command.is_none() && self.shelf.game(&game).is_some_and(crate::Game::is_group) {
            if !self.shelf.reveal(&game) {
                return vec![];
            }
            self.moved_collection();
            return vec![Effect::Window(WindowCommand::ShowShelf)];
        }
        let session = &self.session;
        if session.launching.iter().any(|l| l.game == game) {
            return vec![];
        }
        if session.running(&game).is_some_and(|r| r.appid().is_some()) {
            return self.show_running(&game);
        }
        let Some(g) = self.shelf.game(&game) else {
            return vec![];
        };
        let watch = g.mpv_file().map(std::path::Path::to_path_buf).filter(|_| command.is_none());
        let (launch, key) = (command.unwrap_or_else(|| g.launch.clone()), g.key.clone());
        let appid = match launch {
            Launch::Steam { appid } => Some(appid),
            _ => None,
        };
        let (secs, n) = self.session.new_name(self.unix_secs);
        let instance = Instance::Run { key, secs, n };
        let settings = self.settings_of(&game);
        let env = self.env_of(&game);
        self.session.launching.push(Launching { game: game.clone(), instance: instance.clone(), appid });
        // Shown once its first window appears.
        self.session.pending_show = Some(instance.clone());
        vec![Effect::Save(Save::Resume(self.resume())), Effect::Launch { game, instance, settings, env, launch, watch }]
    }

    /// A launch has started (or a Steam game Steam started is seen): the entry runs (one launch more), and
    /// the second screen lights up again for it to darken.
    pub(crate) fn started(&mut self, game: Arc<str>) {
        let session = &mut self.session;
        let appid = match session.launching.iter().position(|l| l.game == game) {
            Some(i) => session.launching.remove(i).appid,
            None => None,
        };
        match session.running.iter_mut().find(|r| r.id == game) {
            Some(r) => r.launches += 1,
            None => {
                let key = self.shelf.game(&game).map_or_else(|| clamshell_library::key_of(&game), |g| g.key.clone());
                let entry = self.shelf.game(&game).cloned().map(Box::new);
                session.running.push(Running { id: game, key, origin: Origin::Ours { appid }, start: self.unix_secs, launches: 1, adopted: false, entry });
            }
        }
        self.bottom.woken = false;
    }

    /// A launch ended after `duration` seconds from `start`: its play counted, one launch less.
    pub(crate) fn played(&mut self, game: &str, start: u64, duration: u64) {
        if self.shelf.add_play(game, start, duration) {
            self.saves.resume.touch(self.unix_secs);
        }
        self.ended(game);
    }

    /// A launch of `game` ended (or couldn't start): it no longer runs once none does; a browsed file's,
    /// or one whose runner hands its app over, soon after it started runs on by its windows (see
    /// [`Origin::Handed`]).
    pub(crate) fn ended(&mut self, game: &str) {
        self.session.launching.retain(|l| &*l.game != game || l.appid.is_some());
        let browsed = self.shelf.game(game).is_some_and(|g| g.browsed.is_some()) || self.hands_over(game);
        let now = self.unix_secs;
        let session = &mut self.session;
        if let Some(r) = session.running.iter_mut().find(|r| &*r.id == game) {
            r.launches = r.launches.saturating_sub(1);
            if r.launches == 0 && browsed && matches!(r.origin, Origin::Ours { .. }) && now.saturating_sub(r.start) <= HANDOFF {
                let seen = session.windows.iter().any(|w| Instance::key_of(&w.workspace) == Some(r.key.as_str()));
                r.origin = Origin::Handed { until: r.start + HANDOFF, seen };
            }
        }
        session.running.retain(|r| r.launches > 0 || matches!(r.origin, Origin::Steam(_) | Origin::Handed { .. }));
    }

    /// Whether `game`'s runner hands its app over (see [`Origin::Handed`]).
    fn hands_over(&self, game: &str) -> bool {
        self.game(game).and_then(|g| g.runner.as_ref()).is_some_and(|r| r.handover)
    }

    /// A launch that never started (its scope was gone before it was seen): a browsed file's waits for
    /// its window (see [`Origin::Handed`]).
    pub(crate) fn launch_failed(&mut self, game: Arc<str>) {
        if let Some(i) = self.session.launching.iter().position(|l| l.game == game) {
            self.session.launching.remove(i);
        }
        let Some(entry) = self.shelf.game(&game).filter(|g| g.browsed.is_some()).cloned() else {
            return;
        };
        let key = entry.key.clone();
        if self.session.running(&game).is_none() {
            let now = self.unix_secs;
            let origin = Origin::Handed { until: now + HANDOFF, seen: false };
            self.session.running.push(Running { id: game, key, origin, start: now, launches: 0, adopted: false, entry: Some(Box::new(entry)) });
        }
    }

    /// After the windows changed, and every second: a new window of no launch of ours (not shelf's, not
    /// a Steam game's, on no running entry's workspace) goes to the latest entry waiting for one (see
    /// [`Origin::Handed`]), shown; those that had windows and have none, or waited in vain, have ended.
    /// `known`: the windows there before.
    pub(crate) fn follow_handed(&mut self, known: &[i64]) -> Vec<Effect> {
        let now = self.unix_secs;
        let session = &self.session;
        let stray: Vec<i64> = session
            .windows
            .iter()
            .filter(|w| !known.contains(&w.id) && w.home().is_none() && w.steam_id().is_none() && !w.down && session.running_of(w).is_none())
            .map(|w| w.id)
            .collect();
        let mut effects = Vec::new();
        let mut taken = Vec::new();
        // A window whose app id is the desktop file's name of a running entry whose runner hands its app
        // over is that entry's, whenever it comes.
        let mut rest = Vec::new();
        for id in stray {
            let app_id = session.windows.iter().find(|w| w.id == id).map(|w| w.app_id.as_str()).unwrap_or("");
            match session.running.iter().find(|r| r.id.strip_prefix("desktop:") == Some(app_id) && self.hands_over(&r.id)) {
                Some(r) => {
                    effects.push(Effect::Window(WindowCommand::Move { windows: vec![id], to: r.workspace() }));
                    effects.push(Effect::Window(WindowCommand::Focus(r.workspace())));
                    taken.push(r.id.clone());
                }
                None => rest.push(id),
            }
        }
        // Else to the latest launch still waiting for one.
        let waiting = session.running.iter().filter(|r| matches!(r.origin, Origin::Handed { until, seen: false } if now <= until)).max_by_key(|r| r.start);
        if let Some(r) = waiting.filter(|_| !rest.is_empty()) {
            effects.push(Effect::Window(WindowCommand::Move { windows: rest, to: r.workspace() }));
            effects.push(Effect::Window(WindowCommand::Focus(r.workspace())));
            taken.push(r.id.clone());
        }
        // Those with windows on their workspace (a window just taken is still where it came up).
        let windows = &self.session.windows;
        let with: Vec<Arc<str>> = self
            .session
            .running
            .iter()
            .filter(|r| taken.contains(&r.id) || windows.iter().any(|w| Instance::key_of(&w.workspace) == Some(r.key.as_str())))
            .map(|r| r.id.clone())
            .collect();
        self.session.running.retain_mut(|r| match &mut r.origin {
            Origin::Handed { until, seen } => {
                let has = with.contains(&r.id);
                *seen |= has;
                r.launches > 0 || if *seen { has } else { now <= *until }
            }
            _ => true,
        });
        effects
    }

    /// After systemd's list of our scopes changed: an entry with scopes listed that isn't known to run
    /// (started before ClamShell was) runs, adopted; one adopted whose scopes are all gone has ended.
    pub(crate) fn follow_units(&mut self) {
        let session = &mut self.session;
        let units = &session.units;
        session.running.retain(|r| !r.adopted || units.iter().any(|u| u.key() == Some(r.key.as_str())));
        for unit in units {
            let Some(key) = unit.key() else {
                continue;
            };
            if session.running.iter().any(|r| r.key == key) || session.launching.iter().any(|l| l.instance.key() == Some(key)) {
                continue;
            }
            let Some(g) = self.shelf.library().games.iter().find(|g| g.key == key) else {
                continue;
            };
            let launches = u32::try_from(units.iter().filter(|u| u.key() == Some(key)).count()).unwrap_or(u32::MAX);
            let start = self.unix_secs;
            let entry = Some(Box::new(g.clone()));
            session.running.push(Running {
                id: g.id.clone(),
                key: key.to_owned(),
                origin: Origin::Ours { appid: None },
                start,
                launches,
                adopted: true,
                entry,
            });
        }
    }

    /// The app whose window shows browsed file `id` (running): the desktop file named as the window's
    /// app id says (`org.kde.kate`: `desktop:org.kde.kate`; any case), else the one whose program it is.
    /// For its picture only: the file runs as itself.
    pub fn app_showing(&self, id: &str) -> Option<&crate::Game> {
        self.shelf.game(id).filter(|g| g.browsed.is_some())?;
        let session = &self.session;
        let window = session.windows.iter().find(|w| session.running_of(w).is_some_and(|r| &*r.id == id))?;
        let app = window.app_id.to_lowercase();
        let games = &self.shelf.library().games;
        let program = |g: &&crate::Game| match &g.launch {
            Launch::Program { argv, .. } => argv.first().and_then(|p| p.rsplit('/').next()).is_some_and(|p| p.to_lowercase() == app),
            _ => false,
        };
        let named = |g: &&crate::Game| g.id.strip_prefix("desktop:").is_some_and(|d| d.to_lowercase() == app);
        games.iter().find(named).or_else(|| games.iter().filter(|g| g.id.starts_with("desktop:")).find(program))
    }

    /// The windows of a running entry (moved to the bottom screen too).
    pub(crate) fn windows_of(&self, game: &str) -> Vec<i64> {
        let session = &self.session;
        session.windows.iter().filter(|w| session.running_of(w).is_some_and(|r| &*r.id == game)).map(|w| w.id).collect()
    }

    /// Quit: the entry's windows asked to close; once none is left (a "save changes?" window keeps it
    /// until answered) it is ended if it still runs (see [`State::follow_quitting`]).
    pub(crate) fn quit(&mut self, game: &Arc<str>) -> Vec<Effect> {
        let windows = self.windows_of(game);
        self.session.quitting.retain(|(g, _)| g != game);
        self.session.quitting.push((game.clone(), false));
        let mut effects = if windows.is_empty() { vec![] } else { vec![Effect::Window(WindowCommand::Close(windows))] };
        effects.extend(self.follow_quitting());
        effects
    }

    /// Force kill: the entry ended at once.
    pub(crate) fn force_kill(&mut self, game: &str) -> Vec<Effect> {
        self.session.quitting.retain(|(g, _)| &**g != game);
        self.end(game, true).into_iter().collect()
    }

    /// Ending a running entry: its slice stopped (`force`: killed), or (Steam's) the processes under
    /// Steam's launcher for it killed.
    fn end(&self, game: &str, force: bool) -> Option<Effect> {
        let r = self.session.running(game)?;
        Some(match r.appid() {
            Some(appid) => Effect::EndSteam { appid },
            None => Effect::EndRun { key: r.key.clone(), force },
        })
    }

    /// Entries quitting: one whose windows are all gone while it still runs is ended (once); one that no
    /// longer runs is done.
    pub(crate) fn follow_quitting(&mut self) -> Vec<Effect> {
        let mut quitting = std::mem::take(&mut self.session.quitting);
        quitting.retain(|(game, _)| self.session.running(game).is_some());
        let mut effects = vec![];
        for (game, ended) in &mut quitting {
            if !*ended && self.windows_of(game).is_empty() {
                *ended = true;
                effects.extend(self.end(game, false));
            }
        }
        self.session.quitting = quitting;
        effects
    }

    /// After the windows changed: a Steam game's window of a library entry not running yet starts its play
    /// (it runs beside the others); Steam's windows go to their entry's workspace; a game Steam started
    /// whose last window closed has ended (its play recorded here).
    pub(crate) fn follow_steam(&mut self) -> Vec<Effect> {
        let mut effects = Vec::new();
        let windows = &self.session.windows;
        // Steam's games whose windows are all gone.
        let ended: Vec<(Arc<str>, u64)> = self
            .session
            .running
            .iter()
            .filter(|r| matches!(r.origin, Origin::Steam(appid) if !windows.iter().any(|w| w.steam_id() == Some(appid))))
            .map(|r| (r.id.clone(), r.start))
            .collect();
        for (game, start) in ended {
            let duration = self.unix_secs.saturating_sub(start);
            effects.push(Effect::RecordPlay { game: game.clone(), start, duration });
            if self.shelf.add_play(&game, start, duration) {
                self.saves.resume.touch(self.unix_secs);
            }
            self.session.running.retain(|r| r.id != game);
        }
        // A new Steam game.
        let session = &self.session;
        let new = session.windows.iter().filter_map(Window::steam_id).find_map(|appid| {
            let running = session.running.iter().any(|r| r.appid() == Some(appid)) || session.launching.iter().any(|l| l.appid == Some(appid));
            let game = self.shelf.library().games.iter().find(|g| g.launch == Launch::Steam { appid });
            game.filter(|_| !running).map(|g| (g.id.clone(), g.key.clone(), appid))
        });
        if let Some((game, key, appid)) = new {
            let start = self.unix_secs;
            let entry = self.shelf.game(&game).cloned().map(Box::new);
            self.session.running.push(Running { id: game, key: key.clone(), origin: Origin::Steam(appid), start, launches: 0, adopted: false, entry });
            self.session.pending_show = Some(Instance::Run { key, secs: start, n: 0 });
            self.bottom.woken = false;
        }
        // Steam's windows to their entry's workspace.
        let session = &self.session;
        for r in &session.running {
            let Some(appid) = r.appid() else {
                continue;
            };
            let stray: Vec<i64> = session
                .windows
                .iter()
                .filter(|w| w.steam_id() == Some(appid) && !w.down && Instance::key_of(&w.workspace) != Some(&r.key))
                .map(|w| w.id)
                .collect();
            if !stray.is_empty() {
                effects.push(Effect::Window(WindowCommand::Move { windows: stray, to: r.workspace() }));
            }
        }
        effects
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use crate::running::HANDOFF;
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

    fn key(id: &str) -> String {
        clamshell_library::key_of(id)
    }

    #[test]
    fn an_entry_runs_its_plays_count_and_it_may_run_twice() {
        let mut s = state();
        let first = s.shelf().selected().cloned().unwrap();
        let fx = tap(&mut s, Id::Shelf(ShelfId::Launch));
        assert!(launches(&fx, &first));
        assert!(!launches(&tap(&mut s, Id::Shelf(ShelfId::Launch)), &first), "not twice while starting");
        s.apply(GameEvent::Started(first.clone()).into());
        let workspace = Instance::workspace_of(&key(&first));
        assert_eq!(tap(&mut s, Id::Shelf(ShelfId::Launch)), vec![], "running, no window yet: nothing to show");
        s.apply(Event::Windows(vec![win(3, "game", &workspace, false)]));
        assert_eq!(tap(&mut s, Id::Shelf(ShelfId::Launch)), vec![Effect::Window(WindowCommand::Focus(workspace))], "running: shown");
        // Another launch (the dock's Open another): it runs twice, until both end.
        assert!(launches(&s.launch(first.clone()), &first));
        s.apply(GameEvent::Started(first.clone()).into());
        s.apply(GameEvent::Played { game: first.clone(), start: 1000, duration: 600 }.into());
        assert!(s.session().running(&first).is_some(), "the other still runs");
        s.apply(GameEvent::Played { game: first.clone(), start: 1000, duration: 60 }.into());
        assert!(s.session().running(&first).is_none());
        let g = s.game(&first).unwrap();
        assert_eq!((g.plays.count, g.plays.seconds), (2, 660));
        assert!(s.shelf().library().collections.iter().any(|c| c.name == "Recent"));
    }

    #[test]
    fn entries_run_side_by_side() {
        let mut s = state();
        s.apply(Event::Tick { unix_secs: 100 });
        let zelda = launch(&mut s, "zelda-lttp");
        assert_eq!(zelda, Instance::Run { key: key("zelda-lttp"), secs: 100, n: 0 });
        tap(&mut s, game("ff6"));
        assert!(launches(&tap(&mut s, Id::Shelf(ShelfId::Launch)), "ff6"), "no choice: it starts");
        s.apply(GameEvent::Started("ff6".into()).into());
        assert_eq!(s.session().running_entries().len(), 2);
    }

    #[test]
    fn quit_closes_the_windows_then_ends_the_slice_force_kill_at_once() {
        let mut s = state();
        let zelda = launch(&mut s, "zelda-lttp");
        let ws = zelda.workspace();
        let window = |id, title: &str| Window { unit: zelda.unit(), title: title.into(), ..win(id, "zelda3", &ws, true) };
        s.apply(Event::Windows(vec![window(1, "game")]));
        let id: std::sync::Arc<str> = "zelda-lttp".into();
        assert_eq!(s.quit(&id), vec![Effect::Window(WindowCommand::Close(vec![1]))]);
        let fx = s.apply(Event::Windows(vec![window(2, "Save changes?")])).1;
        assert!(!fx.iter().any(|e| matches!(e, Effect::EndRun { .. })), "a window left: waits");
        let fx = s.apply(Event::Windows(vec![])).1;
        assert!(fx.contains(&Effect::EndRun { key: key("zelda-lttp"), force: false }), "{fx:?}");
        assert_eq!(s.force_kill("zelda-lttp"), vec![Effect::EndRun { key: key("zelda-lttp"), force: true }]);
    }

    #[test]
    fn a_game_steam_started_runs_beside_and_ends_with_its_windows() {
        let mut s = steam_ff6();
        s.apply(Event::Tick { unix_secs: 1000 });
        let zelda = launch(&mut s, "zelda-lttp").workspace();
        let landing = Instance::Landing.name();
        let (_, fx) = s.apply(Event::Windows(vec![win(1, "zelda3", &zelda, false), win(7, "steam_app_620", &landing, true)]));
        assert_eq!(s.session().running_entries().len(), 2, "the other keeps running");
        let ff6 = Instance::workspace_of(&key("ff6"));
        assert!(fx.contains(&Effect::Window(WindowCommand::Move { windows: vec![7], to: ff6 })), "{fx:?}");
        assert_eq!(s.force_kill("ff6"), vec![Effect::EndSteam { appid: 620 }], "its processes, under Steam's launcher");
        s.apply(Event::Tick { unix_secs: 1600 });
        let (_, fx) = s.apply(Event::Windows(vec![win(1, "zelda3", &zelda, true)]));
        assert_eq!(fx.first(), Some(&Effect::RecordPlay { game: "ff6".into(), start: 1000, duration: 600 }));
        assert!(s.session().running("ff6").is_none());
        // A Steam game the library doesn't have isn't one.
        s.apply(Event::Windows(vec![win(1, "zelda3", &zelda, false), win(9, "steam_app_1", &landing, true)]));
        assert_eq!(s.session().running_entries().len(), 1, "zelda only");
    }

    #[test]
    fn a_steam_game_shelf_started_ends_by_the_launcher_not_its_windows() {
        let mut s = steam_ff6();
        launch(&mut s, "ff6");
        assert_eq!(s.session().running("ff6").map(|r| r.origin), Some(Origin::Ours { appid: Some(620) }));
        let landing = Instance::Landing.name();
        let fx = s.apply(Event::Windows(vec![win(7, "steam_app_620", &landing, true)])).1;
        let ff6 = Instance::workspace_of(&key("ff6"));
        assert!(fx.contains(&Effect::Window(WindowCommand::Move { windows: vec![7], to: ff6 })), "its windows go to its workspace: {fx:?}");
        let fx = s.apply(Event::Windows(vec![])).1;
        assert!(!fx.iter().any(|e| matches!(e, Effect::RecordPlay { .. })), "{fx:?}");
        assert!(s.session().running("ff6").is_some());
    }

    /// A Files tab with a file of its own and one that is a library entry's (zelda's).
    fn files() -> State {
        let mut library = clamshell_library::demo_library();
        if let Some(zelda) = library.games.first_mut() {
            zelda.file = Some("/roms/zelda.sfc".into());
        }
        let browsed = |id: &str, file: &str| crate::Game {
            file: Some(file.into()),
            launch: Launch::program(vec!["xdg-open".into(), file.into()]),
            browsed: Some(crate::Browsed::default()),
            ..crate::Game::new(id, file)
        };
        let n = library.games.len();
        library.games.push(browsed("browse:Home/notes.md", "/h/notes.md"));
        library.games.push(browsed("browse:Home/zelda.sfc", "/roms/zelda.sfc"));
        library.collections.insert(0, crate::Collection::new("Files", vec![n, n + 1]));
        crate::testkit::state_with(library)
    }

    #[test]
    fn a_file_that_is_a_library_entry_opens_as_it() {
        let mut s = files();
        s.apply(ShelfEvent::ListStep(1).into());
        let fx = tap(&mut s, Id::Shelf(ShelfId::Launch));
        assert!(launches(&fx, "zelda-lttp"), "{fx:?}");
    }

    #[test]
    fn a_file_handed_to_another_program_runs_by_its_window() {
        let mut s = files();
        s.apply(Event::Tick { unix_secs: 100 });
        let shelf = win(1, windows::SHELF_APP_ID, &Instance::Shelf.name(), true);
        let other = win(5, "steam", "3", false);
        s.apply(Event::Windows(vec![shelf.clone(), other.clone()]));
        let fx = tap(&mut s, Id::Shelf(ShelfId::Launch));
        let ws = fx.iter().find_map(|e| match e {
            Effect::Launch { instance, .. } => Some(instance.workspace()),
            _ => None,
        });
        let ws = ws.unwrap_or_default();
        assert!(!ws.is_empty(), "{fx:?}");
        // xdg-open ended before its scope was seen; the app's window comes up on shelf's workspace.
        s.apply(GameEvent::LaunchFailed("browse:Home/notes.md".into()).into());
        assert!(s.session().running("browse:Home/notes.md").is_some(), "waits for its window");
        // (The demo's Terminal: its desktop file is foot's.)
        let app = win(6, "foot", &Instance::Shelf.name(), true);
        let (_, fx) = s.apply(Event::Windows(vec![shelf.clone(), other.clone(), app.clone()]));
        assert!(fx.contains(&Effect::Window(WindowCommand::Move { windows: vec![6], to: ws.clone() })), "only the new one: {fx:?}");
        assert!(fx.contains(&Effect::Window(WindowCommand::Focus(ws.clone()))));
        s.apply(Event::Windows(vec![shelf.clone(), other.clone(), Window { workspace: ws, ..app }]));
        s.apply(Event::Tick { unix_secs: 200 });
        assert!(s.session().running("browse:Home/notes.md").is_some(), "runs while its window is open");
        assert_eq!(s.app_showing("browse:Home/notes.md").map(|g| &*g.id), Some("desktop:foot"), "its picture: the app's");
        // Its file gone from the library meanwhile: still shown by what it was.
        s.apply(Event::Library(Box::new(clamshell_library::demo_library())));
        assert!(s.shelf().game("browse:Home/notes.md").is_none());
        assert_eq!(s.game("browse:Home/notes.md").map(|g| g.title.as_str()), Some("/h/notes.md"));
        s.apply(Event::Library(Box::new(files().shelf().library().clone())));
        s.apply(Event::Windows(vec![shelf.clone(), other.clone()]));
        assert!(s.session().running("browse:Home/notes.md").is_none(), "its window closed: ended");
        // Another that never shows a window: gone after a while.
        tap(&mut s, Id::Shelf(ShelfId::Launch));
        s.apply(GameEvent::LaunchFailed("browse:Home/notes.md".into()).into());
        s.apply(Event::Tick { unix_secs: 200 + HANDOFF });
        assert!(s.session().running("browse:Home/notes.md").is_some());
        s.apply(Event::Tick { unix_secs: 201 + HANDOFF });
        assert!(s.session().running("browse:Home/notes.md").is_none());
        assert!(s.dock().items().is_empty(), "off the dock");
    }

    #[test]
    fn an_app_handed_over_runs_by_its_window_found_by_its_app_id() {
        let mut library = clamshell_library::demo_library();
        let firefox = library.games.iter_mut().find(|g| &*g.id == "desktop:firefox").unwrap();
        firefox.runner = Some(clamshell_library::RunnerOf { handover: true, ..clamshell_library::RunnerOf::new("android", "Android", true) });
        let mut s = State::new(Init { library, ..init() });
        s.apply(Event::Tick { unix_secs: 100 });
        let shelf = win(1, windows::SHELF_APP_ID, &Instance::Shelf.name(), true);
        s.apply(Event::Windows(vec![shelf.clone()]));
        let fx = s.launch("desktop:firefox".into());
        let firefox = fx.iter().find_map(|e| match e {
            Effect::Launch { instance, .. } => Some(instance.clone()),
            _ => None,
        });
        let firefox = firefox.unwrap();
        s.apply(GameEvent::Started("desktop:firefox".into()).into());
        // Its launch handed the app over (to the Waydroid session) and ended.
        s.apply(GameEvent::Played { game: "desktop:firefox".into(), start: 100, duration: 1 }.into());
        assert!(s.session().running("desktop:firefox").is_some(), "waits for its window");
        let app = win(6, "firefox", &Instance::Shelf.name(), false);
        let (_, fx) = s.apply(Event::Windows(vec![shelf.clone(), app.clone()]));
        assert!(fx.contains(&Effect::Window(WindowCommand::Move { windows: vec![6], to: firefox.workspace() })), "{fx:?}");
        s.apply(Event::Windows(vec![shelf.clone(), Window { workspace: firefox.workspace(), ..app }]));
        s.apply(Event::Tick { unix_secs: 101 + HANDOFF });
        assert!(s.session().running("desktop:firefox").is_some(), "runs while its window is open");
        s.apply(Event::Windows(vec![shelf]));
        assert!(s.session().running("desktop:firefox").is_none(), "its window closed: ended");
        // Another runner's: ended with its launch.
        let mut s = state();
        s.launch("desktop:firefox".into());
        s.apply(GameEvent::Started("desktop:firefox".into()).into());
        s.apply(GameEvent::Played { game: "desktop:firefox".into(), start: 0, duration: 1 }.into());
        assert!(s.session().running("desktop:firefox").is_none());
    }

    #[test]
    fn entries_started_before_clamshell_are_found_by_their_scopes() {
        let mut s = state();
        let foot = Instance::Run { key: key("desktop:foot"), secs: 1, n: 0 };
        s.apply(Event::Units(vec![foot.clone()]));
        assert!(s.session().running("desktop:foot").is_some(), "adopted");
        s.apply(Event::Windows(vec![win(4, "foot", &foot.workspace(), false)]));
        assert_eq!(tap(&mut s, Id::Dock(DockId::Entry(0))), vec![Effect::Window(WindowCommand::Focus(foot.workspace()))], "on the dock, shown");
        s.apply(Event::Units(vec![]));
        assert!(s.session().running("desktop:foot").is_none(), "its scopes gone: ended");
    }

    #[test]
    fn the_settings_follow_the_front() {
        let mut s = state();
        let zelda = launch(&mut s, "zelda-lttp");
        let in_front = |s: &mut State, w: Window| s.apply(Event::Windows(vec![w]));
        tap(&mut s, game("ff6"));
        let id = |s: &State| s.context().map(|g| g.id.to_string());
        in_front(&mut s, Window { unit: zelda.unit(), ..win(2, "zelda3", &zelda.workspace(), true) });
        assert_eq!(id(&s).as_deref(), Some("zelda-lttp"), "the entry in front");
        in_front(&mut s, win(1, windows::SHELF_APP_ID, &Instance::Shelf.name(), true));
        assert_eq!(id(&s).as_deref(), Some("ff6"), "shelf in front: its selection");
        in_front(&mut s, win(3, "foot", "1", true));
        assert_eq!(s.context(), None, "a window of none of ours: the defaults");
    }
}
