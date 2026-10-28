//! The compositor's windows, as the dock and the running entries need them. Each launch of a library entry
//! is a systemd unit we started. Every window of its processes, such as a second window or a program
//! started from its terminal, goes in the entry's group as a tab ([`Group::Entry`]). An entry that runs in
//! another program's processes, such as a Steam game or a Waydroid app, has a group too, and its windows
//! are found by their app id (see [`crate::session::running`]). A window that belongs to no entry has a
//! group of its own ([`Group::Window`]).

use std::sync::Arc;

use crate::Session;
use crate::session::launch_unit::LaunchUnit;

/// A home group on the top screen, whose windows are shown as tabs, one at a time.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum Group {
    /// An entry's, by its key: the windows of all its launches.
    Entry(Arc<str>),
    /// A window's own, for a window of no entry, by the window's id.
    Window(i64),
}

impl Group {
    /// The group of the entry with key `key`.
    pub fn entry(key: &str) -> Group {
        Group::Entry(key.into())
    }

    /// The entry's key, for an entry's group.
    pub fn key(&self) -> Option<&str> {
        match self {
            Group::Entry(key) => Some(key),
            Group::Window(_) => None,
        }
    }
}

/// Where the session has placed a window. A top group contains all launches of one entry.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WindowPlace<'a> {
    /// On the top screen, in this group, or in none until the session places it.
    Top(Option<&'a Group>),
    Bottom,
}

/// A change to where windows are and which ones are shown, decided by the session. The compositor shows
/// the result, read from the state after apply.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum WindowCommand {
    /// Select this top window, including its tab within its group.
    FocusWindow(i64),
    /// Give a client keyboard focus without changing the selected content on either screen.
    KeyboardFocus(i64),
    /// Bring shelf to the front.
    ShowShelf,
    /// Move a window to the bottom screen and show it there, returning the one there to its group. The
    /// top screen keeps the keyboard.
    Down(i64),
    /// Move a window that was moved down back to the top screen, to its group, and select it.
    Up(i64),
    /// Return the displaced bottom window to its group without activating that program.
    Return(i64),
}

impl Session {
    /// Carries out a placement decision on the session's windows.
    pub(crate) fn command(&mut self, command: WindowCommand) {
        match &command {
            WindowCommand::KeyboardFocus(id) => self.keys_to(Some(*id)),
            WindowCommand::FocusWindow(id) => self.select_top(*id),
            WindowCommand::ShowShelf => {
                self.keys = super::Keys::Top;
                self.top = None;
                self.remember_in_front(super::InFront::Shelf);
            }
            WindowCommand::Down(id) => {
                // A dialog goes with its main window: the stack goes down (see [`Session::stack`]).
                let id = &self.root_of(*id);
                if let Some(other) = self.bottom.filter(|other| other != id) {
                    self.command(WindowCommand::Return(other));
                }
                // The top screen keeps the keyboard, or takes it back from the window that was below.
                self.keys = super::Keys::Top;
                if self.window(*id).is_some() {
                    self.bottom = Some(*id);
                }
                if self.top.is_some_and(|top| self.root_of(top) == *id) {
                    self.top = None;
                }
                let group = self.window(*id).and_then(|w| w.group.clone());
                if let Some(group) = group.filter(|_| self.top.is_none()) {
                    self.focus_group(&group);
                }
            }
            WindowCommand::Up(window) => {
                if self.below(*window) {
                    self.bottom = None;
                }
                self.select_top(*window);
            }
            WindowCommand::Return(window) => {
                // Behind in its group it isn't shown, so it can't keep the keyboard: ClamShell's UI takes it
                // until the window in front does (see Down).
                if self.keys == super::Keys::Bottom && self.bottom == Some(self.root_of(*window)) {
                    self.keys = super::Keys::Ui;
                }
                let group = self.window(*window).and_then(|w| w.group.clone());
                if !self.windows.iter().any(|w| !self.below(w.id) && w.group == group) {
                    self.recent.retain(|was| !matches!(was, super::InFront::Group(g) if Some(g) == group.as_ref()));
                }
                if self.bottom == Some(*window) {
                    self.bottom = None;
                }
            }
        }
        self.remember_front();
        let refocus = matches!(command, WindowCommand::Down(_)).then_some(self.top).flatten();
        if let Some(id) = refocus {
            self.command(WindowCommand::FocusWindow(id));
        }
    }

    fn focus_group(&mut self, group: &Group) {
        match self.group_window(group).map(|w| w.id) {
            Some(id) => self.select_top(id),
            None => self.top = None,
        }
    }

    /// The window `group` shows: the one selected in it, else its last.
    fn group_window(&self, group: &Group) -> Option<&Window> {
        let selected = self.selected.iter().find(|(g, _)| g == group).map(|(_, id)| *id);
        self.windows
            .iter()
            .filter(|w| !self.below(w.id) && w.group.as_ref() == Some(group))
            .max_by_key(|w| (Some(w.id) == selected, Some(w.id) == self.top, w.id))
    }

