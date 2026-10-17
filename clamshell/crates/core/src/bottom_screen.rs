//! The bottom screen shared with programs' windows (a dual-screen emulator's second screen). Home+Down
//! moves the window shown on the top screen down (sway marks it, so it stays), Home+Up the one shown
//! below back up. The windows down there are tabs of one workspace (a tap on a tab shows it); Home+Select
//! switches the bottom screen between ClamShell's view and them (nothing without one); Home+Start turns
//! the bottom screen off and on. None of it takes the keyboard from the window in front on the top
//! screen.
//!
//! Moving a game's or a dock app's window down is remembered as a rule ([`WindowRule`]: the program, the
//! window's app id and title): the next time it opens a window like it, the window goes down by itself.
//! Moving it up forgets the rule. The rules are saved in `window-rules.txt` ([`WindowRules::text`]).

use std::fmt::Write;
use std::sync::Arc;

use crate::State;
use crate::effect::{Effect, Save, WindowCommand};
use crate::hardware::NoticeKind;
use crate::instance::Instance;
use crate::windows::Window;

/// Whose window a rule is about: a game's (by its id in the library) or a dock app's (by its key).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Owner {
    Game(Arc<str>),
    App(String),
}

/// A window that goes to the bottom screen when its program opens it: the program's, with this app id
/// and title.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WindowRule {
    pub owner: Owner,
    pub app_id: String,
    pub title: String,
}

impl WindowRule {
    fn matches(&self, owner: &Owner, w: &Window) -> bool {
        self.owner == *owner && self.app_id == w.app_id && self.title == w.title
    }
}

/// The window rules file's first line: its format and version.
pub const WINDOW_RULES_FORMAT: &str = "# ClamShell window rules, format 1";

/// The rules: which programs' windows go to the bottom screen.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct WindowRules {
    list: Vec<WindowRule>,
}

impl WindowRules {
    pub fn list(&self) -> &[WindowRule] {
        &self.list
    }

    /// As text: the format's line, then a rule per line, tab-separated: `bottom`, `game` and its id or
    /// `app` and its key, the app id, the title.
    pub fn text(&self) -> String {
        let mut text = format!("{WINDOW_RULES_FORMAT}\n");
        for r in &self.list {
            let (kind, name) = match &r.owner {
                Owner::Game(id) => ("game", id.as_ref()),
                Owner::App(key) => ("app", key.as_str()),
            };
            let _ = writeln!(text, "bottom\t{kind}\t{name}\t{}\t{}", r.app_id, r.title);
        }
        text
    }

