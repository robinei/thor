//! Running entries: what was launched from shelf or the dock, games and apps alike. Each launch is a scope
//! of its own in the entry's slice, with its windows on the entry's workspace (see [`Instance`]). An entry
//! may run more than once; another launch is another scope and another tab.
//!
//! Some entries run in another program's processes, not in a scope of ours, and are known by their
//! windows' app id ([`app_id_of`]). These are Steam's games (`steam_app_<id>`, in Steam's scope) and the
//! entries of a runner that hands its apps over, such as Waydroid's, whose app id is their desktop file's
//! name and which run in the Waydroid session. Their windows are put on their entry's workspace as they
//! open. A window that opens while the entry doesn't run, such as a game started in Steam or an app opened by another app, starts the entry.
//! Such an entry runs once, so another launch shows it instead. It runs on by its windows after its launch
//! ends, until the last one closes.
//!
//! A file opened from the Files tab, a browsed entry, may have a launch that hands it to a program not
//! ours and ends, as with `xdg-open` to an app's own instance or with D-Bus activation. It runs on by its
//! windows too: a new window of no launch of ours that appears within [`HANDOFF`] is moved to its
//! workspace, and the file has ended when that window has closed, or when none came.
//!
//! In shelf, activating a running entry shows it, and activating a stopped one starts it. Quit asks an
//! entry's windows to close and, once none is left and it still runs, ends it. That stops its slice, or for
//! a Steam game the processes under Steam's launcher for it, and runs its runner's `stop` command. Force
//! kill ends it at once. What is in front, whether shelf, a running entry or another window, decides where
//! the pad goes, what Home does and what the settings page edits.

use std::sync::Arc;

use clamshell_library::{Launch, ids};

use crate::bottom::Tab;
use crate::bottom::notice::{NoticeKind, NoticeSlot};
use crate::effect::{Effect, Save};
use crate::out::Out;
use crate::session::found::window_entry;
use crate::session::instance::{Instance, STEAM_APP_PREFIX};
use crate::session::tray::{TrayItem, tray_entry};
use crate::session::windows::WindowCommand;
use crate::session::windows::{Window, WindowFacts};
use crate::settings::Settings;
use crate::{Clock, Entry, Library, Resume, Session, Shelf};

/// What is in front on the main screen.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Front {
    Shelf,
    /// A window of a running entry.
    Program,
    /// Another window, not one of ours.
    Other,
}

/// How a running entry runs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Origin {
    /// By its launches. The launcher thread says when each has ended.
    Ours,
    /// A program not started by ClamShell, found by its tray icon (see [`crate::session::tray`]) or by a
    /// window of it (see [`crate::session::found`]). It runs while the icon is there or a window of it is
    /// open, until it is moved into a scope of ours, after which it runs as [`Origin::Ours`].
    ///
    /// `pid` is its own process, the tray icon's or its window's; windows of that process, or of a process
    /// it started, are the entry's. It is none if the program runs in another program's process.
    Found { pid: Option<u32> },
    /// By its windows, since `since` (monotonic seconds, as in [`crate::Event::Tick`]). It was started by
    /// another program, or its launches ended while it ran on in a program not ours, and then `since` is
    /// when it was launched. It ends once it has
    /// had windows ([`Running::seen`]) and has none left, or when none came by `until`, and its play from
    /// `since` is recorded then. Its windows are those with its app id, or, for a browsed file, a new window
    /// of no launch of ours, taken until `until`.
    Windows { since: u64, until: u64 },
}

/// How long a quit waits for the windows to close, in seconds, before it is given up.
pub const QUIT_WAIT: u64 = 10;

/// How long a launch that handed over waits for its window, in seconds.
pub const HANDOFF: u64 = 15;

/// An entry that runs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Running {
    pub id: Arc<str>,
    /// Its key, which names its workspace and its launches' scopes and slice.
    pub key: String,
    pub origin: Origin,
    /// The app id its windows have, if it runs in another program's processes (see [`app_id_of`]).
    pub app_id: Option<Arc<str>>,
    /// When it first started, in monotonic seconds as in [`crate::Event::Tick`].
    pub start: u64,
    /// Whether a window of it has been there since it started, or since it went on by its windows, or,
    /// unless it runs by its windows, a tray icon of it. Until then it is starting.
    pub(crate) seen: bool,
    /// The key of the tray icon it was found by, if it was. It stays once the program is moved into a
    /// scope of ours, since the icon's process is known to be in that scope only once the tray is read
    /// again.
    pub(crate) tray: Option<Arc<str>>,
    /// Its entry as it was when it started. The dock shows it by this if it leaves the library while it
    /// runs, for example because its file was renamed or deleted.
    pub entry: Option<Box<crate::Entry>>,
    /// The resolution the compositor told it its screen has, when it connected (see
    /// [`crate::Reading::Told`]); a change of the setting applies the next time it starts.
    pub told: Option<crate::settings::Resolution>,
}

impl Running {
    /// `entry` running as `origin` since `start`, with no launch followed and no window seen yet.
    pub(crate) fn of(entry: &Entry, origin: Origin, start: u64) -> Running {
        let (id, key) = (entry.id.clone(), entry.key.clone());
        Running { id, key, origin, app_id: None, start, seen: false, tray: None, entry: Some(Box::new(entry.clone())), told: None }
    }

    /// Its own process, if it was found and has one (see [`Origin::Found`]).
    pub fn pid(&self) -> Option<u32> {
        match self.origin {
            Origin::Found { pid, .. } => pid,
            _ => None,
        }
    }

    /// The icon's key, if it was found by a tray icon.
    pub fn tray(&self) -> Option<&Arc<str>> {
        self.tray.as_ref()
    }

    pub fn found(&self) -> bool {
        matches!(self.origin, Origin::Found { .. })
    }

    /// Whether `w` is one of its windows by its app id.
    pub fn owns(&self, w: &WindowFacts) -> bool {
        self.app_id.as_deref() == Some(w.app_id.as_str())
    }

    /// Whether `w` is one of its windows by its process: its own process, or one that process started.
    pub fn fathered(&self, w: &WindowFacts) -> bool {
        self.pid().is_some_and(|pid| w.pids.contains(&pid))
    }

    /// Whether its windows are found other than by its scope, by app id or by process, and so are moved
    /// to its workspace wherever they open.
    pub fn outside(&self) -> bool {
        self.app_id.is_some() || self.pid().is_some()
    }

    /// Its workspace's name.
    pub fn workspace(&self) -> String {
        Instance::workspace_of(&self.key)
    }
}

/// A launch asked for and not started yet. The launcher thread says when it has started, or that it
/// couldn't; without an answer by [`LAUNCH_WAIT`] it is given up.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Launching {
    pub entry: Arc<str>,
    pub instance: Instance,
    /// The app id its windows have, if it runs in another program's processes.
    pub app_id: Option<Arc<str>>,
    /// When it was asked for, in monotonic seconds.
    pub at: u64,
    /// The resolution its process was told as it connected, before its unit was listed (see
    /// [`Running::told`]).
    pub told: Option<crate::settings::Resolution>,
}

/// How long a launch waits for an answer from the launcher, its scope or its failure, in seconds, before
/// it is given up as not started.
pub const LAUNCH_WAIT: u64 = 30;

/// The app id an entry's windows have when it runs in another program's processes, which makes them its
/// windows wherever they open. For Steam's games it is `steam_app_<id>`. For an entry whose runner hands
/// its apps over, such as Waydroid's, it is the desktop file's name. It is none for the rest, which run in
/// scopes of ours.
pub fn app_id_of(g: &Entry) -> Option<Arc<str>> {
    match &g.launch {
        Launch::Steam { appid } => Some(format!("{STEAM_APP_PREFIX}{appid}").into()),
        _ => handed_over_app(g).map(Into::into),
    }
}

/// Whether `app_id` is [`app_id_of`] `g`, without building it, since this is asked for every window and
/// entry.
pub fn has_app_id(g: &Entry, app_id: &str) -> bool {
    match &g.launch {
        Launch::Steam { appid } => app_id.strip_prefix(STEAM_APP_PREFIX).and_then(|n| n.parse::<u32>().ok()) == Some(*appid),
        _ => handed_over_app(g) == Some(app_id),
    }
}

