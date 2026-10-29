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
use clamshell_library::BuiltinApp;
use launch_unit::LaunchUnit;
use running::{Launching, Running};
use windows::{Group, Window, WindowFacts, WindowPlace};

/// What was in front, as [`Session::follow_front`] goes back to it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum InFront {
    /// One of ClamShell's own screens, with no window in front.
    App(BuiltinApp),
    Group(Group),
}

/// The launch to show when its first window appears, and when waiting for it gives up: set for a program
/// still running whose window in front closed (see [`Session::follow_front`]); none for a launch.
#[derive(Debug, Clone)]
pub(crate) struct FrontLaunch {
    unit: LaunchUnit,
    until: Option<u64>,
    /// The launcher's dialogs that brought its group in front, while any is open. It is not remembered as
    /// having been in front, so that what comes after the game is what would have come without it.
    promoted: Vec<i64>,
}

/// A window's group as worked out (see [`Session::home_of`]), borrowed: an entry's, by its key, or a
/// window's own.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Home<'a> {
    Entry(&'a str),
    Window(i64),
}

impl Home<'_> {
    /// Whether it is `group`.
    pub(crate) fn is(self, group: &Group) -> bool {
        match (self, group) {
            (Home::Entry(key), Group::Entry(other)) => key == &**other,
            (Home::Window(id), Group::Window(other)) => id == *other,
            _ => false,
        }
    }

    /// It as a group.
    pub(crate) fn group(self) -> Group {
        match self {
            Home::Entry(key) => Group::entry(key),
            Home::Window(id) => Group::Window(id),
        }
    }
}

