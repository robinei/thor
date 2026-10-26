//! The compositor's windows, as the dock and the running entries need them. Each launch of a library entry
//! is a systemd scope we started. Every window of its processes, such as a second window or a program
//! started from its terminal, goes on the entry's workspace as a tab (see [`Instance`]). An entry that
//! runs in another program's processes, such as a Steam game or a Waydroid app, gets a workspace of its own
//! too, and its windows are found by their app id (see [`crate::session::running`]).

use std::sync::Arc;

use crate::Session;
use crate::session::instance::Instance;

/// Where the session has placed a window. A top group contains all launches of one entry.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WindowPlace<'a> {
    Top(&'a str),
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
    /// Put these windows in this group. Used for an entry known by app id, to move its windows from where
    /// they opened.
    Move { windows: Vec<i64>, to: String },
    /// Bring shelf to the front.
    ShowShelf,
    /// Move a window to the bottom screen and show it there. It is marked so it stays there. The top
    /// screen keeps the keyboard.
    Down(i64),
    /// Move a window that was moved down back to the top screen, to its group or the one in front.
    Up { window: i64, to: String },
    /// Return the displaced bottom window to its home group without activating that program.
    Return { window: i64, to: String },
    /// Show on the bottom screen this window that was moved there. The top screen keeps the keyboard.
    ShowDown(i64),
}

impl Session {
    /// Carries out a placement decision on the session's windows.
    pub(crate) fn command(&mut self, command: WindowCommand) {
        if let WindowCommand::Down(id) = &command {
            self.return_bottom(*id);
        }
        match &command {
            WindowCommand::KeyboardFocus(id) => {
                if self.window(*id).is_some() {
                    self.keyboard_focus = Some(*id);
                }
            }
            WindowCommand::FocusWindow(id) => self.select_top(*id),
            WindowCommand::ShowShelf => {
                if let Some(id) = self.windows.iter().find(|w| w.is_shelf() && !w.down).map(|w| w.id) {
                    self.select_top(id);
                } else {
                    self.keyboard_focus = None;
                    for w in self.windows.iter_mut().filter(|w| !w.down) {
                        w.focused = false;
                        w.visible = false;
                    }
                }
            }
            WindowCommand::Move { windows, to } => {
                for w in self.windows.iter_mut().filter(|w| windows.contains(&w.id)) {
                    w.workspace.clone_from(to);
                }
            }
            WindowCommand::Down(id) => {
                if self.keyboard_focus == Some(*id) {
                    self.keyboard_focus = None;
                }
                let group = self.windows.iter().find(|w| w.id == *id).map(|w| w.workspace.clone());
                for w in &mut self.windows {
                    if w.down {
                        w.visible = w.id == *id;
                    }
                    if w.id == *id {
                        w.down = true;
                        w.focused = false;
                        w.visible = true;
                    }
                }
                if let Some(group) = group.filter(|_| !self.windows.iter().any(|w| w.focused && !w.down)) {
                    self.focus_group(&group);
                }
            }
            WindowCommand::Up { window, to } => {
                if let Some(w) = self.windows.iter_mut().find(|w| w.id == *window) {
                    w.down = false;
                    w.workspace.clone_from(to);
                }
                if !self.windows.iter().any(|w| w.down && w.visible)
                    && let Some(w) = self.windows.iter_mut().rev().find(|w| w.down)
                {
                    w.visible = true;
                }
                self.select_top(*window);
            }
            WindowCommand::Return { window, to } => {
                if !self.windows.iter().any(|w| !w.down && w.workspace == *to) {
                    self.recent.retain(|group| group != to);
                }
                if let Some(w) = self.windows.iter_mut().find(|w| w.id == *window) {
                    w.down = false;
                    w.visible = false;
                    w.focused = false;
                    w.workspace.clone_from(to);
                }
            }
            WindowCommand::ShowDown(id) => {
                for w in self.windows.iter_mut().filter(|w| w.down) {
                    w.visible = w.id == *id;
                }
            }
        }
        self.remember_front();
        let refocus = matches!(command, WindowCommand::Down(_)).then(|| self.top_window().map(|w| w.id)).flatten();
        if let Some(id) = refocus {
            self.command(WindowCommand::FocusWindow(id));
        }
    }

    pub(crate) fn return_bottom(&mut self, except: i64) {
        while let Some(w) = self.windows.iter().find(|w| w.down && w.id != except) {
            self.command(WindowCommand::Return { window: w.id, to: w.workspace.clone() });
        }
    }

    fn focus_group(&mut self, group: &str) {
        let id = self.group_window(group).map(|w| w.id);
        if let Some(id) = id {
            self.select_top(id);
        } else {
            for w in self.windows.iter_mut().filter(|w| !w.down) {
                w.focused = false;
                w.visible = false;
            }
        }
    }

    fn group_window(&self, group: &str) -> Option<&Window> {
        let selected = self.selected.iter().find(|(g, _)| g == group).map(|(_, id)| *id);
        self.windows.iter().filter(|w| !w.down && w.workspace == group).max_by_key(|w| (Some(w.id) == selected, w.focused, w.visible, w.id))
    }

    pub(crate) fn focus(&mut self, group: &str) {
        if let Some(id) = self.group_window(group).map(|w| w.id) {
            self.command(WindowCommand::FocusWindow(id));
        }
    }