    pub(crate) fn focus(&mut self, group: &Group) {
        if let Some(id) = self.group_window(group).map(|w| w.id) {
            self.command(WindowCommand::FocusWindow(id));
        }
    }

    /// Shows window `id` on the top screen, with the keyboard, unless it is below. A window with dialogs
    /// open shows its last, which takes the input (see [`Session::stack`]).
    pub(crate) fn select_top(&mut self, id: i64) {
        if self.below(id) {
            return;
        }
        let root = self.root_of(id);
        let Some(group) = self.windows.iter().find(|w| w.id == root).map(|w| &w.group) else { return };
        // (A window not placed yet is selected in its group once it is.)
        if let Some(group) = group {
            match self.selected.iter_mut().find(|(g, _)| g == group) {
                Some((_, selected)) => *selected = root,
                None => self.selected.push((group.clone(), root)),
            }
        }
        self.keys = super::Keys::Top;
        self.top = Some(root);
    }
}

/// Changes to one window. Repeated opens and messages for a closed window do nothing. A backend must
/// close an id before reusing it for another window.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum WindowEvent {
    Opened(WindowFacts),
    Closed(i64),
    Title {
        window: i64,
        title: String,
    },
    /// Backend facts which can change after opening, such as a process being adopted into a scope.
    Changed(WindowFacts),
    /// A client's request to show its window, after the backend has checked its activation token.
    Activate(i64),
    /// Keyboard focus changed without changing which content either screen shows. A window no screen shows
    /// is ignored (see [`Session::keys_to`]).
    KeyboardFocus(Option<i64>),
}

impl From<WindowEvent> for crate::Event {
    fn from(event: WindowEvent) -> Self {
        Self::Window(event)
    }
}

impl crate::Session {
    /// Applies a change to a window. Returns whether the bottom screen is to show its window: one a client
    /// asked to show that is there.
    pub(crate) fn on_window(&mut self, event: WindowEvent, library: &crate::Library, now: u64, out: &mut crate::out::Out) -> bool {
        match event {
            WindowEvent::Opened(facts) => self.open(facts, library, now),
            WindowEvent::Closed(id) => self.close(id),
            WindowEvent::Title { window, title } => self.retitle(window, title),
            WindowEvent::Changed(facts) => self.change(facts),
            WindowEvent::KeyboardFocus(id) => self.focus_keyboard(id),
            WindowEvent::Activate(id) => return self.activate(id, library, out),
        }
        false
    }

    /// A window opened. Its group is known at once if it is a dialog, whose is its parent's, or a window
    /// of an entry, by its app id, its owner (see [`Session::running_of`]) or its launch's unit; another
    /// gets one as the windows are
    /// followed ([`Session::follow_windows`]). It comes in front as [`Session::opening_focus`] says. A
    /// dialog of a window shown, on either screen, is shown over it and takes the input at once: the
    /// screen draws a stack whole (see [`Session::stack`]).
    fn open(&mut self, facts: WindowFacts, library: &crate::Library, now: u64) {
        if self.window(facts.id).is_some() {
            return;
        }
        let parent = facts.parent.and_then(|id| self.window(id)).map(|w| (self.below(w.id), self.root_of(w.id)));
        let parent_below = parent.as_ref().is_some_and(|(below, _)| *below);
        let parent_on_top = parent.as_ref().is_some_and(|(below, root)| !below && self.top.is_some_and(|top| self.root_of(top) == *root));
        let group = Some(self.home_of(&facts, library).group());
        let focus = parent_below || parent_on_top || self.opening_focus(&facts, library);
        if focus
            && self.launcher_dialog(&facts, library)
            && let Some(front) = self.front_launch.as_mut()
        {
            front.promoted.push(facts.id);
        }
        let id = facts.id;
        self.opened.push(id);
        self.windows.push(Window { facts, group, claim: None, opened: now });
        if parent_below {
            self.keys = super::Keys::Bottom;
        } else if focus {
            self.select_top(id);
        }
    }

    /// A client asks to show its window, after the compositor checked its token. Its program is thawed
    /// first, so that it draws as it is shown; a window below keeps its screen and gets the keyboard.
    /// Returns whether it is below, for the bottom screen to show it.
    fn activate(&mut self, id: i64, library: &crate::Library, out: &mut crate::out::Out) -> bool {
        if self.window(id).is_none() {
            return false;
        }
        self.cancel_front_launch();
        if let Some(owner) = self.window(id).and_then(|w| self.owner_of(w)) {
            self.thaw(&owner, out);
            self.resume_runner_of(&owner, library, out);
        }
        if self.below(id) {
            self.command(WindowCommand::KeyboardFocus(id));
            return true;
        }
        self.command(WindowCommand::FocusWindow(id));
        false
    }