/// A handed-over entry's app id: its desktop file's name.
/// A play that ended: the entry and when it started, in monotonic seconds. State records it in shelf.
pub(crate) type Play = (Arc<str>, u64);

fn handed_over_app(g: &Entry) -> Option<&str> {
    ids::desktop_file(&g.id).filter(|_| g.runner.as_ref().is_some_and(|r| r.handover))
}

impl Session {
    /// Runs after systemd's list of our scopes changed, and after any event. A launch with no answer by
    /// [`LAUNCH_WAIT`] is given up, with a notice that it didn't start. An entry with scopes listed that
    /// isn't known to run, because an earlier ClamShell started it, runs from when its first scope
    /// started, by the time in the scope's name. One the library doesn't have, because the earlier
    /// ClamShell found it, runs as found again by its windows or tray icon ([`found_in_scope`]). An entry
    /// found by its tray icon whose process is now in a scope of ours runs as a launch. Then the entries
    /// whose launches have ended are handled (see [`Session::follow_ours`]). Returns whether a launch of
    /// ours started, which may let a second screen tapped awake go dark again.
    pub(crate) fn follow_units(&mut self, clock: &Clock, library: &Library, notice: &mut NoticeSlot, plays: &mut Vec<Play>) -> bool {
        let (now, unix) = (clock.mono(), clock.unix());
        // The entries of launches whose scope is there run as launches of ours, including one that was
        // handed over to its windows and launched again.
        let started: Vec<Launching> = {
            let units = &self.units;
            let (started, waiting) = std::mem::take(&mut self.launching).into_iter().partition(|l| units.contains(&l.instance));
            self.launching = waiting;
            started
        };
        // A launch the launcher never answered is given up, so that it can be launched again.
        while let Some(i) = self.launching.iter().position(|l| now >= l.at + LAUNCH_WAIT) {
            let l = self.launching.remove(i);
            if self.front_launch.as_ref() == Some(&l.instance) {
                self.front_launch = None;
            }
            let title = library.entry(&l.entry).map_or_else(|| l.entry.to_string(), |g| g.title.clone());
            notice.show(NoticeKind::NotStarted(title), now);
        }
        let any_started = !started.is_empty();
        for l in started {
            match self.running.iter_mut().find(|r| r.id == l.entry) {
                Some(r) => r.origin = Origin::Ours,
                None => {
                    // An entry gone from the library meanwhile is known by its id alone.
                    let known = library.entry(&l.entry).cloned();
                    let entry = known.clone().unwrap_or_else(|| Entry::new(&l.entry, &l.entry));
                    let r = Running { app_id: l.app_id, told: l.told, ..Running::of(&entry, Origin::Ours, now) };
                    self.running.push(Running { entry: r.entry.filter(|_| known.is_some()), ..r });
                }
            }
        }
        let units = &self.units;
        for r in self.running.iter_mut().filter(|r| r.found()) {
            if units.iter().any(|u| u.key() == Some(r.key.as_str())) {
                // Its windows are now its scope's, not every window of that app id or process. That would
                // tie another instance of the program, started later, to it.
                r.origin = Origin::Ours;
                r.app_id = None;
            }
        }
        for unit in units {
            let Some(key) = unit.key() else {
                continue;
            };
            // A launch of the entry waiting doesn't hold it off: this scope is another launch's, such as
            // one given up that started late, and the entry runs.
            if self.running.iter().any(|r| r.key == key) {
                continue;
            }
            let first = units.iter().filter_map(|u| match u {
                Instance::Run { key: k, secs, .. } if k == key => Some(*secs),
                _ => None,
            });
            // Its scopes are named by the wall clock.
            let start = first.min().map_or(now, |secs| now.saturating_sub(unix.saturating_sub(secs)));
            if let Some(g) = library.entries.iter().find(|g| g.key == key) {
                self.running.push(Running { app_id: app_id_of(g), ..Running::of(g, Origin::Ours, start) });
                continue;
            }
            // A program an earlier ClamShell found and moved into a scope of ours, or a file it opened,
            // runs as found by its scope, once a window or tray icon of it says what it is.
            let Some(g) = found_in_scope(key, &self.windows, &self.tray, library).filter(|g| !self.running.iter().any(|r| r.id == g.id)) else {
                continue;
            };
            self.running.push(Running::of(&Entry { key: key.to_owned(), ..g }, Origin::Ours, start));
        }
        self.follow_ours(clock.mono(), library, notice, plays);
        any_started
    }

    /// Our launches' entries run while systemd lists a scope of theirs. A Steam game's scope holds its
    /// follower between Steam's setup and the game, and an entry started by an earlier ClamShell has its
    /// scopes listed too. Once there are none, an entry known by its app id whose windows are open or may
    /// still come, or a browsed file's entry soon after it started, runs on by its windows (see
    /// [`Origin::Windows`]). The rest have ended, and their plays are added to `plays`.
    fn follow_ours(&mut self, now: u64, library: &Library, notice: &mut NoticeSlot, plays: &mut Vec<Play>) {
        let mut ended = Vec::new();
        let mut handed = Vec::new();
        let mut unstarted = Vec::new();
        for r in self.running.iter().filter(|r| r.origin == Origin::Ours) {
            if self.has_units(&r.key) {
                continue;
            }
            let has = self.windows_of(&r.id).next().is_some();
            let soon = now.saturating_sub(r.start) <= HANDOFF;
            let browsed = library.entry(&r.id).is_some_and(|g| g.browsed.is_some());
            // Its play goes on from its launch's start.
            if r.app_id.is_some() && (has || soon) {
                handed.push((r.id.clone(), Origin::Windows { since: r.start, until: now + HANDOFF }, has));
            } else if browsed && soon {
                handed.push((r.id.clone(), Origin::Windows { since: r.start, until: r.start + HANDOFF }, has));
            } else {
                ended.push((r.id.clone(), r.start));
                // It ended without ever having a window and without being quit, so it didn't start, as
                // with a Steam game that never came.
                if !r.seen && !self.is_quitting(&r.id) {
                    unstarted.push(r.entry.as_ref().map_or_else(|| r.id.to_string(), |g| g.title.clone()));
                }
            }
        }
        for title in unstarted {
            notice.show(NoticeKind::NotStarted(title), now);
        }
        for (id, origin, has) in handed {
            if let Some(r) = self.running.iter_mut().find(|r| r.id == id) {
                r.origin = origin;
                r.seen = has;
            }
        }
        self.running.retain(|r| !ended.iter().any(|(id, _)| *id == r.id));
        plays.extend(ended);
    }

    /// A launch that never started, its scope gone before it was seen. A browsed file's launch, or one
    /// known by its app id, waits for its window (see [`Origin::Windows`]). A failure of no launch waiting
    /// is a late answer to one already given up ([`LAUNCH_WAIT`]), which said so then, and is ignored.
    pub(crate) fn launch_failed(&mut self, entry: Arc<str>, library: &Library, clock: &Clock, notice: &mut NoticeSlot) {
        let Some(i) = self.launching.iter().position(|l| l.entry == entry) else {
            return;
        };
        let app_id = self.launching.remove(i).app_id;
        let Some(known) = library.entry(&entry).filter(|g| g.browsed.is_some() || app_id.is_some()) else {
            // There is nothing to wait for, so it didn't start, and a notice says so.
            let title = library.entry(&entry).map_or_else(|| entry.to_string(), |g| g.title.clone());
            if self.running(&entry).is_none() {
                notice.show(NoticeKind::NotStarted(title), clock.mono());
            }
            return;
        };
        if self.running(&entry).is_none() {
            let now = clock.mono();
            let origin = Origin::Windows { since: now, until: now + HANDOFF };
            self.running.push(Running { app_id, ..Running::of(known, origin, now) });
        }
    }

