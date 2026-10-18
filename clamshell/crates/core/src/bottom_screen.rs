//! The bottom screen shared with programs' windows (a dual-screen emulator's second screen). Home+Down
//! moves the window shown on the top screen down (sway marks it, so it stays), Home+Up the one shown
//! below back up. The windows down there are tabs of one workspace (a tap on a tab shows it); Home+Select
//! switches the bottom screen between ClamShell's view and them (nothing without one); Home+Start turns
//! the bottom screen off and on. None of it takes the keyboard from the window in front on the top
//! screen.
//!
//! Moving a game's or a dock app's window down is remembered as a rule ([`WindowRule`]: the program and
//! the window's app id; its title too only when the program has another window of that app id open, as a
//! dual-screen emulator does, since titles change with what a window shows): the next time it opens a
//! window like it, the window goes down by itself. Moving it up forgets the rule. The rules are saved, and
//! given to sway ([`WindowCommand::DownRules`]), which places a matching window on the bottom screen as
//! it opens, before it is drawn; ClamShell then marks it as one moved down (and moves it there itself if
//! sway's rule didn't: by app id and title alone, sway can't tell whose a window is).

use std::sync::Arc;

use crate::State;
use crate::effect::{Effect, Save, WindowCommand};
use crate::hardware::NoticeKind;
use crate::instance::Instance;
use crate::windows::Window;

/// A window that goes to the bottom screen when its entry opens it: the entry's (by its id), with this
/// app id and, if said, this title.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WindowRule {
    pub owner: Arc<str>,
    pub app_id: String,
    pub title: Option<String>,
}

impl WindowRule {
    fn matches(&self, owner: &str, w: &Window) -> bool {
        &*self.owner == owner && self.down_rule().matches(w)
    }

    /// What sway can match a window by: its app id and title (not whose it is).
    pub fn down_rule(&self) -> DownRule {
        DownRule { app_id: self.app_id.clone(), title: self.title.clone() }
    }
}

/// A window that goes to the bottom screen as it opens, as sway matches it: by app id, and title if
/// said.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct DownRule {
    pub app_id: String,
    pub title: Option<String>,
}

impl DownRule {
    pub fn matches(&self, w: &Window) -> bool {
        self.app_id == w.app_id && self.title.as_ref().is_none_or(|t| *t == w.title)
    }
}

/// The rules: which programs' windows go to the bottom screen.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct WindowRules {
    list: Vec<WindowRule>,
}

impl WindowRules {
    /// These rules, as saved.
    pub fn from_list(list: Vec<WindowRule>) -> WindowRules {
        WindowRules { list }
    }

    pub fn list(&self) -> &[WindowRule] {
        &self.list
    }

    /// Adds a rule (unless there is one like it); whether it was new.
    fn add(&mut self, rule: WindowRule) -> bool {
        if self.list.contains(&rule) {
            return false;
        }
        self.list.push(rule);
        true
    }

    /// Forgets the rules a window of `owner`'s matches; whether there were any.
    fn forget(&mut self, owner: &str, w: &Window) -> bool {
        let before = self.list.len();
        self.list.retain(|r| !r.matches(owner, w));
        self.list.len() != before
    }

    fn wants(&self, owner: &str, w: &Window) -> bool {
        self.list.iter().any(|r| r.matches(owner, w))
    }

    /// For sway: what the rules match, each once.
    pub fn down_rules(&self) -> Vec<DownRule> {
        let mut out: Vec<DownRule> = Vec::new();
        for rule in self.list.iter().map(WindowRule::down_rule) {
            if !out.contains(&rule) {
                out.push(rule);
            }
        }
        out
    }
}

/// What the bottom screen shows, besides being on or off.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct BottomScreen {
    /// The windows moved down (the tab sway shows), over ClamShell's view; else ClamShell's view.
    pub(crate) windows: bool,
    /// Turned off with Home+Start.
    pub(crate) off: bool,
}

impl State {
    /// The windows moved to the bottom screen, in the compositor's order.
    pub fn windows_down(&self) -> impl Iterator<Item = &Window> {
        self.session.windows.iter().filter(|w| w.down)
    }

    /// Whether the bottom screen shows the windows moved there instead of ClamShell's view.
    pub fn bottom_shows_windows(&self) -> bool {
        self.bottom.screen.windows && self.windows_down().next().is_some()
    }

    /// Whether the bottom screen is turned off.
    pub fn bottom_off(&self) -> bool {
        self.bottom.screen.off
    }