    pub(crate) fn select_top(&mut self, id: i64) {
        let Some(group) = self.windows.iter().find(|w| w.id == id && !w.down).map(|w| &w.workspace) else { return };
        self.keyboard_focus = Some(id);
        match self.selected.iter_mut().find(|(g, _)| g == group) {
            Some((_, selected)) => *selected = id,
            None => self.selected.push((group.clone(), id)),
        }
        for w in self.windows.iter_mut().filter(|w| !w.down) {
            w.focused = w.id == id;
            w.visible = w.focused;
        }
    }
}

/// Changes to one window. Repeated opens and messages for a closed window do nothing. A backend must
/// close an id before reusing it for another window.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum WindowEvent {
    Opened(Window),
    Closed(i64),
    Title {
        window: i64,
        title: String,
    },
    /// Backend facts which can change after opening, such as a process being adopted into a scope.
    Changed(Window),
    /// Focus and visibility observed by the sway host. Missing fields are unchanged, so an identity
    /// update cannot restore an observation made before a placement decision. Only for a backend that
    /// decides presentation itself (sway); the built-in compositor shows what the state decided and never
    /// sends this, so the state stays the one writer of what is in front.
    Presentation {
        window: i64,
        focused: Option<bool>,
        visible: Option<bool>,
    },
    /// The backend's traversal order, when moving a window changes the order of the tabs.
    Order(Vec<i64>),
    /// A client's request to show its window, after the backend has checked its activation token.
    Activate(i64),
    /// Keyboard focus changed without changing which content either screen shows. Unknown ids are ignored.
    KeyboardFocus(Option<i64>),
}

impl From<WindowEvent> for crate::Event {
    fn from(event: WindowEvent) -> Self {
        Self::Window(event)
    }
}

impl crate::Session {
    pub(crate) fn on_window(&mut self, event: WindowEvent) {
        match event {
            WindowEvent::Opened(window) => {
                if self.windows.iter().any(|w| w.id == window.id) {
                    return;
                }
                if window.down {
                    self.return_bottom(window.id);
                }
                let focus = window.focused && !window.down;
                if window.focused {
                    self.keyboard_focus = Some(window.id);
                }
                let id = window.id;
                if window.down && window.visible {
                    for w in self.windows.iter_mut().filter(|w| w.down) {
                        w.visible = false;
                    }
                }
                self.opened.push(id);
                self.windows.push(window);
                if focus {
                    self.select_top(id);
                }
            }
            WindowEvent::Closed(id) => {
                let parent = self.window(id).and_then(|w| w.parent).filter(|parent| self.window(*parent).is_some_and(|w| !w.down));
                if self.keyboard_focus == Some(id) {
                    self.keyboard_focus = None;
                }
                let group = self.windows.iter().find(|w| w.id == id && w.focused && !w.down).map(|w| w.workspace.clone());
                let bottom = self.windows.iter().any(|w| w.id == id && w.down && w.visible);
                self.windows.retain(|w| w.id != id);
                if let Some(group) = group.filter(|g| self.windows.iter().any(|w| !w.down && w.workspace == *g)) {
                    if let Some(parent) = parent {
                        self.command(WindowCommand::FocusWindow(parent));
                    } else {
                        self.focus(&group);
                    }
                }
                if bottom && let Some(w) = self.windows.iter_mut().rev().find(|w| w.down) {
                    w.visible = true;
                }
            }
            WindowEvent::Title { window, title } => {
                if let Some(w) = self.windows.iter_mut().find(|w| w.id == window) {
                    w.title = title;
                }
            }
            WindowEvent::Changed(window) => {
                if let Some(w) = self.windows.iter_mut().find(|w| w.id == window.id) {
                    w.app_id = window.app_id;
                    w.unit = window.unit;
                    w.pids = window.pids;
                    w.parent = window.parent;
                    w.limits = window.limits;
                    w.role = window.role;
                    w.colour = window.colour;
                }
            }
            WindowEvent::Presentation { window, focused, visible } => {
                if self.window(window).is_some() {
                    if focused == Some(true) {
                        self.keyboard_focus = Some(window);
                    } else if focused == Some(false) && self.keyboard_focus == Some(window) {
                        self.keyboard_focus = None;
                    }
                }
                if focused == Some(true) {
                    self.select_top(window);
                }
                if visible == Some(true) && self.window(window).is_some_and(|w| w.down) {
                    for w in self.windows.iter_mut().filter(|w| w.down) {
                        w.visible = w.id == window;
                    }
                }
                if let Some(w) = self.windows.iter_mut().find(|w| w.id == window) {
                    if let Some(focused) = focused {
                        w.focused = focused && !w.down;
                    }
                    if let Some(visible) = visible {
                        w.visible = visible;
                    }
                }
            }
            WindowEvent::Order(ids) => {
                self.windows.sort_unstable_by_key(|w| ids.iter().position(|id| *id == w.id));
            }
            WindowEvent::KeyboardFocus(id) => {
                if id.is_none_or(|id| self.window(id).is_some()) {
                    self.keyboard_focus = id;
                }
            }
            WindowEvent::Activate(id) => {
                if let Some(window) = self.windows.iter().find(|w| w.id == id) {
                    let command = if window.down {
                        WindowCommand::ShowDown(id)
                    } else if window.is_shelf() {
                        WindowCommand::ShowShelf
                    } else {
                        WindowCommand::FocusWindow(id)
                    };
                    self.command(command);
                }
            }
        }
    }
}