    /// A window closed. If it was the one in front, its parent or another window of its group comes in
    /// front.
    fn close(&mut self, id: i64) {
        let parent = self.window(id).and_then(|w| w.parent).filter(|parent| self.window(*parent).is_some() && !self.below(*parent));
        let group = self.top_window().filter(|w| w.id == id).and_then(|w| w.group.clone());
        // A stack's root closing leaves its first dialog as the root of what is left (see
        // [`Session::root_of`]), on its screen.
        let heir = self.windows.iter().find(|w| w.parent == Some(id)).map(|w| w.id);
        self.windows.retain(|w| w.id != id);
        if self.top == Some(id) {
            self.top = heir;
        }
        if self.bottom == Some(id) {
            self.bottom = heir;
        }
        // A launcher promoted for its dialogs goes back behind once the last of them closed.
        let promotion_ended = self.front_launch.as_mut().is_some_and(|front| {
            let before = front.promoted.len();
            front.promoted.retain(|d| *d != id);
            before > 0 && front.promoted.is_empty()
        });
        if let Some(group) = group.filter(|g| self.windows.iter().any(|w| !self.below(w.id) && w.group.as_ref() == Some(g))) {
            if promotion_ended {
                self.go_back();
            } else if let Some(parent) = parent {
                self.command(WindowCommand::FocusWindow(parent));
            } else {
                self.focus(&group);
            }
        }
    }

    fn retitle(&mut self, id: i64, title: String) {
        if let Some(w) = self.windows.iter_mut().find(|w| w.id == id) {
            w.facts.title = title;
        }
    }

    /// A window's facts changed. Its title has an event of its own.
    fn change(&mut self, facts: WindowFacts) {
        if let Some(w) = self.windows.iter_mut().find(|w| w.id == facts.id) {
            w.facts = WindowFacts { title: std::mem::take(&mut w.facts.title), x11: w.facts.x11, ..facts };
        }
    }

    /// The compositor moved the keyboard to `id`, or to ClamShell's UI (see [`Session::keys_to`]).
    fn focus_keyboard(&mut self, id: Option<i64>) {
        self.keys_to(id);
    }
}

/// What the compositor knows of an open toplevel, as it reports it ([`WindowEvent::Opened`],
/// [`WindowEvent::Changed`]). Where it goes and whether it is shown are the session's ([`Window`]).
#[derive(Debug, Clone, PartialEq, Eq, Default, serde::Serialize, serde::Deserialize)]
pub struct WindowFacts {
    /// The compositor's id for it, to focus or close it by.
    pub id: i64,
    /// Its Wayland app id, or its X11 class.
    pub app_id: String,
    pub title: String,
    pub parent: Option<i64>,
    pub limits: crate::presentation::WindowLimits,
    pub role: crate::presentation::WindowRole,
    /// Whether it is an X11 window, shown through Xwayland.
    pub x11: bool,
    pub colour: Option<Arc<crate::colour::ColourDescription>>,
    /// The systemd unit its process runs in.
    pub unit: Option<String>,
    /// Its process, then that process's parent, and so on, leaving out pid 1. A program found by its tray
    /// icon's process has the windows of that process and of the processes it started.
    pub pids: Vec<u32>,
    /// The game its process's launcher marked it with, if one did (see [`super::launchers`]).
    pub game: Option<String>,
}

impl WindowFacts {
    /// Whether it is known by `mark`, as its app id or its launcher's game (see
    /// [`super::running::mark_of`]).
    pub(crate) fn marked(&self, mark: &str) -> bool {
        self.app_id == mark || self.game.as_deref() == Some(mark)
    }

    /// Whether it is a dialog: by its role, or by having a parent.
    pub(crate) fn is_dialog(&self) -> bool {
        self.role == crate::presentation::WindowRole::Dialog || self.parent.is_some()
    }

    /// The launch it belongs to: its unit's, if we started it.
    pub fn launch_unit(&self) -> Option<LaunchUnit> {
        self.unit.as_deref().and_then(LaunchUnit::from_unit)
    }

    /// Whether it is in a unit we started ([`WindowFacts::launch_unit`]), found without allocating.
    pub(crate) fn launched(&self) -> bool {
        self.unit.as_deref().is_some_and(LaunchUnit::is_launch_unit)
    }
}

/// An open toplevel as the session has it: the compositor's facts, and its home group. Which window each
/// screen shows is the session's ([`Session::top_window`], [`Session::bottom_window`]).
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Window {
    #[serde(flatten)]
    pub facts: WindowFacts,
    /// Its home group, retained while it is on the bottom screen (see [`Session::home_of`]): worked out from
    /// the facts and the claim, as it opens and at every settle.
    pub group: Option<Group>,
    /// A choice made on it, not in the facts: a browsed file's entry took it, a new window of no launch of
    /// ours (see [`Session::follow_windows`]), or a window of no program went to the program in front's
    /// group. It belongs there for as long as it is open.
    #[serde(default)]
    pub claim: Option<Group>,
    /// When it opened, in monotonic seconds: a browsed file's entry takes the windows of no program opened
    /// since its launch began (see [`Session::follow_windows`]).
    #[serde(default)]
    pub opened: u64,
}

impl std::ops::Deref for Window {
    type Target = WindowFacts;

    fn deref(&self) -> &WindowFacts {
        &self.facts
    }
}

#[cfg(test)]
mod tests;
