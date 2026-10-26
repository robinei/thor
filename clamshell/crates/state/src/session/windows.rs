//! The compositor's windows, as the dock and the running entries need them. Each launch of a library entry
//! is a systemd unit we started. Every window of its processes, such as a second window or a program
//! started from its terminal, goes in the entry's group as a tab ([`Group::Entry`]). An entry that runs in
//! another program's processes, such as a Steam game or a Waydroid app, has a group too, and its windows
//! are found by their app id (see [`crate::session::running`]). A window that belongs to no entry has a
//! group of its own ([`Group::Window`]).

use std::sync::Arc;

use crate::Session;
use crate::session::instance::Instance;

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
    /// Put these windows in this group. Used for an entry known by app id, to move its windows from where
    /// they opened.
    Move { windows: Vec<i64>, to: Group },
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
            WindowCommand::KeyboardFocus(id) => {
                if self.window(*id).is_some() {
                    self.keyboard_focus = Some(*id);
                }
            }
            WindowCommand::FocusWindow(id) => self.select_top(*id),
            WindowCommand::ShowShelf => {
                self.keyboard_focus = None;
                self.top = None;
            }
            WindowCommand::Move { windows, to } => {
                for w in self.windows.iter_mut().filter(|w| windows.contains(&w.id)) {
                    w.group = Some(to.clone());
                }
            }
            WindowCommand::Down(id) => {
                if let Some(other) = self.bottom.filter(|other| other != id) {
                    self.command(WindowCommand::Return(other));
                }
                if self.keyboard_focus == Some(*id) {
                    self.keyboard_focus = None;
                }
                if self.window(*id).is_some() {
                    self.bottom = Some(*id);
                }
                if self.top == Some(*id) {
                    self.top = None;
                }
                let group = self.window(*id).and_then(|w| w.group.clone());
                if let Some(group) = group.filter(|_| self.top.is_none()) {
                    self.focus_group(&group);
                }
            }
            WindowCommand::Up(window) => {
                if self.bottom == Some(*window) {
                    self.bottom = None;
                }
                self.select_top(*window);
            }
            WindowCommand::Return(window) => {
                let group = self.window(*window).and_then(|w| w.group.clone());
                if !self.windows.iter().any(|w| !self.below(w.id) && w.group == group) {
                    self.recent.retain(|g| Some(g) != group.as_ref());
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

    /// Shows window `id` on the top screen, with the keyboard, unless it is below.
    pub(crate) fn select_top(&mut self, id: i64) {
        if self.below(id) {
            return;
        }
        let Some(group) = self.windows.iter().find(|w| w.id == id).map(|w| &w.group) else { return };
        // (A window not placed yet is selected in its group once it is.)
        if let Some(group) = group {
            match self.selected.iter_mut().find(|(g, _)| g == group) {
                Some((_, selected)) => *selected = id,
                None => self.selected.push((group.clone(), id)),
            }
        }
        self.keyboard_focus = Some(id);
        self.top = Some(id);
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
    /// Keyboard focus changed without changing which content either screen shows. Unknown ids are ignored.
    KeyboardFocus(Option<i64>),
}

impl From<WindowEvent> for crate::Event {
    fn from(event: WindowEvent) -> Self {
        Self::Window(event)
    }
}

impl crate::Session {
    /// A window opened, in `group` if it is known yet, and selected on the top screen if `focus`.
    pub(crate) fn open(&mut self, facts: WindowFacts, group: Option<Group>, focus: bool) {
        let id = facts.id;
        self.opened.push(id);
        self.windows.push(Window { facts, group });
        if focus {
            self.select_top(id);
        }
    }

    /// A window closed. If it was the one in front, its parent or another window of its group comes in
    /// front.
    pub(crate) fn close(&mut self, id: i64) {
        let parent = self.window(id).and_then(|w| w.parent).filter(|parent| self.window(*parent).is_some() && !self.below(*parent));
        if self.keyboard_focus == Some(id) {
            self.keyboard_focus = None;
        }
        let group = self.top_window().filter(|w| w.id == id).and_then(|w| w.group.clone());
        self.windows.retain(|w| w.id != id);
        if self.top == Some(id) {
            self.top = None;
        }
        if self.bottom == Some(id) {
            self.bottom = None;
        }
        if let Some(group) = group.filter(|g| self.windows.iter().any(|w| !self.below(w.id) && w.group.as_ref() == Some(g))) {
            if let Some(parent) = parent {
                self.command(WindowCommand::FocusWindow(parent));
            } else {
                self.focus(&group);
            }
        }
    }

    pub(crate) fn retitle(&mut self, id: i64, title: String) {
        if let Some(w) = self.windows.iter_mut().find(|w| w.id == id) {
            w.facts.title = title;
        }
    }

    /// A window's facts changed. Its title has an event of its own.
    pub(crate) fn change(&mut self, facts: WindowFacts) {
        if let Some(w) = self.windows.iter_mut().find(|w| w.id == facts.id) {
            w.facts = WindowFacts { title: std::mem::take(&mut w.facts.title), x11: w.facts.x11, ..facts };
        }
    }

    /// Keyboard focus moved to `id`, or to ClamShell's UI; a window that isn't open is ignored.
    pub(crate) fn focus_keyboard(&mut self, id: Option<i64>) {
        if id.is_none_or(|id| self.window(id).is_some()) {
            self.keyboard_focus = id;
        }
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
    pub limits: super::presentation::WindowLimits,
    pub role: super::presentation::WindowRole,
    /// Whether it is an X11 window, shown through Xwayland.
    pub x11: bool,
    pub colour: Option<Arc<super::presentation::ColourDescription>>,
    /// The systemd unit its process runs in.
    pub unit: Option<String>,
    /// Its process, then that process's parent, and so on, leaving out pid 1. A program found by its tray
    /// icon's process has the windows of that process and of the processes it started.
    pub pids: Vec<u32>,
}

impl WindowFacts {
    /// The launch it belongs to: its scope's, if we started it.
    pub fn instance(&self) -> Option<Instance> {
        self.unit.as_deref().and_then(Instance::from_unit)
    }

    /// Whether it is in a scope we started ([`WindowFacts::instance`]), found without allocating.
    pub(crate) fn launched(&self) -> bool {
        self.unit.as_deref().is_some_and(Instance::is_launch_unit)
    }
}

/// An open toplevel as the session has it: the compositor's facts, and its home group. Which window each
/// screen shows is the session's ([`Session::top_window`], [`Session::bottom_window`]).
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Window {
    #[serde(flatten)]
    pub facts: WindowFacts,
    /// Its home group, retained while it is on the bottom screen. None until the session has placed it: a
    /// window that belongs to no entry is placed after the event that opened it.
    pub group: Option<Group>,
}

impl std::ops::Deref for Window {
    type Target = WindowFacts;

    fn deref(&self) -> &WindowFacts {
        &self.facts
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::device::buttons::DeviceKey;
    use crate::testkit::*;
    use crate::{Effect, Event};

    /// The session's windows' facts.
    fn facts(s: &crate::State) -> Vec<WindowFacts> {
        s.session().windows().iter().map(|w| w.facts.clone()).collect()
    }

    #[test]
    fn opening_focus_respects_launch_intent_and_shown_program() {
        let mut s = state();
        let unit = "clamshell-run-one-1-1.scope";
        let first = WindowFacts { unit: Some(unit.into()), ..win(7, "one") };
        assert!(!s.session().opening_focus(&first));
        s.observed(vec![first.clone()]);
        s.applied(WindowEvent::Activate(7).into());
        let next = WindowFacts { id: 8, ..first.clone() };
        assert!(s.session().opening_focus(&next));
        let other_launch = WindowFacts { unit: Some("clamshell-run-one-1-2.scope".into()), ..next.clone() };
        assert!(!s.session().opening_focus(&other_launch));
        assert!(s.session().opening_focus(&WindowFacts { parent: Some(7), ..next.clone() }), "a dialog of the program in front");
        s.applied(Event::Home);
        assert!(!s.session().opening_focus(&next));
        assert!(!s.session().opening_focus(&WindowFacts { parent: Some(7), ..next }), "a dialog of a program behind stays behind");
        assert!(s.session().opening_focus(&win(9, "foot")));
    }

    #[test]
    fn a_window_opens_in_front_as_its_opening_focus_says() {
        let mut s = state();
        let one = launch_of("one");
        // The first window of a launch with no launch waiting for it opens behind.
        assert!(s.applied(WindowEvent::Opened(win_of(7, "one", &one)).into()).is_empty());
        assert!(s.session().shelf_in_front());
        s.applied(WindowEvent::Activate(7).into());
        // A later window of the program in front comes in front, with the keyboard.
        s.applied(WindowEvent::Opened(win_of(8, "one", &one)).into());
        assert_eq!(s.session().top_window().map(|w| w.id), Some(8));
        assert_eq!(s.session().keyboard_focus(), Some(8));
        // Another program's window comes in front at once.
        s.applied(WindowEvent::Opened(win(9, "foot")).into());
        assert_eq!(s.session().top_window().map(|w| w.id), Some(9));
    }

    #[test]
    fn lifecycle_messages_keep_one_window_and_ignore_late_messages() {
        let mut s = state();
        let window = win_of(7, "test", &launch_of("one"));
        assert!(s.applied(WindowEvent::Opened(window.clone()).into()).is_empty());
        assert_eq!(facts(&s), std::slice::from_ref(&window));
        let duplicate = WindowFacts { title: "duplicate".into(), ..window.clone() };
        assert!(s.applied(WindowEvent::Opened(duplicate).into()).is_empty());
        assert_eq!(facts(&s), std::slice::from_ref(&window));
        assert!(s.applied(WindowEvent::Title { window: 7, title: "new title".into() }.into()).is_empty());
        let renamed = WindowFacts { title: "new title".into(), ..window };
        assert_eq!(facts(&s), &[renamed]);
        assert!(s.applied(WindowEvent::Closed(7).into()).is_empty());
        assert!(s.session().windows().is_empty());
        for event in [WindowEvent::Closed(7), WindowEvent::Title { window: 7, title: "late".into() }, WindowEvent::Activate(7)] {
            assert!(s.applied(event.into()).is_empty());
            assert!(s.session().windows().is_empty());
        }
        let reused = win_of(7, "another", &launch_of("two"));
        assert!(s.applied(WindowEvent::Opened(reused.clone()).into()).is_empty());
        assert_eq!(facts(&s), &[reused]);
    }

    #[test]
    fn a_backend_batch_keeps_replacement_history_until_all_changes_are_applied() {
        let mut s = state();
        let other = launch_of("other");
        s.observed(vec![win_of(7, "test", &other)]);
        s.applied(WindowEvent::Activate(7).into());
        let replacement = win_of(8, "test", &other);
        let event = Event::WindowBatch(vec![WindowEvent::Closed(7), WindowEvent::Opened(replacement.clone())]);
        // Closing first and settling would prune this group from history. One dispatch keeps the
        // replacement present throughout settling.
        assert_eq!(s.applied(event), vec![Effect::PadRoute(crate::PadRoute::Ui)]);
        assert_eq!(facts(&s), &[replacement]);
        assert!(s.applied(Event::Home).is_empty());
        assert_eq!(s.applied(WindowEvent::Activate(8).into()), vec![Effect::PadRoute(crate::PadRoute::Game)]);
        assert_eq!(s.session().top_window().map(|w| w.id), Some(8));
    }

    #[test]
    fn backend_changes_keep_the_placement() {
        let mut s = state();
        let other = launch_of("other");
        let one = win_of(7, "one", &other);
        let two = win_of(8, "two", &other);
        s.observed(vec![one.clone(), two.clone()]);
        s.applied(WindowEvent::Activate(8).into());
        let changed = WindowFacts { title: "new title".into(), pids: vec![100], ..two };
        let windows = vec![one, changed];
        assert!(s.observed(windows.clone()).is_empty());
        assert_eq!(facts(&s), windows);
        assert!(s.observed(windows.clone()).is_empty());
        assert_eq!(facts(&s), windows);
        assert_eq!(s.session().top_window().map(|w| w.id), Some(8));
        assert!(s.applied(WindowEvent::Changed(win(99, "late")).into()).is_empty());
        assert_eq!(facts(&s), windows);
    }

    #[test]
    fn lifecycle_close_falls_back_once_and_retains_the_other_windows() {
        let mut s = state();
        let other = win_of(6, "other", &launch_of("two"));
        s.observed(vec![other.clone(), win_of(7, "test", &launch_of("one"))]);
        s.applied(WindowEvent::Activate(7).into());
        assert_eq!(s.applied(WindowEvent::Closed(7).into()), vec![Effect::PadRoute(crate::PadRoute::Ui)]);
        assert_eq!(facts(&s), std::slice::from_ref(&other));
        assert!(s.session().shelf_in_front());
        assert!(s.applied(WindowEvent::Closed(7).into()).is_empty());
        assert_eq!(facts(&s), &[other]);
    }

    #[test]
    fn activation_selects_the_window_on_its_screen() {
        let mut s = state();
        s.observed(vec![win_of(7, "test", &launch_of("one")), win_of(8, "test", &launch_of("two"))]);
        s.applied(WindowEvent::Activate(8).into());
        s.applied(Event::Device(DeviceKey::WindowDown));
        s.applied(Event::Device(DeviceKey::BottomView));
        assert_eq!(s.applied(WindowEvent::Activate(7).into()), vec![Effect::PadRoute(crate::PadRoute::Game)]);
        assert_eq!(s.session().top_window().map(|w| w.id), Some(7));
        assert_eq!(s.applied(WindowEvent::Activate(8).into()), vec![Effect::Watch(crate::Watch::Metrics, false), Effect::PadActivity(true)]);
        assert_eq!(s.session().bottom_window().map(|w| w.id), Some(8));
        assert_eq!(s.session().keyboard_focus(), Some(8));
        assert_eq!(s.session().top_window().map(|w| w.id), Some(7));
        assert!(s.bottom().shows_windows(s.session()));
    }

    /// Window 7 of launch "top" in front and window 8 of launch "bottom" below.
    fn top_and_bottom(s: &mut crate::State) {
        s.observed(vec![win_of(7, "top", &launch_of("top")), win_of(8, "bottom", &launch_of("bottom"))]);
        s.applied(WindowEvent::Activate(8).into());
        s.applied(Event::Device(DeviceKey::WindowDown));
        s.applied(WindowEvent::Activate(7).into());
    }

    #[test]
    fn keyboard_focus_is_independent_of_both_screens_selected_content() {
        let mut s = state();
        top_and_bottom(&mut s);
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
        let instance = launch(&mut s, "zelda-lttp");
        assert!(s.session().front_launch().is_some());
        s.applied(Event::Home);
        assert!(s.session().front_launch().is_none());
        s.applied(WindowEvent::Opened(win_of(7, "zelda3", &instance)).into());
        assert_eq!(s.session().front(), crate::session::running::Front::Shelf);
    }

    #[test]
    fn lifecycle_open_and_close_follow_launch_ownership() {
        let mut s = state();
        s.applied(Event::tick(100));
        let instance = launch(&mut s, "zelda-lttp");
        let window = win_of(7, "zelda3", &instance);
        assert_eq!(s.applied(WindowEvent::Opened(window.clone()).into()), vec![Effect::PadRoute(crate::PadRoute::Game)]);
        assert_eq!(s.session().windows_of("zelda-lttp").map(|w| &w.facts).collect::<Vec<_>>(), vec![&window]);
        assert_eq!(s.session().top_window().map(|w| w.id), Some(7));
        assert!(s.session().front_launch().is_none());
        assert_eq!(s.applied(WindowEvent::Closed(7).into()), vec![Effect::PadRoute(crate::PadRoute::Ui)]);
        assert!(s.session().windows_of("zelda-lttp").next().is_none());
        assert!(s.session().shelf_in_front());
    }

    #[test]
    fn activating_another_window_cancels_a_launchs_late_request_to_take_the_front() {
        let mut s = state();
        s.observed(vec![win_of(7, "other", &launch_of("other"))]);
        s.applied(WindowEvent::Activate(7).into());
        let instance = launch(&mut s, "zelda-lttp");
        assert!(s.session().front_launch().is_some());
        assert!(s.applied(WindowEvent::Activate(7).into()).is_empty());
        assert!(s.session().front_launch().is_none());
        s.applied(WindowEvent::Opened(win_of(8, "zelda3", &instance)).into());
        assert_eq!(s.session().top_window().map(|w| w.id), Some(7));
    }

    #[test]
    fn activation_thaws_a_suspended_owner_before_selecting_its_window() {
        let mut s = state();
        let instance = launch(&mut s, "zelda-lttp");
        s.observed(vec![win_of(7, "zelda3", &instance)]);
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
        s.applied(WindowEvent::Opened(win_of(7, "zelda3", &instance)).into());
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
        let group = Group::entry(&clamshell_library::key_of("zelda-lttp"));
        let unit = Instance { key: clamshell_library::key_of("zelda-lttp"), secs: 1, n: 0 };
        assert_eq!(s.applied(WindowEvent::Opened(win_of(7, "game", &unit)).into()), vec![Effect::Watch(crate::Watch::Metrics, false)]);
        assert_eq!(s.session().place(7), Some(WindowPlace::Bottom));
        assert_eq!(s.session().window(7).and_then(|w| w.group.clone()), Some(group.clone()));
        assert!(s.session().units().is_empty());
        assert_eq!(s.session().bottom_window().map(|w| w.id), Some(7));
    }

    #[test]
    fn stale_identity_facts_cannot_undo_a_placement() {
        let mut s = state();
        let instance = launch(&mut s, "zelda-lttp");
        let before = win_of(7, "game", &instance);
        s.applied(WindowEvent::Opened(before.clone()).into());
        s.applied(Event::Device(DeviceKey::WindowDown));
        let identity = WindowFacts { pids: vec![100, 50], ..before.clone() };
        assert!(s.applied(WindowEvent::Changed(identity).into()).is_empty());
        let window = s.session().window(7);
        assert_eq!(s.session().place(7), Some(WindowPlace::Bottom));
        assert_eq!(window.and_then(|w| w.group.clone()), Some(group_of(&instance)));
        assert_eq!(window.map(|w| w.pids.as_slice()), Some(&[100, 50][..]));
        assert_eq!(s.session().bottom_window().map(|w| w.id), Some(7));
    }

    #[test]
    fn a_hidden_group_keeps_its_selection_and_closing_it_selects_the_next_window() {
        let mut s = state();
        let one = launch(&mut s, "zelda-lttp");
        let two = launch(&mut s, "ff6");
        s.observed(vec![win_of(7, "game", &one), win_of(8, "game", &one), win_of(9, "other", &two)]);
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
        s.applied(WindowEvent::Opened(win_of(7, "game", &instance)).into());
        let dialog = WindowFacts { parent: Some(7), unit: Some("portal.service".into()), ..win(8, "dialog") };
        assert!(s.applied(WindowEvent::Opened(dialog).into()).is_empty());
        assert_eq!(s.session().place(8), Some(WindowPlace::Top(Some(&group_of(&instance)))));
        assert_eq!(s.session().top_window().map(|w| w.id), Some(8));
        assert_eq!(s.session().window(8).and_then(|w| s.session().running_of(w)).map(|r| &*r.id), Some("zelda-lttp"));
        assert!(s.applied(WindowEvent::Closed(7).into()).is_empty());
        assert_eq!(s.session().top_window().map(|w| w.id), Some(8));
    }

    #[test]
    fn a_dialog_of_a_window_below_is_placed_below_before_it_is_shown() {
        let mut s = state();
        let instance = launch(&mut s, "zelda-lttp");
        s.applied(WindowEvent::Opened(win_of(7, "game", &instance)).into());
        s.applied(Event::Device(DeviceKey::WindowDown));
        let dialog = WindowFacts { parent: Some(7), ..win(8, "dialog") };
        assert_eq!(s.applied(WindowEvent::Opened(dialog).into()), vec![Effect::PadRoute(crate::PadRoute::Game)]);
        assert_eq!(s.session().place(8), Some(WindowPlace::Bottom));
        assert_eq!(s.session().bottom_window().map(|w| w.id), Some(8));
        assert_eq!(s.session().top_window().map(|w| w.id), Some(7));
    }

    #[test]
    fn windows_know_their_instance() {
        let w = |unit: Option<&str>, app_id: &str| WindowFacts { unit: unit.map(Into::into), app_id: app_id.into(), ..WindowFacts::default() };
        let terminal = Instance { key: "foot_1".into(), secs: 1234, n: 7 };
        let foot = w(Some("clamshell-run-foot_1-1234-7.scope"), "foot");
        assert_eq!(foot.instance(), Some(terminal.clone()));
        // Firefox started from that terminal belongs to the terminal.
        assert_eq!(w(Some("clamshell-run-foot_1-1234-7.scope"), "firefox").instance(), Some(terminal));
        assert_eq!(w(Some("app-steam.scope"), "steam_app_620").instance(), None, "Steam's: placed by the program");
        assert_eq!(w(Some("app-steam.scope"), "steam").instance(), None, "not ours: left alone");
        // The same without allocating.
        for w in [foot, w(Some("app-steam.scope"), "steam"), w(None, "foot")] {
            assert_eq!(w.launched(), w.instance().is_some(), "{w:?}");
        }
    }

    #[test]
    fn touching_a_home_arrow_consumes_the_release_but_direct_home_still_shows_shelf() {
        let mut s = state();
        let one = launch_of("one");
        s.observed(vec![win_of(7, "one", &one), win_of(8, "one", &one)]);
        s.applied(WindowEvent::Activate(7).into());
        s.applied(Event::HomeHeld(true));
        assert!(s.applied(Event::Device(DeviceKey::WindowNext)).is_empty());
        s.applied(Event::HomeHeld(false));
        assert!(s.applied(Event::HomeTap).is_empty());
        assert_eq!(s.session().top_window().map(|w| w.id), Some(8));
        s.applied(Event::Home);
        assert!(s.session().shelf_in_front());
        s.applied(WindowEvent::Activate(8).into());
        s.applied(Event::HomeHeld(true));
        s.applied(Event::HomeHeld(false));
        s.applied(Event::HomeTap);
        assert!(s.session().shelf_in_front());
    }

    #[test]
    fn presentation_facts_and_colour_do_not_change_placement_or_selection() {
        use crate::session::presentation::{ColourDescription, ColourPrimaries, TransferFunction, WindowLayout, WindowLimits, WindowRole};
        let mut s = state();
        let one = launch_of("one");
        let window = win_of(7, "one", &one);
        s.applied(WindowEvent::Opened(window.clone()).into());
        s.applied(WindowEvent::Activate(7).into());
        let colour = ColourDescription {
            primaries: ColourPrimaries::Bt2020,
            transfer: TransferFunction::Pq,
            luminance: Some(crate::session::presentation::ColourLuminance { min: 10, max: 1000, reference: 203 }),
            target: None,
        };
        let changed =
            WindowFacts { limits: WindowLimits { min: (400, 300), max: (400, 300) }, role: WindowRole::Utility, colour: Some(Arc::new(colour)), ..window };
        for _ in 0..2 {
            assert!(s.applied(WindowEvent::Changed(changed.clone()).into()).is_empty());
            let current = s.session().window(7);
            assert_eq!(s.session().place(7), Some(WindowPlace::Top(Some(&group_of(&one)))));
            assert_eq!(current.and_then(|w| w.colour.as_deref()).copied(), Some(colour));
            assert_eq!(s.session().top_window().map(|w| w.id), Some(7));
            assert_eq!(s.session().keyboard_focus(), Some(7));
            assert_eq!(s.window_presentation(7).map(|p| p.layout), Some(WindowLayout::Centred));
        }
    }

    #[test]
    fn remembering_a_bottom_rule_does_not_move_existing_siblings_of_other_launches() {
        let mut s = state();
        let instance = crate::Instance { key: clamshell_library::key_of("desktop:foot"), secs: 100, n: 0 };
        let another = crate::Instance { key: clamshell_library::key_of("desktop:foot"), secs: 200, n: 1 };
        s.applied(Event::Units(vec![instance.clone(), another.clone()]));
        s.observed(vec![win_of(7, "foot", &instance), win_of(8, "foot", &another)]);
        s.applied(WindowEvent::Activate(7).into());
        s.applied(Event::Device(DeviceKey::WindowDown));
        assert_eq!(s.session().bottom_window().map(|w| w.id), Some(7));
        assert_eq!(s.session().top_window().map(|w| w.id), Some(8));
        // A later matching window does use that rule and displaces the bottom occupant.
        s.applied(WindowEvent::Opened(win_of(9, "foot", &another)).into());
        assert_eq!(s.session().bottom_window().map(|w| w.id), Some(9));
        assert_eq!(s.session().place(7), Some(WindowPlace::Top(Some(&group_of(&instance)))));
        assert_eq!(s.session().top_window().map(|w| w.id), Some(8));
    }

    #[test]
    fn displacing_another_programs_bottom_window_does_not_activate_it() {
        let mut s = state();
        let (one, two) = (launch_of("one"), launch_of("two"));
        s.observed(vec![win_of(7, "one", &one), win_of(8, "two", &two), win_of(9, "one", &one)]);
        s.applied(WindowEvent::Activate(8).into());
        s.applied(Event::Device(DeviceKey::WindowDown));
        s.applied(WindowEvent::Activate(7).into());
        s.applied(Event::Device(DeviceKey::WindowDown));
        // The returned foreign program receives no focus.
        assert_eq!(s.session().place(8), Some(WindowPlace::Top(Some(&group_of(&two)))));
        assert_eq!(s.session().bottom_window().map(|w| w.id), Some(7));
        assert_eq!(s.session().top_window().map(|w| w.id), Some(9));
        assert_eq!(s.session().keyboard_focus(), Some(9));
    }

    #[test]
    fn top_window_switching_wraps_within_the_program_and_skips_the_bottom_slot() {
        let mut s = state();
        let one = launch_of("one");
        s.observed(vec![win_of(7, "one", &one), win_of(8, "one", &one), win_of(9, "other", &launch_of("other"))]);
        s.applied(WindowEvent::Activate(7).into());
        assert!(s.session().has_other_top_window());
        for (action, id) in [(DeviceKey::WindowNext, 8), (DeviceKey::WindowNext, 7), (DeviceKey::WindowPrevious, 8)] {
            assert!(s.applied(Event::Device(action)).is_empty());
            assert_eq!(s.session().top_window().map(|w| w.id), Some(id));
        }
        s.applied(Event::Device(DeviceKey::WindowDown));
        assert!(!s.session().has_other_top_window());
        assert!(s.applied(Event::Device(DeviceKey::WindowNext)).is_empty());
        assert_eq!(s.session().top_window().map(|w| w.id), Some(7));
        assert_eq!(s.session().bottom_window().map(|w| w.id), Some(8));
    }

    #[test]
    fn pad_activity_returns_keyboard_focus_to_top_without_moving_windows() {
        let mut s = state();
        top_and_bottom(&mut s);
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
        s.observed(vec![win_of(7, "game", &instance), win_of(8, "second", &instance)]);
        s.applied(WindowEvent::Activate(8).into());
        s.applied(Event::Device(DeviceKey::WindowDown));
        s.applied(Event::Device(DeviceKey::BottomView));
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
        top_and_bottom(&mut s);
        s.applied(WindowEvent::KeyboardFocus(Some(8)).into());
        assert_eq!(s.session().top_window().map(|w| w.id), Some(7));
        s.applied(Event::Device(DeviceKey::WindowDown));
        assert_eq!(s.session().bottom_window().map(|w| w.id), Some(7));
        assert_eq!(s.session().place(8), Some(WindowPlace::Top(Some(&group_of(&launch_of("bottom"))))));
        assert!(s.session().shelf_in_front());
    }
}
