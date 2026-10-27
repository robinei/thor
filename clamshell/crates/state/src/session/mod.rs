//! What runs and what is in front: the running entries (see [`crate::session::running`]), our units as
//! systemd lists them, the compositor's windows, and the launch to show once its first window appears.

pub mod autostart;
pub mod background;
pub mod below;
pub mod dock;
pub mod enqueue;
pub(crate) mod found;
mod front;
pub mod launch_unit;
pub mod launchers;
pub mod overview;
pub mod program;
pub(crate) mod running;
pub mod tray;
pub mod windows;

use std::sync::Arc;

use crate::effect::Effect;
use crate::session::windows::WindowCommand;
use crate::{Entry, Library};
use launch_unit::LaunchUnit;
use running::{Front, Launching, Running};
use windows::{Group, Window, WindowFacts, WindowPlace};

/// The launch to show when its first window appears, and when waiting for it gives up: set for a program
/// still running whose window in front closed (see [`Session::follow_front`]); none for a launch.
#[derive(Debug, Clone)]
pub(crate) struct FrontLaunch {
    unit: LaunchUnit,
    until: Option<u64>,
}

/// How soon after the window in front closed a new window in its group is shown again, in seconds.
pub(crate) const REOPEN_SECS: u64 = 2;

#[derive(Debug, Default)]
pub struct Session {
    /// The entries that run, in the order they started.
    running: Vec<Running>,
    /// The id of the program in force (see [`crate::session::program`]).
    program: Option<Arc<str>>,
    /// The entry launched last and when (monotonic seconds), while its window is yet to show (see
    /// [`Session::refresh_launch`]).
    refresh_launch: Option<(Arc<str>, u64)>,
    /// Launches asked for that haven't started.
    launching: Vec<Launching>,
    /// The compositor's windows.
    windows: Vec<Window>,
    /// The window the top screen shows; none while shelf is in front.
    top: Option<i64>,
    /// The window placed on the bottom screen, if any. It is shown there while the windows' tab is open
    /// (see [`crate::Bottom::shows_windows`]).
    bottom: Option<i64>,
    /// The program in front's windows shown together on the top screen (see [`crate::session::overview`]).
    overview: Option<overview::Overview>,
    /// Which programs' windows go to the bottom screen (see [`crate::session::below`]).
    window_rules: below::WindowRules,
    /// Windows a rule sent to the bottom screen. Each is sent once and dropped from here when it closes.
    sent_down: Vec<i64>,
    /// The client with keyboard focus; none while ClamShell's own UI receives keys.
    keyboard_focus: Option<i64>,
    home_held: bool,
    home_used: bool,
    /// Our running units, as systemd lists them.
    units: Vec<LaunchUnit>,
    /// Groups by when they were last in front, most recent first.
    recent: Vec<Group>,
    /// The selected window of each top group, retained while the group is hidden.
    selected: Vec<(Group, i64)>,
    /// The launch to show when its first window appears. Until then, other programs' new windows open
    /// behind (see [`Session::opening_focus`]).
    front_launch: Option<FrontLaunch>,
    /// The group whose window in front closed, leaving it empty, and when (see
    /// [`Session::follow_replacement`]).
    closed_front: Option<(Group, u64)>,
    /// Entries quitting (see [`Quitting`]).
    quitting: Vec<Quitting>,
    /// The counter that numbers new instances' names. Together with the time, it makes them unique across
    /// restarts.
    next_unit: u64,
    /// Whether systemd has listed our units at least once, so that what runs is known.
    units_known: bool,
    /// What the last [`crate::State::settle`] saw: the windows' ids and the group in front on the main
    /// screen. They tell which windows are new and when the one in front closes.
    opened: Vec<i64>,
    followed_front: Option<Group>,
    /// Which running entries have windows and which have a tray icon, in their order, kept so that each
    /// pass reuses it (see [`Session::end_windowless`]).
    with_windows: Vec<(bool, bool)>,
    /// Programs' tray icons (see [`crate::session::tray`]).
    tray: Vec<tray::TrayItem>,
    /// Programs paused or ended while hidden (see [`crate::session::background`]).
    background: background::Background,
    /// The entries started with ClamShell, and whether they have been started yet (see
    /// [`crate::session::autostart`]).
    autostart: Vec<Arc<str>>,
    autostarted: bool,
    /// ClamShell is the device's own session (see [`crate::Init::device_session`]).
    device_session: bool,
}

