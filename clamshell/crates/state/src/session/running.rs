//! Running entries: what was launched from shelf or the dock, games and apps alike. Each launch is a unit
//! of its own in the entry's slice, with its windows in the entry's group (see [`Instance`]). An entry
//! may run more than once; another launch is another unit and another tab.
//!
//! Some entries run in another program's processes, not in a unit of ours, and are known by their
//! windows' app id ([`app_id_of`]). These are Steam's games (`steam_app_<id>`, in Steam's scope) and the
//! entries of a runner that hands its apps over, such as Waydroid's, whose app id is their desktop file's
//! name and which run in the Waydroid session. Their windows are put in their entry's group as they
//! open. A window that opens while the entry doesn't run, such as a game started in Steam or an app opened by another app, starts the entry.
//! Such an entry runs once, so another launch shows it instead. It runs on by its windows after its launch
//! ends, until the last one closes.
//!
//! A file opened from the Files tab, a browsed entry, may have a launch that hands it to a program not
//! ours and ends, as with `xdg-open` to an app's own instance or with D-Bus activation. It runs on by its
//! windows too: a new window of no launch of ours that appears within [`HANDOFF`] is moved to its
//! group, and the file has ended when that window has closed, or when none came.
//!
//! In shelf, activating a running entry shows it, and activating a stopped one starts it. Quit asks an
//! entry's windows to close and, once none is left and it still runs, ends it. That stops its slice, or for
//! a Steam game the processes under Steam's launcher for it, and runs its runner's `stop` command. Force
//! kill ends it at once. What is in front, whether shelf, a running entry or another window, decides where
//! the pad goes, what Home does and what the settings page edits.

use crate::Compositor;
use std::sync::Arc;

use clamshell_library::{Launch, ids};

use crate::bottom::notice::{NoticeKind, NoticeSlot};
use crate::effect::Effect;
use crate::out::Out;
use crate::session::found::window_entry;
use crate::session::instance::{Instance, STEAM_APP_PREFIX};
use crate::session::tray::{TrayItem, tray_entry};
use crate::session::windows::WindowCommand;
use crate::session::windows::{Group, Window, WindowFacts};
use crate::settings::Settings;
use crate::{Clock, Entry, Library, Session};

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
    /// when it was launched. It runs until `until`: before it has had windows ([`Running::seen`]), the
    /// time given for the first to come; while it has windows, for good; once they are gone, a moment
    /// longer ([`super::REOPEN_SECS`]), since a program may close its only window and open another (as
    /// Waydroid's apps do as they start). Its play from `since` is recorded when it ends. Its windows are
    /// those with its app id, or, for a browsed file, a new window of no launch of ours, taken until
    /// `until`.
    Windows { since: u64, until: u64 },
}

/// How long a quit waits for the windows to close, in seconds, before it is given up.
pub(crate) const QUIT_WAIT: u64 = 10;

/// How long a launch that handed over waits for its window, in seconds.
pub(crate) const HANDOFF: u64 = 15;

