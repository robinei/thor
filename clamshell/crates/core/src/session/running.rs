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
//! In shelf, activating a running entry shows it, a stopped one starts. Quit asks an entry's windows to close
//! and, once none is left and it still runs, ends it: its slice stopped, or (Steam's) the processes under
//! Steam's launcher for it, and its runner's `stop` command run; Force kill ends it at once. What is in front
//! (shelf, a running entry, another window) decides where the pad goes, what Home does and what the settings
//! page edits.

use std::sync::Arc;

use clamshell_library::{Launch, ids};

use crate::effect::{Effect, Save, WindowCommand};
use crate::session::instance::{Instance, STEAM_APP_PREFIX};
use crate::{Entry, State};

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
    /// A program not started by ClamShell, found by its tray icon (see [`crate::session::tray`]) or a window of it
    /// (see [`crate::session::found`]): it runs while the icon is there, or a window of it is open; until it is
    /// moved into a scope of ours (then it runs as [`Origin::Ours`]). `pid`: its own process (the tray
    /// icon's, or its window's: windows of it, or of a process it started, are the entry's), none if
    /// it runs in another program's; `tray`: the icon's key, if it has one.
    Found { pid: Option<u32>, tray: Option<Arc<str>> },
    /// By its windows, since `since` (monotonic seconds, as in [`crate::Event::Tick`]): started by another program, or its launches ended
    /// while it ran on in a program not ours. It ends once it has had windows ([`Running::seen`]) and has
    /// none, or none came by `until`; its play from `since` recorded then. Its windows: those with its app
    /// id, else (a browsed file's) a new window of no launch of ours, taken until `until`.
    Windows { since: u64, until: u64 },
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
    /// When it first started (monotonic seconds, as in [`crate::Event::Tick`]).
    pub start: u64,
    /// A window of it has been there (since it started, or since it went on by its windows): before, it
    /// is starting.
    pub(crate) seen: bool,
    /// Its entry as it was when it started: the dock shows it by this if it leaves the library while it
    /// runs (its file renamed or deleted).
    pub entry: Option<Box<crate::Entry>>,
}