/// An entry by its id, in `library` or else among the `running` (see [`Session::entry`]).
fn entry_in<'a>(library: &'a Library, running: &'a [Running], id: &str) -> Option<&'a Entry> {
    library.entry(id).or_else(|| running.iter().find(|r| &*r.id == id).and_then(|r| r.entry.as_deref()))
}

/// An entry quitting. Its windows are asked to close at `asked`, in monotonic seconds, and once none is
/// left it is ended if it still runs, which sets `ended`. `stop` is its runner's stop, for an entry known
/// by its app id that ends with its windows.
#[derive(Debug, Clone)]
pub(crate) struct Quitting {
    entry: Arc<str>,
    ended: bool,
    stop: Option<Effect>,
    asked: u64,
}

impl Session {
    pub(crate) fn new(autostart: Vec<Arc<str>>, device_session: bool, window_rules: below::WindowRules) -> Session {
        Session { autostart, device_session, window_rules, ..Session::default() }
    }

    /// Whether ClamShell is the device's own session (see [`crate::Init::device_session`]).
    pub(crate) fn device_session(&self) -> bool {
        self.device_session
    }

    /// Whether systemd lists a unit of the entry with `key`.
    pub(crate) fn has_units(&self, key: &str) -> bool {
        self.units.iter().any(|u| u.key == key)
    }

    /// Whether entry `id` is quitting.
    pub(crate) fn is_quitting(&self, id: &str) -> bool {
        self.quitting.iter().any(|q| &*q.entry == id)
    }

    /// The entries that run, in the order they started.
    pub fn running_entries(&self) -> &[Running] {
        &self.running
    }

    /// The running entry whose windows are known by app id `app`.
    pub(crate) fn running_app(&self, app: &str) -> Option<Arc<str>> {
        self.running.iter().find(|r| r.mark.as_deref() == Some(app)).map(|r| r.id.clone())
    }

    /// An entry by its id: the library's, or else, for an entry that is still running after leaving the
    /// library because its file was deleted, the entry as it was.
    pub fn entry<'a>(&'a self, library: &'a Library, id: &str) -> Option<&'a Entry> {
        entry_in(library, &self.running, id)
    }

    /// A running entry, by its id.
    pub fn running(&self, id: &str) -> Option<&Running> {
        self.running.iter().find(|r| &*r.id == id)
    }

    /// The program in force: an entry's id.
    pub fn program(&self) -> Option<&Arc<str>> {
        self.program.as_ref()
    }

    /// The entry launched last whose settings the main screen's refresh rate follows, ahead of the
    /// program in force's, so that the rate changes before its window shows. Until the program in force
    /// changes, or the entry has a window, or it is gone, or [`crate::session::running::LAUNCH_WAIT`]
    /// has passed since its launch (one that never shows a window, such as a tray program).
    pub(crate) fn refresh_launch(&self) -> Option<&Arc<str>> {
        self.refresh_launch.as_ref().map(|(id, _)| id)
    }

    pub fn windows(&self) -> &[Window] {
        &self.windows
    }

    pub fn window(&self, id: i64) -> Option<&Window> {
        self.windows.iter().find(|w| w.id == id)
    }

    pub fn keyboard_focus(&self) -> Option<i64> {
        self.keyboard_focus
    }

    pub fn home_held(&self) -> bool {
        self.home_held
    }

    pub(crate) fn hold_home(&mut self, held: bool) {
        if held && !self.home_held {
            self.home_used = false;
        }
        self.home_held = held;
    }

    pub(crate) fn use_home(&mut self) {
        self.home_used |= self.home_held;
    }

    pub(crate) fn home_tap(&mut self) -> bool {
        !std::mem::take(&mut self.home_used)
    }

    pub fn has_other_top_window(&self) -> bool {
        self.top_window().is_some_and(|front| self.windows.iter().any(|w| !self.below(w.id) && w.id != front.id && w.group == front.group))
    }

    /// Whether a window opening comes in front. While a launch waits for its window, only its own windows
    /// come in front. A dialog comes in front if its parent's program is in front. First windows of our
    /// units follow foreground launch intent; later windows of the shown program may select themselves.
    /// An unowned program's window is shown at once.
    pub(crate) fn opening_focus(&self, window: &WindowFacts) -> bool {
        if let Some(FrontLaunch { unit, .. }) = &self.front_launch {
            return self.of_launch(unit, window);
        }
        if let Some(parent) = window.parent.and_then(|id| self.window(id)) {
            return self.top_window().is_some_and(|top| top.group == parent.group);
        }
        if window.parent.is_some() || !window.unit.as_deref().is_some_and(LaunchUnit::is_launch_unit) {
            return true;
        }
        let key = window.unit.as_deref().and_then(LaunchUnit::key_of);
        self.top_window().is_some_and(|top| top.group.as_ref().and_then(Group::key) == key) && self.windows.iter().any(|old| old.unit == window.unit)
    }