    /// Starts entries that were started elsewhere, found by their windows' app ids. A window's app id says
    /// whose it is before its scope does, since a game Steam started is in Steam's scope. A Steam game is
    /// to be launched, and the launcher finds it running: its id is added to `steam`. The rest run by
    /// their windows, and the last is shown. Returns whether one of those runs now, which may let a second
    /// screen tapped awake go dark again.
    pub(crate) fn start_by_app_id(&mut self, clock: &Clock, library: &Library, steam: &mut Vec<Arc<str>>) -> bool {
        let now = clock.mono();
        let mut new: Vec<&Entry> = Vec::new();
        for w in self.windows.iter().filter(|w| !self.running.iter().any(|r| r.owns(w))) {
            let Some(g) = library.entries.iter().find(|g| has_app_id(g, &w.app_id)) else {
                continue;
            };
            if !self.launching.iter().any(|l| l.entry == g.id) && !new.iter().any(|n| n.id == g.id) {
                new.push(g);
            }
        }
        let (steam_games, new): (Vec<&Entry>, Vec<&Entry>) = new.into_iter().partition(|g| matches!(g.launch, Launch::Steam { .. }));
        steam.extend(steam_games.into_iter().map(|g| g.id.clone()));
        let new: Vec<Running> =
            new.into_iter().map(|g| Running { app_id: app_id_of(g), seen: true, ..Running::of(g, Origin::Windows { since: now, until: now }, now) }).collect();
        let Some(r) = new.last() else {
            return false;
        };
        self.set_front_launch(Instance::Run { key: r.key.clone(), secs: clock.unix(), n: 0 }, None);
        self.running.extend(new);
        true
    }

    /// Runs after every event, after the entries started by their windows' app ids
    /// ([`Session::start_by_app_id`]).
    ///
    /// - A running entry's windows found by app id go to its workspace.
    /// - A new window of no launch of ours goes to the latest browsed file waiting for one, shown.
    /// - A window that still belongs to no entry makes its program an entry found by it (see
    ///   [`crate::session::found`]).
    /// - An unplaced window that belongs to no entry receives a home group.
    /// - An entry running by its windows that had windows and has none, or that waited in vain, has ended.
    ///   Its play is added to `plays`.
    pub(crate) fn follow_windows(&mut self, clock: &Clock, library: &Library, plays: &mut Vec<Play>, out: &mut Out) {
        self.place_owned_windows();
        let mut given = Vec::new();
        let taken = self.claim_opened_windows(clock.mono(), &mut given);
        self.follow_found_windows(&given, clock, library, out);
        if taken.is_none() {
            self.place_unowned();
        }
        self.end_windowless(taken.as_ref(), clock.mono(), plays);
    }

    /// Moves windows found by app id or by process to their entry's workspace.
    fn place_owned_windows(&mut self) {
        let mut commands = Vec::new();
        for r in &self.running {
            let away: Vec<i64> = self.windows_of(&r.id).filter(|w| Instance::key_of(&w.workspace) != Some(&r.key)).map(|w| w.id).collect();
            if !away.is_empty() {
                commands.push(WindowCommand::Move { windows: away, to: r.workspace() });
            }
        }
        for command in commands {
            self.command(command);
        }
    }

    /// Moves new windows of no launch of ours, those the last pass didn't see, to the latest browsed file
    /// waiting for one, and shows it. Returns the entry that took them, if one did, and adds those windows
    /// to `taken`.
    fn claim_opened_windows(&mut self, now: u64, taken: &mut Vec<i64>) -> Option<Arc<str>> {
        let opened = &self.opened;
        let stray: Vec<i64> =
            self.windows.iter().filter(|w| opened.contains(&w.id) && !w.launched() && !w.down && self.running_of(w).is_none()).map(|w| w.id).collect();
        let waiting = self
            .running
            .iter()
            .filter(|r| r.app_id.is_none() && !r.seen && matches!(r.origin, Origin::Windows { until, .. } if now <= until))
            .max_by_key(|r| r.start);
        let r = waiting.filter(|_| !stray.is_empty())?;
        let id = r.id.clone();
        let group = r.workspace();
        taken.extend(&stray);
        self.command(WindowCommand::Move { windows: stray, to: group.clone() });
        Some(id)
    }

    /// Places unowned windows after file handoffs and program discovery have had their chance. A
    /// starting launch already has its group, and is left alone. A service dialog uses the current
    /// program's group; without one it gets a group of its own so Home can still return to shelf.
    fn place_unowned(&mut self) {
        let starting = |w: &crate::session::windows::Window| self.launching.iter().any(|l| l.app_id.as_deref() == Some(w.app_id.as_str()));
        let ids: Vec<i64> = self.windows.iter().filter(|w| w.workspace.is_empty() && self.running_of(w).is_none() && !starting(w)).map(|w| w.id).collect();
        for id in ids {
            let down = self.window(id).is_some_and(|w| w.down);
            let group = self
                .windows
                .iter()
                .find(|w| !down && w.focused && !w.down && !w.workspace.is_empty())
                .map(|w| w.workspace.clone())
                .unwrap_or_else(|| format!("clamshell-window-{id}"));
            self.command(WindowCommand::Move { windows: vec![id], to: group.clone() });
            if !down {
                self.command(WindowCommand::FocusWindow(id));
            }
        }
    }

    /// Marks entries seen once they have a window or a tray icon. Ends entries running by their windows, or
    /// found by a tray icon, once those are gone. `taken` is an
    /// entry that was just given a window, which is still where it came up. A play by windows is added to
    /// `plays`.
    fn end_windowless(&mut self, taken: Option<&Arc<str>>, now: u64, plays: &mut Vec<Play>) {
        // Which have windows is worked out before any is removed, since a window's entry is found among
        // all of them.
        let mut with = std::mem::take(&mut self.with_windows);
        with.clear();
        with.extend(self.running.iter().map(|r| (taken == Some(&r.id) || self.windows_of(&r.id).next().is_some(), self.tray_of(&r.id).is_some())));
        let (icons, units) = (&self.tray, &self.units);
        let mut has_windows = with.iter();
        self.running.retain_mut(|r| {
            let (has, icon) = has_windows.next().copied().unwrap_or((false, false));
            // A program with a tray icon has started, window or not. One running by its windows is seen
            // by them alone, since it ends once they are gone.
            r.seen |= has || (icon && !matches!(r.origin, Origin::Windows { .. }));
            match r.origin {
                Origin::Found { .. } => r.tray.as_ref().is_some_and(|k| icons.iter().any(|t| t.key == *k)) || has,
                Origin::Windows { since, until } => {
                    // An entry with a scope listed has a launch of its own running.
                    let launched = units.iter().any(|u| u.key() == Some(r.key.as_str()));
                    let runs = launched || if r.seen { has } else { now <= until };
                    if !runs && r.seen {
                        plays.push((r.id.clone(), since));
                    }
                    runs
                }
                Origin::Ours => true,
            }
        });
        self.with_windows = with;
    }

    /// The app whose window shows the running browsed file `id`. It is the app whose desktop file is named
    /// as the window's app id, ignoring case, such as `desktop:org.kde.kate` for `org.kde.kate`, or else
    /// the one whose program it is. This is for the file's picture only; the file runs as itself.
    pub fn app_showing<'a>(&self, id: &str, library: &'a Library) -> Option<&'a crate::Entry> {
        library.entry(id).filter(|g| g.browsed.is_some())?;
        let window = self.windows_of(id).next()?;
        let app = window.app_id.as_str();
        let entries = &library.entries;
        let program = |g: &&crate::Entry| match &g.launch {
            Launch::Program { argv, .. } => argv.first().and_then(|p| p.rsplit('/').next()).is_some_and(|p| p.eq_ignore_ascii_case(app)),
            _ => false,
        };
        let named = |g: &&crate::Entry| ids::desktop_file(&g.id).is_some_and(|d| d.eq_ignore_ascii_case(app));
        entries.iter().find(named).or_else(|| entries.iter().filter(|g| ids::desktop_file(&g.id).is_some()).find(program))
    }

    /// Remembers the windows this pass saw and the workspace in front, for the next pass (see
    /// [`Session::claim_opened_windows`] and [`Session::follow_front`]).
    pub(crate) fn followed(&mut self) {
        self.opened.clear();
        let front = self.top_window().map(|w| w.workspace.as_str());
        if self.followed_front.as_deref() != front {
            self.followed_front = front.map(str::to_owned);
        }
    }
}

