//! The bottom screen shared with programs' windows, such as a dual-screen emulator's second screen.
//!
//! Home+Down moves the window shown on the top screen down, and sway marks it so it stays there. Home+Up
//! moves the one shown below back up. The windows on the bottom screen are tabs of one workspace, and a
//! tap on a tab shows it. Home+Select switches the bottom screen between ClamShell's view and those
//! windows, and does nothing if there are none. Home+Start turns the bottom screen off and on. None of
//! this takes the keyboard from the window in front on the top screen.
//!
//! Moving a game's or a dock app's window down is remembered as a rule ([`WindowRule`]), so the next
//! time the program opens a window like it, the window goes down by itself. Moving it up forgets the
//! rule. A rule holds the program and the window's app id. It holds the title too, but only when the
//! program has another window with that app id open, as a dual-screen emulator does, since titles change
//! with what a window shows.
//!
//! The rules are saved and given to sway ([`WindowCommand::DownRules`]), which places a matching window
//! on the bottom screen as it opens, before it is drawn. ClamShell then marks it as moved down. If sway's
//! rule didn't move it, ClamShell moves it itself, since by app id and title alone sway can't tell which
//! program a window belongs to.

use std::sync::Arc;

use crate::bottom::Bottom;
use crate::bottom::notice::NoticeKind;
use crate::effect::{Effect, Save, WindowCommand};
use crate::out::Out;
use crate::session::instance::Instance;
use crate::session::windows::Window;
use crate::settings::BottomWhenIdle;
use crate::{Clock, Session};

/// A window that goes to the bottom screen when its entry opens it. The rule names the entry by its id,
/// the app id, and optionally the title.
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

    /// What sway can match a window by: its app id and title, but not which program it belongs to.
    pub fn down_rule(&self) -> DownRule {
        DownRule { app_id: self.app_id.clone(), title: self.title.clone() }
    }
}

/// A window that goes to the bottom screen as it opens, as sway matches it: by app id, and by title if
/// one is given.
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

/// The rules for which programs' windows go to the bottom screen.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct WindowRules {
    list: Vec<WindowRule>,
}

impl WindowRules {
    /// The rules from their saved list.
    pub fn from_list(list: Vec<WindowRule>) -> WindowRules {
        WindowRules { list }
    }

    pub fn list(&self) -> &[WindowRule] {
        &self.list
    }

    /// Adds a rule unless there is one like it. Returns whether it did.
    fn add(&mut self, rule: WindowRule) -> bool {
        if self.list.contains(&rule) {
            return false;
        }
        self.list.push(rule);
        true
    }

    /// Forgets the rules that match a window of `owner`. Returns whether there were any.
    fn forget(&mut self, owner: &str, w: &Window) -> bool {
        let before = self.list.len();
        self.list.retain(|r| !r.matches(owner, w));
        self.list.len() != before
    }

    /// The rules in the form sway uses to place windows.
    pub(crate) fn effect(&self) -> Effect {
        let mut rules = Vec::new();
        self.down_rules(&mut rules);
        Effect::Window(WindowCommand::DownRules(rules))
    }

    fn wants(&self, owner: &str, w: &Window) -> bool {
        self.list.iter().any(|r| r.matches(owner, w))
    }

    /// Appends what the rules match, for sway, without duplicates, to `out`.
    pub fn down_rules(&self, out: &mut Vec<DownRule>) {
        for rule in self.list.iter().map(WindowRule::down_rule) {
            if !out.contains(&rule) {
                out.push(rule);
            }
        }
    }
}

/// What the bottom screen shows, besides being on or off.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct BottomScreen {
    /// Whether the bottom screen shows the windows moved down (the tab sway shows) over ClamShell's
    /// view, rather than ClamShell's view.
    windows: bool,
    /// Turned off with Home+Start.
    off: bool,
    /// Turned off by itself, when idle or dark on battery ([`Bottom::follow_power`]).
    idle_off: bool,
}

/// What the second screen's power follows from the rest of the state (see [`Bottom::follow_power`]).
#[derive(Debug, Clone, Copy)]
pub(crate) struct Power {
    /// Windows are moved to the bottom screen.
    pub any_down: bool,
    /// A program is in force.
    pub in_game: bool,
    /// The setting in force for the second screen.
    pub when_idle: BottomWhenIdle,
    /// Running on battery.
    pub discharging: bool,
    /// A refresh calibration is running.
    pub refreshing: bool,
}