    /// Whether `window` is one of launch `unit`'s: in its unit, of its entry by app id or process, or
    /// a dialog of one in its group.
    fn of_launch(&self, unit: &LaunchUnit, window: &WindowFacts) -> bool {
        let running = self.running.iter().find(|r| r.key == unit.key);
        let in_unit = window.unit.as_deref().and_then(LaunchUnit::unit_name).is_some_and(|name| unit.is(name));
        let parent = window.parent.and_then(|id| self.window(id)).is_some_and(|parent| parent.group.as_ref().and_then(Group::key) == Some(&unit.key));
        in_unit || parent || running.is_some_and(|r| r.owns(window) || r.fathered(window))
    }

    /// Makes `unit` the launch to show when its window appears, waiting until `until` if given.
    pub(crate) fn set_front_launch(&mut self, unit: LaunchUnit, until: Option<u64>) {
        self.front_launch = Some(FrontLaunch { unit, until });
    }

    pub(crate) fn step_window(&mut self, next: bool) {
        let Some(front) = self.top_window() else { return };
        let mut group = self.windows.iter().filter(|w| !self.below(w.id) && w.group == front.group).cycle();
        let count = self.windows.iter().filter(|w| !self.below(w.id) && w.group == front.group).count();
        let id = if next {
            group.find(|w| w.id == front.id).and_then(|_| group.next()).map(|w| w.id)
        } else {
            group.find(|w| w.id == front.id).and_then(|_| group.nth(count.saturating_sub(2))).map(|w| w.id)
        };
        if let Some(id) = id.filter(|id| *id != front.id) {
            self.use_home();
            self.cancel_front_launch();
            self.command(WindowCommand::FocusWindow(id));
        }
    }

    pub(crate) fn pad_active(&mut self) {
        if let Some(id) = self.top_window().map(|w| w.id).filter(|id| Some(*id) != self.keyboard_focus) {
            self.command(WindowCommand::FocusWindow(id));
        }
    }

    /// The window the top screen shows, or none while shelf is in front.
    pub fn top_window(&self) -> Option<&Window> {
        self.top.and_then(|id| self.window(id))
    }

    /// The window placed on the bottom screen, if any.
    pub fn bottom_window(&self) -> Option<&Window> {
        self.bottom.and_then(|id| self.window(id))
    }

    /// Whether window `id` is in view: the one the top screen shows, or the one placed on the bottom
    /// screen.
    pub fn shown(&self, id: i64) -> bool {
        self.top == Some(id) || self.bottom == Some(id)
    }

    /// Whether window `id` is the one placed on the bottom screen.
    pub fn below(&self, id: i64) -> bool {
        self.bottom == Some(id)
    }