    /// Whose a window is, for a rule: the running entry's it belongs to.
    fn owner(&self, w: &Window) -> Option<Arc<str>> {
        self.session.running_of(w).map(|r| r.id.clone())
    }

    /// Home+Down: the window in front on the top screen to the bottom screen, shown there, and
    /// remembered for its program.
    pub(crate) fn window_down(&mut self) -> Vec<Effect> {
        let Some(w) = self.session.windows.iter().find(|w| w.focused && !w.down && w.home() != Some(Instance::Shelf)).cloned() else {
            return vec![];
        };
        let mut effects = self.show_down(w.id);
        // (Its title only to tell it from another window of its program with its app id.)
        let others = self.session.windows.iter().any(|o| o.id != w.id && o.app_id == w.app_id && o.unit == w.unit);
        let title = others.then(|| w.title.clone());
        if let Some(owner) = self.owner(&w)
            && self.window_rules.add(WindowRule { owner, app_id: w.app_id.clone(), title })
        {
            self.notify(NoticeKind::BottomRule(true));
            effects.push(Effect::Save(Save::WindowRules(self.window_rules.clone())));
            effects.push(self.down_rules_effect());
        }
        effects
    }

    /// A window moved down and shown there (the bottom screen on again if it was off).
    fn show_down(&mut self, id: i64) -> Vec<Effect> {
        self.bottom.screen.windows = true;
        let mut effects = vec![Effect::Window(WindowCommand::Down(id))];
        if std::mem::take(&mut self.bottom.screen.off) {
            effects.push(Effect::Window(WindowCommand::BottomPower(true)));
        }
        effects
    }

    /// Home+Up: the window the bottom screen shows (its tab in view) back to its place on the top screen,
    /// and its rule forgotten. The bottom screen shows the next tab, or ClamShell's view after the last.
    pub(crate) fn window_up(&mut self) -> Vec<Effect> {
        if !self.bottom_shows_windows() {
            return vec![];
        }
        let Some(w) = self.windows_down().find(|w| w.visible).or_else(|| self.windows_down().next()).cloned() else {
            return vec![];
        };
        let mut effects = vec![Effect::Window(WindowCommand::Up { window: w.id, to: w.home() })];
        if let Some(owner) = self.owner(&w)
            && self.window_rules.forget(&owner, &w)
        {
            self.notify(NoticeKind::BottomRule(false));
            effects.push(Effect::Save(Save::WindowRules(self.window_rules.clone())));
            effects.push(self.down_rules_effect());
        }
        effects
    }

    /// The rules as sway is to place windows by them.
    pub(crate) fn down_rules_effect(&self) -> Effect {
        Effect::Window(WindowCommand::DownRules(self.window_rules.down_rules()))
    }

    /// Home+Select: the bottom screen shows the windows moved there, or ClamShell's view again; nothing
    /// without a window down.
    pub(crate) fn toggle_bottom_view(&mut self) -> Vec<Effect> {
        if self.windows_down().next().is_some() {
            self.bottom.screen.windows = !self.bottom.screen.windows;
        }
        vec![]
    }

    /// A window of the program in front on the top screen that is on the bottom screen, hidden by
    /// ClamShell's view (the bar then offers to show them). By the windows' scopes, which our programs run
    /// in (no allocation: the bar asks every frame).
    pub fn hidden_down(&self) -> Option<i64> {
        if self.bottom_shows_windows() {
            return None;
        }
        let front = self.session.windows.iter().find(|w| w.focused && !w.down)?.unit.as_deref().filter(|u| Instance::is_ours(u))?;
        self.windows_down().find(|w| w.unit.as_deref() == Some(front)).map(|w| w.id)
    }

    /// A tap on a program whose windows are all on the bottom screen: shown there, its first tab in view.
    pub(crate) fn show_program_down(&mut self, window: i64) -> Vec<Effect> {
        self.bottom.screen.windows = true;
        vec![Effect::Window(WindowCommand::ShowDown(window))]
    }

    /// Home+Start: the bottom screen off, or on again.
    pub(crate) fn toggle_bottom_power(&mut self) -> Vec<Effect> {
        self.bottom.screen.off = !self.bottom.screen.off;
        vec![Effect::Window(WindowCommand::BottomPower(!self.bottom.screen.off))]
    }

