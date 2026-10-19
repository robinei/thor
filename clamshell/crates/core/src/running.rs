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
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Origin {
    /// By its launches: the launcher thread says when each has ended.
    Ours,
    /// A program not started by ClamShell, found by its tray icon (see [`crate::tray`]) or a window of it
    /// (see [`crate::found`]): it runs while the icon is there, or a window of it is open; until it is
    /// moved into a scope of ours (then it runs as [`Origin::Ours`]). `pid`: its own process (the tray
    /// icon's, or its window's: windows of it, or of a process it started, are the entry's), none if
    /// it runs in another program's; `tray`: the icon's key, if it has one.
    Found { pid: Option<u32>, tray: Option<Arc<str>> },
    /// By its windows, since `since` (epoch seconds): started by another program, or its launches ended
    /// while it ran on in a program not ours. It ends once it has had windows ([`Running::seen`]) and has
    /// none, or none came by `until`; its play from `since` recorded then. Its windows: those with its app
    /// id, else (a browsed file's) a new window of no launch of ours, taken until `until`.
    Windows { since: u64, until: u64 },
}

/// How long a launch that handed over waits for its window (seconds).
pub const HANDOFF: u64 = 15;
/// A launch without a window this long after it started says it is starting (seconds).
pub const STARTING_AFTER: u64 = 3;

/// An entry that runs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Running {
    pub id: Arc<str>,
    /// Its key: its workspace's (and its launches' scopes' and slice's) names.
    pub key: String,
    pub origin: Origin,
    /// The app id its windows have, if it runs in another program's processes (see [`app_id_of`]).
    pub app_id: Option<Arc<str>>,
    /// When it first started (epoch seconds).
    pub start: u64,
    /// Its launches the launcher still follows (ours: see [`State::follow_ours`]).
    pub(crate) launches: u32,
    /// A window of it has been there (since it started, or since it went on by its windows): before, it
    /// is starting.
    pub(crate) seen: bool,
    /// Its slow start said (see [`STARTING_AFTER`]).
    pub(crate) starting_said: bool,
    /// Its entry as it was when it started: the dock shows it by this if it leaves the library while it
    /// runs (its file renamed or deleted).
    pub entry: Option<Box<crate::Game>>,
}

impl Running {
    /// Entry `game` running as `origin` since `start` (no launch followed, no window seen yet).
    pub(crate) fn of(game: &Game, origin: Origin, start: u64) -> Running {
        let entry = Some(Box::new(game.clone()));
        Running { id: game.id.clone(), key: game.key.clone(), origin, app_id: None, start, launches: 0, seen: false, starting_said: false, entry }
    }

    /// Found: its own process, if it has one (see [`Origin::Found`]).
    pub fn pid(&self) -> Option<u32> {
        match self.origin {
            Origin::Found { pid, .. } => pid,
            _ => None,
        }
    }

    /// Found by a tray icon: the icon's key.
    pub fn tray(&self) -> Option<&Arc<str>> {
        match &self.origin {
            Origin::Found { tray, .. } => tray.as_ref(),
            _ => None,
        }
    }

    pub fn found(&self) -> bool {
        matches!(self.origin, Origin::Found { .. })
    }

    /// Whether `w` is one of its windows by its app id.
    pub fn owns(&self, w: &crate::windows::Window) -> bool {
        self.app_id.as_deref() == Some(w.app_id.as_str())
    }

    /// Whether `w` is one of its windows by its process: the tray icon's, or one it started.
    pub fn fathered(&self, w: &crate::windows::Window) -> bool {
        self.pid().is_some_and(|pid| w.pids.contains(&pid))
    }