/// The entry that the program in our scope of entry `key`, which the library doesn't have, runs as: named
/// by a window or tray icon in that scope, as when it was found (see [`crate::session::found`]). Of the two
/// names, the one whose key is the scope's is the one it was found by. Without a window or icon there is
/// none.
fn found_in_scope(key: &str, windows: &[Window], tray: &[TrayItem], library: &Library) -> Option<Entry> {
    let in_scope = |unit: Option<&str>| unit.and_then(Instance::key_of) == Some(key);
    let by_window = windows.iter().filter(|w| in_scope(w.unit.as_deref())).find_map(|w| {
        let pid = w.pids.first().copied();
        (!w.app_id.is_empty() || pid.is_some()).then(|| window_entry(library, w, pid.unwrap_or_default()))
    });
    let by_icon = tray.iter().find(|t| in_scope(t.unit.as_deref())).map(|t| tray_entry(library, t));
    let named = |g: &Option<Entry>| g.as_ref().is_some_and(|g| g.key == key);
    if named(&by_icon) {
        by_icon
    } else if named(&by_window) {
        by_window
    } else {
        by_window.or(by_icon)
    }
}

/// A launch asked for: the entry, the command to run instead of its own, if any, and how it starts.
#[derive(Debug, Clone)]
pub(crate) struct LaunchRequest {
    pub(crate) entry: Arc<str>,
    pub(crate) command: Option<Launch>,
    pub(crate) start: Start,
}

/// How a launch starts.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Start {
    /// Its workspace is shown once its first window appears.
    Front,
    /// In the background, so that what was going to be shown still is.
    Background,
}

/// What State does in place of a launch: open the Device view on the main screen, show a group open in
/// shelf, or show the entry, which runs already.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Instead {
    Device,
    Group,
    Show,
}

impl Session {
    /// Shows running entry `id`: its workspace, if a window of it is there to see. If all its windows are
    /// on the bottom screen, the first of them is returned, for the bottom screen to show. Otherwise what
    /// is in front stays, and a notice says it has no window. It may run without one, not have shown one
    /// yet, or have lost its display, in which case its dock menu's Quit ends it. It never shows an empty
    /// screen.
    pub(crate) fn show(&mut self, id: &str, clock: &Clock, library: &Library, notice: &mut NoticeSlot, out: &mut Out) -> Option<i64> {
        let r = self.running(id)?;
        if self.windows_of(&r.id).next().is_some() {
            let down = self.windows_of(&r.id).all(|w| w.down).then(|| self.windows_of(&r.id).next().map(|w| w.id)).flatten();
            let group = r.workspace();
            self.cancel_front_launch();
            // If it or its runner is paused, it is thawed first, so that it draws as it is shown.
            self.thaw(id, out);
            self.resume_runner_of(id, library, out);
            if down.is_none() {
                self.focus(&group);
            }
            return down;
        }
        let title = r.entry.as_ref().map_or_else(|| id.to_owned(), |g| g.title.clone());
        // With no window but a tray icon, the program is asked to show itself, as for Steam started
        // silently.
        if !self.activate_tray(id, clock, out) {
            notice.show(NoticeKind::NoWindow(title), clock.mono());
        }
        None
    }

    /// Whether a launch of entry `id` was asked for and hasn't started.
    pub(crate) fn is_launching(&self, id: &str) -> bool {
        self.launching.iter().any(|l| &*l.entry == id)
    }

    /// Records a launch of `entry`, whose key is `key`, asked for now, and returns its instance. Started in
    /// front, its workspace is shown once its first window appears.
    fn begin_launch(&mut self, entry: Arc<str>, key: String, app_id: Option<Arc<str>>, clock: &Clock, start: Start) -> Instance {
        let instance = Instance::Run { key, secs: clock.unix(), n: self.next_instance() };
        self.launching.push(Launching { entry, instance: instance.clone(), app_id, at: clock.mono(), told: None });
        if start == Start::Front {
            self.set_front_launch(instance.clone(), None);
        }
        instance
    }

    /// Launches the entry `request` names, again in another tab if it runs, unless a launch of it is
    /// starting. It runs the request's command instead of its own if given, and a group can then be
    /// launched too. Before it starts, where shelf and the second screen (its tab `tab`) are is saved.
    ///
    /// What isn't launched is returned, for State to do instead: the Device view and a group are shown,
    /// and so is an entry that runs in another program's processes, since Steam doesn't start a game
    /// twice and Android doesn't start an app twice.
    pub(crate) fn launch(&mut self, request: LaunchRequest, shelf: &Shelf, tab: Tab, settings: &Settings, clock: &Clock, out: &mut Out) -> Option<Instead> {
        let LaunchRequest { entry, command, start } = request;
        let library = shelf.library();
        if let Some(Launch::Tool(tool)) = library.entry(&entry).map(|g| &g.launch) {
            // The Device view is ClamShell's own, on the main screen.
            if *tool == clamshell_library::Tool::Device {
                return Some(Instead::Device);
            }
            out.effects.push(Effect::Tool(*tool));
            return None;
        }
        if command.is_none() && library.entry(&entry).is_some_and(Entry::is_group) {
            return Some(Instead::Group);
        }
        if self.is_launching(&entry) {
            return None;
        }
        if self.running(&entry).is_some_and(|r| r.app_id.is_some()) {
            return Some(Instead::Show);
        }
        let g = library.entry(&entry)?;
        let app_id = app_id_of(g).filter(|_| command.is_none());
        let (launch, key) = (command.unwrap_or_else(|| g.launch.clone()), g.key.clone());
        let runner = g.runner.as_ref().map(|r| &*r.key);
        let values = settings.of_in(&entry, runner.as_slice());
        let mut env = Vec::new();
        settings.env_in(&entry, runner.as_slice(), &mut env);
        // If its runner, and so its container, is paused, it is resumed first.
        self.resume_runner_of(&entry, library, out);
        let instance = self.begin_launch(entry.clone(), key, app_id, clock, start);
        out.effects.push(Effect::Save(Save::Resume(Resume::at(shelf, tab))));
        out.effects.push(Effect::Launch { entry, instance, settings: values, env, launch });
        None
    }

    /// Quit, at `now`: asks the entry's windows to close. Once none is left, it is ended if it still runs,
    /// or its runner's stop is run if it ended with them (see [`Session::follow_quitting`]). A window such
    /// as "save changes?" keeps it until it is answered, for up to [`QUIT_WAIT`].
    pub(crate) fn quit(&mut self, entry: &Arc<str>, now: u64, library: &Library, out: &mut Out) {
        // If it is paused, it is thawed so that it can answer its windows' closing.
        self.thaw(entry, out);
        let windows: Vec<i64> = self.windows_of(entry).map(|w| w.id).collect();
        let stop = self.stop_of(entry, library);
        self.quitting.retain(|q| q.entry != *entry);
        self.quitting.push(crate::session::Quitting { entry: entry.clone(), ended: false, stop, asked: now });
        if !windows.is_empty() {
            out.effects.push(Effect::Close(windows));
        }
        self.follow_quitting(now, library, out);
    }

    /// Force kill: ends the entry at once.
    pub(crate) fn force_kill(&mut self, entry: &str, library: &Library, out: &mut Out) {
        self.quitting.retain(|q| &*q.entry != entry);
        self.thaw(entry, out);
        self.end(entry, true, library, out);
    }

    /// Ends a running entry. Its slice is stopped, or killed with `force`; this also covers a Steam game,
    /// which was moved into its launch's scope. Its runner's `stop` command is run, with `{id}` as its app
    /// id.
    fn end(&self, entry: &str, force: bool, library: &Library, out: &mut Out) {
        let Some(r) = self.running(entry) else {
            return;
        };
        // Found and not moved into a scope of ours, it is ended by its process, if that is its own.
        if r.found() {
            if let Some(pid) = r.pid() {
                out.effects.push(Effect::EndProcesses { pids: vec![pid], force });
            }
            return;
        }
        out.effects.push(Effect::EndRun { key: r.key.clone(), force });
        out.effects.extend(self.stop_of(entry, library));
    }

    /// The runner's `stop` command for a running entry known by its app id, with `{id}` replaced by the
    /// app id.
    fn stop_of(&self, entry: &str, library: &Library) -> Option<Effect> {
        let app_id = self.running(entry)?.app_id.as_ref()?;
        let stop = self.entry(library, entry)?.runner.as_ref().map(|o| &o.stop).filter(|s| !s.is_empty())?;
        Some(Effect::RunnerCommand(stop.iter().map(|a| a.replace("{id}", app_id)).collect()))
    }