/// An entry that runs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Running {
    pub id: Arc<str>,
    /// Its key, which names its group and its launches' units and slice.
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
    pub(crate) fn fathered(&self, w: &WindowFacts) -> bool {
        self.pid().is_some_and(|pid| w.pids.contains(&pid))
    }

    /// Whether its windows are found other than by its unit, by app id or by process, and so are moved
    /// to its group wherever they open.
    pub fn outside(&self) -> bool {
        self.app_id.is_some() || self.pid().is_some()
    }

    /// Its group, which holds its windows.
    pub fn group(&self) -> Group {
        Group::entry(&self.key)
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

/// How long a launch waits for an answer from the launcher, its unit or its failure, in seconds, before
/// it is given up as not started.
pub(crate) const LAUNCH_WAIT: u64 = 30;

/// The app id an entry's windows have when it runs in another program's processes, which makes them its
/// windows wherever they open. For Steam's games it is `steam_app_<id>`. For an entry whose runner hands
/// its apps over, such as Waydroid's, it is the desktop file's name. It is none for the rest, which run in
/// units of ours.
pub(crate) fn app_id_of(g: &Entry) -> Option<Arc<str>> {
    match &g.launch {
        Launch::Steam { appid } => Some(format!("{STEAM_APP_PREFIX}{appid}").into()),
        _ => handed_over_app(g).map(Into::into),
    }
}

/// Whether `app_id` is [`app_id_of`] `g`, without building it, since this is asked for every window and
/// entry.
pub(crate) fn has_app_id(g: &Entry, app_id: &str) -> bool {
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
    /// Runs after systemd's list of our units changed, and after any event. A launch with no answer by
    /// [`LAUNCH_WAIT`] is given up, with a notice that it didn't start. An entry with units listed that
    /// isn't known to run, because an earlier ClamShell started it, runs from when its first unit
    /// started, by the time in the unit's name. One the library doesn't have, because the earlier
    /// ClamShell found it, runs as found again by its windows or tray icon ([`found_in_scope`]). An entry
    /// found by its tray icon whose process is now in a scope of ours runs as a launch. Then the entries
    /// whose launches have ended are handled (see [`Session::follow_ours`]). Returns whether a launch of
    /// ours started, which may let a second screen tapped awake go dark again.
    pub(crate) fn follow_units(&mut self, clock: &Clock, library: &Library, notice: &mut NoticeSlot, plays: &mut Vec<Play>) -> bool {
        let (now, unix) = (clock.mono(), clock.unix());
        // The entries of launches whose unit is there run as launches of ours, including one that was
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
            if self.front_launch.as_ref().is_some_and(|f| f.instance == l.instance) {
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
            if units.iter().any(|u| u.key == r.key.as_str()) {
                // Its windows are now its unit's, not every window of that app id or process. That would
                // tie another instance of the program, started later, to it.
                r.origin = Origin::Ours;
                r.app_id = None;
            }
        }
        for unit in units {
            let key = unit.key.as_str();
            // A launch of the entry waiting doesn't hold it off: this unit is another launch's, such as
            // one given up that started late, and the entry runs.
            if self.running.iter().any(|r| r.key == key) {
                continue;
            }
            let first = units.iter().filter_map(|u| match u {
                Instance { key: k, secs, .. } if k == key => Some(*secs),
                _ => None,
            });
            // Its units are named by the wall clock.
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

    /// Our launches' entries run while systemd lists a unit of theirs. A Steam game's unit holds its
    /// follower between Steam's setup and the game, and an entry started by an earlier ClamShell has its
    /// units listed too. Once there are none, an entry known by its app id whose windows are open or may
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

    /// A launch that never started, its unit gone before it was seen. A browsed file's launch, or one
    /// known by its app id, waits for its window (see [`Origin::Windows`]). A failure of no launch waiting
    /// is a late answer to one already given up ([`LAUNCH_WAIT`]), which said so then, and is ignored: a
    /// newer launch of the same entry waits on.
    pub(crate) fn launch_failed(&mut self, entry: Arc<str>, instance: &Instance, library: &Library, clock: &Clock, notice: &mut NoticeSlot) {
        let Some(i) = self.launching.iter().position(|l| l.entry == entry && l.instance == *instance) else {
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
    /// whose it is before its unit does, since a game Steam started is in Steam's scope. A Steam game is
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
        self.set_front_launch(Instance { key: r.key.clone(), secs: clock.unix(), n: 0 }, None);
        self.running.extend(new);
        true
    }

    /// Runs after every event, after the entries started by their windows' app ids
    /// ([`Session::start_by_app_id`]).
    ///
    /// - A running entry's windows found by app id go to its group.
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

    /// Moves windows found by app id or by process to their entry's group.
    fn place_owned_windows(&mut self) {
        let mut commands = Vec::new();
        for r in &self.running {
            let away: Vec<i64> = self.windows_of(&r.id).filter(|w| w.group.as_ref().and_then(Group::key) != Some(&r.key)).map(|w| w.id).collect();
            if !away.is_empty() {
                commands.push(WindowCommand::Move { windows: away, to: r.group() });
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
        let stray: Vec<i64> = self
            .windows
            .iter()
            .filter(|w| opened.contains(&w.id) && !w.launched() && !self.below(w.id) && self.running_of(w).is_none())
            .map(|w| w.id)
            .collect();
        let waiting = self
            .running
            .iter()
            .filter(|r| r.app_id.is_none() && !r.seen && matches!(r.origin, Origin::Windows { until, .. } if now <= until))
            .max_by_key(|r| r.start);
        let r = waiting.filter(|_| !stray.is_empty())?;
        let id = r.id.clone();
        let group = r.group();
        taken.extend(&stray);
        self.command(WindowCommand::Move { windows: stray, to: group });
        Some(id)
    }

    /// Places unowned windows after file handoffs and program discovery have had their chance. A
    /// starting launch already has its group, and is left alone. A service dialog uses the current
    /// program's group; without one it gets a group of its own so Home can still return to shelf.
    fn place_unowned(&mut self) {
        let starting = |w: &crate::session::windows::Window| self.launching.iter().any(|l| l.app_id.as_deref() == Some(w.app_id.as_str()));
        let ids: Vec<i64> = self.windows.iter().filter(|w| w.group.is_none() && self.running_of(w).is_none() && !starting(w)).map(|w| w.id).collect();
        for id in ids {
            let down = self.below(id);
            let group = self.top_window().filter(|_| !down).and_then(|w| w.group.clone()).unwrap_or(Group::Window(id));
            self.command(WindowCommand::Move { windows: vec![id], to: group });
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
                Origin::Windows { since, ref mut until } => {
                    // An entry with a unit listed has a launch of its own running.
                    let launched = units.iter().any(|u| u.key == r.key.as_str());
                    if r.seen {
                        *until = match (has, *until) {
                            (true, _) => u64::MAX,
                            (false, u64::MAX) => now + super::REOPEN_SECS,
                            (false, at) => at,
                        };
                    }
                    let runs = launched || has || now <= *until;
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

    /// Remembers the windows this pass saw and the group in front, for the next pass (see
    /// [`Session::claim_opened_windows`] and [`Session::follow_front`]).
    pub(crate) fn followed(&mut self) {
        self.opened.clear();
        let front = self.top_window().and_then(|w| w.group.as_ref());
        if self.followed_front.as_ref() != front {
            self.followed_front = front.cloned();
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
    /// Its group is shown once its first window appears.
    Front,
    /// In the background, so that what was going to be shown still is.
    Background,
}

/// What came of a launch asked for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Launched {
    Yes,
    /// Nothing is to be done: a launch of it is starting, a tool was run, or there is no such entry.
    No,
    /// What State does in place of a launch.
    Instead(Instead),
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
    /// Shows running entry `id`: its group, if a window of it is there to see. If all its windows are
    /// on the bottom screen, it returns true, for the bottom screen to show them. Otherwise what
    /// is in front stays, and a notice says it has no window. It may run without one, not have shown one
    /// yet, or have lost its display, in which case its dock menu's Quit ends it. It never shows an empty
    /// screen.
    pub(crate) fn show(&mut self, id: &str, clock: &Clock, library: &Library, notice: &mut NoticeSlot, out: &mut Out) -> bool {
        let Some(r) = self.running(id) else { return false };
        if self.windows_of(&r.id).next().is_some() {
            let down = self.windows_of(&r.id).all(|w| self.below(w.id));
            let group = r.group();
            self.cancel_front_launch();
            // If it or its runner is paused, it is thawed first, so that it draws as it is shown.
            self.thaw(id, out);
            self.resume_runner_of(id, library, out);
            if !down {
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
        false
    }

    /// Whether a launch of entry `id` was asked for and hasn't started.
    pub(crate) fn is_launching(&self, id: &str) -> bool {
        self.launching.iter().any(|l| &*l.entry == id)
    }

    /// Records a launch of `entry`, whose key is `key`, asked for now, and returns its instance. Started in
    /// front, its group is shown once its first window appears.
    fn begin_launch(&mut self, entry: Arc<str>, key: String, app_id: Option<Arc<str>>, clock: &Clock, start: Start) -> Instance {
        let instance = Instance { key, secs: clock.unix(), n: self.next_instance() };
        self.launching.push(Launching { entry, instance: instance.clone(), app_id, at: clock.mono(), told: None });
        if start == Start::Front {
            self.set_front_launch(instance.clone(), None);
        }
        instance
    }

    /// Launches the entry `request` names, again in another tab if it runs, unless a launch of it is
    /// starting. It runs the request's command instead of its own if given, and a group can then be
    /// launched too.
    ///
    /// What isn't launched says what State does instead: the Device view and a group are shown, and so is
    /// an entry that runs in another program's processes, since Steam doesn't start a game twice and
    /// Android doesn't start an app twice.
    pub(crate) fn launch(&mut self, request: LaunchRequest, library: &Library, settings: &Settings, clock: &Clock, out: &mut Out) -> Launched {
        let LaunchRequest { entry, command, start } = request;
        if let Some(Launch::Tool(tool)) = library.entry(&entry).map(|g| &g.launch) {
            // The Device view is ClamShell's own, on the main screen.
            if *tool == clamshell_library::Tool::Device {
                return Launched::Instead(Instead::Device);
            }
            out.effects.push(Effect::Tool(*tool));
            return Launched::No;
        }
        if command.is_none() && library.entry(&entry).is_some_and(Entry::is_group) {
            return Launched::Instead(Instead::Group);
        }
        if self.is_launching(&entry) {
            return Launched::No;
        }
        if self.running(&entry).is_some_and(|r| r.app_id.is_some()) {
            return Launched::Instead(Instead::Show);
        }
        let Some(g) = library.entry(&entry) else {
            return Launched::No;
        };
        let app_id = app_id_of(g).filter(|_| command.is_none());
        let (launch, key) = (command.unwrap_or_else(|| g.launch.clone()), g.key.clone());
        let runner = g.runner.as_ref().map(|r| &*r.key);
        let values = settings.of_in(&entry, runner.as_slice());
        // The cap's variables first, so the entry's own may override them.
        let mut env = cap_env(&values);
        settings.env_in(&entry, runner.as_slice(), &mut env);
        // If its runner, and so its container, is paused, it is resumed first.
        self.resume_runner_of(&entry, library, out);
        let instance = self.begin_launch(entry.clone(), key, app_id, clock, start);
        self.refresh_launch = Some((entry.clone(), clock.mono()));
        out.effects.push(Effect::Launch { entry, instance, settings: values, env, launch });
        Launched::Yes
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
            out.effects.push(Effect::Compositor(Compositor::Close(windows)));
        }
        self.follow_quitting(now, library, out);
    }

    /// Force kill: ends the entry at once. It is kept as quitting, already ended, until it no longer runs,
    /// so that nothing quits it again meanwhile (its windows gone, Quits when closed would).
    pub(crate) fn force_kill(&mut self, entry: &Arc<str>, now: u64, library: &Library, out: &mut Out) {
        self.quitting.retain(|q| q.entry != *entry);
        self.thaw(entry, out);
        if self.running(entry).is_some() {
            self.quitting.push(crate::session::Quitting { entry: entry.clone(), ended: true, stop: None, asked: now });
        }
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

/// A capped game presents in FIFO, so that it waits for the frame callbacks the compositor paces to the
/// cap: in mailbox or immediate it wouldn't. Mesa's Vulkan WSI, and GL's swap interval kept at 1 or more
/// (`vblank_mode` 3). Set at launch, so a cap set while the game runs reaches only games presenting in
/// FIFO already.
fn cap_env(values: &crate::settings::Values) -> Vec<(String, String)> {
    if values.get(crate::settings::Setting::FpsCap) == 0 {
        return Vec::new();
    }
    vec![("MESA_VK_WSI_PRESENT_MODE".into(), "fifo".into()), ("vblank_mode".into(), "3".into())]
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests;