/// The window events between two snapshots of a backend's window list, as one batch. For a backend that
/// only observes snapshots (sway) and for tests; the state's model takes events and never snapshots. The
/// backend keeps the snapshots, and the batch is built outside `apply`.
pub fn changes(previous: &[Window], windows: &[Window]) -> crate::Event {
    let mut events = Vec::new();
    for old in previous {
        if !windows.iter().any(|w| w.id == old.id) {
            events.push(WindowEvent::Closed(old.id));
        }
    }
    for window in windows {
        let Some(old) = previous.iter().find(|w| w.id == window.id) else {
            events.push(WindowEvent::Opened(window.clone()));
            continue;
        };
        if old == window {
            continue;
        }
        let mut facts = window.clone();
        facts.title.clone_from(&old.title);
        facts.workspace.clone_from(&old.workspace);
        facts.down = old.down;
        facts.focused = old.focused;
        facts.visible = old.visible;
        if old != &facts {
            events.push(WindowEvent::Changed(facts));
        }
        if old.title != window.title {
            events.push(WindowEvent::Title { window: window.id, title: window.title.clone() });
        }
        let focused = (old.focused != window.focused).then_some(window.focused);
        let visible = (old.visible != window.visible).then_some(window.visible);
        if focused.is_some() || visible.is_some() {
            events.push(WindowEvent::Presentation { window: window.id, focused, visible });
        }
    }
    if !previous.iter().map(|w| w.id).eq(windows.iter().map(|w| w.id)) {
        events.push(WindowEvent::Order(windows.iter().map(|w| w.id).collect()));
    }
    crate::Event::WindowBatch(events)
}

/// The app id of shelf's window, its own Wayland surface on the main screen.
pub const SHELF_APP_ID: &str = "clamshell.top";

/// An open toplevel. The session owns its home group, screen and selection.
#[derive(Debug, Clone, PartialEq, Eq, Default, serde::Serialize, serde::Deserialize)]
pub struct Window {
    /// The compositor's id for it, to focus or close it by.
    pub id: i64,
    /// Its Wayland app id, or its X11 class.
    pub app_id: String,
    pub title: String,
    pub parent: Option<i64>,
    pub limits: super::presentation::WindowLimits,
    pub role: super::presentation::WindowRole,
    /// Whether it is an X11 window, shown through Xwayland.
    pub x11: bool,
    pub colour: Option<Arc<super::presentation::ColourDescription>>,
    /// Whether this is the selected top content. Keyboard focus is independent: see
    /// [`Session::keyboard_focus`]. On opening, the backend may supply an initial focus observation.
    pub focused: bool,
    /// The systemd unit its process runs in.
    pub unit: Option<String>,
    /// Its process, then that process's parent, and so on, leaving out pid 1. A program found by its tray
    /// icon's process has the windows of that process and of the processes it started.
    pub pids: Vec<u32>,
    /// Its home group, retained when it is moved below. Sway translates this into a workspace name.
    pub workspace: String,
    /// Whether the session placed it on the bottom screen.
    pub down: bool,
    /// Whether it is in view: the tab its workspace shows, on a workspace that is shown.
    pub visible: bool,
}