    /// Follows the entries quitting. One whose windows are all gone while it still runs is ended, once. One
    /// that no longer runs is done, and if it ended with its windows before being ended, its runner's stop
    /// is run. One whose windows are still there [`QUIT_WAIT`] after they were asked to close, because the
    /// program refused, as with "save changes?" cancelled, runs on as before.
    pub(crate) fn follow_quitting(&mut self, now: u64, library: &Library, out: &mut Out) {
        let mut quitting = std::mem::take(&mut self.quitting);
        quitting.retain_mut(|q| {
            let runs = self.running(&q.entry).is_some();
            if !runs && !q.ended {
                out.effects.extend(q.stop.take());
            }
            runs
        });
        for q in &mut quitting {
            if !q.ended && self.windows_of(&q.entry).next().is_none() {
                q.ended = true;
                self.end(&q.entry, false, library, out);
            }
        }
        quitting.retain(|q| q.ended || now < q.asked + QUIT_WAIT);
        self.quitting = quitting;
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use crate::session::running::HANDOFF;
    use crate::testkit::*;
    use crate::*;

    fn launches(fx: &[Effect], id: &str) -> bool {
        fx.iter().any(|e| matches!(e, Effect::Launch { entry, .. } if &**entry == id))
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
        s.applied(Event::Tick { unix_secs: 100_000, mono_secs: 1000 });
        crate::testkit::start(&mut s, &first);
        // A minute later by the monotonic clock, with the wall clock set forward a day.
        s.applied(Event::Tick { unix_secs: 100_000 + 86_400 + 60, mono_secs: 1060 });
        let fx = crate::testkit::end(&mut s, &first);
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
        s.applied(Event::tick(1000));
        crate::testkit::start(&mut s, &first);
        let started = s.session().units().first().cloned().unwrap();
        assert_eq!(tap(&mut s, Id::Shelf(ShelfId::Launch)), vec![], "running, no window yet: nothing to show");
        s.observed(vec![win_of(3, "game", &started)]);
        assert_eq!(tap(&mut s, Id::Shelf(ShelfId::Launch)), vec![Effect::PadRoute(crate::PadRoute::Game)]);
        assert_eq!(s.session().top_window().map(|w| w.id), Some(3), "running: shown");
        // Another launch, as from the dock's Open another, runs it twice until both end.
        assert!(launches(&launch_now(&mut s, &first), &first));
        crate::testkit::start(&mut s, &first);
        // The first launch's scope is gone and the second's is listed.
        let second: Vec<Instance> = s.session().units().iter().skip(1).cloned().collect();
        s.applied(Event::Units(second));
        assert!(s.session().running(&first).is_some(), "the other still runs");
        s.applied(Event::tick(1660));
        let fx = crate::testkit::end(&mut s, &first);
        assert!(s.session().running(&first).is_none());
        assert!(fx.contains(&Effect::RecordPlay { entry: first.clone(), start: 1000, duration: 660 }), "{fx:?}");
        let g = s.entry(&first).unwrap();
        assert_eq!((g.plays.count, g.plays.seconds), (1, 660), "one play: from its first start to its last end");
        assert!(s.shelf().library().collections.iter().any(|c| c.name == "Recent"));
    }

    #[test]
    fn entries_run_side_by_side() {
        let mut s = state();
        s.applied(Event::tick(100));
        let zelda = launch(&mut s, "zelda-lttp");
        assert_eq!(zelda, Instance::Run { key: key("zelda-lttp"), secs: 100, n: 0 });
        tap(&mut s, entry("ff6"));
        assert!(launches(&tap(&mut s, Id::Shelf(ShelfId::Launch)), "ff6"), "no choice: it starts");
        crate::testkit::start(&mut s, "ff6");
        assert_eq!(s.session().running_entries().len(), 2);
    }

    #[test]
    fn quit_closes_the_windows_then_ends_the_slice_force_kill_at_once() {
        let mut s = state();
        let zelda = launch(&mut s, "zelda-lttp");
        let window = |id, title: &str| WindowFacts { title: title.into(), ..win_of(id, "zelda3", &zelda) };
        s.observed(vec![window(1, "game")]);
        let id: std::sync::Arc<str> = "zelda-lttp".into();
        assert_eq!(quit(&mut s, &id), vec![Effect::Close(vec![1])]);
        let fx = s.observed(vec![window(2, "Save changes?")]);
        assert!(!fx.iter().any(|e| matches!(e, Effect::EndRun { .. })), "a window left: waits");
        let fx = s.observed(vec![]);
        assert!(fx.contains(&Effect::EndRun { key: key("zelda-lttp"), force: false }), "{fx:?}");
        assert_eq!(force_kill(&mut s, "zelda-lttp"), vec![Effect::EndRun { key: key("zelda-lttp"), force: true }]);
    }

    /// A quit the program refuses, its "save changes?" answered with cancel, is given up once its windows
    /// are still there [`super::QUIT_WAIT`] after they were asked to close: the entry runs on, is paused
    /// hidden again, and isn't ended when its windows close later.
    #[test]
    fn a_quit_refused_is_given_up() {
        use crate::settings::Choice;
        let mut s = state();
        let zelda = launch(&mut s, "zelda-lttp");
        let key = key("zelda-lttp");
        s.settings_mut().set(Some(&"zelda-lttp".into()), crate::settings::Setting::Background, Some(crate::settings::InBackground::Pauses.value()));
        s.applied(Event::tick(100));
        s.observed(vec![win_of(1, "zelda3", &zelda)]);
        quit(&mut s, "zelda-lttp");
        s.applied(Event::Home);
        s.applied(Event::tick(100 + super::QUIT_WAIT - 1));
        assert!(s.session().is_quitting("zelda-lttp"), "still waiting");
        s.applied(Event::tick(100 + super::QUIT_WAIT));
        assert!(!s.session().is_quitting("zelda-lttp"), "given up");
        s.applied(Event::tick(101 + super::QUIT_WAIT));
        let fx = s.applied(Event::tick(101 + super::QUIT_WAIT + crate::session::background::GRACE));
        assert!(fx.contains(&Effect::Freeze { slice: Instance::slice_of(&key), frozen: true }), "paused hidden again: {fx:?}");
        let fx = s.observed(vec![]);
        assert!(!fx.iter().any(|e| matches!(e, Effect::EndRun { .. })), "{fx:?}");
        assert!(s.session().running("zelda-lttp").is_some());
    }

    /// A launch the launcher never answers is given up after [`super::LAUNCH_WAIT`], says it didn't start,
    /// and can be launched again.
    #[test]
    fn a_launch_never_answered_is_given_up() {
        let mut s = state();
        s.applied(Event::tick(100));
        let fx: Vec<Effect> = tap(&mut s, entry("zelda-lttp")).into_iter().chain(tap(&mut s, entry("zelda-lttp"))).collect();
        assert!(launches(&fx, "zelda-lttp"));
        s.applied(Event::tick(100 + super::LAUNCH_WAIT - 1));
        assert!(s.session().is_launching("zelda-lttp"));
        s.applied(Event::tick(100 + super::LAUNCH_WAIT));
        assert!(!s.session().is_launching("zelda-lttp"));
        assert!(matches!(notice(&s), Some(crate::bottom::notice::NoticeKind::NotStarted(t)) if t == "A Link to the Past"), "{:?}", notice(&s));
        assert_eq!(s.session().front_launch(), None);
        assert!(launches(&tap(&mut s, entry("zelda-lttp")), "zelda-lttp"), "launched again");
    }

    /// A launch given up that starts after all runs as a launch of ours, though another launch of the
    /// entry waits meanwhile; until then its windows would be taken for a program found by them, with its
    /// scope listed.
    #[test]
    fn a_launch_given_up_that_starts_late_runs_beside_the_one_waiting() {
        let mut s = state();
        s.applied(Event::tick(100));
        let instance = |fx: &[Effect]| {
            fx.iter().find_map(|e| match e {
                Effect::Launch { instance, .. } => Some(instance.clone()),
                _ => None,
            })
        };
        let first = instance(&launch_now(&mut s, "desktop:foot")).unwrap();
        s.applied(Event::tick(100 + super::LAUNCH_WAIT));
        assert!(!s.session().is_launching("desktop:foot"), "given up");
        let second = instance(&launch_now(&mut s, "desktop:foot")).unwrap();
        s.applied(Event::Units(vec![first.clone()]));
        let running = s.session().running("desktop:foot").map(|r| r.origin.clone());
        assert_eq!(running, Some(Origin::Ours), "runs by the scope that started late");
        let fx = s.observed(vec![win_of(5, "foot", &first)]);
        assert!(!fx.iter().any(|e| matches!(e, Effect::Adopt { .. })), "its window is its own: {fx:?}");
        assert!(s.session().is_launching("desktop:foot"));
        s.applied(Event::Units(vec![first, second]));
        assert!(!s.session().is_launching("desktop:foot"));
        assert_eq!(s.session().running_entries().iter().filter(|r| &*r.id == "desktop:foot").count(), 1);
    }

    /// The launcher's failure coming after the launch was given up doesn't say "didn't start" a second
    /// time, nor start a browsed file's wait for its window.
    #[test]
    fn a_launch_failure_after_it_was_given_up_says_nothing() {
        let mut s = state();
        s.applied(Event::tick(100));
        tap(&mut s, entry("zelda-lttp"));
        tap(&mut s, entry("zelda-lttp"));
        s.applied(Event::tick(100 + super::LAUNCH_WAIT));
        assert!(matches!(notice(&s), Some(crate::bottom::notice::NoticeKind::NotStarted(_))), "{:?}", notice(&s));
        s.applied(Event::tick(200));
        assert_eq!(notice(&s), None);
        s.applied(LaunchEvent::LaunchFailed("zelda-lttp".into()).into());
        assert_eq!(notice(&s), None, "said once");
        assert!(s.session().running("zelda-lttp").is_none());
        let mut s = files();
        s.applied(Event::tick(100));
        tap(&mut s, Id::Shelf(ShelfId::Launch));
        s.applied(Event::tick(100 + super::LAUNCH_WAIT));
        s.applied(LaunchEvent::LaunchFailed("browse:Home/notes.md".into()).into());
        assert!(s.session().running("browse:Home/notes.md").is_none(), "no wait for a window");
    }

    #[test]
    fn a_game_steam_started_is_followed_as_a_launch_beside_the_others() {
        let mut s = steam_ff6();
        s.applied(Event::tick(1000));
        let zelda = launch(&mut s, "zelda-lttp");
        let windows = vec![win_of(1, "zelda3", &zelda), win(7, "steam_app_620")];
        let fx = s.observed(windows.clone());
        // Found by its window, it is launched; the launcher finds it running and moves it into the
        // launch's scope. Its window is placed synchronously before the scope is listed.
        assert!(launches(&fx, "ff6"), "{fx:?}");
        let ff6 = Instance::workspace_of(&key("ff6"));
        assert_eq!(s.session().windows().iter().find(|w| w.id == 7).map(|w| w.workspace.as_str()), Some(ff6.as_str()));
        assert!(!launches(&s.observed(windows), "ff6"), "once");
        crate::testkit::start(&mut s, "ff6");
        assert_eq!(s.session().running_entries().len(), 2, "the other keeps running");
        let fx = s.observed(vec![win_of(1, "zelda3", &zelda), win(7, "steam_app_620")]);
        assert!(fx.is_empty(), "already placed: {fx:?}");
        // It is ended like any launch, through its slice.
        assert_eq!(force_kill(&mut s, "ff6"), vec![Effect::EndRun { key: key("ff6"), force: true }]);
        s.applied(Event::tick(1600));
        s.observed(vec![win_of(1, "zelda3", &zelda)]);
        let fx = crate::testkit::end(&mut s, "ff6");
        assert!(fx.contains(&Effect::RecordPlay { entry: "ff6".into(), start: 1000, duration: 600 }), "{fx:?}");
        assert!(s.session().running("ff6").is_none());
        // A Steam game the library doesn't have isn't one.
        let fx = s.observed(vec![win_of(1, "zelda3", &zelda), win(9, "steam_app_1")]);
        assert!(!fx.iter().any(|e| matches!(e, Effect::Launch { .. })), "{fx:?}");
        assert_eq!(s.session().running_entries().len(), 1, "zelda only");
        // A game started by a Steam of ours is in Steam's scope, but the app id says whose it is.
        let steam = Instance::Run { key: key("desktop:steam"), secs: 1, n: 0 };
        let in_steam = win_of(8, "steam_app_620", &steam);
        let fx = s.observed(vec![win_of(1, "zelda3", &zelda), in_steam]);
        assert!(launches(&fx, "ff6"), "{fx:?}");
    }

    #[test]
    fn a_steam_game_shelf_started_ends_by_the_launcher_not_its_windows() {
        let mut s = steam_ff6();
        launch(&mut s, "ff6");
        let ff6 = s.session().running("ff6").unwrap();
        assert_eq!((ff6.origin.clone(), ff6.app_id.as_deref()), (Origin::Ours, Some("steam_app_620")));
        s.observed(vec![win(7, "steam_app_620")]);
        let ff6 = Instance::workspace_of(&key("ff6"));
        assert_eq!(s.session().window(7).map(|w| w.workspace.as_str()), Some(ff6.as_str()), "its windows go to its workspace");
        let fx = s.observed(vec![]);
        assert!(!fx.iter().any(|e| matches!(e, Effect::RecordPlay { .. })), "{fx:?}");
        assert!(s.session().running("ff6").is_some());
    }

    #[test]
    fn a_steam_games_window_is_the_games_though_it_runs_in_steams_scope() {
        let mut s = steam_ff6();
        let steam = Instance::Run { key: key("desktop:steam"), secs: 1, n: 0 };
        s.applied(Event::Units(vec![steam.clone()]));
        launch(&mut s, "ff6");
        let window = win_of(7, "steam_app_620", &steam);
        s.observed(vec![window.clone()]);
        let ff6 = Instance::workspace_of(&key("ff6"));
        assert_eq!(s.session().window(7).map(|w| w.workspace.as_str()), Some(ff6.as_str()), "the game's, not Steam's");
        assert_eq!(s.session().running_of(&window).map(|r| &*r.id), Some("ff6"));
    }

    /// While a Steam game's launch waits for its window, Steam's own windows (raised by the launch, or a
    /// dialog of Steam's) open behind; the game's comes in front.
    #[test]
    fn steams_windows_open_behind_a_steam_games_launch() {
        let mut s = steam_ff6();
        let steam = Instance::Run { key: key("desktop:steam"), secs: 1, n: 0 };
        s.applied(Event::Units(vec![steam.clone()]));
        let client = win_of(5, "steam", &steam);
        s.observed(vec![client.clone()]);
        s.applied(crate::WindowEvent::Activate(5).into());
        launch(&mut s, "ff6");
        assert!(!s.session().opening_focus(&WindowFacts { id: 6, ..client.clone() }), "Steam's window");
        assert!(!s.session().opening_focus(&WindowFacts { id: 6, parent: Some(5), ..client.clone() }), "a dialog of Steam's");
        let game = win_of(7, "steam_app_620", &steam);
        assert!(s.session().opening_focus(&game), "the game's, though in Steam's scope");
    }

    /// Another tool than the Device view is asked of the system; an id the library doesn't have plays
    /// nothing.
    #[test]
    fn play_of_a_tool_asks_for_it_and_of_an_unknown_id_does_nothing() {
        let mut library = clamshell_library::demo::library();
        library.entries.push(Entry { launch: Launch::Tool(clamshell_library::Tool::Rescan), ..Entry::new("tool:rescan", "Rescan") });
        let mut s = state_with(library);
        let fx = s.applied(Event::Entry("tool:rescan".into(), EntryAct::Play));
        assert_eq!(fx, vec![Effect::Tool(clamshell_library::Tool::Rescan)]);
        assert!(!s.session().is_launching("tool:rescan") && s.session().running("tool:rescan").is_none());
        assert_eq!(s.applied(Event::Entry("nothing-such".into(), EntryAct::Play)), vec![]);
        assert!(s.session().launching().is_empty());
        assert_eq!(notice(&s), None);
    }

    /// A Steam game's launch that failed may still come by its window, as when Steam started it anyway:
    /// it waits [`HANDOFF`] for one, which then makes it run, and without one it is gone with no play.
    #[test]
    fn a_failed_launch_known_by_app_id_waits_for_its_window() {
        let mut s = steam_ff6();
        s.ui_up();
        s.applied(Event::tick(1000));
        tap(&mut s, entry("ff6"));
        tap(&mut s, entry("ff6"));
        s.applied(LaunchEvent::LaunchFailed("ff6".into()).into());
        let ff6 = s.session().running("ff6").unwrap();
        assert_eq!(ff6.origin, Origin::Windows { since: 1000, until: 1000 + HANDOFF });
        assert_eq!(notice(&s).map(|n| matches!(n, crate::bottom::notice::NoticeKind::NotStarted(_))), Some(false), "not said to have failed");
        s.applied(Event::tick(1010));
        s.observed(vec![win(7, "steam_app_620")]);
        assert_eq!(s.session().window(7).map(|w| w.workspace.as_str()), Some(Instance::workspace_of(&key("ff6")).as_str()));
        s.applied(Event::tick(1100));
        assert!(s.session().running("ff6").is_some(), "runs by its window");
        let fx = s.observed(vec![]);
        assert!(fx.contains(&Effect::RecordPlay { entry: "ff6".into(), start: 1000, duration: 100 }), "{fx:?}");
        // Without a window it is gone once the wait is over, with no play.
        tap(&mut s, entry("ff6"));
        s.applied(LaunchEvent::LaunchFailed("ff6".into()).into());
        s.applied(Event::tick(1100 + HANDOFF));
        assert!(s.session().running("ff6").is_some());
        let fx = s.applied(Event::tick(1101 + HANDOFF));
        assert!(s.session().running("ff6").is_none());
        assert!(!fx.iter().any(|e| matches!(e, Effect::RecordPlay { .. })), "{fx:?}");
    }

    /// A Steam game whose launch's scope ends while its window is open, long after it started, runs on by
    /// its window. Its play, recorded when the window closes, is from its launch.
    #[test]
    fn a_game_known_by_app_id_runs_on_by_its_windows_after_its_scope() {
        let mut s = steam_ff6();
        s.ui_up();
        s.applied(Event::tick(1000));
        launch(&mut s, "ff6");
        s.observed(vec![win(7, "steam_app_620")]);
        s.applied(Event::tick(1600));
        let fx = crate::testkit::end(&mut s, "ff6");
        assert!(!fx.iter().any(|e| matches!(e, Effect::RecordPlay { .. })), "{fx:?}");
        let ff6 = s.session().running("ff6").unwrap();
        assert!(matches!(ff6.origin, Origin::Windows { .. }) && ff6.seen, "{ff6:?}");
        s.applied(Event::tick(1900));
        let fx = s.observed(vec![]);
        assert!(fx.contains(&Effect::RecordPlay { entry: "ff6".into(), start: 1000, duration: 900 }), "{fx:?}");
        assert!(s.session().running("ff6").is_none());
    }

    /// Quit or force kill of an entry that doesn't run does nothing, and leaves nothing quitting.
    #[test]
    fn quit_and_force_kill_of_an_entry_not_running_do_nothing() {
        let mut s = state();
        assert_eq!(s.applied(Event::Entry("zelda-lttp".into(), EntryAct::Quit)), vec![]);
        assert!(!s.session().is_quitting("zelda-lttp"));
        assert_eq!(s.applied(Event::Entry("zelda-lttp".into(), EntryAct::ForceKill)), vec![]);
    }

    /// The program in force is the focused window's owner; with shelf or another window in front it is
    /// kept while it runs; once it ends, the entry started last that runs takes over, then none.
    #[test]
    fn the_program_in_force_falls_back_to_the_last_started() {
        let mut s = state();
        s.applied(Event::tick(1000));
        let zelda = launch(&mut s, "zelda-lttp");
        let ff6 = launch(&mut s, "ff6");
        let program = |s: &State| s.session().program().map(|p| p.to_string());
        let zelda_window = win_of(3, "zelda3", &zelda);
        let ff6_window = win_of(4, "ff6", &ff6);
        s.observed(vec![zelda_window.clone(), ff6_window.clone()]);
        s.applied(crate::WindowEvent::Activate(3).into());
        assert_eq!(program(&s).as_deref(), Some("zelda-lttp"), "in front");
        s.applied(Event::Home);
        assert_eq!(program(&s).as_deref(), Some("zelda-lttp"), "kept with shelf in front");
        s.observed(vec![foot(), zelda_window, ff6_window.clone()]);
        assert_eq!(program(&s).as_deref(), Some("zelda-lttp"), "kept with another window in front");
        s.observed(vec![foot(), ff6_window]);
        crate::testkit::end(&mut s, "zelda-lttp");
        assert_eq!(program(&s).as_deref(), Some("ff6"), "the last started that runs");
        s.observed(vec![foot()]);
        crate::testkit::end(&mut s, "ff6");
        assert_eq!(program(&s), None);
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
        s.applied(ShelfEvent::ListStep(1).into());
        let fx = tap(&mut s, Id::Shelf(ShelfId::Launch));
        assert!(launches(&fx, "zelda-lttp"), "{fx:?}");
    }

    #[test]
    fn a_file_handed_to_another_program_runs_by_its_window() {
        let mut s = files();
        s.applied(Event::tick(100));
        let other = win_of(5, "steam", &launch_of("steam"));
        s.observed(vec![other.clone()]);
        let fx = tap(&mut s, Id::Shelf(ShelfId::Launch));
        let ws = fx.iter().find_map(|e| match e {
            Effect::Launch { instance, .. } => Some(instance.workspace()),
            _ => None,
        });
        let ws = ws.unwrap_or_default();
        assert!(!ws.is_empty(), "{fx:?}");
        // xdg-open ended before its scope was seen; the app's window comes up with no group.
        s.applied(LaunchEvent::LaunchFailed("browse:Home/notes.md".into()).into());
        assert!(s.session().running("browse:Home/notes.md").is_some(), "waits for its window");
        // The demo's Terminal has foot's desktop file.
        let app = win(6, "foot");
        s.observed(vec![other.clone(), app.clone()]);
        assert_eq!(s.session().window(6).map(|w| w.workspace.as_str()), Some(ws.as_str()));
        assert_eq!(s.session().window(other.id).map(|w| w.workspace.as_str()), Some(launch_of("steam").workspace().as_str()), "only the new one");
        assert_eq!(s.session().top_window().map(|w| w.id), Some(6));
        s.observed(vec![other.clone(), app]);
        s.applied(Event::tick(200));
        assert!(s.session().running("browse:Home/notes.md").is_some(), "runs while its window is open");
        assert_eq!(s.session().app_showing("browse:Home/notes.md", s.shelf().library()).map(|g| &*g.id), Some("desktop:foot"), "its picture: the app's");
        // With its file gone from the library meanwhile, it is still shown by what it was.
        s.applied(Event::Library(Box::new(clamshell_library::demo::library())));
        assert!(s.shelf().entry("browse:Home/notes.md").is_none());
        assert_eq!(s.entry("browse:Home/notes.md").map(|g| g.title.as_str()), Some("/h/notes.md"));
        s.applied(Event::Library(Box::new(files().shelf().library().clone())));
        s.observed(vec![other.clone()]);
        assert!(s.session().running("browse:Home/notes.md").is_none(), "its window closed: ended");
        // Another launch that never shows a window is gone after a while.
        // (The library read again left shelf on another collection: the file is selected first.)
        while s.shelf().selected().map(|g| &**g) != Some("browse:Home/notes.md") {
            s.applied(ShelfEvent::CollectionStep(1).into());
        }
        assert!(launches(&tap(&mut s, Id::Shelf(ShelfId::Launch)), "browse:Home/notes.md"));
        s.applied(LaunchEvent::LaunchFailed("browse:Home/notes.md".into()).into());
        s.applied(Event::tick(200 + HANDOFF));
        assert!(s.session().running("browse:Home/notes.md").is_some());
        s.applied(Event::tick(201 + HANDOFF));
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
        s.applied(Event::tick(100));
        s.observed(vec![]);
        let ws = Instance::workspace_of(&key("desktop:firefox"));
        assert!(launches(&launch_now(&mut s, "desktop:firefox"), "desktop:firefox"));
        crate::testkit::start(&mut s, "desktop:firefox");
        // Its launch handed the app over to the Waydroid session and ended.
        crate::testkit::end(&mut s, "desktop:firefox");
        assert!(s.session().running("desktop:firefox").is_some(), "waits for its window");
        let app = win(6, "firefox");
        s.observed(vec![app.clone()]);
        assert_eq!(s.session().window(6).map(|w| w.workspace.as_str()), Some(ws.as_str()));
        s.observed(vec![app.clone()]);
        assert!(!launches(&launch_now(&mut s, "desktop:firefox"), "desktop:firefox"), "it runs once: shown");
        s.applied(Event::tick(400));
        assert!(s.session().running("desktop:firefox").is_some(), "runs while its window is open");
        let fx = force_kill(&mut s, "desktop:firefox");
        assert!(fx.contains(&Effect::RunnerCommand(vec!["stop".into(), "firefox".into()])), "its runner's stop: {fx:?}");
        let fx = s.observed(vec![]);
        assert!(s.session().running("desktop:firefox").is_none(), "its window closed: ended");
        assert!(fx.contains(&Effect::RecordPlay { entry: "desktop:firefox".into(), start: 100, duration: 300 }), "played until then: {fx:?}");
        // Opened by another app, it runs, and its window goes to its workspace and is shown.
        s.observed(vec![app.clone()]);
        assert!(s.session().running("desktop:firefox").is_some());
        assert_eq!(s.session().window(6).map(|w| w.workspace.as_str()), Some(ws.as_str()));
        assert_eq!(s.session().top_window().map(|w| w.id), Some(6), "shown in the same event");
        // Quit closes its window. It ends with the window, and its runner's stop runs then, once.
        let stop = Effect::RunnerCommand(vec!["stop".into(), "firefox".into()]);
        let fx = quit(&mut s, "desktop:firefox");
        assert!(fx.contains(&Effect::Close(vec![6])) && !fx.contains(&stop), "{fx:?}");
        let fx = s.observed(vec![]);
        assert!(fx.contains(&stop), "{fx:?}");
        assert!(!s.observed(vec![]).contains(&stop), "once");
        s.observed(vec![app]);
        // A window of no entry and no launch of ours gets a group of its own on the main screen.
        s.observed(vec![win(9, "gone")]);
        assert_eq!(s.session().windows().iter().find(|w| w.id == 9).map(|w| w.workspace.as_str()), Some("clamshell-window-9"));
        // An entry of another runner ends with its launch.
        let mut s = state();
        launch_now(&mut s, "desktop:firefox");
        crate::testkit::start(&mut s, "desktop:firefox");
        crate::testkit::end(&mut s, "desktop:firefox");
        assert!(s.session().running("desktop:firefox").is_none());
    }

    #[test]
    fn entries_started_before_clamshell_are_found_by_their_scopes() {
        let mut s = state();
        s.applied(Event::tick(1000));
        let foot = Instance::Run { key: key("desktop:foot"), secs: 400, n: 0 };
        s.applied(Event::Units(vec![foot.clone()]));
        assert!(s.session().running("desktop:foot").is_some(), "adopted");
        s.observed(vec![win_of(4, "foot", &foot)]);
        assert_eq!(tap(&mut s, Id::Dock(DockId::Entry(0))), vec![Effect::PadRoute(crate::PadRoute::Game)]);
        assert_eq!(s.session().top_window().map(|w| w.id), Some(4), "on the dock, shown");
        s.applied(Event::tick(1300));
        let fx = s.applied(Event::Units(vec![]));
        assert!(s.session().running("desktop:foot").is_none(), "its scopes gone: ended");
        assert!(fx.contains(&Effect::RecordPlay { entry: "desktop:foot".into(), start: 400, duration: 900 }), "played since its launch: {fx:?}");
    }

    #[test]
    fn a_launch_runs_while_a_scope_of_it_is_listed() {
        let mut s = state();
        s.applied(Event::tick(100));
        let fx = tap(&mut s, entry("zelda-lttp"));
        let fx: Vec<Effect> = fx.into_iter().chain(tap(&mut s, entry("zelda-lttp"))).collect();
        let zelda = fx.iter().find_map(|e| match e {
            Effect::Launch { instance, .. } => Some(instance.clone()),
            _ => None,
        });
        let zelda = zelda.unwrap();
        assert!(s.session().running("zelda-lttp").is_none(), "asked for, its scope not there yet: starting");
        // While its scope is listed it runs. Once it is no longer listed, it has ended and its play is
        // recorded once.
        s.applied(Event::Units(vec![zelda.clone()]));
        assert!(s.session().running("zelda-lttp").is_some());
        s.applied(Event::tick(400));
        let fx = s.applied(Event::Units(vec![]));
        assert!(s.session().running("zelda-lttp").is_none());
        assert!(fx.contains(&Effect::RecordPlay { entry: "zelda-lttp".into(), start: 100, duration: 300 }), "once: {fx:?}");
        assert!(!s.applied(Event::Units(vec![])).iter().any(|e| matches!(e, Effect::RecordPlay { .. })), "once");
    }

    /// Show, as from shelf, of an entry whose windows are all on the bottom screen shows them there, as a
    /// tap on its dock tile does, and doesn't focus its empty workspace on the main screen.
    #[test]
    fn show_of_an_entry_whose_windows_are_all_below_shows_them_there() {
        let mut s = state();
        let zelda = launch(&mut s, "zelda-lttp");
        s.observed(vec![win_of(3, "zelda3", &zelda)]);
        s.applied(Event::Device(crate::device::buttons::DeviceKey::WindowDown));
        s.applied(Event::Device(crate::device::buttons::DeviceKey::BottomView));
        assert!(!s.bottom().shows_windows(s.session()));
        show(&mut s, "zelda-lttp");
        assert!(s.bottom().shows_windows(s.session()));
        assert_eq!(s.session().bottom_window().map(|w| w.id), Some(3));
        assert!(s.session().shelf_in_front());
        s.applied(Event::Device(crate::device::buttons::DeviceKey::BottomView));
        assert!(!s.bottom().shows_windows(s.session()));
        tap(&mut s, Id::Dock(DockId::Entry(0)));
        assert!(s.bottom().shows_windows(s.session()), "the dock tile's tap: the same");
        assert!(s.session().shelf_in_front());
        // The bottom screen turned off by hand comes on again for them.
        s.applied(Event::Device(crate::device::buttons::DeviceKey::BottomPower));
        assert!(s.bottom().off());
        s.applied(Event::Device(crate::device::buttons::DeviceKey::BottomView));
        tap(&mut s, Id::Dock(DockId::Entry(0)));
        assert!(!s.bottom().off() && s.bottom().shows_windows(s.session()), "on again");
        // With a window of it on the main screen, that is shown.
        s.observed(vec![win_of(3, "zelda3", &zelda), win_of(4, "second", &zelda)]);
        show(&mut s, "zelda-lttp");
        assert_eq!(s.session().top_window().map(|w| w.id), Some(4));
    }

    #[test]
    fn the_settings_follow_the_front() {
        let mut s = state();
        let zelda = launch(&mut s, "zelda-lttp");
        let in_front = |s: &mut State, w: WindowFacts| {
            let id = w.id;
            s.observed(vec![w]);
            s.applied(crate::WindowEvent::Activate(id).into())
        };
        tap(&mut s, entry("ff6"));
        let id = |s: &State| s.panel().context().map(|g| g.id.to_string());
        in_front(&mut s, win_of(2, "zelda3", &zelda));
        assert_eq!(id(&s).as_deref(), Some("zelda-lttp"), "the entry in front");
        s.applied(Event::Home);
        assert_eq!(id(&s).as_deref(), Some("ff6"), "shelf in front: its selection");
        in_front(&mut s, win(3, "foot"));
        assert_eq!(s.panel().context(), None, "a window of none of ours: the defaults");
    }
}
