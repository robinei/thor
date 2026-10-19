//! Running entries: what was launched from shelf or the dock, a game or an app alike. Each launch is a
//! scope of its own in the entry's slice, its windows on the entry's workspace (see [`Instance`]); an
//! entry may run more than once (another launch is another scope, another tab).
//!
//! Some entries run in another program's processes, not a scope of ours, and are known by their windows'
//! app id ([`app_id_of`]): Steam's games (`steam_app_<id>`, in Steam's scope) and the entries of a runner
//! that hands its apps over (Waydroid's, in the Waydroid session; their desktop file's name). Their
//! windows land on a workspace of their own (sway's rules, [`WindowCommand::LandRules`]) and are moved to
//! their entry's; one opening while the entry doesn't run (the game started in Steam, the app by another
//! app) starts it. Such an entry runs once (another launch shows it), and runs on by its windows after its
//! launch ends, until the last closes.
//!
//! A file opened from the Files tab (a browsed entry) whose launch hands it to a program not ours and ends
//! (`xdg-open` to an app's own instance, D-Bus activation) runs on by its windows too: a new window of no
//! launch of ours appearing within [`HANDOFF`] is moved to its workspace, and it has ended when that
//! window has closed (or when none came).
//!
//! In shelf, activating a running entry shows it, a stopped one starts. Quit asks an entry's windows to
//! close and, once none is left and it still runs, ends it: its slice stopped, or (Steam's) the processes
//! under Steam's launcher for it, and its runner's `stop` command run; Force kill ends it at once. What is in front (shelf, a running entry,
//! another window) decides where the pad goes, what Home does and what the settings page edits.

use std::sync::Arc;

use clamshell_library::Launch;

use crate::effect::{Effect, Save, WindowCommand};
use crate::instance::{Instance, STEAM_APP_PREFIX};
use crate::{Game, State};

/// What is in front on the main screen.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Front {
    Shelf,
    /// A window of a running entry.
    Program,
    /// Another window (none of ours).
    Other,
}

/// How a running entry runs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Origin {
    /// By its launches: the launcher thread says when each has ended.
    Ours,
    /// Found by its tray icon (a program not started by ClamShell, see [`crate::tray`]): it runs while the
    /// icon is there, or a window of it is open.
    Tray,
    /// By its windows, since `since` (epoch seconds): started by another program, or its launches ended
    /// while it ran on in a program not ours. It ends once it has had windows (`seen`) and has none, or
    /// none came by `until`; its play from `since` recorded then. Its windows: those with its app id, else
    /// (a browsed file's) a new window of no launch of ours, taken until `until`.
    Windows { since: u64, until: u64, seen: bool },
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
    /// The app id its windows have, if it runs in another program's processes (see [`app_id_of`]).
    pub app_id: Option<Arc<str>>,
    /// Found by a tray icon: the icon's process (windows of it, or of a process it started, are the
    /// entry's) and the icon's key.
    pub pid: Option<u32>,
    pub tray: Option<Arc<str>>,
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
    pub fn steam_appid(&self) -> Option<u32> {
        match self.entry.as_deref().map(|g| &g.launch) {
            Some(Launch::Steam { appid }) => Some(*appid),
            _ => None,
        }
    }

    /// Whether `w` is one of its windows by its app id.
    pub fn owns(&self, w: &crate::windows::Window) -> bool {
        self.app_id.as_deref() == Some(w.app_id.as_str())
    }

    /// Whether `w` is one of its windows by its process: the tray icon's, or one it started.
    pub fn fathered(&self, w: &crate::windows::Window) -> bool {
        self.pid.is_some_and(|pid| w.pids.contains(&pid))
    }

    /// Whether its windows are found otherwise than by its scope (by app id, or by process), and so are
    /// moved to its workspace wherever they open.
    pub fn outside(&self) -> bool {
        self.app_id.is_some() || self.pid.is_some()
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
    /// The app id its windows have, if it runs in another program's processes.
    pub app_id: Option<Arc<str>>,
}