    /// Whether its windows are found otherwise than by its scope (by app id, or by process), and so are
    /// moved to its workspace wherever they open.
    pub fn outside(&self) -> bool {
        self.app_id.is_some() || self.pid().is_some()
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
        if session.windows_of(&r.id).next().is_some() {
            let focus = Effect::Window(WindowCommand::Focus(r.workspace()));
            // (Paused, or its runner: thawed first, so it draws as it is shown.)
            let resumed = self.resume_runner_of(id);
            return self.thaw(id).into_iter().chain(resumed).chain([focus]).collect();
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
            return vec![Effect::Tool(*tool)];
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
        // (Its runner paused, its container with it: resumed first.)
        let resumed = self.resume_runner_of(&game);
        self.session.launching.push(Launching { game: game.clone(), instance: instance.clone(), app_id });
        // Shown once its first window appears.
        self.session.pending_show = Some(instance.clone());
        let mut effects: Vec<Effect> = resumed.into_iter().collect();
        effects.extend([Effect::Save(Save::Resume(self.resume())), Effect::Launch { game, instance, settings, env, launch, watch }]);
        effects
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
                let start = self.unix_secs;
                // (An entry gone from the library meanwhile: by its id alone.)
                let entry = self.shelf.game(&game).cloned().unwrap_or_else(|| Game::new(&game, &game));
                let entry = Running { app_id, launches: 1, ..Running::of(&entry, Origin::Ours, start) };
                session.running.push(Running { entry: entry.entry.filter(|_| self.shelf.game(&game).is_some()), ..entry });
            }
        }
        self.bottom.woken = false;
    }

    /// The launcher is done with a launch of `game`: one launch less (see [`State::follow_ours`]).
    pub(crate) fn ended(&mut self, game: &str) -> Vec<Effect> {
        self.session.launching.retain(|l| &*l.game != game || l.app_id.is_some());
        if let Some(r) = self.session.running.iter_mut().find(|r| &*r.id == game) {
            r.launches = r.launches.saturating_sub(1);
        }
        self.follow_ours()
    }

    /// Our launches' entries run while the launcher follows a launch of theirs or systemd lists a scope
    /// of theirs (a Steam game's between Steam's setup and the game: the launcher's; one started by a
    /// ClamShell before this one: its scopes). Once neither, one known by its app id whose windows are
    /// open or may still come, or a browsed file's soon after it started, runs on by its windows (see
    /// [`Origin::Windows`]); the rest have ended, their play recorded.
    pub(crate) fn follow_ours(&mut self) -> Vec<Effect> {
        let now = self.unix_secs;
        let session = &self.session;
        let mut ended = Vec::new();
        let mut handed = Vec::new();
        let mut unstarted = Vec::new();
        for r in session.running.iter().filter(|r| r.origin == Origin::Ours && r.launches == 0) {
            if session.units.iter().any(|u| u.key() == Some(r.key.as_str())) {
                continue;
            }
            let has = session.windows_of(&r.id).next().is_some();
            let soon = now.saturating_sub(r.start) <= HANDOFF;
            let browsed = self.shelf.game(&r.id).is_some_and(|g| g.browsed.is_some());
            if r.app_id.is_some() && (has || soon) {
                handed.push((r.id.clone(), Origin::Windows { since: now, until: now + HANDOFF }, has));
            } else if browsed && soon {
                handed.push((r.id.clone(), Origin::Windows { since: now, until: r.start + HANDOFF }, has));
            } else {
                ended.push((r.id.clone(), r.start));
                // (Ended without ever a window, not quit: it didn't start, a Steam game that never came.)
                if !r.seen && !session.is_quitting(&r.id) {
                    unstarted.push(r.entry.as_ref().map_or_else(|| r.id.to_string(), |g| g.title.clone()));
                }
            }
        }
        for title in unstarted {
            self.notify(crate::hardware::NoticeKind::NotStarted(title));
        }
        for (id, origin, has) in handed {
            if let Some(r) = self.session.running.iter_mut().find(|r| r.id == id) {
                r.origin = origin;
                r.seen = has;
            }
        }
        self.session.running.retain(|r| !ended.iter().any(|(id, _)| *id == r.id));
        ended.into_iter().map(|(game, start)| self.record_play(game, start)).collect()
    }

    /// A play of `game` from `start` until now: counted, and recorded.
    fn record_play(&mut self, game: Arc<str>, start: u64) -> Effect {
        let now = self.unix_secs;
        let duration = now.saturating_sub(start);
        if self.shelf.add_play(&game, start, duration) {
            self.saves.resume.touch(now);
        }
        Effect::RecordPlay { game, start, duration }
    }

    /// A launch that never started (its scope was gone before it was seen): a browsed file's, or one known
    /// by its app id, waits for its window (see [`Origin::Windows`]).
    pub(crate) fn launch_failed(&mut self, game: Arc<str>) {
        let app_id = match self.session.launching.iter().position(|l| l.game == game) {
            Some(i) => self.session.launching.remove(i).app_id,
            None => None,
        };
        let Some(entry) = self.shelf.game(&game).filter(|g| g.browsed.is_some() || app_id.is_some()).cloned() else {
            // (Nothing to wait for: it didn't start, and says so.)
            let title = self.shelf.game(&game).map_or_else(|| game.to_string(), |g| g.title.clone());
            if self.session.running(&game).is_none() {
                self.notify(crate::hardware::NoticeKind::NotStarted(title));
            }
            return;
        };
        if self.session.running(&game).is_none() {
            let now = self.unix_secs;
            let origin = Origin::Windows { since: now, until: now + HANDOFF };
            self.session.running.push(Running { app_id, ..Running::of(&entry, origin, now) });
        }
    }

    /// After the windows changed, and every second (`known`: the windows there before):
    /// - a window with the app id of a library entry that doesn't run starts it, shown (see
    ///   [`app_id_of`]: an Android app opened by another, running by its windows; a game started in
    ///   Steam, as a launch of it: the launcher finds it running and moves it into the launch's scope, so
    ///   it is paused and ended as any launch);
    /// - a running entry's windows found by app id go to its workspace;
    /// - a new window of no launch of ours goes to the latest browsed file waiting for one, shown;
    /// - one still no entry's makes its program an entry found by it (see [`crate::found`]);
    /// - one left on the landing workspace that is no entry's goes to the main screen;
    /// - an entry running by its windows that had windows and has none, or waited in vain, has ended.
    pub(crate) fn follow_windows(&mut self, known: &[i64]) -> Vec<Effect> {
        let now = self.unix_secs;
        let mut effects = Vec::new();
        // Started elsewhere. (A window's app id says whose it is before its scope does: a game Steam
        // started is in Steam's scope.)
        let session = &self.session;
        let mut new: Vec<&Game> = Vec::new();
        for w in session.windows.iter().filter(|w| !session.running.iter().any(|r| r.owns(w))) {
            let games = &self.shelf.library().games;
            let Some(g) = games.iter().find(|g| app_id_of(g).as_deref() == Some(w.app_id.as_str())) else {
                continue;
            };
            if !session.launching.iter().any(|l| l.game == g.id) && !new.iter().any(|n| n.id == g.id) {
                new.push(g);
            }
        }
        let (steam, new): (Vec<&Game>, Vec<&Game>) = new.into_iter().partition(|g| matches!(g.launch, Launch::Steam { .. }));
        let steam: Vec<Arc<str>> = steam.into_iter().map(|g| g.id.clone()).collect();
        let new: Vec<Running> =
            new.into_iter().map(|g| Running { app_id: app_id_of(g), seen: true, ..Running::of(g, Origin::Windows { since: now, until: now }, now) }).collect();
        if let Some(r) = new.last() {
            self.session.pending_show = Some(Instance::Run { key: r.key.clone(), secs: now, n: 0 });
            self.bottom.woken = false;
        }
        self.session.running.extend(new);
        for game in steam {
            effects.extend(self.launch(game));
        }
        // Windows found by app id or by process to their entry's workspace.
        let session = &self.session;
        for r in session.running.iter().filter(|r| r.outside()) {
            let away: Vec<i64> = session.windows_of(&r.id).filter(|w| !w.down && Instance::key_of(&w.workspace) != Some(&r.key)).map(|w| w.id).collect();
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
            .filter(|r| r.app_id.is_none() && !r.seen && matches!(r.origin, Origin::Windows { until, .. } if now <= until))
            .max_by_key(|r| r.start);
        let mut taken = None;
        if let Some(r) = waiting.filter(|_| !stray.is_empty()) {
            effects.push(Effect::Window(WindowCommand::Move { windows: stray.clone(), to: r.workspace() }));
            effects.push(Effect::Window(WindowCommand::Focus(r.workspace())));
            taken = Some(r.id.clone());
        }
        // Still no entry's: their programs' (see [`crate::found`]).
        let given: Vec<i64> = if taken.is_some() { stray } else { Vec::new() };
        effects.extend(self.follow_found_windows(&given));
        let session = &self.session;
        // Left on the landing workspace (its entry gone from the library since its rule was given).
        // (Not one whose entry's launch is starting: a Steam game found running, see above.)
        let starting = |w: &crate::windows::Window| session.launching.iter().any(|l| l.app_id.as_deref() == Some(w.app_id.as_str()));
        if taken.is_none() {
            for w in session.windows.iter().filter(|w| w.workspace == landing && session.running_of(w).is_none() && !starting(w)) {
                effects.push(Effect::Window(WindowCommand::Up { window: w.id, to: None }));
            }
        }
        // Those with windows (a window just taken is still where it came up).
        let with: Vec<Arc<str>> =
            session.running.iter().filter(|r| taken.as_ref() == Some(&r.id) || session.windows_of(&r.id).next().is_some()).map(|r| r.id.clone()).collect();
        let mut ended = Vec::new();
        let icons: Vec<Arc<str>> = self.tray.iter().map(|t| t.key.clone()).collect();
        self.session.running.retain_mut(|r| {
            let has = with.contains(&r.id);
            r.seen |= has;
            match r.origin {
                Origin::Found { ref tray, .. } => tray.as_ref().is_some_and(|k| icons.contains(k)) || has,
                Origin::Windows { since, until } => {
                    let runs = r.launches > 0 || if r.seen { has } else { now <= until };
                    if !runs && r.seen {
                        ended.push((r.id.clone(), since));
                    }
                    runs
                }
                Origin::Ours => true,
            }
        });
        for (game, start) in ended {
            effects.push(self.record_play(game, start));
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
    /// (started by a ClamShell before this one) runs, since its first scope started (the time in its
    /// name); one found by its tray icon whose process is in a scope of ours now runs as a launch; then
    /// those whose launches have ended (see [`State::follow_ours`]).
    pub(crate) fn follow_units(&mut self) -> Vec<Effect> {
        let now = self.unix_secs;
        let session = &mut self.session;
        let units = &session.units;
        for r in session.running.iter_mut().filter(|r| r.found()) {
            if units.iter().any(|u| u.key() == Some(r.key.as_str())) {
                // (Its windows are its scope's now: not every window of that app id or process, which
                // would keep another one of the program, started later, to it.)
                r.origin = Origin::Ours;
                r.app_id = None;
            }
        }
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
            let first = units.iter().filter_map(|u| match u {
                Instance::Run { key: k, secs, .. } if k == key => Some(*secs),
                _ => None,
            });
            let start = first.min().unwrap_or(now);
            session.running.push(Running { app_id: app_id_of(g), ..Running::of(g, Origin::Ours, start) });
        }
        self.follow_ours()
    }

    /// The app whose window shows browsed file `id` (running): the desktop file named as the window's
    /// app id says (`org.kde.kate`: `desktop:org.kde.kate`; any case), else the one whose program it is.
    /// For its picture only: the file runs as itself.
    pub fn app_showing(&self, id: &str) -> Option<&crate::Game> {
        self.shelf.game(id).filter(|g| g.browsed.is_some())?;
        let session = &self.session;
        let window = session.windows_of(id).next()?;
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
        session.windows_of(game).map(|w| w.id).collect()
    }

    /// Quit: the entry's windows asked to close; once none is left (a "save changes?" window keeps it
    /// until answered) it is ended if it still runs, or its runner's stop run if it ended with them (see
    /// [`State::follow_quitting`]).
    pub(crate) fn quit(&mut self, game: &Arc<str>) -> Vec<Effect> {
        // (Paused: thawed, to answer its windows' closing.)
        let thawed = self.thaw(game);
        let windows = self.windows_of(game);
        let stop = self.stop_of(game);
        self.session.quitting.retain(|q| q.game != *game);
        self.session.quitting.push(crate::session::Quitting { game: game.clone(), ended: false, stop });
        let mut effects: Vec<Effect> = thawed.into_iter().collect();
        if !windows.is_empty() {
            effects.push(Effect::Window(WindowCommand::Close(windows)));
        }
        effects.extend(self.follow_quitting());
        effects
    }

    /// Force kill: the entry ended at once.
    pub(crate) fn force_kill(&mut self, game: &str) -> Vec<Effect> {
        self.session.quitting.retain(|q| &*q.game != game);
        let thawed = self.thaw(game);
        thawed.into_iter().chain(self.end(game, true)).collect()
    }

    /// Ending a running entry: its slice stopped (`force`: killed; a Steam game's too, moved into its
    /// launch's scope), and its runner's `stop` command run (`{id}` its app id).
    fn end(&self, game: &str, force: bool) -> Vec<Effect> {
        let Some(r) = self.session.running(game) else {
            return vec![];
        };
        // (Found, not moved into a scope of ours: its process, if it is its own; in a desktop's session
        // the desktop's, not ended.)
        if r.found() {
            return match r.pid().filter(|_| self.device_session) {
                Some(pid) => vec![Effect::EndProcesses { pids: vec![pid], force }],
                None => vec![],
            };
        }
        let mut effects = vec![Effect::EndRun { key: r.key.clone(), force }];
        effects.extend(self.stop_of(game));
        effects
    }

    /// The runner's `stop` command for a running entry known by its app id (`{id}` that).
    fn stop_of(&self, game: &str) -> Option<Effect> {
        let app_id = self.session.running(game)?.app_id.as_ref()?;
        let stop = self.game(game)?.runner.as_ref().map(|o| &o.stop).filter(|s| !s.is_empty())?;
        Some(Effect::RunnerCommand(stop.iter().map(|a| a.replace("{id}", app_id)).collect()))
    }

    /// Entries quitting: one whose windows are all gone while it still runs is ended (once); one that no
    /// longer runs is done, its runner's stop run if it ended with its windows before that.
    pub(crate) fn follow_quitting(&mut self) -> Vec<Effect> {
        let mut quitting = std::mem::take(&mut self.session.quitting);
        let mut effects = vec![];
        quitting.retain_mut(|q| {
            let runs = self.session.running(&q.game).is_some();
            if !runs && !q.ended {
                effects.extend(q.stop.take());
            }
            runs
        });
        for q in &mut quitting {
            if !q.ended && self.windows_of(&q.game).is_empty() {
                q.ended = true;
                effects.extend(self.end(&q.game, false));
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
        s.apply(Event::Tick { unix_secs: 1000 });
        s.apply(GameEvent::Started(first.clone()).into());
        let workspace = Instance::workspace_of(&key(&first));
        assert_eq!(tap(&mut s, Id::Shelf(ShelfId::Launch)), vec![], "running, no window yet: nothing to show");
        s.apply(Event::Windows(vec![win(3, "game", &workspace, false)]));
        assert_eq!(tap(&mut s, Id::Shelf(ShelfId::Launch)), vec![Effect::Window(WindowCommand::Focus(workspace))], "running: shown");
        // Another launch (the dock's Open another): it runs twice, until both end.
        assert!(launches(&s.launch(first.clone()), &first));
        s.apply(GameEvent::Started(first.clone()).into());
        s.apply(GameEvent::Ended(first.clone()).into());
        assert!(s.session().running(&first).is_some(), "the other still runs");
        s.apply(Event::Tick { unix_secs: 1660 });
        let (_, fx) = s.apply(GameEvent::Ended(first.clone()).into());
        assert!(s.session().running(&first).is_none());
        assert!(fx.contains(&Effect::RecordPlay { game: first.clone(), start: 1000, duration: 660 }), "{fx:?}");
        let g = s.game(&first).unwrap();
        assert_eq!((g.plays.count, g.plays.seconds), (1, 660), "one play: from its first start to its last end");
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
    fn a_game_steam_started_is_followed_as_a_launch_beside_the_others() {
        let mut s = steam_ff6();
        s.apply(Event::Tick { unix_secs: 1000 });
        let zelda = launch(&mut s, "zelda-lttp").workspace();
        let landing = Instance::Landing.name();
        let windows = vec![win(1, "zelda3", &zelda, false), win(7, "steam_app_620", &landing, true)];
        let (_, fx) = s.apply(Event::Windows(windows.clone()));
        // Found by its window: launched (the launcher finds it running and moves it into the launch's
        // scope), its window left where it is meanwhile.
        assert!(launches(&fx, "ff6"), "{fx:?}");
        assert!(!fx.iter().any(|e| matches!(e, Effect::Window(WindowCommand::Up { window: 7, .. }))), "{fx:?}");
        assert!(!launches(&s.apply(Event::Windows(windows)).1, "ff6"), "once");
        s.apply(GameEvent::Started("ff6".into()).into());
        assert_eq!(s.session().running_entries().len(), 2, "the other keeps running");
        let (_, fx) = s.apply(Event::Windows(vec![win(1, "zelda3", &zelda, false), win(7, "steam_app_620", &landing, true)]));
        let ff6 = Instance::workspace_of(&key("ff6"));
        assert!(fx.contains(&Effect::Window(WindowCommand::Move { windows: vec![7], to: ff6 })), "{fx:?}");
        // Ended as any launch: its slice.
        assert_eq!(s.force_kill("ff6"), vec![Effect::EndRun { key: key("ff6"), force: true }]);
        s.apply(Event::Tick { unix_secs: 1600 });
        s.apply(Event::Windows(vec![win(1, "zelda3", &zelda, true)]));
        let (_, fx) = s.apply(GameEvent::Ended("ff6".into()).into());
        assert!(fx.contains(&Effect::RecordPlay { game: "ff6".into(), start: 1000, duration: 600 }), "{fx:?}");
        assert!(s.session().running("ff6").is_none());
        // A Steam game the library doesn't have isn't one.
        let (_, fx) = s.apply(Event::Windows(vec![win(1, "zelda3", &zelda, false), win(9, "steam_app_1", &landing, true)]));
        assert!(!fx.iter().any(|e| matches!(e, Effect::Launch { .. })), "{fx:?}");
        assert_eq!(s.session().running_entries().len(), 1, "zelda only");
        // Started by a Steam of ours: in Steam's scope, but the app id says whose it is.
        let steam = Instance::Run { key: key("desktop:steam"), secs: 1, n: 0 };
        let in_steam = Window { unit: steam.unit(), ..win(8, "steam_app_620", &steam.workspace(), true) };
        let (_, fx) = s.apply(Event::Windows(vec![win(1, "zelda3", &zelda, false), in_steam]));
        assert!(launches(&fx, "ff6"), "{fx:?}");
    }

    #[test]
    fn a_steam_game_shelf_started_ends_by_the_launcher_not_its_windows() {
        let mut s = steam_ff6();
        launch(&mut s, "ff6");
        let ff6 = s.session().running("ff6").unwrap();
        assert_eq!((ff6.origin.clone(), ff6.app_id.as_deref()), (Origin::Ours, Some("steam_app_620")));
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
        s.apply(GameEvent::Ended("desktop:firefox".into()).into());
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
        assert!(fx.contains(&Effect::RunnerCommand(vec!["stop".into(), "firefox".into()])), "its runner's stop: {fx:?}");
        let (_, fx) = s.apply(Event::Windows(vec![shelf.clone()]));
        assert!(s.session().running("desktop:firefox").is_none(), "its window closed: ended");
        assert!(fx.contains(&Effect::RecordPlay { game: "desktop:firefox".into(), start: 100, duration: 300 }), "played until then: {fx:?}");
        // Opened by another app: it runs, its window to its workspace, shown.
        let (_, fx) = s.apply(Event::Windows(vec![shelf.clone(), app]));
        assert!(s.session().running("desktop:firefox").is_some());
        assert!(fx.contains(&Effect::Window(WindowCommand::Move { windows: vec![6], to: ws.clone() })), "{fx:?}");
        let (_, fx) = s.apply(Event::Windows(vec![shelf.clone(), Window { focused: true, ..moved.clone() }]));
        assert!(fx.contains(&Effect::Window(WindowCommand::Focus(ws))), "{fx:?}");
        // Quit: its window closed; it ends with it, its runner's stop run then (once).
        let stop = Effect::RunnerCommand(vec!["stop".into(), "firefox".into()]);
        let fx = s.quit(&"desktop:firefox".into());
        assert!(fx.contains(&Effect::Window(WindowCommand::Close(vec![6]))) && !fx.contains(&stop), "{fx:?}");
        let (_, fx) = s.apply(Event::Windows(vec![shelf.clone()]));
        assert!(fx.contains(&stop), "{fx:?}");
        assert!(!s.apply(Event::Windows(vec![shelf.clone()])).1.contains(&stop), "once");
        s.apply(Event::Windows(vec![shelf.clone(), moved]));
        // A landed window that is no entry's (its rule outlived it): to the main screen.
        let (_, fx) = s.apply(Event::Windows(vec![shelf.clone(), win(9, "gone", &landing, false)]));
        assert!(fx.contains(&Effect::Window(WindowCommand::Up { window: 9, to: None })), "{fx:?}");
        // Another runner's: ended with its launch.
        let mut s = state();
        s.launch("desktop:firefox".into());
        s.apply(GameEvent::Started("desktop:firefox".into()).into());
        s.apply(GameEvent::Ended("desktop:firefox".into()).into());
        assert!(s.session().running("desktop:firefox").is_none());
    }

    #[test]
    fn entries_started_before_clamshell_are_found_by_their_scopes() {
        let mut s = state();
        s.apply(Event::Tick { unix_secs: 1000 });
        let foot = Instance::Run { key: key("desktop:foot"), secs: 400, n: 0 };
        s.apply(Event::Units(vec![foot.clone()]));
        assert!(s.session().running("desktop:foot").is_some(), "adopted");
        s.apply(Event::Windows(vec![win(4, "foot", &foot.workspace(), false)]));
        assert_eq!(tap(&mut s, Id::Dock(DockId::Entry(0))), vec![Effect::Window(WindowCommand::Focus(foot.workspace()))], "on the dock, shown");
        s.apply(Event::Tick { unix_secs: 1300 });
        let (_, fx) = s.apply(Event::Units(vec![]));
        assert!(s.session().running("desktop:foot").is_none(), "its scopes gone: ended");
        assert!(fx.contains(&Effect::RecordPlay { game: "desktop:foot".into(), start: 400, duration: 900 }), "played since its launch: {fx:?}");
    }

    #[test]
    fn a_launch_runs_while_the_launcher_follows_it_or_a_scope_of_it_is_listed() {
        let mut s = state();
        s.apply(Event::Tick { unix_secs: 100 });
        let zelda = launch(&mut s, "zelda-lttp");
        // Its scope gone while the launcher still follows it (Steam between its setup and the game): runs.
        s.apply(Event::Units(vec![zelda.clone()]));
        s.apply(Event::Units(vec![]));
        assert!(s.session().running("zelda-lttp").is_some(), "the launcher follows it");
        // The launcher done, its scope listed again: runs, until that is gone too.
        s.apply(Event::Units(vec![zelda.clone()]));
        assert!(played(&mut s, "zelda-lttp").is_empty());
        assert!(s.session().running("zelda-lttp").is_some(), "its scope is listed");
        s.apply(Event::Tick { unix_secs: 400 });
        let (_, fx) = s.apply(Event::Units(vec![]));
        assert!(s.session().running("zelda-lttp").is_none());
        assert!(fx.contains(&Effect::RecordPlay { game: "zelda-lttp".into(), start: 100, duration: 300 }), "once: {fx:?}");
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