    /// After the windows changed: a window whose program has a rule for it goes down (once).
    pub(crate) fn follow_window_rules(&mut self) -> Vec<Effect> {
        let mut effects = Vec::new();
        let wanted: Vec<i64> = self
            .session
            .windows
            .iter()
            .filter(|w| !w.down && !self.session.sent_down.contains(&w.id))
            .filter(|w| self.owner(w).is_some_and(|o| self.window_rules.wants(&o, w)))
            .map(|w| w.id)
            .collect();
        for id in wanted {
            self.session.sent_down.push(id);
            effects.extend(self.show_down(id));
        }
        let open: Vec<i64> = self.session.windows.iter().map(|w| w.id).collect();
        self.session.sent_down.retain(|id| open.contains(id));
        effects
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::indexing_slicing)]
mod tests {
    use super::*;
    use crate::Event;
    use crate::hardware::DeviceKey;
    use crate::running::{Origin, Running};
    use crate::testkit::state;

    fn win(id: i64, unit: &str, title: &str, focused: bool, down: bool) -> Window {
        Window {
            id,
            visible: true,
            app_id: "emu".into(),
            title: title.into(),
            focused,
            unit: Some(unit.into()),
            workspace: if down { Instance::Bottom.name() } else { Instance::from_unit(unit).map_or_else(String::new, |i| i.workspace()) },
            down,
        }
    }

    const GAME: &str = "clamshell-run-melonds_1-99-1.scope";

    /// The emulator's launch.
    fn emu() -> Instance {
        Instance::Run { key: "melonds_1".into(), secs: 99, n: 1 }
    }

    fn running(id: &str, key: &str) -> Running {
        Running { id: id.into(), key: key.into(), origin: Origin::Ours { appid: None }, start: 0, launches: 1, adopted: false, entry: None }
    }

    /// A game running (shelf's), with a top window and a second window, the second in front.
    fn emulator() -> State {
        let mut s = state();
        s.session.running.push(running("melonds", "melonds_1"));
        s.apply(Event::Windows(vec![win(1, GAME, "Top", false, false), win(2, GAME, "Bottom", true, false)]));
        s
    }

    fn key(s: &mut State, k: DeviceKey) -> Vec<Effect> {
        s.apply(Event::Device(k)).1
    }

    #[test]
    fn a_programs_only_window_is_remembered_by_its_app_id_alone() {
        let mut s = state();
        s.session.running.push(running("melonds", "melonds_1"));
        s.apply(Event::Windows(vec![win(2, GAME, "Page one", true, false)]));
        let fx = key(&mut s, DeviceKey::WindowDown);
        let rule = WindowRule { owner: "melonds".into(), app_id: "emu".into(), title: None };
        assert!(matches!(&fx[1], Effect::Save(Save::WindowRules(r)) if r.list() == [rule.clone()]), "{fx:?}");
        // Another title next time (a page, a command): still its.
        assert!(rule.matches("melonds", &win(3, GAME, "Page two", true, false)));
    }

    #[test]
    fn a_window_moved_down_is_shown_there_and_remembered() {
        let mut s = emulator();
        assert_eq!(key(&mut s, DeviceKey::BottomView), vec![], "nothing down: nothing to switch to");
        assert!(!s.bottom_shows_windows());
        let fx = key(&mut s, DeviceKey::WindowDown);
        assert_eq!(fx[0], Effect::Window(WindowCommand::Down(2)));
        // (Its title too: the emulator has another window of its app id.)
        let rule = WindowRule { owner: "melonds".into(), app_id: "emu".into(), title: Some("Bottom".into()) };
        assert!(matches!(&fx[1], Effect::Save(Save::WindowRules(r)) if r.list() == [rule.clone()]), "{fx:?}");
        assert_eq!(fx[2], Effect::Window(WindowCommand::DownRules(vec![rule.down_rule()])), "for sway");
        assert_eq!(s.bottom().notice().map(|n| &n.what), Some(&NoticeKind::BottomRule(true)));
        // sway reports it down: shown there.
        s.apply(Event::Windows(vec![win(1, GAME, "Top", true, false), win(2, GAME, "Bottom", false, true)]));
        assert!(s.bottom_shows_windows());
        // Home+Select: ClamShell's view (Home+Up then does nothing), then the windows again.
        key(&mut s, DeviceKey::BottomView);
        assert!(!s.bottom_shows_windows());
        assert_eq!(key(&mut s, DeviceKey::WindowUp), vec![]);
        key(&mut s, DeviceKey::BottomView);
        assert!(s.bottom_shows_windows());
        // Up: back to its workspace, the rule forgotten, ClamShell's view below.
        let fx = key(&mut s, DeviceKey::WindowUp);
        assert_eq!(fx[0], Effect::Window(WindowCommand::Up { window: 2, to: Some(emu()) }));
        assert!(matches!(&fx[1], Effect::Save(Save::WindowRules(r)) if r.list().is_empty()), "{fx:?}");
        s.apply(Event::Windows(vec![win(1, GAME, "Top", true, false), win(2, GAME, "Bottom", false, false)]));
        assert!(!s.bottom_shows_windows(), "none down: ClamShell's view");
    }