impl Window {
    pub fn place(&self) -> WindowPlace<'_> {
        if self.down { WindowPlace::Bottom } else { WindowPlace::Top(&self.workspace) }
    }
    /// The launch it belongs to: its scope's, if we started it.
    pub fn instance(&self) -> Option<Instance> {
        self.unit.as_deref().and_then(Instance::from_unit)
    }

    /// Whose workspace it belongs on: its launch's, if we started it, or shelf's. Otherwise none, and it is
    /// left where it is; Steam's games and the like are placed by the state, which knows their entries.
    pub fn home(&self) -> Option<Instance> {
        self.instance().or_else(|| (self.app_id == SHELF_APP_ID).then_some(Instance::Shelf))
    }

    /// Whether it has a home ([`Window::home`]), found without allocating.
    pub fn has_home(&self) -> bool {
        self.launched() || self.app_id == SHELF_APP_ID
    }

    /// Whether its home is shelf's workspace ([`Window::home`]), found without allocating.
    pub fn is_shelf(&self) -> bool {
        !self.launched() && self.app_id == SHELF_APP_ID
    }

    /// Whether it is in a scope we started.
    fn launched(&self) -> bool {
        self.unit.as_deref().is_some_and(Instance::is_launch_unit)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testkit::*;
    use crate::{Effect, Event};

    #[test]
    fn compositor_opening_focus_respects_launch_intent_and_shown_program() {
        let mut s = state();
        let unit = "clamshell-run-one-1-1.scope";
        let first = Window { unit: Some(unit.into()), ..win(7, "one", "clamshell-run-one", false) };
        assert!(!s.session().opening_focus(&first));
        s.observed(vec![Window { focused: true, visible: true, ..first.clone() }]);
        let next = Window { id: 8, ..first.clone() };
        assert!(s.session().opening_focus(&next));
        let other_launch = Window { unit: Some("clamshell-run-one-1-2.scope".into()), ..next.clone() };
        assert!(!s.session().opening_focus(&other_launch));
        assert!(s.session().opening_focus(&Window { parent: Some(7), ..next.clone() }), "a dialog of the program in front");
        s.applied(Event::Home);
        assert!(!s.session().opening_focus(&next));
        assert!(!s.session().opening_focus(&Window { parent: Some(7), ..next }), "a dialog of a program behind stays behind");
        assert!(s.session().opening_focus(&win(9, "foot", "outside", false)));
    }

    #[test]
    fn lifecycle_messages_keep_one_window_and_ignore_late_messages() {
        let mut s = state();
        let window = win(7, "test", "1", false);
        assert!(s.applied(WindowEvent::Opened(window.clone()).into()).is_empty());
        assert_eq!(s.session().windows(), std::slice::from_ref(&window));
        let duplicate = Window { title: "duplicate".into(), ..window.clone() };
        assert!(s.applied(WindowEvent::Opened(duplicate).into()).is_empty());
        assert_eq!(s.session().windows(), std::slice::from_ref(&window));
        assert!(s.applied(WindowEvent::Title { window: 7, title: "new title".into() }.into()).is_empty());
        let renamed = Window { title: "new title".into(), ..window };
        assert_eq!(s.session().windows(), &[renamed]);
        assert!(s.applied(WindowEvent::Closed(7).into()).is_empty());
        assert!(s.session().windows().is_empty());
        for event in [WindowEvent::Closed(7), WindowEvent::Title { window: 7, title: "late".into() }, WindowEvent::Activate(7)] {
            assert!(s.applied(event.into()).is_empty());
            assert!(s.session().windows().is_empty());
        }
        let reused = win(7, "another", "2", false);
        assert!(s.applied(WindowEvent::Opened(reused.clone()).into()).is_empty());
        assert_eq!(s.session().windows(), &[reused]);
    }

    #[test]
    fn a_backend_batch_keeps_replacement_history_until_all_changes_are_applied() {
        let mut s = state();
        s.observed(vec![shelf_window(false), win(7, "test", "other", true)]);
        let windows = vec![shelf_window(true), win(8, "test", "other", false)];
        let event = crate::session::windows::changes(s.session().windows(), &windows);
        // Closing first and settling would prune this workspace from history and briefly show shelf.
        // One backend dispatch keeps the replacement present throughout settling.
        assert_eq!(s.applied(event), vec![Effect::PadRoute(crate::PadRoute::Ui)]);
        assert_eq!(s.session().windows(), windows);
        assert!(s.applied(Event::Home).is_empty());
        assert_eq!(s.applied(WindowEvent::Activate(8).into()), vec![Effect::PadRoute(crate::PadRoute::Game)]);
        assert_eq!(s.session().top_window().map(|w| w.id), Some(8));
    }

    #[test]
    fn backend_changes_preserve_titles_and_tab_order() {
        let mut s = state();
        let one = win(7, "one", "other", false);
        let two = win(8, "two", "other", false);
        s.observed(vec![one.clone(), two.clone()]);
        let changed = Window { title: "new title".into(), unit: Some("app-two.scope".into()), visible: true, ..two };
        let windows = vec![changed, one];
        assert_eq!(s.observed(windows.clone()), vec![Effect::PadActivity(true), Effect::PadRoute(crate::PadRoute::Game)]);
        assert_eq!(s.session().windows(), windows);
        assert!(s.observed(windows.clone()).is_empty());
        assert_eq!(s.session().windows(), windows);
        assert!(s.applied(WindowEvent::Changed(win(99, "late", "other", false)).into()).is_empty());
        assert_eq!(s.session().windows(), windows);
    }

    #[test]
    fn lifecycle_close_falls_back_once_and_retains_the_other_windows() {
        let mut s = state();
        let shelf = shelf_window(false);
        let front = win(7, "test", "1", true);
        s.observed(vec![shelf.clone(), front]);
        assert_eq!(s.applied(WindowEvent::Closed(7).into()), vec![Effect::PadRoute(crate::PadRoute::Ui)]);
        let shelf = Window { focused: true, visible: true, ..shelf };
        assert_eq!(s.session().windows(), std::slice::from_ref(&shelf));
        assert!(s.applied(WindowEvent::Closed(7).into()).is_empty());
        assert_eq!(s.session().windows(), &[shelf]);
    }

    #[test]
    fn activation_selects_the_window_on_its_screen() {
        let mut s = state();
        let top = win(7, "test", "1", false);
        let down = Window { down: true, ..win(8, "test", "clamshell-bottom", false) };
        let shelf = shelf_window(true);
        let shelf_id = shelf.id;
        s.observed(vec![shelf, top, down]);
        assert_eq!(s.applied(WindowEvent::Activate(7).into()), vec![Effect::PadRoute(crate::PadRoute::Game)]);
        assert_eq!(s.session().top_window().map(|w| w.id), Some(7));
        assert_eq!(s.applied(WindowEvent::Activate(8).into()), vec![Effect::Watch(crate::Watch::Metrics, false), Effect::PadActivity(true)]);
        assert_eq!(s.session().bottom_window().map(|w| w.id), Some(8));
        assert_eq!(s.session().keyboard_focus(), Some(8));
        assert_eq!(s.session().top_window().map(|w| w.id), Some(7));
        assert!(s.bottom().shows_windows(s.session()));
        assert_eq!(s.applied(WindowEvent::Activate(shelf_id).into()), vec![Effect::PadActivity(false), Effect::PadRoute(crate::PadRoute::Ui)]);
        assert!(s.session().shelf_in_front());
    }

    #[test]
    fn keyboard_focus_is_independent_of_both_screens_selected_content() {
        let mut s = state();
        s.observed(vec![win(7, "top", "top", true), Window { down: true, visible: true, ..win(8, "bottom", "bottom", false) }]);
        assert_eq!(s.session().keyboard_focus(), Some(7));
        for (target, effects) in [(Some(8), vec![Effect::PadActivity(true)]), (None, vec![]), (Some(7), vec![Effect::PadActivity(false)])] {
            assert_eq!(s.applied(WindowEvent::KeyboardFocus(target).into()), effects);
            assert_eq!(s.session().keyboard_focus(), target);
            assert_eq!(s.session().top_window().map(|w| w.id), Some(7));
            assert_eq!(s.session().bottom_window().map(|w| w.id), Some(8));
        }
        assert!(s.applied(WindowEvent::KeyboardFocus(Some(99)).into()).is_empty());
        assert_eq!(s.session().keyboard_focus(), Some(7));
        s.applied(WindowEvent::KeyboardFocus(Some(8)).into());
        s.applied(WindowEvent::Closed(8).into());
        assert_eq!(s.session().keyboard_focus(), None);
        assert_eq!(s.session().top_window().map(|w| w.id), Some(7));
        assert_eq!(s.session().bottom_window(), None);
    }

    #[test]
    fn home_cancels_a_launchs_late_request_to_take_the_front() {
        let mut s = state();
        s.observed(vec![shelf_window(true)]);
        let instance = launch(&mut s, "zelda-lttp");
        assert!(s.session().front_launch().is_some());
        s.applied(Event::Home);
        assert!(s.session().front_launch().is_none());
        let window = Window { unit: instance.unit(), ..win(7, "zelda3", &instance.workspace(), false) };
        s.applied(WindowEvent::Opened(window).into());
        assert_eq!(s.session().front(), crate::session::running::Front::Shelf);
    }

    #[test]
    fn lifecycle_open_and_close_follow_launch_ownership() {
        let mut s = state();
        s.applied(Event::tick(100));
        let instance = launch(&mut s, "zelda-lttp");
        let window = Window { unit: instance.unit(), ..win(7, "zelda3", &instance.workspace(), false) };
        assert_eq!(s.applied(WindowEvent::Opened(window.clone()).into()), vec![Effect::PadRoute(crate::PadRoute::Game)]);
        let window = Window { focused: true, visible: true, ..window };
        assert_eq!(s.session().windows_of("zelda-lttp").collect::<Vec<_>>(), vec![&window]);
        assert!(s.session().front_launch().is_none());
        assert_eq!(s.applied(WindowEvent::Closed(7).into()), vec![Effect::PadRoute(crate::PadRoute::Ui)]);
        assert!(s.session().windows_of("zelda-lttp").next().is_none());
        assert!(s.session().shelf_in_front());
    }

    #[test]
    fn activating_another_window_cancels_a_launchs_late_request_to_take_the_front() {
        let mut s = state();
        s.observed(vec![win(7, "other", "other", true)]);
        let instance = launch(&mut s, "zelda-lttp");
        assert!(s.session().front_launch().is_some());
        assert!(s.applied(WindowEvent::Activate(7).into()).is_empty());
        assert!(s.session().front_launch().is_none());
        let window = Window { unit: instance.unit(), ..win(8, "zelda3", &instance.workspace(), false) };
        s.applied(WindowEvent::Opened(window).into());
        assert_eq!(s.session().top_window().map(|w| w.id), Some(7));
    }

    #[test]
    fn activation_thaws_a_suspended_owner_before_selecting_its_window() {
        let mut s = state();
        let instance = launch(&mut s, "zelda-lttp");
        s.observed(vec![shelf_window(false), Window { unit: instance.unit(), ..win(7, "zelda3", &instance.workspace(), true) }]);
        s.applied(Event::Entry("zelda-lttp".into(), crate::EntryAct::Suspend));
        assert!(s.session().suspended("zelda-lttp"));
        // The client must be able to draw when the compositor shows it.
        assert_eq!(
            s.applied(WindowEvent::Activate(7).into()),
            vec![
                Effect::Freeze { slice: Instance::slice_of(&clamshell_library::key_of("zelda-lttp")), frozen: false },
                Effect::PadRoute(crate::PadRoute::Game),
            ]
        );
        assert!(!s.session().suspended("zelda-lttp"));
        assert_eq!(s.session().top_window().map(|w| w.id), Some(7));
        assert_eq!(s.session().keyboard_focus(), Some(7));
        assert!(s.applied(WindowEvent::Activate(7).into()).iter().all(|effect| !matches!(effect, Effect::Freeze { .. })));
    }

    #[test]
    fn lifecycle_title_applies_a_bottom_rule_once() {
        use crate::bottom::screen::{WindowRule, WindowRules};
        let rules = WindowRules::from_list(vec![WindowRule { owner: "zelda-lttp".into(), app_id: "zelda3".into(), title: Some("second".into()) }]);
        let mut s = started(crate::Init { window_rules: rules, ..init() });
        s.applied(Event::tick(100));
        let instance = launch(&mut s, "zelda-lttp");
        let window = Window { unit: instance.unit(), ..win(7, "zelda3", &instance.workspace(), false) };
        s.applied(WindowEvent::Opened(window).into());
        let event = || WindowEvent::Title { window: 7, title: "second".into() }.into();
        let effects = s.applied(event());
        assert_eq!(effects, vec![Effect::Watch(crate::Watch::Metrics, false), Effect::PadRoute(crate::PadRoute::Ui)]);
        assert_eq!(s.session().bottom_window().map(|w| w.id), Some(7));
        assert!(s.session().shelf_in_front());
        assert_eq!(s.session().windows().first().map(|w| w.title.as_str()), Some("second"));
        assert!(s.bottom().shows_windows(s.session()));
        assert!(s.applied(event()).is_empty());
    }

    #[test]
    fn a_scope_window_and_its_rule_are_placed_before_the_scope_is_listed() {
        use crate::bottom::screen::{WindowRule, WindowRules};
        let rules = WindowRules::from_list(vec![WindowRule { owner: "zelda-lttp".into(), app_id: "game".into(), title: None }]);
        let mut s = started(crate::Init { window_rules: rules, ..init() });
        let group = Instance::workspace_of(&clamshell_library::key_of("zelda-lttp"));
        let unit = Instance::Run { key: clamshell_library::key_of("zelda-lttp"), secs: 1, n: 0 };
        let window = Window { id: 7, app_id: "game".into(), unit: unit.unit(), ..Window::default() };
        assert_eq!(s.applied(WindowEvent::Opened(window).into()), vec![Effect::Watch(crate::Watch::Metrics, false)]);
        assert_eq!(s.session().window(7).map(Window::place), Some(WindowPlace::Bottom));
        assert_eq!(s.session().window(7).map(|w| w.workspace.as_str()), Some(group.as_str()));
        assert!(s.session().units().is_empty());
        assert_eq!(s.session().bottom_window().map(|w| w.id), Some(7));
    }

    #[test]
    fn stale_identity_facts_cannot_undo_a_placement() {
        let mut s = state();
        let instance = launch(&mut s, "zelda-lttp");
        let before = Window { unit: instance.unit(), ..win(7, "game", &instance.workspace(), true) };
        s.applied(WindowEvent::Opened(before.clone()).into());
        s.applied(Event::Device(crate::device::buttons::DeviceKey::WindowDown));
        let identity = Window { pids: vec![100, 50], ..before.clone() };
        let event = crate::session::windows::changes(&[before], &[identity]);
        assert!(s.applied(event).is_empty());
        let window = s.session().window(7);
        assert_eq!(window.map(Window::place), Some(WindowPlace::Bottom));
        assert_eq!(window.map(|w| w.workspace.as_str()), Some(instance.workspace().as_str()));
        assert_eq!(window.map(|w| w.pids.as_slice()), Some(&[100, 50][..]));
        assert_eq!(s.session().bottom_window().map(|w| w.id), Some(7));
    }

    #[test]
    fn a_hidden_group_keeps_its_selection_and_closing_it_selects_the_next_window() {
        let mut s = state();
        let one = launch(&mut s, "zelda-lttp");
        let two = launch(&mut s, "ff6");
        s.observed(vec![
            Window { unit: one.unit(), ..win(7, "game", &one.workspace(), false) },
            Window { unit: one.unit(), ..win(8, "game", &one.workspace(), false) },
            Window { unit: two.unit(), ..win(9, "other", &two.workspace(), true) },
        ]);
        s.applied(WindowEvent::Activate(7).into());
        show(&mut s, "ff6");
        assert_eq!(s.session().top_window().map(|w| w.id), Some(9));
        show(&mut s, "zelda-lttp");
        assert_eq!(s.session().top_window().map(|w| w.id), Some(7));
        s.applied(WindowEvent::Closed(7).into());
        assert_eq!(s.session().top_window().map(|w| w.id), Some(8));
    }

    #[test]
    fn a_dialog_inherits_its_parent_and_survives_the_parent_closing() {
        let mut s = state();
        let instance = launch(&mut s, "zelda-lttp");
        s.applied(WindowEvent::Opened(Window { unit: instance.unit(), ..win(7, "game", &instance.workspace(), true) }).into());
        let dialog = Window { id: 8, app_id: "dialog".into(), parent: Some(7), unit: Some("portal.service".into()), focused: true, ..Window::default() };
        assert!(s.applied(WindowEvent::Opened(dialog).into()).is_empty());
        assert_eq!(s.session().window(8).map(Window::place), Some(WindowPlace::Top(&instance.workspace())));
        assert_eq!(s.session().top_window().map(|w| w.id), Some(8));
        assert_eq!(s.session().window(8).and_then(|w| s.session().running_of(w)).map(|r| &*r.id), Some("zelda-lttp"));
        assert!(s.applied(WindowEvent::Closed(7).into()).is_empty());
        assert_eq!(s.session().top_window().map(|w| w.id), Some(8));
    }

    #[test]
    fn a_dialog_of_a_window_below_is_placed_below_before_it_is_shown() {
        let mut s = state();
        let instance = launch(&mut s, "zelda-lttp");
        s.applied(WindowEvent::Opened(Window { unit: instance.unit(), ..win(7, "game", &instance.workspace(), true) }).into());
        s.applied(Event::Device(crate::device::buttons::DeviceKey::WindowDown));
        let dialog = Window { id: 8, app_id: "dialog".into(), parent: Some(7), focused: true, ..Window::default() };
        assert_eq!(s.applied(WindowEvent::Opened(dialog).into()), vec![Effect::PadRoute(crate::PadRoute::Game)]);
        assert_eq!(s.session().window(8).map(Window::place), Some(WindowPlace::Bottom));
        assert_eq!(s.session().bottom_window().map(|w| w.id), Some(8));
        assert_eq!(s.session().top_window().map(|w| w.id), Some(7));
    }

    #[test]
    fn windows_know_their_instance_and_home() {
        let w = |unit: Option<&str>, app_id: &str| Window { unit: unit.map(Into::into), app_id: app_id.into(), ..Window::default() };
        let terminal = Instance::Run { key: "foot_1".into(), secs: 1234, n: 7 };
        let foot = w(Some("clamshell-run-foot_1-1234-7.scope"), "foot");
        assert_eq!(foot.instance(), Some(terminal.clone()));
        assert_eq!(foot.home(), Some(terminal.clone()));
        // Firefox started from that terminal belongs to the terminal.
        assert_eq!(w(Some("clamshell-run-foot_1-1234-7.scope"), "firefox").home(), Some(terminal));
        assert_eq!(w(Some("app-steam.scope"), "steam_app_620").home(), None, "Steam's: placed by the program");
        assert_eq!(w(Some("app-steam.scope"), "steam").home(), None, "not ours: left alone");
        assert_eq!(w(Some("clamshell.service"), SHELF_APP_ID).home(), Some(Instance::Shelf));
        // The same without allocating.
        let windows = [foot, w(Some("app-steam.scope"), "steam"), w(Some("clamshell.service"), SHELF_APP_ID), w(None, SHELF_APP_ID)];
        for w in &windows {
            assert_eq!((w.has_home(), w.is_shelf()), (w.home().is_some(), w.home() == Some(Instance::Shelf)), "{w:?}");
        }
    }

    #[test]
    fn touching_a_home_arrow_consumes_the_release_but_direct_home_still_shows_shelf() {
        let mut s = state();
        s.observed(vec![shelf_window(false), win(7, "one", "one", true), win(8, "one", "one", false)]);
        s.applied(Event::HomeHeld(true));
        assert!(s.applied(Event::Device(crate::device::buttons::DeviceKey::WindowNext)).is_empty());
        s.applied(Event::HomeHeld(false));
        assert!(s.applied(Event::HomeTap).is_empty());
        assert_eq!(s.session().top_window().map(|w| w.id), Some(8));
        s.applied(Event::Home);
        assert!(s.session().shelf_in_front());
        s.applied(WindowEvent::Activate(8).into());
        s.applied(Event::HomeHeld(true));
        s.applied(Event::HomeHeld(false));
        s.applied(Event::HomeTap);
        assert!(s.session().top_window().is_some_and(Window::is_shelf));
    }

    #[test]
    fn presentation_facts_and_colour_do_not_change_placement_or_selection() {
        use crate::session::presentation::{ColourDescription, ColourPrimaries, TransferFunction, WindowLayout, WindowLimits, WindowRole};
        let mut s = state();
        let window = win(7, "one", "one", true);
        s.applied(WindowEvent::Opened(window.clone()).into());
        let colour = ColourDescription {
            primaries: ColourPrimaries::Bt2020,
            transfer: TransferFunction::Pq,
            luminance: Some(crate::session::presentation::ColourLuminance { min: 10, max: 1000, reference: 203 }),
            target: None,
        };
        let changed = Window { limits: WindowLimits { min: (400, 300), max: (400, 300) }, role: WindowRole::Utility, colour: Some(Arc::new(colour)), ..window };
        for _ in 0..2 {
            assert!(s.applied(WindowEvent::Changed(changed.clone()).into()).is_empty());
            let current = s.session().window(7);
            assert_eq!(current.map(Window::place), Some(WindowPlace::Top("one")));
            assert_eq!(current.and_then(|w| w.colour.as_deref()).copied(), Some(colour));
            assert_eq!(s.session().top_window().map(|w| w.id), Some(7));
            assert_eq!(s.session().keyboard_focus(), Some(7));
            assert_eq!(s.window_presentation(7).map(|p| p.layout), Some(WindowLayout::Centred));
        }
    }

    #[test]
    fn remembering_a_bottom_rule_does_not_move_existing_siblings_of_other_launches() {
        let mut s = state();
        let instance = crate::Instance::Run { key: clamshell_library::key_of("desktop:foot"), secs: 100, n: 0 };
        let another = crate::Instance::Run { key: clamshell_library::key_of("desktop:foot"), secs: 200, n: 1 };
        s.applied(Event::Units(vec![instance.clone(), another.clone()]));
        s.observed(vec![
            Window { unit: instance.unit(), ..win(7, "foot", &instance.workspace(), true) },
            Window { unit: another.unit(), ..win(8, "foot", &another.workspace(), false) },
        ]);
        s.applied(Event::Device(crate::device::buttons::DeviceKey::WindowDown));
        assert_eq!(s.session().bottom_window().map(|w| w.id), Some(7));
        assert_eq!(s.session().top_window().map(|w| w.id), Some(8));
        // A later matching window does use that rule and displaces the bottom occupant.
        s.applied(WindowEvent::Opened(Window { unit: another.unit(), ..win(9, "foot", &another.workspace(), false) }).into());
        assert_eq!(s.session().bottom_window().map(|w| w.id), Some(9));
        assert_eq!(s.session().window(7).map(Window::place), Some(WindowPlace::Top(&instance.workspace())));
        assert_eq!(s.session().top_window().map(|w| w.id), Some(8));
        assert_eq!(s.session().windows_down().count(), 1);
    }

    #[test]
    fn displacing_another_programs_bottom_window_does_not_activate_it() {
        let mut s = state();
        s.observed(vec![shelf_window(false), win(7, "one", "one", true), win(8, "two", "two", false), win(9, "one", "one", false)]);
        s.applied(WindowEvent::Activate(8).into());
        s.applied(Event::Device(crate::device::buttons::DeviceKey::WindowDown));
        s.applied(WindowEvent::Activate(7).into());
        s.applied(Event::Device(crate::device::buttons::DeviceKey::WindowDown));
        // The returned foreign program receives no focus.
        assert_eq!(s.session().window(8).map(Window::place), Some(WindowPlace::Top("two")));
        assert_eq!(s.session().bottom_window().map(|w| w.id), Some(7));
        assert_eq!(s.session().top_window().map(|w| w.id), Some(9));
        assert_eq!(s.session().keyboard_focus(), Some(9));
    }

    #[test]
    fn top_window_switching_wraps_within_the_program_and_skips_the_bottom_slot() {
        let mut s = state();
        s.observed(vec![win(7, "one", "one", true), win(8, "one", "one", false), win(9, "other", "other", false)]);
        assert!(s.session().has_other_top_window());
        for (action, id) in [
            (crate::device::buttons::DeviceKey::WindowNext, 8),
            (crate::device::buttons::DeviceKey::WindowNext, 7),
            (crate::device::buttons::DeviceKey::WindowPrevious, 8),
        ] {
            assert!(s.applied(Event::Device(action)).is_empty());
            assert_eq!(s.session().top_window().map(|w| w.id), Some(id));
        }
        s.applied(Event::Device(crate::device::buttons::DeviceKey::WindowDown));
        assert!(!s.session().has_other_top_window());
        assert!(s.applied(Event::Device(crate::device::buttons::DeviceKey::WindowNext)).is_empty());
        assert_eq!(s.session().top_window().map(|w| w.id), Some(7));
        assert_eq!(s.session().bottom_window().map(|w| w.id), Some(8));
    }

    #[test]
    fn pad_activity_returns_keyboard_focus_to_top_without_moving_windows() {
        let mut s = state();
        s.observed(vec![win(7, "one", "one", true), Window { down: true, visible: true, ..win(8, "two", "two", false) }]);
        assert_eq!(s.applied(WindowEvent::KeyboardFocus(Some(8)).into()), vec![Effect::PadActivity(true)]);
        // Focus returns to the top window and the activity watch is disarmed; another focus loss can then
        // arm it.
        assert_eq!(s.applied(Event::PadActive), vec![Effect::PadActivity(false)]);
        assert_eq!(s.session().keyboard_focus(), Some(7));
        assert_eq!(s.session().bottom_window().map(|w| w.id), Some(8));
        assert!(s.applied(Event::PadActive).is_empty());
        for held in [true, true, false, false] {
            assert!(s.applied(Event::HomeHeld(held)).is_empty());
            assert_eq!(s.session().home_held(), held);
        }
    }

    #[test]
    fn activating_a_program_reveals_its_bottom_window_without_moving_or_focusing_it() {
        let mut s = state();
        let instance = launch(&mut s, "zelda-lttp");
        s.observed(vec![
            Window { unit: instance.unit(), ..win(7, "game", &instance.workspace(), true) },
            Window { unit: instance.unit(), ..win(8, "second", &instance.workspace(), false) },
        ]);
        s.applied(WindowEvent::Activate(8).into());
        s.applied(Event::Device(crate::device::buttons::DeviceKey::WindowDown));
        s.applied(Event::Device(crate::device::buttons::DeviceKey::BottomView));
        assert!(!s.bottom().shows_windows(s.session()));
        assert!(show(&mut s, "zelda-lttp").is_empty());
        assert_eq!(s.session().keyboard_focus(), Some(7));
        assert_eq!(s.session().top_window().map(|w| w.id), Some(7));
        assert_eq!(s.session().bottom_window().map(|w| w.id), Some(8));
        assert!(s.bottom().shows_windows(s.session()));
        s.applied(WindowEvent::Closed(8).into());
        assert_eq!(s.session().top_window().map(|w| w.id), Some(7));
        assert_eq!(s.session().bottom_window(), None);
        assert!(!s.bottom().shows_windows(s.session()));
    }

    #[test]
    fn moving_the_top_window_does_not_require_keyboard_focus_there() {
        let mut s = state();
        s.observed(vec![shelf_window(false), win(7, "one", "one", true), Window { down: true, visible: true, ..win(8, "two", "two", false) }]);
        s.applied(WindowEvent::Presentation { window: 7, focused: Some(false), visible: None }.into());
        s.applied(WindowEvent::KeyboardFocus(Some(8)).into());
        assert_eq!(s.session().top_window().map(|w| w.id), Some(7));
        s.applied(Event::Device(crate::device::buttons::DeviceKey::WindowDown));
        assert_eq!(s.session().bottom_window().map(|w| w.id), Some(7));
        assert_eq!(s.session().window(8).map(Window::place), Some(WindowPlace::Top("two")));
        assert!(s.session().top_window().is_some_and(Window::is_shelf));
        assert_eq!(s.session().windows_down().count(), 1);
    }
}