/// How long, in seconds, the second screen goes untouched and without the pad's focus before it dims,
/// and after that before it powers off by itself. It only powers off on battery and when the setting
/// isn't "Screen on" (see [`Bottom::follow_power`]).
const DIM_AFTER: u64 = 60;
const OFF_AFTER: u64 = 180;

impl Bottom {
    /// Whether the second screen is dark. It is when the program in force (`in_game`) wants it dark and
    /// the screen hasn't been tapped since.
    pub(crate) fn dark(&self, in_game: bool, when_idle: BottomWhenIdle) -> bool {
        in_game && when_idle == BottomWhenIdle::DarkInGame && !self.woken
    }

    /// Whether the second screen is drawn dimmed. It is when it shows ClamShell's view and has gone
    /// untouched, without the pad's focus, for [`DIM_AFTER`]. Windows moved there are never dimmed, since
    /// they are watched without touching. This spares the OLED while a game played with the pad on the
    /// main screen keeps the device awake and nobody looks below. A touch on it, or the pad's focus
    /// coming to it, brings it back. Kept by [`Bottom::follow_dim`], which runs after every event.
    pub fn dimmed(&self) -> bool {
        self.dimmed
    }

    /// Seconds since the second screen was last used at `now`, or 0 before the clock's first reading.
    fn idle_for(&self, now: u64) -> u64 {
        self.used_at.map_or(0, |at| now.saturating_sub(at))
    }

    /// The second screen is in use, so it is lit.
    pub(crate) fn used(&mut self, session: &Session, clock: &Clock) {
        self.used_at = Some(clock.mono());
        self.follow_dim(session, clock);
    }

    /// Runs after any event. The time counts from the first clock reading.
    pub(crate) fn follow_dim(&mut self, session: &Session, clock: &Clock) {
        if self.used_at.is_none() && clock.read() {
            self.used_at = Some(clock.mono());
        }
        self.dimmed = !self.shows_windows(session) && self.idle_for(clock.mono()) >= DIM_AFTER;
    }

    /// Whether the bottom screen shows the windows moved there instead of ClamShell's view.
    pub fn shows_windows(&self, session: &Session) -> bool {
        self.shows(session.windows_down().next().is_some())
    }

    /// Whether the bottom screen shows the windows moved there, when there are any (`any_down`).
    fn shows(&self, any_down: bool) -> bool {
        self.screen.windows && any_down
    }

    /// Whether the bottom screen is turned off, by hand or by itself.
    pub fn off(&self) -> bool {
        self.screen.off || self.screen.idle_off
    }

    /// Whether the bottom screen should be off by itself at `now`. That needs all of: running on battery,
    /// the setting in force not "Screen on", no refresh calibration running, ClamShell's view on it rather
    /// than a window moved down, and nothing being typed there. On top of that, the screen must be dark
    /// for a game and not tapped since, or untouched and without the pad's focus for [`DIM_AFTER`] plus
    /// [`OFF_AFTER`]. The panel is driven in video mode and streamed every frame whatever it shows, so
    /// only turning it off saves power.
    fn idle_wanted(&self, p: Power, now: u64) -> bool {
        let untouched = self.idle_for(now) >= DIM_AFTER + OFF_AFTER;
        p.discharging
            && !p.refreshing
            && p.when_idle != BottomWhenIdle::On
            && !self.shows(p.any_down)
            && self.editor().is_none()
            && (self.dark(p.in_game, p.when_idle) || untouched)
    }

    /// Runs after any event. Turns the bottom screen off or on by itself as [`Bottom::idle_wanted`] says.
    /// A touch, the pad's focus, a window moved down or the charger turn it on again. The screen is told
    /// after the event (see [`crate::sync::Sent::outputs`]).
    pub(crate) fn follow_power(&mut self, p: Power, now: u64) {
        self.screen.idle_off = self.idle_wanted(p, now);
    }

    /// The darkened second screen was tapped awake. This lasts until the next game.
    pub(crate) fn wake(&mut self) {
        self.woken = true;
    }

    /// A game started, so the screen may go dark again.
    pub(crate) fn unwake(&mut self) {
        self.woken = false;
    }