/// Where the keyboard is: the top screen's stack, the bottom screen's, or ClamShell's own UI.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
enum Keys {
    #[default]
    Top,
    Bottom,
    Ui,
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
    /// Launcher games asked for while their launcher's own launch was starting, and how each starts: they
    /// are launched once it has (see [`Session::due_after_launchers`]).
    after_launcher: Vec<(Arc<str>, Arc<str>, running::Start)>,
    /// The compositor's windows.
    windows: Vec<Window>,
    /// The root of the stack the top screen shows (see [`Session::root_of`]); none while the Library is in
    /// front. The window taking the input is computed: its topmost ([`Session::top_window`]).
    top: Option<i64>,
    /// The root of the stack placed on the bottom screen, if any. It is shown there while the windows' tab is open
    /// (see [`crate::Bottom::shows_windows`]).
    bottom: Option<i64>,
    /// The program in front's windows shown together on the top screen (see [`crate::session::overview`]).
    overview: Option<overview::Overview>,
    /// Which programs' windows go to the bottom screen (see [`crate::session::below`]).
    window_rules: below::WindowRules,
    /// Windows a rule sent to the bottom screen. Each is sent once and dropped from here when it closes.
    sent_down: Vec<i64>,
    /// Where the keyboard is; the window with it is computed ([`Session::keyboard_focus`]).
    keys: Keys,
    home_held: bool,
    home_used: bool,
    /// Our running units, as systemd lists them.
    units: Vec<LaunchUnit>,
    /// What was in front, most recent first: groups, and the built-ins once something put them in front
    /// ([`WindowCommand::Show`]), so that a closed window goes back to where it was opened from.
    recent: Vec<InFront>,
    /// The built-in shown while no window is in front (see [`Session::shown_app`]).
    app: BuiltinApp,
    /// The apps that run while open (see [`crate::builtin_apps::Runs`]) that are: shown, or on the dock
    /// behind what is in front, until closed. By [`BuiltinApp::index`].
    open: [bool; BuiltinApp::ALL.len()],
    /// The selected stack of each top group, by its root, retained while the group is hidden.
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

/// Whether built-in `b` runs, with `open` the apps open (see [`Session::runs_app`]).
fn runs(open: &[bool; BuiltinApp::ALL.len()], b: BuiltinApp) -> bool {
    crate::builtin_apps::definition(b).runs == crate::builtin_apps::Runs::Always || open.get(b.index()).copied().unwrap_or(false)
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

impl Quitting {
    /// Whether it is still going on at `now`: ended, or its windows asked to close less than
    /// [`running::QUIT_WAIT`] ago. Past that, the quit is given up and the entry runs on as before.
    fn going_on(&self, now: u64) -> bool {
        self.ended || now < self.asked + running::QUIT_WAIT
    }
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

    /// Whether entry `id` is quitting at `now` (see [`Quitting::going_on`]).
    pub(crate) fn is_quitting(&self, id: &str, now: u64) -> bool {
        self.quitting.iter().any(|q| &*q.entry == id && q.going_on(now))
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

    /// The window with the keyboard: the topmost of the stack in front on the screen it is on, none while
    /// ClamShell's own UI takes the keys or that screen shows no window. A window no screen shows can't
    /// have it.
    pub fn keyboard_focus(&self) -> Option<i64> {
        let slot = match self.keys {
            Keys::Top => self.top,
            Keys::Bottom => self.bottom,
            Keys::Ui => None,
        };
        slot.map(|root| self.topmost(root))
    }

    /// Puts the keyboard on window `id`'s screen, or on ClamShell's UI for none. A window no screen
    /// shows is ignored: the keyboard stays where it is.
    pub(crate) fn keys_to(&mut self, id: Option<i64>) {
        match id {
            None => self.keys = Keys::Ui,
            Some(id) if self.top.is_some_and(|top| top == self.root_of(id)) => self.keys = Keys::Top,
            Some(id) if self.below(id) => self.keys = Keys::Bottom,
            Some(_) => {}
        }
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

    /// Whether the group in front has another window to step to (see [`Session::step_window`]).
    pub fn has_other_top_window(&self) -> bool {
        self.top_window()
            .and_then(|front| front.group.as_ref())
            .is_some_and(|group| self.windows.iter().filter(|w| self.top_level_in(w, group)).nth(1).is_some())
    }

    /// Whether `w` is a top-level window of `group` on the top screen: the root of its stack (see
    /// [`Session::root_of`]), which is what stepping through the group's windows and the overview go by.
    pub(crate) fn top_level_in(&self, w: &Window, group: &Group) -> bool {
        self.root_of(w.id) == w.id && !self.below(w.id) && w.group.as_ref() == Some(group)
    }

    /// Whether a window opening comes in front. While a launch waits for its window, only its own windows
    /// come in front. A dialog comes in front if its parent's program is in front. First windows of our
    /// units follow foreground launch intent; later windows of the shown program may select themselves.
    /// An unowned program's window is shown at once.
    pub(crate) fn opening_focus(&self, window: &WindowFacts, library: &Library) -> bool {
        if let Some(FrontLaunch { unit, .. }) = &self.front_launch {
            return self.of_launch(unit, window) || self.launcher_dialog(window, library);
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

    /// Whether `window`, opening while a launcher's game starts, is a dialog of that launcher, such as
    /// Steam asking which version to launch: a dialog, or a window with a parent, of the launcher's entry.
    /// It comes in front, its group promoted for as long as the game's launch waits (see
    /// [`FrontLaunch::promoted`]); the launcher's other windows stay behind.
    pub(crate) fn launcher_dialog(&self, window: &WindowFacts, library: &Library) -> bool {
        let Some(launcher) = self.front_launcher(library) else { return false };
        window.is_dialog() && self.running_of(window).is_some_and(|r| r.key == launcher)
    }

    /// The key of the launcher's entry while the launch waiting for its window is a launcher's game,
    /// whichever way it came to wait (see [`launchers::launcher_of`]).
    fn front_launcher<'a>(&self, library: &'a Library) -> Option<&'a str> {
        let key = &self.front_launch.as_ref()?.unit.key;
        let game = library.entries.iter().find(|g| &g.key == key)?;
        launchers::launcher_of(game)?.entry(library).map(|l| l.key.as_str())
    }

    /// Makes `unit` the launch to show when its window appears, waiting until `until` if given.
    pub(crate) fn set_front_launch(&mut self, unit: LaunchUnit, until: Option<u64>) {
        self.front_launch = Some(FrontLaunch { unit, until, promoted: Vec::new() });
    }

    /// Whether `group` is a launcher's, promoted for a dialog of it while its game starts.
    fn promoted(&self, group: &Group) -> bool {
        self.front_launch.as_ref().is_some_and(|f| f.promoted.iter().any(|d| self.window(*d).is_some_and(|w| w.group.as_ref() == Some(group))))
    }

    /// Brings the group in front's next or previous top-level window in front, with its dialogs over it.
    pub(crate) fn step_window(&mut self, next: bool) {
        let Some((group, root)) = self.top_window().and_then(|front| Some((front.group.clone()?, self.root_of(front.id)))) else { return };
        let roots: Vec<i64> = self.windows.iter().filter(|w| self.top_level_in(w, &group)).map(|w| w.id).collect();
        let Some(at) = roots.iter().position(|id| *id == root) else { return };
        let to = if next { (at + 1) % roots.len() } else { (at + roots.len() - 1) % roots.len() };
        if let Some(to) = roots.get(to).copied().filter(|to| *to != root) {
            self.bring_in_front(to);
        }
    }

    /// Brings window `id` in front, as the user asked: from Home, the launch waiting for its window
    /// given up.
    pub(crate) fn bring_in_front(&mut self, id: i64) {
        self.use_home();
        self.cancel_front_launch();
        self.command(WindowCommand::FocusWindow(id));
    }

    pub(crate) fn pad_active(&mut self) {
        if let Some(id) = self.top_window().map(|w| w.id).filter(|id| Some(*id) != self.keyboard_focus()) {
            self.command(WindowCommand::FocusWindow(id));
        }
    }

    /// The window the top screen shows in front, taking the input: its stack's topmost (see
    /// [`Session::topmost`]). None while the Library is in front.
    pub fn top_window(&self) -> Option<&Window> {
        self.top.map(|root| self.topmost(root)).and_then(|id| self.window(id))
    }

    /// The window placed on the bottom screen, if any.
    pub fn bottom_window(&self) -> Option<&Window> {
        self.bottom.and_then(|id| self.window(id))
    }

    /// What a screen shows, the bottom screen's if `below`, back to front: the window in its slot's root
    /// (see [`Session::root_of`]), then the root's dialogs in the order they opened. The last takes the
    /// input; the compositor darkens those before it, which shows why they don't.
    pub fn stack(&self, below: bool) -> impl Iterator<Item = &Window> {
        let slot = if below { self.bottom } else { self.top };
        slot.into_iter().flat_map(|slot| self.stack_of(slot))
    }

    /// Window `id`'s stack as a screen would show it (see [`Session::stack`]), as the overview shows it in
    /// its cell.
    pub fn stack_of(&self, id: i64) -> impl Iterator<Item = &Window> {
        let root = self.root_of(id);
        self.windows.iter().filter(move |w| self.root_of(w.id) == root)
    }

    /// The window that window `id`'s parents lead to, its main window: itself if it has no parent. A
    /// window whose parent closed leads to the first of that parent's dialogs still open, which stands for
    /// it: the dialogs left by a window stay one stack.
    pub fn root_of(&self, id: i64) -> i64 {
        let mut at = id;
        // A bound, so that a cycle of parents can't loop.
        for _ in 0..self.windows.len() {
            let next = match self.window(at).and_then(|w| w.parent).filter(|parent| *parent != at) {
                Some(parent) if self.window(parent).is_some() => Some(parent),
                Some(parent) => self.windows.iter().find(|w| w.parent == Some(parent)).map(|w| w.id).filter(|first| *first != at),
                None => None,
            };
            match next {
                Some(next) => at = next,
                None => break,
            }
        }
        at
    }

    /// The window of `root`'s stack that takes the input: its last dialog opened, or itself.
    pub(crate) fn topmost(&self, root: i64) -> i64 {
        self.windows.iter().rev().find(|w| self.root_of(w.id) == root).map_or(root, |w| w.id)
    }

    /// Whether window `id` is in view: in the top screen's stack or the bottom screen's (see
    /// [`Session::stack`]).
    pub fn shown(&self, id: i64) -> bool {
        self.top.is_some_and(|top| self.root_of(top) == self.root_of(id)) || self.below(id)
    }

    /// Whether window `id` is on the bottom screen: the window placed there, or one of its stack.
    pub fn below(&self, id: i64) -> bool {
        self.bottom.is_some_and(|bottom| bottom == id || self.root_of(bottom) == self.root_of(id))
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
        selected && open(self.top) && open(self.bottom) && self.top.is_none_or(|top| !self.below(top))
    }

    pub fn units(&self) -> &[LaunchUnit] {
        &self.units
    }

    /// The built-in shown on the main screen, if no window is in front.
    pub fn shown_app(&self) -> Option<BuiltinApp> {
        self.top_window().is_none().then_some(self.app)
    }

    /// Whether built-in `b` runs: always, or while it is open, as its definition says.
    pub fn runs_app(&self, b: BuiltinApp) -> bool {
        runs(&self.open, b)
    }

    /// Whether entry `id` runs: a launch or a program found, or a built-in.
    pub fn runs(&self, id: &str) -> bool {
        self.running(id).is_some() || BuiltinApp::of(id).is_some_and(|b| self.runs_app(b))
    }

    /// Closes `app`, which runs while open. If it showed, the home shows in its place.
    pub(crate) fn close_app(&mut self, app: BuiltinApp) {
        if let Some(open) = self.open.get_mut(app.index()) {
            *open = false;
        }
        self.recent.retain(|was| *was != InFront::App(app));
        if self.app == app {
            self.app = crate::builtin_apps::home();
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

    /// The running entry a window belongs to: its main window's (see [`Session::root_of`]), by the first of
    /// these that matches, each a fact or a claim, never the group it was placed in:
    ///
    /// - its mark (see [`crate::session::running::mark_of`]), as for a launcher's game that runs in the
    ///   launcher's unit;
    /// - its unit, which also covers windows moved down;
    /// - its process, for a program found by its window or tray icon (see [`Running::fathered`]);
    /// - another main window of its process and app id, by these: the same program, whatever its client
    ///   was identified as;
    /// - the claim made on it (see [`Window::claim`]): a program known by the facts above takes its window
    ///   back from a claim made before it ran;
    /// - the app id of a program found by another window of it, which a claim stands against: a browsed
    ///   file's window is the file's, though another window of that app opened elsewhere.
    pub(crate) fn running_of(&self, w: &WindowFacts) -> Option<&Running> {
        let root = self.root_facts(w);
        let by_key = |k: Option<&str>| self.running.iter().find(|r| k == Some(r.key.as_str()));
        // A unit's windows are its key's entry's, unless that is found and not adopted yet: its windows are
        // those it is found by (see `Session::follow_adopted`).
        let by_unit = |k: Option<&str>| by_key(k).filter(|r| !r.found());
        let by_facts = |f: &WindowFacts| {
            self.running
                .iter()
                .find(|r| !r.found() && r.owns(f))
                .or_else(|| by_unit(f.unit.as_deref().and_then(LaunchUnit::key_of)))
                .or_else(|| self.running.iter().find(|r| r.fathered(f)))
        };
        by_facts(root)
            .or_else(|| {
                let process = root.pids.first()?;
                let main = |o: &&Window| o.id != root.id && o.parent.is_none() && o.pids.first() == Some(process) && o.app_id == root.app_id;
                self.windows.iter().filter(main).find_map(|o| by_facts(o))
            })
            .or_else(|| by_key(self.window(root.id).and_then(|w| w.claim.as_ref()).and_then(Group::key)))
            .or_else(|| self.running.iter().find(|r| r.found() && r.owns(root)))
    }

    /// The facts of window `w`'s main window: of the window its parents lead to (see
    /// [`Session::root_of`]), `w` itself for one opening that has no parent open.
    fn root_facts<'a>(&'a self, w: &'a WindowFacts) -> &'a WindowFacts {
        let root = match self.window(w.id) {
            Some(_) => self.root_of(w.id),
            None => w.parent.filter(|p| self.window(*p).is_some()).map_or(w.id, |p| self.root_of(p)),
        };
        self.window(root).map_or(w, |root| &root.facts)
    }

    /// The group window `w` is placed in: its main window's program's (see [`Session::running_of`]), else
    /// the claim made on its main window, else the library entry its main window is known by (a launch
    /// still starting, see [`running::has_mark`]) or its launch unit's entry, else a group of its own, its
    /// main window's. Borrowed, so that working it out allocates nothing.
    pub(crate) fn home_of<'a>(&'a self, w: &'a WindowFacts, library: &'a Library) -> Home<'a> {
        let root = self.root_facts(w);
        if let Some(r) = self.running_of(w) {
            return Home::Entry(&r.key);
        }
        match self.window(root.id).and_then(|w| w.claim.as_ref()) {
            Some(Group::Entry(key)) => return Home::Entry(key),
            Some(Group::Window(id)) => return Home::Window(*id),
            None => {}
        }
        library
            .entries
            .iter()
            .find(|g| running::has_mark(library, g, root))
            .map(|g| Home::Entry(&g.key))
            .or_else(|| root.unit.as_deref().and_then(LaunchUnit::key_of).filter(|_| root.launched()).map(Home::Entry))
            .unwrap_or(Home::Window(root.id))
    }

    /// Places every window in its group (see [`Session::home_of`]), changing only those whose group changed,
    /// and the groups' selections and history with them.
    pub(crate) fn place_windows(&mut self, library: &Library) {
        let mut moved = false;
        for i in 0..self.windows.len() {
            let Some(w) = self.windows.get(i) else { continue };
            let home = self.home_of(w, library);
            if w.group.as_ref().is_some_and(|group| home.is(group)) {
                continue;
            }
            let home = home.group();
            if let Some(w) = self.windows.get_mut(i) {
                w.group = Some(home);
                moved = true;
            }
        }
        if moved {
            self.remember_front();
        }
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

    /// The window Home with B acts on. It is the one in front on the top screen. If there is none, it is the one taking the input on the bottom screen while it shows the windows moved there
    /// (`shows_windows`).
    pub(crate) fn home_b_window(&self, shows_windows: bool) -> Option<&Window> {
        let bottom = || self.bottom.map(|b| self.topmost(b)).and_then(|id| self.window(id));
        self.top_window().or_else(|| shows_windows.then(bottom).flatten())
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

    /// The entries quitting at `now`.
    #[cfg(test)]
    pub(crate) fn quitting_entries(&self, now: u64) -> impl Iterator<Item = &Arc<str>> {
        self.quitting.iter().filter(move |q| q.going_on(now)).map(|q| &q.entry)
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
        let mut selected = std::mem::take(&mut self.selected);
        selected.retain(|(group, id)| !self.below(*id) && self.windows.iter().any(|w| w.id == *id && w.group.as_ref() == Some(group)));
        self.selected = selected;
        if let Some(w) = self.top.and_then(|id| self.windows.iter().find(|w| w.id == id))
            && let Some(group) = &w.group
        {
            match self.selected.iter_mut().find(|(g, _)| g == group) {
                Some((_, id)) => *id = w.id,
                None => self.selected.push((group.clone(), w.id)),
            }
        }
        let present = |group: &Group| self.windows.iter().any(|w| w.group.as_ref() == Some(group));
        let open = self.open;
        self.recent.retain(|was| match was {
            InFront::App(b) => runs(&open, *b),
            InFront::Group(group) => present(group),
        });
        if let Some(group) = self.top_window().and_then(|w| w.group.clone()).filter(|g| !self.promoted(g)) {
            self.remember_in_front(InFront::Group(group));
        }
    }

    /// Puts `front` first in what was in front. Already first, it stays.
    fn remember_in_front(&mut self, front: InFront) {
        if self.recent.first() != Some(&front) {
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