    #[test]
    fn a_rule_sends_the_window_down_when_its_program_opens_it_again() {
        let mut s = emulator();
        key(&mut s, DeviceKey::WindowDown);
        let rules = s.window_rules.clone();
        // A fresh start with the rules: the game's windows open on top; the one the rule names goes down,
        // once.
        let mut s = state();
        s.window_rules = WindowRules::from_list(rules.list().to_vec());
        s.session.running.push(running("melonds", "melonds_1"));
        let fx = s.apply(Event::Windows(vec![win(5, GAME, "Top", true, false), win(6, GAME, "Bottom", false, false)])).1;
        assert!(fx.contains(&Effect::Window(WindowCommand::Down(6))), "{fx:?}");
        assert!(!fx.contains(&Effect::Window(WindowCommand::Down(5))));
        let fx = s.apply(Event::Windows(vec![win(5, GAME, "Top", true, false), win(6, GAME, "Bottom", false, false)])).1;
        assert!(!fx.contains(&Effect::Window(WindowCommand::Down(6))), "asked once");
        // Another game's window of that title: not its rule.
        s.session.running.push(running("other", "other_2"));
        let fx = s.apply(Event::Windows(vec![win(7, "clamshell-run-other_2-5-2.scope", "Bottom", false, false)])).1;
        assert!(!fx.iter().any(|e| matches!(e, Effect::Window(WindowCommand::Down(_)))), "{fx:?}");
    }

    #[test]
    fn two_windows_down_are_tabs_and_the_one_in_view_comes_back_up() {
        let mut s = emulator();
        key(&mut s, DeviceKey::WindowDown);
        s.apply(Event::Windows(vec![win(1, GAME, "Top", true, false), win(2, GAME, "Bottom", false, true)]));
        key(&mut s, DeviceKey::WindowDown);
        let hidden = |w: Window| Window { visible: false, ..w };
        s.apply(Event::Windows(vec![win(1, GAME, "Top", false, true), hidden(win(2, GAME, "Bottom", false, true))]));
        let fx = key(&mut s, DeviceKey::WindowUp);
        assert!(fx.contains(&Effect::Window(WindowCommand::Up { window: 1, to: Some(emu()) })), "the tab in view: {fx:?}");
        s.apply(Event::Windows(vec![win(1, GAME, "Top", true, false), win(2, GAME, "Bottom", false, true)]));
        assert!(s.bottom_shows_windows(), "the other tab");
        let fx = key(&mut s, DeviceKey::WindowUp);
        assert!(fx.contains(&Effect::Window(WindowCommand::Up { window: 2, to: Some(emu()) })), "{fx:?}");
    }

    #[test]
    fn the_front_programs_windows_hidden_below_are_offered_and_shown() {
        let mut s = emulator();
        key(&mut s, DeviceKey::WindowDown);
        s.apply(Event::Windows(vec![win(1, GAME, "Top", true, false), win(2, GAME, "Bottom", false, true)]));
        assert_eq!(s.hidden_down(), None, "shown");
        key(&mut s, DeviceKey::BottomView);
        assert_eq!(s.hidden_down(), Some(2), "ClamShell's view over them");
        let fx = s.apply(Event::Pointer(crate::PointerEvent::Activate(crate::Id::BottomWindows))).1;
        assert_eq!(fx, vec![Effect::Window(WindowCommand::ShowDown(2))]);
        assert!(s.bottom_shows_windows());
        // Another program in front: not its windows.
        key(&mut s, DeviceKey::BottomView);
        s.apply(Event::Windows(vec![
            win(1, GAME, "Top", false, false),
            win(2, GAME, "Bottom", false, true),
            win(3, "clamshell-run-x_1-1-0.scope", "x", true, false),
        ]));
        assert_eq!(s.hidden_down(), None);
    }

    #[test]
    fn the_bottom_screen_turns_off_and_on() {
        let mut s = emulator();
        assert_eq!(key(&mut s, DeviceKey::BottomPower), vec![Effect::Window(WindowCommand::BottomPower(false))]);
        assert!(s.bottom_off());
        // A window moved down turns it on again.
        let fx = key(&mut s, DeviceKey::WindowDown);
        assert!(fx.contains(&Effect::Window(WindowCommand::BottomPower(true))), "{fx:?}");
        assert!(!s.bottom_off());
    }
}