    /// Home+Down moves the window in front on the top screen to the bottom screen, shows it there, and
    /// remembers it for its program.
    pub(crate) fn window_down(&mut self, session: &Session, now: u64, out: &mut Out) {
        let Some(w) = session.windows().iter().find(|w| w.focused && !w.down && !w.is_shelf()) else {
            return;
        };
        self.show_down(w.id, out);
        // Sent down already, so the rule added below doesn't send it again.
        if !self.sent_down.contains(&w.id) {
            self.sent_down.push(w.id);
        }
        // The title is only used to tell it apart from another window of its program with the same
        // app id.
        let others = session.windows().iter().any(|o| o.id != w.id && o.app_id == w.app_id && o.unit == w.unit);
        let title = others.then(|| w.title.clone());
        if let Some(owner) = session.owner_of(w)
            && self.window_rules.add(WindowRule { owner, app_id: w.app_id.clone(), title })
        {
            self.window_rules_changed(true, now, out);
        }
    }

    /// Moves a window down and shows it there, turning the bottom screen on again if it was off.
    fn show_down(&mut self, id: i64, out: &mut Out) {
        self.screen.windows = true;
        out.effects.push(Effect::Window(WindowCommand::Down(id)));
        self.screen.off = false;
    }

    /// Home+Up moves the window the bottom screen shows (the tab in view) back to its place on the top
    /// screen, and forgets its rule. The bottom screen then shows the next tab, or ClamShell's view after
    /// the last one.
    pub(crate) fn window_up(&mut self, session: &Session, now: u64, out: &mut Out) {
        if !self.shows_windows(session) {
            return;
        }
        let Some(w) = session.windows_down().find(|w| w.visible).or_else(|| session.windows_down().next()) else {
            return;
        };
        out.effects.push(Effect::Window(WindowCommand::Up { window: w.id, to: w.home() }));
        if let Some(owner) = session.owner_of(w)
            && self.window_rules.forget(&owner, w)
        {
            self.window_rules_changed(false, now, out);
        }
    }

    /// A rule was added or forgotten. That is shown in a notice, and the rules are saved and given to
    /// sway.
    fn window_rules_changed(&mut self, added: bool, now: u64, out: &mut Out) {
        self.notice.show(NoticeKind::BottomRule(added), now);
        out.effects.push(Effect::Save(Save::WindowRules(self.window_rules.clone())));
        out.effects.push(self.window_rules.effect());
    }

    /// Home+Select switches the bottom screen between the windows moved there and ClamShell's view. It
    /// does nothing when no window is down.
    pub(crate) fn toggle_view(&mut self, session: &Session) {
        if session.windows_down().next().is_some() {
            self.screen.windows = !self.screen.windows;
        }
    }

    /// A window of the program in front on the top screen that is on the bottom screen but hidden by
    /// ClamShell's view. The bar then offers to show them. Windows are matched by their scopes, which our
    /// programs run in. It doesn't allocate, since the bar asks every frame.
    pub fn hidden_down(&self, session: &Session) -> Option<i64> {
        if self.shows_windows(session) {
            return None;
        }
        let front = session.windows().iter().find(|w| w.focused && !w.down)?.unit.as_deref().filter(|u| Instance::is_ours(u))?;
        session.windows_down().find(|w| w.unit.as_deref() == Some(front)).map(|w| w.id)
    }

    /// A tap on a program whose windows are all on the bottom screen shows them there, with its first
    /// tab in view, turning the bottom screen on again if it was off by hand.
    pub(crate) fn show_program_down(&mut self, window: i64, out: &mut Out) {
        self.screen.windows = true;
        out.effects.push(Effect::Window(WindowCommand::ShowDown(window)));
        self.screen.off = false;
    }

    /// Home+Start turns the bottom screen off, or on again. If it had turned off by itself, this turns it
    /// on, as a touch would.
    pub(crate) fn toggle_power(&mut self, p: Power, now: u64) {
        if self.screen.idle_off && !self.screen.off {
            self.used_at = Some(now);
            self.woken = true;
            self.follow_power(p, now);
            return;
        }
        self.screen.off = !self.screen.off;
        // Turned on again by hand, so it is in use and shouldn't turn off by itself either.
        if !self.screen.off {
            self.used_at = Some(now);
            self.woken = true;
            self.screen.idle_off = false;
        }
    }