/// The app id an entry's windows have when it runs in another program's processes, by which they are
/// its wherever they open: Steam's games' `steam_app_<id>`, a desktop file's name for an entry whose
/// runner hands its apps over (Waydroid's). None for the rest, which run in scopes of ours.
pub fn app_id_of(g: &Game) -> Option<Arc<str>> {
    match &g.launch {
        Launch::Steam { appid } => Some(format!("{STEAM_APP_PREFIX}{appid}").into()),
        _ => g.id.strip_prefix("desktop:").filter(|_| g.runner.as_ref().is_some_and(|r| r.handover)).map(Into::into),
    }
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
        // (No window, but a tray icon: the program asked to show itself, Steam started silently.)
        if let Some(show) = self.activate_tray(id) {
            return vec![show];
        }
        self.notify(crate::hardware::NoticeKind::NoWindow(title));
        vec![]
    }

    /// Launches `game` (again, if it runs: another tab), unless a launch of it is starting; one that runs
    /// in another program's processes is shown instead (Steam doesn't start a game twice, Android an app).
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
        if session.running(&game).is_some_and(|r| r.app_id.is_some()) {
            return self.show_running(&game);
        }
        let Some(g) = self.shelf.game(&game) else {
            return vec![];
        };
        let watch = g.mpv_file().map(std::path::Path::to_path_buf).filter(|_| command.is_none());
        let app_id = app_id_of(g).filter(|_| command.is_none());
        let (launch, key) = (command.unwrap_or_else(|| g.launch.clone()), g.key.clone());
        let (secs, n) = self.session.new_name(self.unix_secs);
        let instance = Instance::Run { key, secs, n };
        let settings = self.settings_of(&game);
        let env = self.env_of(&game);
        self.session.launching.push(Launching { game: game.clone(), instance: instance.clone(), app_id });
        // Shown once its first window appears.
        self.session.pending_show = Some(instance.clone());
        vec![Effect::Save(Save::Resume(self.resume())), Effect::Launch { game, instance, settings, env, launch, watch }]
    }

    /// A launch has started: the entry runs (one launch more), and the second screen lights up again for
    /// it to darken.
    pub(crate) fn started(&mut self, game: Arc<str>) {
        let session = &mut self.session;
        let app_id = match session.launching.iter().position(|l| l.game == game) {
            Some(i) => session.launching.remove(i).app_id,
            None => None,
        };
        match session.running.iter_mut().find(|r| r.id == game) {
            Some(r) => {
                r.launches += 1;
                r.origin = Origin::Ours;
            }
            None => {
                let key = self.shelf.game(&game).map_or_else(|| clamshell_library::key_of(&game), |g| g.key.clone());
                let entry = self.shelf.game(&game).cloned().map(Box::new);
                let start = self.unix_secs;
                session.running.push(Running { id: game, key, origin: Origin::Ours, app_id, pid: None, tray: None, start, launches: 1, adopted: false, entry });
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

    /// A launch of `game` ended (or couldn't start): it no longer runs once none does, unless it runs on
    /// by its windows (see [`Origin::Windows`]): one known by its app id whose windows are open or may
    /// still come, a browsed file's soon after it started.
    pub(crate) fn ended(&mut self, game: &str) {
        self.session.launching.retain(|l| &*l.game != game || l.app_id.is_some());
        let browsed = self.shelf.game(game).is_some_and(|g| g.browsed.is_some());
        let now = self.unix_secs;
        let session = &mut self.session;
        let Some(i) = session.running.iter().position(|r| &*r.id == game) else {
            return;
        };
        let has = session.windows.iter().any(|w| session.running_of(w).is_some_and(|o| &*o.id == game));
        let Some(r) = session.running.get_mut(i) else {
            return;
        };
        r.launches = r.launches.saturating_sub(1);
        if r.launches == 0 && r.origin == Origin::Ours {
            let soon = now.saturating_sub(r.start) <= HANDOFF;
            if r.app_id.is_some() && (has || soon) {
                r.origin = Origin::Windows { since: now, until: now + HANDOFF, seen: has };
            } else if browsed && soon {
                r.origin = Origin::Windows { since: now, until: r.start + HANDOFF, seen: has };
            }
        }
        session.running.retain(|r| r.launches > 0 || matches!(r.origin, Origin::Windows { .. }));
    }

    /// A launch that never started (its scope was gone before it was seen): a browsed file's, or one known
    /// by its app id, waits for its window (see [`Origin::Windows`]).
    pub(crate) fn launch_failed(&mut self, game: Arc<str>) {
        let app_id = match self.session.launching.iter().position(|l| l.game == game) {
            Some(i) => self.session.launching.remove(i).app_id,
            None => None,
        };
        let Some(entry) = self.shelf.game(&game).filter(|g| g.browsed.is_some() || app_id.is_some()).cloned() else {
            return;
        };
        let key = entry.key.clone();
        if self.session.running(&game).is_none() {
            let now = self.unix_secs;
            let origin = Origin::Windows { since: now, until: now + HANDOFF, seen: false };
            let entry = Some(Box::new(entry));
            self.session.running.push(Running { id: game, key, origin, app_id, pid: None, tray: None, start: now, launches: 0, adopted: false, entry });
        }
    }

    /// After the windows changed, and every second (`known`: the windows there before):
    /// - a window with the app id of a library entry that doesn't run starts it, shown (see
    ///   [`app_id_of`]: a game started in Steam, an Android app opened by another);
    /// - a running entry's windows found by app id go to its workspace;
    /// - a new window of no launch of ours goes to the latest browsed file waiting for one, shown;
    /// - one left on the landing workspace that is no entry's goes to the main screen;
    /// - an entry running by its windows that had windows and has none, or waited in vain, has ended.
    pub(crate) fn follow_windows(&mut self, known: &[i64]) -> Vec<Effect> {
        let now = self.unix_secs;
        let mut effects = Vec::new();
        // Started elsewhere.
        let session = &self.session;
        let mut new: Vec<&Game> = Vec::new();
        for w in session.windows.iter().filter(|w| session.running_of(w).is_none()) {
            let games = &self.shelf.library().games;
            let Some(g) = games.iter().find(|g| app_id_of(g).as_deref() == Some(w.app_id.as_str())) else {
                continue;
            };
            if !session.launching.iter().any(|l| l.game == g.id) && !new.iter().any(|n| n.id == g.id) {
                new.push(g);
            }
        }
        let new: Vec<Running> = new
            .into_iter()
            .map(|g| Running {
                id: g.id.clone(),
                key: g.key.clone(),
                origin: Origin::Windows { since: now, until: now, seen: true },
                app_id: app_id_of(g),
                pid: None,
                tray: None,
                start: now,
                launches: 0,
                adopted: false,
                entry: Some(Box::new(g.clone())),
            })
            .collect();
        if let Some(r) = new.last() {
            self.session.pending_show = Some(Instance::Run { key: r.key.clone(), secs: now, n: 0 });
            self.bottom.woken = false;
        }
        self.session.running.extend(new);
        // Windows found by app id or by process to their entry's workspace.
        let session = &self.session;
        for r in session.running.iter().filter(|r| r.outside()) {
            let mine = |w: &&crate::windows::Window| session.running_of(w).is_some_and(|o| o.id == r.id);
            let away: Vec<i64> =
                session.windows.iter().filter(mine).filter(|w| !w.down && Instance::key_of(&w.workspace) != Some(&r.key)).map(|w| w.id).collect();
            if !away.is_empty() {
                effects.push(Effect::Window(WindowCommand::Move { windows: away, to: r.workspace() }));
            }
        }
        // New windows of no launch of ours to the latest browsed file waiting for one.
        let landing = Instance::Landing.name();
        let stray: Vec<i64> = session
            .windows
            .iter()
            .filter(|w| !known.contains(&w.id) && w.home().is_none() && !w.down && session.running_of(w).is_none())
            .map(|w| w.id)
            .collect();
        let waiting = session
            .running
            .iter()
            .filter(|r| r.app_id.is_none() && matches!(r.origin, Origin::Windows { until, seen: false, .. } if now <= until))
            .max_by_key(|r| r.start);
        let mut taken = None;
        if let Some(r) = waiting.filter(|_| !stray.is_empty()) {
            effects.push(Effect::Window(WindowCommand::Move { windows: stray.clone(), to: r.workspace() }));
            effects.push(Effect::Window(WindowCommand::Focus(r.workspace())));
            taken = Some(r.id.clone());
        }
        // Left on the landing workspace (its entry gone from the library since its rule was given).
        if taken.is_none() {
            for w in session.windows.iter().filter(|w| w.workspace == landing && session.running_of(w).is_none()) {
                effects.push(Effect::Window(WindowCommand::Up { window: w.id, to: None }));
            }
        }
        // Those with windows (a window just taken is still where it came up).
        let with: Vec<Arc<str>> = session
            .running
            .iter()
            .filter(|r| taken.as_ref() == Some(&r.id) || session.windows.iter().any(|w| session.running_of(w).is_some_and(|o| o.id == r.id)))
            .map(|r| r.id.clone())
            .collect();
        let mut ended = Vec::new();
        let icons: Vec<Arc<str>> = self.tray.iter().map(|t| t.key.clone()).collect();
        self.session.running.retain_mut(|r| match &mut r.origin {
            Origin::Tray => r.tray.as_ref().is_some_and(|k| icons.contains(k)) || with.contains(&r.id),
            Origin::Windows { since, until, seen } => {
                let has = with.contains(&r.id);
                *seen |= has;
                let runs = r.launches > 0 || if *seen { has } else { now <= *until };
                if !runs && *seen {
                    ended.push((r.id.clone(), *since));
                }
                runs
            }
            Origin::Ours => true,
        });
        for (game, start) in ended {
            let duration = now.saturating_sub(start);
            effects.push(Effect::RecordPlay { game: game.clone(), start, duration });
            if self.shelf.add_play(&game, start, duration) {
                self.saves.resume.touch(now);
            }
        }
        effects
    }

    /// The app ids of the library's entries known by them, for sway's rules: their windows land on a
    /// workspace of their own, to be moved to their entry's (see [`State::follow_windows`]).
    pub(crate) fn land_rules(&self) -> Effect {
        let ids = self.shelf.library().games.iter().filter_map(app_id_of).collect();
        Effect::Window(WindowCommand::LandRules(ids))
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
                origin: Origin::Ours,
                app_id: app_id_of(g),
                pid: None,
                tray: None,
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
        self.end(game, true)
    }

    /// Ending a running entry: its slice stopped (`force`: killed), or (Steam's) the processes under
    /// Steam's launcher for it killed; and its runner's `stop` command run (`{id}` its app id).
    fn end(&self, game: &str, force: bool) -> Vec<Effect> {
        let Some(r) = self.session.running(game) else {
            return vec![];
        };
        // (Found by its tray icon: its process.)
        if let (Origin::Tray, Some(pid)) = (r.origin, r.pid) {
            return vec![Effect::Tray(crate::tray::TrayCommand::End { pid, force })];
        }
        let mut effects = vec![match r.steam_appid() {
            Some(appid) => Effect::EndSteam { appid },
            None => Effect::EndRun { key: r.key.clone(), force },
        }];
        let stop = self.game(game).and_then(|g| g.runner.as_ref()).map(|o| &o.stop).filter(|s| !s.is_empty());
        if let (Some(stop), Some(app_id)) = (stop, &r.app_id) {
            effects.push(Effect::Stop(stop.iter().map(|a| a.replace("{id}", app_id)).collect()));
        }
        effects
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
        let ff6 = s.session().running("ff6").unwrap();
        assert_eq!((ff6.origin, ff6.app_id.as_deref()), (Origin::Ours, Some("steam_app_620")));
        let landing = Instance::Landing.name();
        let fx = s.apply(Event::Windows(vec![win(7, "steam_app_620", &landing, true)])).1;
        let ff6 = Instance::workspace_of(&key("ff6"));
        assert!(fx.contains(&Effect::Window(WindowCommand::Move { windows: vec![7], to: ff6 })), "its windows go to its workspace: {fx:?}");
        let fx = s.apply(Event::Windows(vec![])).1;
        assert!(!fx.iter().any(|e| matches!(e, Effect::RecordPlay { .. })), "{fx:?}");
        assert!(s.session().running("ff6").is_some());
    }

    #[test]
    fn a_steam_games_window_is_the_games_though_it_runs_in_steams_scope() {
        let mut s = steam_ff6();
        let steam = Instance::Run { key: key("desktop:steam"), secs: 1, n: 0 };
        s.apply(Event::Units(vec![steam.clone()]));
        launch(&mut s, "ff6");
        let landing = Instance::Landing.name();
        let window = Window { unit: steam.unit(), ..win(7, "steam_app_620", &landing, true) };
        let fx = s.apply(Event::Windows(vec![window.clone()])).1;
        let ff6 = Instance::workspace_of(&key("ff6"));
        assert!(fx.contains(&Effect::Window(WindowCommand::Move { windows: vec![7], to: ff6 })), "the game's, not Steam's: {fx:?}");
        assert_eq!(s.session().running_of(&window).map(|r| &*r.id), Some("ff6"));
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
        let stop = vec!["stop".to_owned(), "{id}".to_owned()].into();
        firefox.runner = Some(clamshell_library::RunnerOf { handover: true, stop, ..clamshell_library::RunnerOf::new("android", "Android", true) });
        let mut s = State::new(Init { library, ..init() });
        s.apply(Event::Tick { unix_secs: 100 });
        let shelf = win(1, windows::SHELF_APP_ID, &Instance::Shelf.name(), true);
        let landing = Instance::Landing.name();
        s.apply(Event::Windows(vec![shelf.clone()]));
        let ws = Instance::workspace_of(&key("desktop:firefox"));
        assert!(matches!(&s.land_rules(), Effect::Window(WindowCommand::LandRules(ids)) if ids.iter().any(|i| &**i == "firefox")), "its windows land");
        assert!(launches(&s.launch("desktop:firefox".into()), "desktop:firefox"));
        s.apply(GameEvent::Started("desktop:firefox".into()).into());
        // Its launch handed the app over (to the Waydroid session) and ended.
        s.apply(GameEvent::Played { game: "desktop:firefox".into(), start: 100, duration: 1 }.into());
        assert!(s.session().running("desktop:firefox").is_some(), "waits for its window");
        let app = win(6, "firefox", &landing, false);
        let (_, fx) = s.apply(Event::Windows(vec![shelf.clone(), app.clone()]));
        assert!(fx.contains(&Effect::Window(WindowCommand::Move { windows: vec![6], to: ws.clone() })), "{fx:?}");
        let moved = Window { workspace: ws.clone(), ..app.clone() };
        s.apply(Event::Windows(vec![shelf.clone(), moved.clone()]));
        assert!(!launches(&s.launch("desktop:firefox".into()), "desktop:firefox"), "it runs once: shown");
        s.apply(Event::Tick { unix_secs: 400 });
        assert!(s.session().running("desktop:firefox").is_some(), "runs while its window is open");
        let fx = s.force_kill("desktop:firefox");
        assert!(fx.contains(&Effect::Stop(vec!["stop".into(), "firefox".into()])), "its runner's stop: {fx:?}");
        let (_, fx) = s.apply(Event::Windows(vec![shelf.clone()]));
        assert!(s.session().running("desktop:firefox").is_none(), "its window closed: ended");
        assert!(fx.contains(&Effect::RecordPlay { game: "desktop:firefox".into(), start: 100, duration: 300 }), "played until then: {fx:?}");
        // Opened by another app: it runs, its window to its workspace, shown.
        let (_, fx) = s.apply(Event::Windows(vec![shelf.clone(), app]));
        assert!(s.session().running("desktop:firefox").is_some());
        assert!(fx.contains(&Effect::Window(WindowCommand::Move { windows: vec![6], to: ws.clone() })), "{fx:?}");
        let (_, fx) = s.apply(Event::Windows(vec![shelf.clone(), Window { focused: true, ..moved }]));
        assert!(fx.contains(&Effect::Window(WindowCommand::Focus(ws))), "{fx:?}");
        // A landed window that is no entry's (its rule outlived it): to the main screen.
        let (_, fx) = s.apply(Event::Windows(vec![shelf.clone(), win(9, "gone", &landing, false)]));
        assert!(fx.contains(&Effect::Window(WindowCommand::Up { window: 9, to: None })), "{fx:?}");
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