    /// Where window `id` is: on the bottom screen, or in its group on the top screen.
    pub fn place(&self, id: i64) -> Option<WindowPlace<'_>> {
        let window = self.window(id)?;
        Some(if self.below(id) { WindowPlace::Bottom } else { WindowPlace::Top(window.group.as_ref()) })
    }

    /// Whether the window slots and the groups' selections name open windows where they should be.
    #[cfg(test)]
    pub(crate) fn selections_valid(&self) -> bool {
        let selected = self.selected.iter().all(|(group, id)| self.window(*id).is_some_and(|w| !self.below(w.id) && w.group.as_ref() == Some(group)));
        // At most one selection per group.
        let selected = selected && self.selected.iter().enumerate().all(|(i, (group, _))| !self.selected.iter().take(i).any(|(other, _)| other == group));
        let open = |slot: Option<i64>| slot.is_none_or(|id| self.window(id).is_some());
        selected && open(self.top) && open(self.bottom) && (self.top.is_none() || self.top != self.bottom)
    }

    pub fn units(&self) -> &[LaunchUnit] {
        &self.units
    }

    /// What is in front on the main screen: shelf, a running entry's window, or another.
    pub fn front(&self) -> Front {
        match self.top_window() {
            Some(w) if self.running_of(w).is_some() => Front::Program,
            Some(_) => Front::Other,
            None => Front::Shelf,
        }
    }

    /// The running entry a client's process belongs to, before it has a window: by its unit, else by the
    /// game its launcher marked it with (see [`launchers`]), as Steam's game launcher starts a game in
    /// Steam's scope.
    pub fn running_of_process(&self, unit: Option<&str>, game: Option<&str>) -> Option<&Running> {
        let by_key = unit.and_then(LaunchUnit::key_of).and_then(|key| self.running.iter().find(|r| r.key == key));
        by_key.or_else(|| self.running.iter().find(|r| game.is_some() && r.mark.as_deref() == game))
    }

    /// Notes the resolution a client's process was told, on the running entry it belongs to (see
    /// [`Session::running_of_process`]).
    pub(crate) fn told(&mut self, unit: Option<&str>, game: Option<&str>, resolution: crate::settings::Resolution) {
        let id = self.running_of_process(unit, game).map(|r| r.id.clone());
        if let Some(running) = id.and_then(|id| self.running.iter_mut().find(|r| r.id == id)) {
            running.told = Some(resolution);
        } else if let Some(key) = unit.and_then(LaunchUnit::key_of)
            && let Some(launching) = self.launching.iter_mut().find(|l| l.unit.key == key)
        {
            // Still starting: its program can connect before systemd lists its unit.
            launching.told = Some(resolution);
        }
    }

    /// The entry a client's process (by its unit, or its launcher's mark) belongs to: one running, or a
    /// launch still starting, whose program can connect before systemd lists its unit.
    pub(crate) fn entry_of_process(&self, unit: Option<&str>, game: Option<&str>) -> Option<Arc<str>> {
        self.running_of_process(unit, game).map(|r| r.id.clone()).or_else(|| {
            let key = unit.and_then(LaunchUnit::key_of)?;
            self.launching.iter().find(|l| l.unit.key == key).map(|l| l.entry.clone())
        })
    }

    /// The running entry a window belongs to, by the first of these that matches:
    ///
    /// - its mark (see [`crate::session::running::mark_of`]), as for a launcher's game that runs in the
    ///   launcher's unit;
    /// - its unit, which also covers windows moved down;
    /// - its process, for a program found by its tray icon;
    /// - its group.
    ///
    /// A dialog's is its parent's.
    pub(crate) fn running_of(&self, w: &WindowFacts) -> Option<&Running> {
        let mut w = w;
        for _ in 0..self.windows.len() {
            match w.parent.and_then(|id| self.window(id)) {
                Some(parent) if parent.id != w.id => w = parent,
                _ => break,
            }
        }
        let by_key = |k: Option<&str>| self.running.iter().find(|r| k == Some(r.key.as_str()));
        self.running
            .iter()
            .find(|r| r.owns(w))
            .or_else(|| by_key(w.unit.as_deref().and_then(LaunchUnit::key_of)))
            .or_else(|| self.running.iter().find(|r| r.fathered(w)))
            .or_else(|| by_key(self.window(w.id).and_then(|w| w.group.as_ref()).and_then(Group::key)))
    }

    /// The id of the running entry a window belongs to (see [`Session::running_of`]).
    pub(crate) fn owner_of(&self, w: &WindowFacts) -> Option<Arc<str>> {
        self.running_of(w).map(|r| r.id.clone())
    }

    /// The windows of running entry `id`, including those moved to the bottom screen.
    pub(crate) fn windows_of<'a>(&'a self, id: &'a str) -> impl Iterator<Item = &'a Window> + 'a {
        self.windows.iter().filter(move |w| self.running_of(w).is_some_and(|r| &*r.id == id))
    }

    /// The `awake` command of the runner of a shown window, if it has one.
    pub(crate) fn awake_command(&self) -> Option<Arc<[String]>> {
        self.top_window().into_iter().chain(self.bottom_window()).find_map(|w| {
            let runner = self.running_of(w)?.entry.as_ref()?.runner.as_ref()?;
            (!runner.awake.is_empty()).then(|| runner.awake.clone())
        })
    }

    /// The window Home with B acts on. It is the one in front on the top screen. If there is none, it is the one in view on the bottom screen while it shows the windows moved there
    /// (`shows_windows`).
    pub(crate) fn home_b_window(&self, shows_windows: bool) -> Option<&Window> {
        self.top_window().or_else(|| shows_windows.then(|| self.bottom_window()).flatten())
    }

    /// Sets the icon of found entry `entry`, once it was found (see [`Effect::FindIcon`]).
    pub(crate) fn set_icon(&mut self, entry: &str, path: std::path::PathBuf) {
        if let Some(g) = self.running.iter_mut().find(|r| &*r.id == entry).and_then(|r| r.entry.as_mut()) {
            g.icon = Some(path);
        }
    }

    /// Has `running` run in unit `unit`, as if it had started and systemd listed the unit.
    #[cfg(test)]
    pub(crate) fn add_run(&mut self, running: Running, unit: LaunchUnit) {
        self.running.push(running);
        self.units.push(unit);
    }

    /// The launches asked for that haven't started.
    #[cfg(test)]
    pub(crate) fn launching(&self) -> &[Launching] {
        &self.launching
    }

    /// The entries quitting.
    #[cfg(test)]
    pub(crate) fn quitting_entries(&self) -> impl Iterator<Item = &Arc<str>> {
        self.quitting.iter().map(|q| &q.entry)
    }

    /// The launch to show when its first window appears.
    #[cfg(test)]
    pub(crate) fn front_launch(&self) -> Option<&LaunchUnit> {
        self.front_launch.as_ref().map(|f| &f.unit)
    }

    /// The counter for a new unit's name (see [`crate::session::launch_unit::LaunchUnit`]).
    pub(crate) fn next_unit(&mut self) -> u64 {
        let n = self.next_unit;
        self.next_unit += 1;
        n
    }

    /// Keeps the front's history after the window set changes, whichever backend reported it.
    pub(crate) fn remember_front(&mut self) {
        let bottom = self.bottom;
        self.selected.retain(|(group, id)| bottom != Some(*id) && self.windows.iter().any(|w| w.id == *id && w.group.as_ref() == Some(group)));
        if let Some(w) = self.top.and_then(|id| self.windows.iter().find(|w| w.id == id))
            && let Some(group) = &w.group
        {
            match self.selected.iter_mut().find(|(g, _)| g == group) {
                Some((_, id)) => *id = w.id,
                None => self.selected.push((group.clone(), w.id)),
            }
        }
        let present = |group: &Group| self.windows.iter().any(|w| w.group.as_ref() == Some(group));
        self.recent.retain(present);
        // Already first, it stays.
        if let Some(front) = self.top_window().and_then(|w| w.group.as_ref()).filter(|f| self.recent.first() != Some(*f)) {
            let front = front.clone();
            self.recent.retain(|n| *n != front);
            self.recent.insert(0, front);
        }
    }

    pub(crate) fn set_units(&mut self, units: Vec<LaunchUnit>) {
        self.units = units;
        self.units_known = true;
    }

    /// If a window appears within [`REOPEN_SECS`] in the group whose window in front closed, shows that
    /// group again. This is for a program that replaces its first window, such as a splash
    /// screen, or Waydroid on a cold start.
    pub(crate) fn follow_replacement(&mut self, now: u64) {
        let Some((group, at)) = self.closed_front.take() else {
            return;
        };
        if now > at + REOPEN_SECS {
            return;
        }
        if self.windows.iter().any(|w| w.group.as_ref() == Some(&group) && !self.below(w.id)) {
            self.focus(&group);
            return;
        }
        self.closed_front = Some((group, at));
    }

    /// Shows the group of the launch just started, once its own first window is there. The entry's group
    /// may already have windows of its other launches. An entry known by its app id has no unit
    /// of ours, so any of its windows counts.
    pub(crate) fn follow_front_launch(&mut self, now: u64) {
        if self.front_launch.as_ref().is_some_and(|f| f.until.is_some_and(|until| now > until)) {
            self.cancel_front_launch();
        }
        let Some(FrontLaunch { unit, .. }) = &self.front_launch else { return };
        let outside = self.running.iter().find(|r| r.key == unit.key).is_some_and(|r| r.outside() || matches!(r.origin, running::Origin::Windows { .. }));
        if !outside && !self.units.contains(unit) && !self.launching.iter().any(|l| l.unit == *unit) {
            self.front_launch = None;
            return;
        }
        let arrived = self
            .windows
            .iter()
            .find(|w| {
                w.group.as_ref().and_then(Group::key) == Some(&unit.key)
                    && (outside || w.unit.as_deref().and_then(LaunchUnit::unit_name).is_some_and(|name| unit.is(name)))
            })
            .map(|w| (w.id, self.below(w.id)));
        if let Some((id, down)) = arrived {
            self.front_launch = None;
            if !down {
                self.command(WindowCommand::FocusWindow(id));
            }
        }
    }

    pub(crate) fn cancel_front_launch(&mut self) {
        self.front_launch = None;
    }
}