impl Running {
    /// Entry `game` running as `origin` since `start` (no launch followed, no window seen yet).
    pub(crate) fn of(game: &Entry, origin: Origin, start: u64) -> Running {
        let entry = Some(Box::new(game.clone()));
        Running { id: game.id.clone(), key: game.key.clone(), origin, app_id: None, start, seen: false, entry }
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
    pub fn owns(&self, w: &crate::session::windows::Window) -> bool {
        self.app_id.as_deref() == Some(w.app_id.as_str())
    }

    /// Whether `w` is one of its windows by its process: the tray icon's, or one it started.
    pub fn fathered(&self, w: &crate::session::windows::Window) -> bool {
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
    pub entry: Arc<str>,
    pub instance: Instance,
    /// The app id its windows have, if it runs in another program's processes.
    pub app_id: Option<Arc<str>>,
}

/// The app id an entry's windows have when it runs in another program's processes, by which they are
/// its wherever they open: Steam's games' `steam_app_<id>`, a desktop file's name for an entry whose
/// runner hands its apps over (Waydroid's). None for the rest, which run in scopes of ours.
pub fn app_id_of(g: &Entry) -> Option<Arc<str>> {
    match &g.launch {
        Launch::Steam { appid } => Some(format!("{STEAM_APP_PREFIX}{appid}").into()),
        _ => handed_over_app(g).map(Into::into),
    }
}

/// Whether `app_id` is [`app_id_of`] `g`, without making it (asked for every window and entry).
pub fn has_app_id(g: &Entry, app_id: &str) -> bool {
    match &g.launch {
        Launch::Steam { appid } => app_id.strip_prefix(STEAM_APP_PREFIX).and_then(|n| n.parse::<u32>().ok()) == Some(*appid),
        _ => handed_over_app(g) == Some(app_id),
    }
}

/// A handed-over entry's app id: its desktop file's name.
fn handed_over_app(g: &Entry) -> Option<&str> {
    ids::desktop_file(&g.id).filter(|_| g.runner.as_ref().is_some_and(|r| r.handover))
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
        let title = self.shelf.entry(&game).map(|g| g.title.clone());
        let effects = self.launch(game);
        // (Said at once: a start may take a while, Android's first, a Steam game's, Proton's setup.)
        if let Some(title) = title.filter(|_| effects.iter().any(|e| matches!(e, Effect::Launch { .. }))) {
            self.notify(crate::bottom::notice::NoticeKind::Starting(title));
        }
        effects
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
        self.notify(crate::bottom::notice::NoticeKind::NoWindow(title));
        vec![]
    }

    /// Launches `game` (again, if it runs: another tab), unless a launch of it is starting; one that runs
    /// in another program's processes is shown instead (Steam doesn't start a game twice, Android an app).
    pub(crate) fn launch(&mut self, game: Arc<str>) -> Vec<Effect> {
        self.launch_with(game, None)
    }

    /// Launches `game` as [`State::launch`] does, by `command` if given (then a group too) instead of its own.
    pub(crate) fn launch_with(&mut self, game: Arc<str>, command: Option<Launch>) -> Vec<Effect> {
        if let Some(Launch::Tool(tool)) = self.shelf.entry(&game).map(|g| &g.launch) {
            // (The Device view is ClamShell's own, on the main screen.)
            if *tool == clamshell_library::Tool::Device {
                return self.open_device();
            }
            return vec![Effect::Tool(*tool)];
        }
        if command.is_none() && self.shelf.entry(&game).is_some_and(crate::Entry::is_group) {
            if !self.shelf.reveal(&game) {
                return vec![];
            }
            self.moved_collection();
            return vec![Effect::Window(WindowCommand::ShowShelf)];
        }
        let session = &self.session;
        if session.launching.iter().any(|l| l.entry == game) {
            return vec![];
        }
        if session.running(&game).is_some_and(|r| r.app_id.is_some()) {
            return self.show_running(&game);
        }
        let Some(g) = self.shelf.entry(&game) else {
            return vec![];
        };
        let app_id = app_id_of(g).filter(|_| command.is_none());
        let (launch, key) = (command.unwrap_or_else(|| g.launch.clone()), g.key.clone());
        let (secs, n) = (self.unix_secs, self.session.next_instance());
        let instance = Instance::Run { key, secs, n };
        let settings = self.settings_of(&game);
        let env = self.env_of(&game);
        // (Its runner paused, its container with it: resumed first.)
        let resumed = self.resume_runner_of(&game);
        self.session.launching.push(Launching { entry: game.clone(), instance: instance.clone(), app_id });
        // Shown once its first window appears.
        self.session.pending_show = Some(instance.clone());
        let mut effects: Vec<Effect> = resumed.into_iter().collect();
        effects.extend([Effect::Save(Save::Resume(self.resume())), Effect::Launch { entry: game, instance, settings, env, launch }]);
        effects
    }

    /// Our launches' entries run while systemd lists a scope of theirs (a Steam game's holds its follower
    /// between Steam's setup and the game; one started by a ClamShell before this one: its scopes). Once
    /// none, one known by its app id whose windows are open or may still come, or a browsed file's soon
    /// after it started, runs on by its windows (see [`Origin::Windows`]); the rest have ended, their
    /// play recorded.
    pub(crate) fn follow_ours(&mut self) -> Vec<Effect> {
        let now = self.mono_secs;
        let session = &self.session;
        let mut ended = Vec::new();
        let mut handed = Vec::new();
        let mut unstarted = Vec::new();
        for r in session.running.iter().filter(|r| r.origin == Origin::Ours) {
            if session.has_units(&r.key) {
                continue;
            }
            let has = session.windows_of(&r.id).next().is_some();
            let soon = now.saturating_sub(r.start) <= HANDOFF;
            let browsed = self.shelf.entry(&r.id).is_some_and(|g| g.browsed.is_some());
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
            self.notify(crate::bottom::notice::NoticeKind::NotStarted(title));
        }
        for (id, origin, has) in handed {
            if let Some(r) = self.session.running.iter_mut().find(|r| r.id == id) {
                r.origin = origin;
                r.seen = has;
            }
        }
        self.session.running.retain(|r| !ended.iter().any(|(id, _)| *id == r.id));
        ended.into_iter().flat_map(|(game, start)| self.record_play(game, start)).collect()
    }

    /// A play of `game` from `start` (monotonic seconds, as in [`crate::Event::Tick`]) until now: counted, and recorded, dated by the
    /// wall clock.
    /// (A video's: where mpv left it read, mpv having written it as it quit.)
    fn record_play(&mut self, game: Arc<str>, start: u64) -> Vec<Effect> {
        let duration = self.mono_secs.saturating_sub(start);
        let start = self.unix_at(start);
        if self.shelf.add_play(&game, start, duration) {
            self.saves.resume.touch(self.mono_secs);
        }
        let file = self.shelf.entry(&game).and_then(|g| g.mpv_file()).map(std::path::Path::to_path_buf);
        let position = file.map(|file| Effect::ReadPosition { entry: game.clone(), file });
        [Effect::RecordPlay { entry: game, start, duration }].into_iter().chain(position).collect()
    }

    /// A launch that never started (its scope was gone before it was seen): a browsed file's, or one known
    /// by its app id, waits for its window (see [`Origin::Windows`]).
    pub(crate) fn launch_failed(&mut self, game: Arc<str>) {
        let app_id = match self.session.launching.iter().position(|l| l.entry == game) {
            Some(i) => self.session.launching.remove(i).app_id,
            None => None,
        };
        let Some(entry) = self.shelf.entry(&game).filter(|g| g.browsed.is_some() || app_id.is_some()).cloned() else {
            // (Nothing to wait for: it didn't start, and says so.)
            let title = self.shelf.entry(&game).map_or_else(|| game.to_string(), |g| g.title.clone());
            if self.session.running(&game).is_none() {
                self.notify(crate::bottom::notice::NoticeKind::NotStarted(title));
            }
            return;
        };
        if self.session.running(&game).is_none() {
            let now = self.mono_secs;
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
    /// - one still no entry's makes its program an entry found by it (see [`crate::session::found`]);
    /// - one left on the landing workspace that is no entry's goes to the main screen;
    /// - an entry running by its windows that had windows and has none, or waited in vain, has ended.
    pub(crate) fn follow_windows(&mut self, known: &[i64]) -> Vec<Effect> {
        let mut effects = self.start_by_app_id();
        effects.extend(self.windows_to_their_entries());
        let (taken, stray) = self.stray_to_waiting(known, &mut effects);
        let given: Vec<i64> = if taken.is_some() { stray } else { Vec::new() };
        effects.extend(self.follow_found_windows(&given));
        if taken.is_none() {
            effects.extend(self.landing_up());
        }
        effects.extend(self.end_windowless(taken.as_ref()));
        effects
    }

    /// Entries started elsewhere, by their windows' app ids (a window's app id says whose it is before
    /// its scope does: a game Steam started is in Steam's scope). A Steam game is launched (the launcher
    /// finds it running); the rest run by their windows, the last shown.
    fn start_by_app_id(&mut self) -> Vec<Effect> {
        let now = self.mono_secs;
        let session = &self.session;
        let mut new: Vec<&Entry> = Vec::new();
        for w in session.windows.iter().filter(|w| !session.running.iter().any(|r| r.owns(w))) {
            let games = &self.shelf.library().entries;
            let Some(g) = games.iter().find(|g| has_app_id(g, &w.app_id)) else {
                continue;
            };
            if !session.launching.iter().any(|l| l.entry == g.id) && !new.iter().any(|n| n.id == g.id) {
                new.push(g);
            }
        }
        let (steam, new): (Vec<&Entry>, Vec<&Entry>) = new.into_iter().partition(|g| matches!(g.launch, Launch::Steam { .. }));
        let steam: Vec<Arc<str>> = steam.into_iter().map(|g| g.id.clone()).collect();
        let new: Vec<Running> =
            new.into_iter().map(|g| Running { app_id: app_id_of(g), seen: true, ..Running::of(g, Origin::Windows { since: now, until: now }, now) }).collect();
        if let Some(r) = new.last() {
            self.session.pending_show = Some(Instance::Run { key: r.key.clone(), secs: self.unix_secs, n: 0 });
            self.bottom.woken = false;
        }
        self.session.running.extend(new);
        steam.into_iter().flat_map(|game| self.launch(game)).collect()
    }

    /// Windows found by app id or by process, to their entry's workspace.
    fn windows_to_their_entries(&self) -> Vec<Effect> {
        let session = &self.session;
        let mut effects = Vec::new();
        for r in session.running.iter().filter(|r| r.outside()) {
            let away: Vec<i64> = session.windows_of(&r.id).filter(|w| !w.down && Instance::key_of(&w.workspace) != Some(&r.key)).map(|w| w.id).collect();
            if !away.is_empty() {
                effects.push(Effect::Window(WindowCommand::Move { windows: away, to: r.workspace() }));
            }
        }
        effects
    }

    /// New windows of no launch of ours (not in `known`) to the latest browsed file waiting for one,
    /// shown. The entry that took them, if one did, and those windows.
    fn stray_to_waiting(&self, known: &[i64], effects: &mut Vec<Effect>) -> (Option<Arc<str>>, Vec<i64>) {
        let now = self.mono_secs;
        let session = &self.session;
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
        match waiting.filter(|_| !stray.is_empty()) {
            Some(r) => {
                effects.push(Effect::Window(WindowCommand::Move { windows: stray.clone(), to: r.workspace() }));
                effects.push(Effect::Window(WindowCommand::Focus(r.workspace())));
                (Some(r.id.clone()), stray)
            }
            None => (None, stray),
        }
    }

    /// Windows left on the landing workspace (their entry gone from the library since its rule was
    /// given) to the main screen; not one whose entry's launch is starting (a Steam game found running).
    fn landing_up(&self) -> Vec<Effect> {
        let session = &self.session;
        let landing = Instance::Landing.name();
        let starting = |w: &crate::session::windows::Window| session.launching.iter().any(|l| l.app_id.as_deref() == Some(w.app_id.as_str()));
        session
            .windows
            .iter()
            .filter(|w| w.workspace == landing && session.running_of(w).is_none() && !starting(w))
            .map(|w| Effect::Window(WindowCommand::Up { window: w.id, to: None }))
            .collect()
    }

    /// Entries running by their windows or found by a tray icon end when those are gone (`taken`: an
    /// entry whose window was just given it, still where it came up); a play by windows is recorded.
    fn end_windowless(&mut self, taken: Option<&Arc<str>>) -> Vec<Effect> {
        let now = self.mono_secs;
        let session = &self.session;
        let with: Vec<Arc<str>> =
            session.running.iter().filter(|r| taken == Some(&r.id) || session.windows_of(&r.id).next().is_some()).map(|r| r.id.clone()).collect();
        let mut ended = Vec::new();
        let icons: Vec<Arc<str>> = self.tray.iter().map(|t| t.key.clone()).collect();
        // (Those with a scope listed: a launch of theirs runs.)
        let launched: Vec<String> = session.running.iter().filter(|r| session.has_units(&r.key)).map(|r| r.key.clone()).collect();
        self.session.running.retain_mut(|r| {
            let has = with.contains(&r.id);
            r.seen |= has;
            match r.origin {
                Origin::Found { ref tray, .. } => tray.as_ref().is_some_and(|k| icons.contains(k)) || has,
                Origin::Windows { since, until } => {
                    let runs = launched.contains(&r.key) || if r.seen { has } else { now <= until };
                    if !runs && r.seen {
                        ended.push((r.id.clone(), since));
                    }
                    runs
                }
                Origin::Ours => true,
            }
        });
        ended.into_iter().flat_map(|(game, start)| self.record_play(game, start)).collect()
    }

    /// The app ids of the library's entries known by them, for sway's rules: their windows land on a
    /// workspace of their own, to be moved to their entry's (see [`State::follow_windows`]).
    pub(crate) fn land_rules(&self) -> Effect {
        let ids = self.shelf.library().entries.iter().filter_map(app_id_of).collect();
        Effect::Window(WindowCommand::LandRules(ids))
    }

    /// After systemd's list of our scopes changed: an entry with scopes listed that isn't known to run
    /// (started by a ClamShell before this one) runs, since its first scope started (the time in its
    /// name); one found by its tray icon whose process is in a scope of ours now runs as a launch; then
    /// those whose launches have ended (see [`State::follow_ours`]).
    pub(crate) fn follow_units(&mut self) -> Vec<Effect> {
        let (now, unix) = (self.mono_secs, self.unix_secs);
        // Launches whose scope is there: their entries run (as launches of ours: one handed over to its
        // windows, launched again, too), the second screen lit for them to darken.
        let session = &mut self.session;
        let started: Vec<Launching> = {
            let units = &session.units;
            let (started, waiting) = std::mem::take(&mut session.launching).into_iter().partition(|l| units.contains(&l.instance));
            session.launching = waiting;
            started
        };
        for l in started {
            match self.session.running.iter_mut().find(|r| r.id == l.entry) {
                Some(r) => r.origin = Origin::Ours,
                None => {
                    // (An entry gone from the library meanwhile: by its id alone.)
                    let known = self.shelf.entry(&l.entry).cloned();
                    let entry = known.clone().unwrap_or_else(|| Entry::new(&l.entry, &l.entry));
                    let r = Running { app_id: l.app_id, ..Running::of(&entry, Origin::Ours, now) };
                    self.session.running.push(Running { entry: r.entry.filter(|_| known.is_some()), ..r });
                }
            }
            self.bottom.woken = false;
        }
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
            let Some(g) = self.shelf.library().entries.iter().find(|g| g.key == key) else {
                continue;
            };
            let first = units.iter().filter_map(|u| match u {
                Instance::Run { key: k, secs, .. } if k == key => Some(*secs),
                _ => None,
            });
            // (Its scopes are named by the wall clock.)
            let start = first.min().map_or(now, |secs| now.saturating_sub(unix.saturating_sub(secs)));
            session.running.push(Running { app_id: app_id_of(g), ..Running::of(g, Origin::Ours, start) });
        }
        self.follow_ours()
    }

    /// The app whose window shows browsed file `id` (running): the desktop file named as the window's
    /// app id says (`org.kde.kate`: `desktop:org.kde.kate`; any case), else the one whose program it is.
    /// For its picture only: the file runs as itself.
    pub fn app_showing(&self, id: &str) -> Option<&crate::Entry> {
        self.shelf.entry(id).filter(|g| g.browsed.is_some())?;
        let session = &self.session;
        let window = session.windows_of(id).next()?;
        let app = window.app_id.as_str();
        let games = &self.shelf.library().entries;
        let program = |g: &&crate::Entry| match &g.launch {
            Launch::Program { argv, .. } => argv.first().and_then(|p| p.rsplit('/').next()).is_some_and(|p| p.eq_ignore_ascii_case(app)),
            _ => false,
        };
        let named = |g: &&crate::Entry| ids::desktop_file(&g.id).is_some_and(|d| d.eq_ignore_ascii_case(app));
        games.iter().find(named).or_else(|| games.iter().filter(|g| ids::desktop_file(&g.id).is_some()).find(program))
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
        self.session.quitting.retain(|q| q.entry != *game);
        self.session.quitting.push(crate::session::Quitting { entry: game.clone(), ended: false, stop });
        let mut effects: Vec<Effect> = thawed.into_iter().collect();
        if !windows.is_empty() {
            effects.push(Effect::Window(WindowCommand::Close(windows)));
        }
        effects.extend(self.follow_quitting());
        effects
    }

    /// Force kill: the entry ended at once.
    pub(crate) fn force_kill(&mut self, game: &str) -> Vec<Effect> {
        self.session.quitting.retain(|q| &*q.entry != game);
        let thawed = self.thaw(game);
        thawed.into_iter().chain(self.end(game, true)).collect()
    }

    /// Ending a running entry: its slice stopped (`force`: killed; a Steam game's too, moved into its
    /// launch's scope), and its runner's `stop` command run (`{id}` its app id).
    fn end(&self, game: &str, force: bool) -> Vec<Effect> {
        let Some(r) = self.session.running(game) else {
            return vec![];
        };
        // (Found, not moved into a scope of ours: its process, if it is its own.)
        if r.found() {
            return match r.pid() {
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
        let stop = self.entry(game)?.runner.as_ref().map(|o| &o.stop).filter(|s| !s.is_empty())?;
        Some(Effect::RunnerCommand(stop.iter().map(|a| a.replace("{id}", app_id)).collect()))
    }

    /// Entries quitting: one whose windows are all gone while it still runs is ended (once); one that no
    /// longer runs is done, its runner's stop run if it ended with its windows before that.
    pub(crate) fn follow_quitting(&mut self) -> Vec<Effect> {
        let mut quitting = std::mem::take(&mut self.session.quitting);
        let mut effects = vec![];
        quitting.retain_mut(|q| {
            let runs = self.session.running(&q.entry).is_some();
            if !runs && !q.ended {
                effects.extend(q.stop.take());
            }
            runs
        });
        for q in &mut quitting {
            if !q.ended && self.windows_of(&q.entry).is_empty() {
                q.ended = true;
                effects.extend(self.end(&q.entry, false));
            }
        }
        self.session.quitting = quitting;
        effects
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use crate::session::running::HANDOFF;
    use crate::testkit::*;
    use crate::*;

    fn launches(fx: &[Effect], id: &str) -> bool {
        fx.iter().any(|e| matches!(e, Effect::Launch { entry: game, .. } if &**game == id))
    }

    fn steam_ff6() -> State {
        let mut library = clamshell_library::demo::library();
        if let Some(g) = library.entries.iter_mut().find(|g| &*g.id == "ff6") {
            g.launch = Launch::Steam { appid: 620 };
        }
        State::new(Init { library, ..init() })
    }

    fn key(id: &str) -> String {
        clamshell_library::key_of(id)
    }

    #[test]
    fn has_app_id_agrees_with_app_id_of() {
        let steam = Entry { launch: Launch::Steam { appid: 620 }, ..Entry::new("steam:620", "Portal 2") };
        let handed = Entry {
            runner: Some(clamshell_library::RunnerOf { handover: true, ..clamshell_library::RunnerOf::new("android", "Android", false) }),
            ..Entry::new("desktop:org.app", "App")
        };
        let ours = Entry::new("desktop:foot", "Terminal");
        for g in [&steam, &handed, &ours] {
            for app_id in ["steam_app_620", "steam_app_62", "org.app", "foot", ""] {
                assert_eq!(super::has_app_id(g, app_id), super::app_id_of(g).as_deref() == Some(app_id), "{} {app_id}", g.id);
            }
        }
    }

    #[test]
    fn a_play_lasts_by_the_monotonic_clock_however_the_wall_clock_steps() {
        let mut s = state();
        let first = s.shelf().selected().cloned().unwrap();
        tap(&mut s, Id::Shelf(ShelfId::Launch));
        s.apply(Event::Tick { unix_secs: 100_000, mono_secs: 1000 });
        crate::testkit::start(&mut s, &first);
        // A minute later by the monotonic clock, the wall clock set on a day.
        s.apply(Event::Tick { unix_secs: 100_000 + 86_400 + 60, mono_secs: 1060 });
        let (_, fx) = crate::testkit::end(&mut s, &first);
        let start = 100_000 + 86_400;
        assert!(fx.contains(&Effect::RecordPlay { entry: first, start, duration: 60 }), "dated by the wall clock as it reads now: {fx:?}");
    }

    #[test]
    fn an_entry_runs_its_plays_count_and_it_may_run_twice() {
        let mut s = state();
        let first = s.shelf().selected().cloned().unwrap();
        let fx = tap(&mut s, Id::Shelf(ShelfId::Launch));
        assert!(launches(&fx, &first));
        assert!(!launches(&tap(&mut s, Id::Shelf(ShelfId::Launch)), &first), "not twice while starting");
        s.apply(Event::tick(1000));
        crate::testkit::start(&mut s, &first);
        let workspace = Instance::workspace_of(&key(&first));
        assert_eq!(tap(&mut s, Id::Shelf(ShelfId::Launch)), vec![], "running, no window yet: nothing to show");
        s.apply(Event::Windows(vec![win(3, "game", &workspace, false)]));
        assert_eq!(tap(&mut s, Id::Shelf(ShelfId::Launch)), vec![Effect::Window(WindowCommand::Focus(workspace))], "running: shown");
        // Another launch (the dock's Open another): it runs twice, until both end.
        assert!(launches(&s.launch(first.clone()), &first));
        crate::testkit::start(&mut s, &first);
        // (The first one's scope gone, the second's listed.)
        let second: Vec<Instance> = s.session.units.iter().skip(1).cloned().collect();
        s.apply(Event::Units(second));
        assert!(s.session().running(&first).is_some(), "the other still runs");
        s.apply(Event::tick(1660));
        let (_, fx) = crate::testkit::end(&mut s, &first);
        assert!(s.session().running(&first).is_none());
        assert!(fx.contains(&Effect::RecordPlay { entry: first.clone(), start: 1000, duration: 660 }), "{fx:?}");
        let g = s.entry(&first).unwrap();
        assert_eq!((g.plays.count, g.plays.seconds), (1, 660), "one play: from its first start to its last end");
        assert!(s.shelf().library().collections.iter().any(|c| c.name == "Recent"));
    }

    #[test]
    fn entries_run_side_by_side() {
        let mut s = state();
        s.apply(Event::tick(100));
        let zelda = launch(&mut s, "zelda-lttp");
        assert_eq!(zelda, Instance::Run { key: key("zelda-lttp"), secs: 100, n: 0 });
        tap(&mut s, game("ff6"));
        assert!(launches(&tap(&mut s, Id::Shelf(ShelfId::Launch)), "ff6"), "no choice: it starts");
        crate::testkit::start(&mut s, "ff6");
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
        s.apply(Event::tick(1000));
        let zelda = launch(&mut s, "zelda-lttp").workspace();
        let landing = Instance::Landing.name();
        let windows = vec![win(1, "zelda3", &zelda, false), win(7, "steam_app_620", &landing, true)];
        let (_, fx) = s.apply(Event::Windows(windows.clone()));
        // Found by its window: launched (the launcher finds it running and moves it into the launch's
        // scope), its window left where it is meanwhile.
        assert!(launches(&fx, "ff6"), "{fx:?}");
        assert!(!fx.iter().any(|e| matches!(e, Effect::Window(WindowCommand::Up { window: 7, .. }))), "{fx:?}");
        assert!(!launches(&s.apply(Event::Windows(windows)).1, "ff6"), "once");
        crate::testkit::start(&mut s, "ff6");
        assert_eq!(s.session().running_entries().len(), 2, "the other keeps running");
        let (_, fx) = s.apply(Event::Windows(vec![win(1, "zelda3", &zelda, false), win(7, "steam_app_620", &landing, true)]));
        let ff6 = Instance::workspace_of(&key("ff6"));
        assert!(fx.contains(&Effect::Window(WindowCommand::Move { windows: vec![7], to: ff6 })), "{fx:?}");
        // Ended as any launch: its slice.
        assert_eq!(s.force_kill("ff6"), vec![Effect::EndRun { key: key("ff6"), force: true }]);
        s.apply(Event::tick(1600));
        s.apply(Event::Windows(vec![win(1, "zelda3", &zelda, true)]));
        let (_, fx) = crate::testkit::end(&mut s, "ff6");
        assert!(fx.contains(&Effect::RecordPlay { entry: "ff6".into(), start: 1000, duration: 600 }), "{fx:?}");
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
        let mut library = clamshell_library::demo::library();
        if let Some(zelda) = library.entries.first_mut() {
            zelda.file = Some("/roms/zelda.sfc".into());
        }
        let browsed = |id: &str, file: &str| crate::Entry {
            file: Some(file.into()),
            launch: Launch::program(vec!["xdg-open".into(), file.into()]),
            browsed: Some(crate::Browsed::default()),
            ..crate::Entry::new(id, file)
        };
        let n = library.entries.len();
        library.entries.push(browsed("browse:Home/notes.md", "/h/notes.md"));
        library.entries.push(browsed("browse:Home/zelda.sfc", "/roms/zelda.sfc"));
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
        s.apply(Event::tick(100));
        let shelf = win(1, crate::session::windows::SHELF_APP_ID, &Instance::Shelf.name(), true);
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
        s.apply(LaunchEvent::LaunchFailed("browse:Home/notes.md".into()).into());
        assert!(s.session().running("browse:Home/notes.md").is_some(), "waits for its window");
        // (The demo's Terminal: its desktop file is foot's.)
        let app = win(6, "foot", &Instance::Shelf.name(), true);
        let (_, fx) = s.apply(Event::Windows(vec![shelf.clone(), other.clone(), app.clone()]));
        assert!(fx.contains(&Effect::Window(WindowCommand::Move { windows: vec![6], to: ws.clone() })), "only the new one: {fx:?}");
        assert!(fx.contains(&Effect::Window(WindowCommand::Focus(ws.clone()))));
        s.apply(Event::Windows(vec![shelf.clone(), other.clone(), Window { workspace: ws, ..app }]));
        s.apply(Event::tick(200));
        assert!(s.session().running("browse:Home/notes.md").is_some(), "runs while its window is open");
        assert_eq!(s.app_showing("browse:Home/notes.md").map(|g| &*g.id), Some("desktop:foot"), "its picture: the app's");
        // Its file gone from the library meanwhile: still shown by what it was.
        s.apply(Event::Library(Box::new(clamshell_library::demo::library())));
        assert!(s.shelf().entry("browse:Home/notes.md").is_none());
        assert_eq!(s.entry("browse:Home/notes.md").map(|g| g.title.as_str()), Some("/h/notes.md"));
        s.apply(Event::Library(Box::new(files().shelf().library().clone())));
        s.apply(Event::Windows(vec![shelf.clone(), other.clone()]));
        assert!(s.session().running("browse:Home/notes.md").is_none(), "its window closed: ended");
        // Another that never shows a window: gone after a while.
        tap(&mut s, Id::Shelf(ShelfId::Launch));
        s.apply(LaunchEvent::LaunchFailed("browse:Home/notes.md".into()).into());
        s.apply(Event::tick(200 + HANDOFF));
        assert!(s.session().running("browse:Home/notes.md").is_some());
        s.apply(Event::tick(201 + HANDOFF));
        assert!(s.session().running("browse:Home/notes.md").is_none());
        assert!(s.dock().items().is_empty(), "off the dock");
    }

    #[test]
    fn an_app_handed_over_runs_by_its_window_found_by_its_app_id() {
        let mut library = clamshell_library::demo::library();
        let firefox = library.entries.iter_mut().find(|g| &*g.id == "desktop:firefox").unwrap();
        let stop = vec!["stop".to_owned(), "{id}".to_owned()].into();
        firefox.runner = Some(clamshell_library::RunnerOf { handover: true, stop, ..clamshell_library::RunnerOf::new("android", "Android", true) });
        let mut s = State::new(Init { library, ..init() });
        s.apply(Event::tick(100));
        let shelf = win(1, crate::session::windows::SHELF_APP_ID, &Instance::Shelf.name(), true);
        let landing = Instance::Landing.name();
        s.apply(Event::Windows(vec![shelf.clone()]));
        let ws = Instance::workspace_of(&key("desktop:firefox"));
        assert!(matches!(&s.land_rules(), Effect::Window(WindowCommand::LandRules(ids)) if ids.iter().any(|i| &**i == "firefox")), "its windows land");
        assert!(launches(&s.launch("desktop:firefox".into()), "desktop:firefox"));
        crate::testkit::start(&mut s, "desktop:firefox");
        // Its launch handed the app over (to the Waydroid session) and ended.
        crate::testkit::end(&mut s, "desktop:firefox");
        assert!(s.session().running("desktop:firefox").is_some(), "waits for its window");
        let app = win(6, "firefox", &landing, false);
        let (_, fx) = s.apply(Event::Windows(vec![shelf.clone(), app.clone()]));
        assert!(fx.contains(&Effect::Window(WindowCommand::Move { windows: vec![6], to: ws.clone() })), "{fx:?}");
        let moved = Window { workspace: ws.clone(), ..app.clone() };
        s.apply(Event::Windows(vec![shelf.clone(), moved.clone()]));
        assert!(!launches(&s.launch("desktop:firefox".into()), "desktop:firefox"), "it runs once: shown");
        s.apply(Event::tick(400));
        assert!(s.session().running("desktop:firefox").is_some(), "runs while its window is open");
        let fx = s.force_kill("desktop:firefox");
        assert!(fx.contains(&Effect::RunnerCommand(vec!["stop".into(), "firefox".into()])), "its runner's stop: {fx:?}");
        let (_, fx) = s.apply(Event::Windows(vec![shelf.clone()]));
        assert!(s.session().running("desktop:firefox").is_none(), "its window closed: ended");
        assert!(fx.contains(&Effect::RecordPlay { entry: "desktop:firefox".into(), start: 100, duration: 300 }), "played until then: {fx:?}");
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
        crate::testkit::start(&mut s, "desktop:firefox");
        crate::testkit::end(&mut s, "desktop:firefox");
        assert!(s.session().running("desktop:firefox").is_none());
    }

    #[test]
    fn entries_started_before_clamshell_are_found_by_their_scopes() {
        let mut s = state();
        s.apply(Event::tick(1000));
        let foot = Instance::Run { key: key("desktop:foot"), secs: 400, n: 0 };
        s.apply(Event::Units(vec![foot.clone()]));
        assert!(s.session().running("desktop:foot").is_some(), "adopted");
        s.apply(Event::Windows(vec![win(4, "foot", &foot.workspace(), false)]));
        assert_eq!(tap(&mut s, Id::Dock(DockId::Entry(0))), vec![Effect::Window(WindowCommand::Focus(foot.workspace()))], "on the dock, shown");
        s.apply(Event::tick(1300));
        let (_, fx) = s.apply(Event::Units(vec![]));
        assert!(s.session().running("desktop:foot").is_none(), "its scopes gone: ended");
        assert!(fx.contains(&Effect::RecordPlay { entry: "desktop:foot".into(), start: 400, duration: 900 }), "played since its launch: {fx:?}");
    }

    #[test]
    fn a_launch_runs_while_a_scope_of_it_is_listed() {
        let mut s = state();
        s.apply(Event::tick(100));
        let fx = tap(&mut s, game("zelda-lttp"));
        let fx: Vec<Effect> = fx.into_iter().chain(tap(&mut s, game("zelda-lttp"))).collect();
        let zelda = fx.iter().find_map(|e| match e {
            Effect::Launch { instance, .. } => Some(instance.clone()),
            _ => None,
        });
        let zelda = zelda.unwrap();
        assert!(s.session().running("zelda-lttp").is_none(), "asked for, its scope not there yet: starting");
        // Its scope listed: it runs; listed no longer: it has ended, its play recorded once.
        s.apply(Event::Units(vec![zelda.clone()]));
        assert!(s.session().running("zelda-lttp").is_some());
        s.apply(Event::tick(400));
        let (_, fx) = s.apply(Event::Units(vec![]));
        assert!(s.session().running("zelda-lttp").is_none());
        assert!(fx.contains(&Effect::RecordPlay { entry: "zelda-lttp".into(), start: 100, duration: 300 }), "once: {fx:?}");
        assert!(!s.apply(Event::Units(vec![])).1.iter().any(|e| matches!(e, Effect::RecordPlay { .. })), "once");
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
        in_front(&mut s, win(1, crate::session::windows::SHELF_APP_ID, &Instance::Shelf.name(), true));
        assert_eq!(id(&s).as_deref(), Some("ff6"), "shelf in front: its selection");
        in_front(&mut s, win(3, "foot", "1", true));
        assert_eq!(s.context(), None, "a window of none of ours: the defaults");
    }
}