    /// Runs after every event. A window whose program has a rule for it is sent down, once.
    pub(crate) fn follow_window_rules(&mut self, session: &Session, out: &mut Out) {
        let wanted: Vec<i64> = session
            .windows()
            .iter()
            .filter(|w| !w.down && !self.sent_down.contains(&w.id))
            .filter(|w| session.owner_of(w).is_some_and(|o| self.window_rules.wants(&o, w)))
            .map(|w| w.id)
            .collect();
        for id in wanted {
            self.sent_down.push(id);
            self.show_down(id, out);
        }
        self.sent_down.retain(|id| session.windows().iter().any(|w| w.id == *id));
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::indexing_slicing)]
mod tests {
    use super::*;
    use crate::device::buttons::DeviceKey;
    use crate::session::running::{Origin, Running};
    use crate::settings::{Choice, Setting};
    use crate::testkit::Applied;
    use crate::testkit::state;
    use crate::{Event, State};

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
            pids: vec![],
        }
    }

    const GAME: &str = "clamshell-run-melonds_1-99-1.scope";

    /// The emulator's launch.
    fn emu() -> Instance {
        Instance::Run { key: "melonds_1".into(), secs: 99, n: 1 }
    }

    fn running(id: &str, key: &str) -> Running {
        Running { id: id.into(), key: key.into(), origin: Origin::Ours, app_id: None, start: 0, seen: false, tray: None, entry: None }
    }

    /// A game started by shelf, running with a top window and a second window, the second in front.
    fn emulator() -> State {
        let mut s = state();
        s.add_run(running("melonds", "melonds_1"), emu());
        s.applied(Event::Windows(vec![win(1, GAME, "Top", false, false), win(2, GAME, "Bottom", true, false)]));
        s
    }

    fn key(s: &mut State, k: DeviceKey) -> Vec<Effect> {
        s.applied(Event::Device(k))
    }

    #[test]
    fn a_programs_only_window_is_remembered_by_its_app_id_alone() {
        let mut s = state();
        s.add_run(running("melonds", "melonds_1"), emu());
        s.applied(Event::Windows(vec![win(2, GAME, "Page one", true, false)]));
        let fx = key(&mut s, DeviceKey::WindowDown);
        let rule = WindowRule { owner: "melonds".into(), app_id: "emu".into(), title: None };
        assert!(matches!(&fx[1], Effect::Save(Save::WindowRules(r)) if r.list() == [rule.clone()]), "{fx:?}");
        // A different title next time, such as a page or a command, still matches.
        assert!(rule.matches("melonds", &win(3, GAME, "Page two", true, false)));
    }

    #[test]
    fn a_window_moved_down_is_shown_there_and_remembered() {
        let mut s = emulator();
        assert_eq!(key(&mut s, DeviceKey::BottomView), vec![], "nothing down: nothing to switch to");
        assert!(!s.bottom().shows_windows(s.session()));
        let fx = key(&mut s, DeviceKey::WindowDown);
        assert_eq!(fx[0], Effect::Window(WindowCommand::Down(2)));
        // The title is included, since the emulator has another window with the same app id.
        let rule = WindowRule { owner: "melonds".into(), app_id: "emu".into(), title: Some("Bottom".into()) };
        assert!(matches!(&fx[1], Effect::Save(Save::WindowRules(r)) if r.list() == [rule.clone()]), "{fx:?}");
        assert_eq!(fx[2], Effect::Window(WindowCommand::DownRules(vec![rule.down_rule()])), "for sway");
        assert_eq!(fx.iter().filter(|e| **e == Effect::Window(WindowCommand::Down(2))).count(), 1, "sent down once, not again by its rule: {fx:?}");
        assert_eq!(s.bottom().notice().map(|n| &n.what), Some(&NoticeKind::BottomRule(true)));
        // sway reports it down, so it is shown there.
        s.applied(Event::Windows(vec![win(1, GAME, "Top", true, false), win(2, GAME, "Bottom", false, true)]));
        assert!(s.bottom().shows_windows(s.session()));
        // Home+Select shows ClamShell's view, where Home+Up does nothing, and then the windows again.
        key(&mut s, DeviceKey::BottomView);
        assert!(!s.bottom().shows_windows(s.session()));
        assert_eq!(key(&mut s, DeviceKey::WindowUp), vec![]);
        key(&mut s, DeviceKey::BottomView);
        assert!(s.bottom().shows_windows(s.session()));
        // Up moves it back to its workspace, forgets the rule, and shows ClamShell's view below.
        let fx = key(&mut s, DeviceKey::WindowUp);
        assert_eq!(fx[0], Effect::Window(WindowCommand::Up { window: 2, to: Some(emu()) }));
        assert!(matches!(&fx[1], Effect::Save(Save::WindowRules(r)) if r.list().is_empty()), "{fx:?}");
        s.applied(Event::Windows(vec![win(1, GAME, "Top", true, false), win(2, GAME, "Bottom", false, false)]));
        assert!(!s.bottom().shows_windows(s.session()), "none down: ClamShell's view");
    }

    #[test]
    fn a_rule_sends_the_window_down_when_its_program_opens_it_again() {
        let mut s = emulator();
        key(&mut s, DeviceKey::WindowDown);
        let rules = s.bottom().window_rules().clone();
        // A fresh start with the rules. The game's windows open on top, and the one the rule names goes
        // down, once.
        let mut s = crate::testkit::started(crate::Init { window_rules: rules.clone(), ..crate::testkit::init() });
        s.add_run(running("melonds", "melonds_1"), emu());
        let fx = s.applied(Event::Windows(vec![win(5, GAME, "Top", true, false), win(6, GAME, "Bottom", false, false)]));
        assert!(fx.contains(&Effect::Window(WindowCommand::Down(6))), "{fx:?}");
        assert!(!fx.contains(&Effect::Window(WindowCommand::Down(5))));
        let fx = s.applied(Event::Windows(vec![win(5, GAME, "Top", true, false), win(6, GAME, "Bottom", false, false)]));
        assert!(!fx.contains(&Effect::Window(WindowCommand::Down(6))), "asked once");
        // Its window appears before its scope is listed, and is sent down when the scope appears.
        let mut s = crate::testkit::started(crate::Init { window_rules: rules.clone(), ..crate::testkit::init() });
        let fx = s.applied(Event::Windows(vec![win(8, GAME, "Bottom", true, false)]));
        assert!(!fx.iter().any(|e| matches!(e, Effect::Window(WindowCommand::Down(_)))), "no entry yet: {fx:?}");
        s.add_run(running("melonds", "melonds_1"), emu());
        let fx = s.applied(Event::Units(vec![emu()]));
        assert!(fx.contains(&Effect::Window(WindowCommand::Down(8))), "{fx:?}");
        let mut s = emulator();
        key(&mut s, DeviceKey::WindowDown);
        // Another game's window with that title doesn't match the rule.
        s.add_run(running("other", "other_2"), Instance::Run { key: "other_2".into(), secs: 5, n: 2 });
        let fx = s.applied(Event::Windows(vec![win(7, "clamshell-run-other_2-5-2.scope", "Bottom", false, false)]));
        assert!(!fx.iter().any(|e| matches!(e, Effect::Window(WindowCommand::Down(_)))), "{fx:?}");
    }

    #[test]
    fn two_windows_down_are_tabs_and_the_one_in_view_comes_back_up() {
        let mut s = emulator();
        key(&mut s, DeviceKey::WindowDown);
        s.applied(Event::Windows(vec![win(1, GAME, "Top", true, false), win(2, GAME, "Bottom", false, true)]));
        key(&mut s, DeviceKey::WindowDown);
        let hidden = |w: Window| Window { visible: false, ..w };
        s.applied(Event::Windows(vec![win(1, GAME, "Top", false, true), hidden(win(2, GAME, "Bottom", false, true))]));
        let fx = key(&mut s, DeviceKey::WindowUp);
        assert!(fx.contains(&Effect::Window(WindowCommand::Up { window: 1, to: Some(emu()) })), "the tab in view: {fx:?}");
        s.applied(Event::Windows(vec![win(1, GAME, "Top", true, false), win(2, GAME, "Bottom", false, true)]));
        assert!(s.bottom().shows_windows(s.session()), "the other tab");
        let fx = key(&mut s, DeviceKey::WindowUp);
        assert!(fx.contains(&Effect::Window(WindowCommand::Up { window: 2, to: Some(emu()) })), "{fx:?}");
    }

    #[test]
    fn the_front_programs_windows_hidden_below_are_offered_and_shown() {
        let mut s = emulator();
        key(&mut s, DeviceKey::WindowDown);
        s.applied(Event::Windows(vec![win(1, GAME, "Top", true, false), win(2, GAME, "Bottom", false, true)]));
        assert_eq!(s.bottom().hidden_down(s.session()), None, "shown");
        key(&mut s, DeviceKey::BottomView);
        assert_eq!(s.bottom().hidden_down(s.session()), Some(2), "ClamShell's view over them");
        let fx = s.applied(Event::Pointer(crate::PointerEvent::Activate(crate::Id::BottomWindows)));
        assert_eq!(fx, vec![Effect::Window(WindowCommand::ShowDown(2)), Effect::Watch(crate::Watch::Metrics, false)], "its view not seen: not watched");
        assert!(s.bottom().shows_windows(s.session()));
        // With another program in front, the bar doesn't offer these windows.
        key(&mut s, DeviceKey::BottomView);
        s.applied(Event::Windows(vec![
            win(1, GAME, "Top", false, false),
            win(2, GAME, "Bottom", false, true),
            win(3, "clamshell-run-x_1-1-0.scope", "x", true, false),
        ]));
        assert_eq!(s.bottom().hidden_down(s.session()), None);
    }

    /// A status reading that says the device is on battery or on the charger.
    fn on_battery(s: &mut State, discharging: bool) -> Vec<Effect> {
        let status = crate::Status { battery: Some(50), charging: !discharging, discharging, ..crate::Status::default() };
        s.applied(Event::Reading(crate::Reading::Status(status)))
    }

    fn power(fx: &[Effect]) -> Option<bool> {
        fx.iter().find_map(|e| match e {
            Effect::Outputs { top: true, bottom } => Some(*bottom),
            _ => None,
        })
    }

    #[test]
    fn on_battery_the_untouched_bottom_screen_goes_off_and_a_touch_brings_it_back() {
        let mut s = state();
        on_battery(&mut s, true);
        s.applied(Event::tick(1000));
        assert_eq!(power(&s.applied(Event::tick(1000 + DIM_AFTER + OFF_AFTER - 1))), None, "dimmed, not yet off");
        assert_eq!(power(&s.applied(Event::tick(1000 + DIM_AFTER + OFF_AFTER))), Some(false), "off by itself");
        assert!(s.bottom().off());
        assert_eq!(power(&s.applied(Event::BottomTouched)), Some(true), "a touch: on again");
        assert!(!s.bottom().off());
    }

    #[test]
    fn off_by_itself_as_the_screens_go_off_it_goes_off_with_them_and_stays_off() {
        use crate::device::idle::IdleEvent;
        let mut s = state();
        on_battery(&mut s, true);
        s.applied(Event::tick(1000));
        s.applied(Event::Idle(IdleEvent::Watching(true)));
        s.applied(Event::tick(1060));
        s.applied(Event::BottomTouched);
        // The screens go off for idle after 5 minutes, and the bottom one by itself 4 minutes after the
        // touch, at the same time.
        let fx = s.applied(Event::tick(1060 + DIM_AFTER + OFF_AFTER));
        assert!(s.bottom().off());
        assert!(fx.contains(&Effect::Outputs { top: false, bottom: false }), "{fx:?}");
        let fx = s.applied(Event::Idle(IdleEvent::DeviceInput));
        assert!(fx.contains(&Effect::Outputs { top: true, bottom: false }), "{fx:?}");
    }

    #[test]
    fn the_wall_clock_set_back_does_not_keep_it_on() {
        let mut s = state();
        on_battery(&mut s, true);
        let year = 365 * 86_400;
        s.applied(Event::Tick { unix_secs: 2 * year, mono_secs: 1000 });
        let at = |mono: u64| Event::Tick { unix_secs: year + mono, mono_secs: mono };
        assert_eq!(power(&s.applied(at(1000 + DIM_AFTER + OFF_AFTER - 1))), None);
        assert!(s.bottom().dimmed(), "dimmed on time");
        assert_eq!(power(&s.applied(at(1000 + DIM_AFTER + OFF_AFTER))), Some(false), "off on time");
    }

    #[test]
    fn on_the_charger_or_set_to_stay_on_it_only_dims() {
        let mut s = state();
        on_battery(&mut s, false);
        s.applied(Event::tick(1000));
        assert_eq!(power(&s.applied(Event::tick(1000 + DIM_AFTER + OFF_AFTER))), None, "on the charger: only dimmed");
        // Unplugged now, but set to Screen on, so it is only dimmed.
        s.settings_mut().global.set(Setting::BottomScreen, BottomWhenIdle::On.value());
        on_battery(&mut s, true);
        assert_eq!(power(&s.applied(Event::tick(1001 + DIM_AFTER + OFF_AFTER))), None);
        // Back to the default, it turns off. With the charger plugged in, it turns on.
        s.settings_mut().global.set(Setting::BottomScreen, BottomWhenIdle::OffWhenIdle.value());
        assert_eq!(power(&s.applied(Event::tick(1002 + DIM_AFTER + OFF_AFTER))), Some(false));
        assert_eq!(power(&on_battery(&mut s, false)), Some(true), "the charger: on again");
    }

    #[test]
    fn dark_for_a_game_on_battery_it_goes_off_and_a_tap_wakes_it() {
        let mut s = state();
        on_battery(&mut s, true);
        s.settings_mut().global.set(Setting::BottomScreen, BottomWhenIdle::DarkInGame.value());
        crate::testkit::start(&mut s, "zelda-lttp");
        assert!(s.bottom_dark());
        assert!(s.bottom().off(), "dark on battery: off at once");
        assert_eq!(power(&s.applied(Event::Pointer(crate::PointerEvent::Activate(crate::Id::Wake)))), Some(true), "tapped: on");
    }

    #[test]
    fn windows_moved_down_keep_it_on() {
        let mut s = emulator();
        on_battery(&mut s, true);
        key(&mut s, DeviceKey::WindowDown);
        s.applied(Event::Windows(vec![win(1, GAME, "Top", true, false), win(2, GAME, "Bottom", false, true)]));
        s.applied(Event::tick(1000));
        assert_eq!(power(&s.applied(Event::tick(1000 + DIM_AFTER + OFF_AFTER))), None, "a window shown there");
    }

    #[test]
    fn home_start_brings_back_a_screen_off_by_hand_and_by_itself_at_once() {
        let mut s = state();
        on_battery(&mut s, true);
        s.applied(Event::tick(1000));
        key(&mut s, DeviceKey::BottomPower);
        s.applied(Event::tick(1000 + DIM_AFTER + OFF_AFTER));
        assert_eq!(power(&key(&mut s, DeviceKey::BottomPower)), Some(true), "one press");
        assert!(!s.bottom().off(), "on, and not off by itself either");
    }

    #[test]
    fn home_start_brings_back_a_screen_off_by_itself() {
        let mut s = state();
        on_battery(&mut s, true);
        s.applied(Event::tick(1000));
        s.applied(Event::tick(1000 + DIM_AFTER + OFF_AFTER));
        assert_eq!(power(&key(&mut s, DeviceKey::BottomPower)), Some(true), "on, as a touch would");
        assert!(!s.bottom().off());
    }

    #[test]
    fn the_bottom_screen_dims_untouched_and_lights_when_used() {
        let mut s = state();
        s.applied(Event::tick(1000));
        s.applied(Event::tick(1000 + DIM_AFTER - 1));
        assert!(!s.bottom().dimmed());
        s.applied(Event::tick(1000 + DIM_AFTER));
        assert!(s.bottom().dimmed(), "untouched a while: dimmed");
        // A touch on it lights it.
        s.applied(Event::BottomTouched);
        assert!(!s.bottom().dimmed());
        // The pad's focus coming to one of its targets lights it too, but not shelf's targets on the main
        // screen.
        s.applied(Event::tick(1000 + 3 * DIM_AFTER));
        assert!(s.bottom().dimmed());
        s.applied(Event::Pointer(crate::PointerEvent::Focus(crate::Id::Shelf(crate::ShelfId::Launch))));
        assert!(s.bottom().dimmed(), "the main screen's");
        s.applied(Event::Pointer(crate::PointerEvent::Focus(crate::Id::Tab(crate::Tab::Dock))));
        assert!(!s.bottom().dimmed());
        // Windows moved there are watched without touching, so they are never dimmed.
        let mut s = emulator();
        key(&mut s, DeviceKey::WindowDown);
        s.applied(Event::Windows(vec![win(1, GAME, "Top", true, false), win(2, GAME, "Bottom", false, true)]));
        s.applied(Event::tick(5000));
        s.applied(Event::tick(5000 + 2 * DIM_AFTER));
        assert!(s.bottom().shows_windows(s.session()) && !s.bottom().dimmed());
    }

    #[test]
    fn the_bottom_screen_turns_off_and_on() {
        let mut s = emulator();
        let off = vec![Effect::Outputs { top: true, bottom: false }, Effect::Watch(crate::Watch::Metrics, false)];
        assert_eq!(key(&mut s, DeviceKey::BottomPower), off, "nothing seen: not watched");
        assert!(s.bottom().off());
        // A window moved down turns it on again.
        let fx = key(&mut s, DeviceKey::WindowDown);
        assert!(fx.contains(&Effect::Outputs { top: true, bottom: true }), "{fx:?}");
        assert!(!s.bottom().off());
    }

    /// Home+Down with no window of a program in front, shelf's or none, does nothing.
    #[test]
    fn window_down_with_no_window_in_front_does_nothing() {
        let mut s = state();
        assert_eq!(key(&mut s, DeviceKey::WindowDown), vec![], "no windows");
        s.applied(Event::Windows(vec![crate::testkit::shelf_window(true), crate::testkit::win(2, "foot", "1", false)]));
        assert_eq!(key(&mut s, DeviceKey::WindowDown), vec![], "shelf in front");
        assert!(s.bottom().window_rules().list().is_empty());
    }

    /// Home+Up says the window's rule is forgotten.
    #[test]
    fn a_window_moved_up_says_its_rule_is_forgotten() {
        let mut s = emulator();
        key(&mut s, DeviceKey::WindowDown);
        s.applied(Event::Windows(vec![win(1, GAME, "Top", true, false), win(2, GAME, "Bottom", false, true)]));
        s.applied(Event::tick(100));
        key(&mut s, DeviceKey::WindowUp);
        assert_eq!(s.bottom().notice().map(|n| &n.what), Some(&NoticeKind::BottomRule(false)));
    }

    /// On battery and untouched, it stays on while the refresh rate is calibrated, since the tool turns
    /// it off and on itself, and while something is typed there.
    #[test]
    fn a_refresh_calibration_or_an_editor_keeps_it_on() {
        let mut s = state();
        on_battery(&mut s, true);
        s.applied(Event::tick(1000));
        s.applied(Event::Pointer(crate::PointerEvent::Activate(crate::Id::Device(crate::DeviceId::Refresh))));
        assert_eq!(power(&s.applied(Event::tick(1000 + DIM_AFTER + OFF_AFTER))), None, "calibrating");
        assert!(!s.bottom().off());
        assert_eq!(power(&s.applied(Event::RefreshCalibrated(Ok(String::new())))), Some(false), "done: off by itself");
        let mut s = state();
        on_battery(&mut s, true);
        s.applied(Event::tick(1000));
        let step = crate::device::account::PasswordStep::Current;
        let editor = crate::Editor { purpose: crate::bottom::editor::Purpose::AccountPassword(step), text: String::new() };
        s.applied(Event::EditOnKeyboard(editor));
        assert_eq!(power(&s.applied(Event::tick(1000 + DIM_AFTER + OFF_AFTER))), None, "typing");
        let fx = s.applied(Event::Pointer(crate::PointerEvent::Activate(crate::Id::Editor(crate::EditorId::Cancel))));
        assert_eq!(power(&fx), None, "the cancel is a touch: in use again");
        assert_eq!(power(&s.applied(Event::tick(1000 + 2 * (DIM_AFTER + OFF_AFTER)))), Some(false), "then off by itself");
    }

    /// The Wake tap lasts until the next game: a game starting lets the screen go dark again.
    #[test]
    fn a_game_starting_lets_a_woken_screen_go_dark_again() {
        let mut s = state();
        s.settings_mut().global.set(Setting::BottomScreen, BottomWhenIdle::DarkInGame.value());
        crate::testkit::launch(&mut s, "zelda-lttp");
        assert!(s.bottom_dark());
        s.applied(Event::Pointer(crate::PointerEvent::Activate(crate::Id::Wake)));
        assert!(!s.bottom_dark(), "woken");
        crate::testkit::launch(&mut s, "ff6");
        assert!(s.bottom_dark(), "another game: dark again");
    }

    #[test]
    fn showing_the_windows_below_turns_the_screen_on_if_off_by_hand() {
        let mut s = emulator();
        key(&mut s, DeviceKey::WindowDown);
        s.applied(Event::Windows(vec![win(1, GAME, "Top", true, false), win(2, GAME, "Bottom", false, true)]));
        key(&mut s, DeviceKey::BottomView);
        key(&mut s, DeviceKey::BottomPower);
        assert!(s.bottom().off());
        let fx = s.applied(Event::Pointer(crate::PointerEvent::Activate(crate::Id::BottomWindows)));
        assert_eq!(fx.first(), Some(&Effect::Window(WindowCommand::ShowDown(2))), "{fx:?}");
        assert_eq!(power(&fx), Some(true), "not shown on a dark panel");
        assert!(!s.bottom().off());
        assert!(s.bottom().shows_windows(s.session()));
    }
}