    /// From text (lines it doesn't understand are skipped).
    pub fn load(text: &str) -> WindowRules {
        let list = text
            .lines()
            .filter(|l| !l.starts_with('#'))
            .filter_map(|line| {
                let mut f = line.splitn(5, '\t');
                let (Some("bottom"), Some(kind), Some(name), Some(app_id), Some(title)) = (f.next(), f.next(), f.next(), f.next(), f.next()) else {
                    return None;
                };
                let owner = match kind {
                    "game" => Owner::Game(name.into()),
                    "app" => Owner::App(name.to_owned()),
                    _ => return None,
                };
                Some(WindowRule { owner, app_id: app_id.to_owned(), title: title.to_owned() })
            })
            .collect();
        WindowRules { list }
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
    fn forget(&mut self, owner: &Owner, w: &Window) -> bool {
        let before = self.list.len();
        self.list.retain(|r| !r.matches(owner, w));
        self.list.len() != before
    }

    fn wants(&self, owner: &Owner, w: &Window) -> bool {
        self.list.iter().any(|r| r.matches(owner, w))
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

    /// Whose a window is, for a rule: the game's it belongs to, or the dock app's.
    fn owner(&self, w: &Window) -> Option<Owner> {
        match w.instance()? {
            Instance::App { key, .. } => Some(Owner::App(key)),
            instance @ Instance::Game { .. } => self.session.games.iter().find(|g| g.instance == instance).map(|g| Owner::Game(g.id.clone())),
            _ => None,
        }
    }

    /// Home+Down: the window in front on the top screen to the bottom screen, shown there, and
    /// remembered for its program.
    pub(crate) fn window_down(&mut self) -> Vec<Effect> {
        let Some(w) = self.session.windows.iter().find(|w| w.focused && !w.down && w.home() != Some(Instance::Shelf)).cloned() else {
            return vec![];
        };
        let mut effects = self.show_down(w.id);
        if let Some(owner) = self.owner(&w)
            && self.window_rules.add(WindowRule { owner, app_id: w.app_id.clone(), title: w.title.clone() })
        {
            self.notify(NoticeKind::BottomRule(true));
            effects.push(Effect::Save(Save::WindowRules(self.window_rules.text())));
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
            effects.push(Effect::Save(Save::WindowRules(self.window_rules.text())));
        }
        effects
    }

    /// Home+Select: the bottom screen shows the windows moved there, or ClamShell's view again; nothing
    /// without a window down.
    pub(crate) fn toggle_bottom_view(&mut self) -> Vec<Effect> {
        if self.windows_down().next().is_some() {
            self.bottom.screen.windows = !self.bottom.screen.windows;
        }
        vec![]
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
    use crate::game::{Origin, RunningGame};
    use crate::hardware::DeviceKey;
    use crate::testkit::state;

    fn win(id: i64, unit: &str, title: &str, focused: bool, down: bool) -> Window {
        Window {
            id,
            visible: true,
            app_id: "emu".into(),
            title: title.into(),
            focused,
            unit: Some(unit.into()),
            workspace: if down { Instance::Bottom.name() } else { unit.trim_end_matches(".scope").into() },
            output: if down { "DSI-1".into() } else { "DSI-2".into() },
            down,
        }
    }

    const GAME: &str = "clamshell-game-99-1.scope";

    /// A game running (shelf's), with a top window and a second window, the second in front.
    fn emulator() -> State {
        let mut s = state();
        s.session.games.push(RunningGame {
            id: "melonds".into(),
            instance: Instance::Game { secs: 99, n: 1 },
            origin: Origin::Shelf { appid: None },
            start: 0,
        });
        s.apply(Event::Windows(vec![win(1, GAME, "Top", false, false), win(2, GAME, "Bottom", true, false)]));
        s
    }

    fn key(s: &mut State, k: DeviceKey) -> Vec<Effect> {
        s.apply(Event::Device(k)).1
    }

    #[test]
    fn a_window_moved_down_is_shown_there_and_remembered() {
        let mut s = emulator();
        assert_eq!(key(&mut s, DeviceKey::BottomView), vec![], "nothing down: nothing to switch to");
        assert!(!s.bottom_shows_windows());
        let fx = key(&mut s, DeviceKey::WindowDown);
        assert_eq!(fx[0], Effect::Window(WindowCommand::Down(2)));
        assert!(matches!(&fx[1], Effect::Save(Save::WindowRules(t)) if t.ends_with("bottom\tgame\tmelonds\temu\tBottom\n")), "{fx:?}");
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
        assert_eq!(fx[0], Effect::Window(WindowCommand::Up { window: 2, to: Some(Instance::Game { secs: 99, n: 1 }) }));
        assert!(matches!(&fx[1], Effect::Save(Save::WindowRules(t)) if t == &format!("{WINDOW_RULES_FORMAT}\n")), "{fx:?}");
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
        s.window_rules = WindowRules::load(&rules.text());
        assert_eq!(s.window_rules, rules, "read back as written");
        s.session.games.push(RunningGame {
            id: "melonds".into(),
            instance: Instance::Game { secs: 99, n: 1 },
            origin: Origin::Shelf { appid: None },
            start: 0,
        });
        let fx = s.apply(Event::Windows(vec![win(5, GAME, "Top", true, false), win(6, GAME, "Bottom", false, false)])).1;
        assert!(fx.contains(&Effect::Window(WindowCommand::Down(6))), "{fx:?}");
        assert!(!fx.contains(&Effect::Window(WindowCommand::Down(5))));
        let fx = s.apply(Event::Windows(vec![win(5, GAME, "Top", true, false), win(6, GAME, "Bottom", false, false)])).1;
        assert!(!fx.contains(&Effect::Window(WindowCommand::Down(6))), "asked once");
        // Another game's window of that title: not its rule.
        s.session.games.push(RunningGame { id: "other".into(), instance: Instance::Game { secs: 5, n: 2 }, origin: Origin::Shelf { appid: None }, start: 0 });
        let fx = s.apply(Event::Windows(vec![win(7, "clamshell-game-5-2.scope", "Bottom", false, false)])).1;
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
        assert!(fx.contains(&Effect::Window(WindowCommand::Up { window: 1, to: Some(Instance::Game { secs: 99, n: 1 }) })), "the tab in view: {fx:?}");
        s.apply(Event::Windows(vec![win(1, GAME, "Top", true, false), win(2, GAME, "Bottom", false, true)]));
        assert!(s.bottom_shows_windows(), "the other tab");
        let fx = key(&mut s, DeviceKey::WindowUp);
        assert!(fx.contains(&Effect::Window(WindowCommand::Up { window: 2, to: Some(Instance::Game { secs: 99, n: 1 }) })), "{fx:?}");
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

    #[test]
    fn rules_files_skip_what_they_dont_understand() {
        let rules = WindowRules::load("# a comment\nbottom\tgame\tx\temu\tBottom screen\nbottom\tpet\tx\ta\tb\nnonsense\n");
        assert_eq!(rules.list().len(), 1);
        assert_eq!(rules.list()[0].title, "Bottom screen");
    }
}
