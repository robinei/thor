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

use crate::State;
use crate::bottom::notice::NoticeKind;
use crate::effect::{Dirty, Effect, Save, WindowCommand};
use crate::session::instance::Instance;
use crate::session::windows::Window;
use crate::settings::BottomWhenIdle;

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

    /// Adds a rule unless there is one like it. Returns whether it was new.
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

    fn wants(&self, owner: &str, w: &Window) -> bool {
        self.list.iter().any(|r| r.matches(owner, w))
    }

    /// What the rules match, for sway, without duplicates.
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
    /// Whether the bottom screen shows the windows moved down (the tab sway shows) over ClamShell's
    /// view, rather than ClamShell's view.
    pub(crate) windows: bool,
    /// Turned off with Home+Start.
    pub(crate) off: bool,
    /// Turned off by itself, when idle or dark on battery ([`State::follow_bottom_power`]).
    pub(crate) idle_off: bool,
}

/// How long, in seconds, the second screen goes untouched and without the pad's focus before it dims,
/// and after that before it powers off by itself. It only powers off on battery and when the setting
/// isn't "Screen on" (see [`State::follow_bottom_power`]).
const DIM_AFTER: u64 = 60;
const OFF_AFTER: u64 = 180;

impl State {
    /// Whether the second screen is dark. It is when the program in force wants it dark and the screen
    /// hasn't been tapped since.
    pub fn bottom_dark(&self) -> bool {
        self.session.program.is_some() && self.in_force().choice::<BottomWhenIdle>() == BottomWhenIdle::DarkInGame && !self.bottom.woken
    }

    /// Whether the second screen is drawn dimmed. It is when it shows ClamShell's view and has gone
    /// untouched, without the pad's focus, for [`DIM_AFTER`]. Windows moved there are never dimmed, since
    /// they are watched without touching. This spares the OLED while a game played with the pad on the
    /// main screen keeps the device awake and nobody looks below. A touch on it, or the pad's focus
    /// coming to it, brings it back.
    pub fn bottom_dimmed(&self) -> bool {
        !self.bottom_shows_windows() && self.bottom_idle_for() >= DIM_AFTER
    }

    /// Seconds since the second screen was last used, or 0 before the clock's first reading.
    fn bottom_idle_for(&self) -> u64 {
        self.bottom.used_at.map_or(0, |at| self.clock.mono().saturating_sub(at))
    }

    /// The second screen is in use, so it is lit, and redrawn if it was dimmed.
    pub(crate) fn bottom_used(&mut self) -> Dirty {
        self.bottom.used_at = Some(self.clock.mono());
        if self.follow_bottom_dim() { Dirty::BOTTOM } else { Dirty::NONE }
    }

    /// Runs after any event. Returns whether the second screen's dimming changed, so it can be redrawn.
    /// The time counts from the first clock reading.
    pub(crate) fn follow_bottom_dim(&mut self) -> bool {
        if self.bottom.used_at.is_none() && self.clock.read() {
            self.bottom.used_at = Some(self.clock.mono());
        }
        let dimmed = self.bottom_dimmed();
        std::mem::replace(&mut self.bottom.dimmed, dimmed) != dimmed
    }

    /// The windows moved to the bottom screen, in the compositor's order.
    pub fn windows_down(&self) -> impl Iterator<Item = &Window> {
        self.session.windows.iter().filter(|w| w.down)
    }

    /// Whether the bottom screen shows the windows moved there instead of ClamShell's view.
    pub fn bottom_shows_windows(&self) -> bool {
        self.bottom.screen.windows && self.windows_down().next().is_some()
    }

    /// Whether the bottom screen is turned off, by hand or by itself.
    pub fn bottom_off(&self) -> bool {
        self.bottom.screen.off || self.bottom.screen.idle_off
    }

    /// Whether the bottom screen should be off by itself. That needs all of: running on battery, the
    /// setting in force not "Screen on", no refresh calibration running, ClamShell's view on it rather
    /// than a window moved down, and nothing being typed there. On top of that, the screen must be dark
    /// for a game and not tapped since, or untouched and without the pad's focus for [`DIM_AFTER`] plus
    /// [`OFF_AFTER`]. The panel is driven in video mode and streamed every frame whatever it shows, so
    /// only turning it off saves power.
    fn bottom_idle_wanted(&self) -> bool {
        let untouched = self.bottom_idle_for() >= DIM_AFTER + OFF_AFTER;
        self.sys.status.discharging
            && self.device.refresh != crate::device::Refresh::Running
            && self.in_force().choice::<BottomWhenIdle>() != BottomWhenIdle::On
            && !self.bottom_shows_windows()
            && self.bottom.editor().is_none()
            && (self.bottom_dark() || untouched)
    }

    /// Runs after any event. Turns the bottom screen off or on by itself as [`State::bottom_idle_wanted`]
    /// says. A touch, the pad's focus, a window moved down or the charger turn it on again. Does nothing
    /// while it is off by hand, or while both screens are off for idle; when the screens come back they
    /// read [`State::bottom_off`].
    pub(crate) fn follow_bottom_power(&mut self) -> Vec<Effect> {
        let wanted = self.bottom_idle_wanted();
        let screens_off = self.idle.stage() >= crate::device::idle::Stage::Off;
        if wanted == std::mem::replace(&mut self.bottom.screen.idle_off, wanted) || self.bottom.screen.off || screens_off {
            return vec![];
        }
        vec![Effect::Window(WindowCommand::BottomPower(!wanted))]
    }

    /// Which running entry a window belongs to, for a rule.
    fn owner(&self, w: &Window) -> Option<Arc<str>> {
        self.session.running_of(w).map(|r| r.id.clone())
    }

    /// Home+Down moves the window in front on the top screen to the bottom screen, shows it there, and
    /// remembers it for its program.
    pub(crate) fn window_down(&mut self) -> Vec<Effect> {
        let Some(w) = self.session.windows.iter().find(|w| w.focused && !w.down && w.home() != Some(Instance::Shelf)).cloned() else {
            return vec![];
        };
        let mut effects = self.show_down(w.id);
        // The title is only used to tell it apart from another window of its program with the same
        // app id.
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

    /// Moves a window down and shows it there, turning the bottom screen on again if it was off.
    fn show_down(&mut self, id: i64) -> Vec<Effect> {
        self.bottom.screen.windows = true;
        let mut effects = vec![Effect::Window(WindowCommand::Down(id))];
        if std::mem::take(&mut self.bottom.screen.off) {
            effects.push(Effect::Window(WindowCommand::BottomPower(true)));
        }
        effects
    }

    /// Home+Up moves the window the bottom screen shows (the tab in view) back to its place on the top
    /// screen, and forgets its rule. The bottom screen then shows the next tab, or ClamShell's view after
    /// the last one.
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

    /// The rules in the form sway uses to place windows.
    pub(crate) fn down_rules_effect(&self) -> Effect {
        Effect::Window(WindowCommand::DownRules(self.window_rules.down_rules()))
    }

    /// Home+Select switches the bottom screen between the windows moved there and ClamShell's view. It
    /// does nothing when no window is down.
    pub(crate) fn toggle_bottom_view(&mut self) -> Vec<Effect> {
        if self.windows_down().next().is_some() {
            self.bottom.screen.windows = !self.bottom.screen.windows;
        }
        vec![]
    }

    /// A window of the program in front on the top screen that is on the bottom screen but hidden by
    /// ClamShell's view. The bar then offers to show them. Windows are matched by their scopes, which our
    /// programs run in. It doesn't allocate, since the bar asks every frame.
    pub fn hidden_down(&self) -> Option<i64> {
        if self.bottom_shows_windows() {
            return None;
        }
        let front = self.session.windows.iter().find(|w| w.focused && !w.down)?.unit.as_deref().filter(|u| Instance::is_ours(u))?;
        self.windows_down().find(|w| w.unit.as_deref() == Some(front)).map(|w| w.id)
    }

    /// A tap on a program whose windows are all on the bottom screen shows them there, with its first
    /// tab in view.
    pub(crate) fn show_program_down(&mut self, window: i64) -> Vec<Effect> {
        self.bottom.screen.windows = true;
        vec![Effect::Window(WindowCommand::ShowDown(window))]
    }

    /// Home+Start turns the bottom screen off, or on again. If it had turned off by itself, this turns it
    /// on, as a touch would.
    pub(crate) fn toggle_bottom_power(&mut self) -> Vec<Effect> {
        if self.bottom.screen.idle_off && !self.bottom.screen.off {
            self.bottom.used_at = Some(self.clock.mono());
            self.bottom.woken = true;
            return self.follow_bottom_power();
        }
        self.bottom.screen.off = !self.bottom.screen.off;
        // Turned on again by hand, so it is in use and shouldn't turn off by itself either.
        if !self.bottom.screen.off {
            self.bottom.used_at = Some(self.clock.mono());
            self.bottom.woken = true;
            self.bottom.screen.idle_off = false;
        }
        vec![Effect::Window(WindowCommand::BottomPower(!self.bottom.screen.off))]
    }

    /// Runs after the windows changed. A window whose program has a rule for it is sent down, once.
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
    use crate::device::buttons::DeviceKey;
    use crate::session::running::{Origin, Running};
    use crate::settings::{Choice, Setting};
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
            pids: vec![],
        }
    }

    const GAME: &str = "clamshell-run-melonds_1-99-1.scope";

    /// The emulator's launch.
    fn emu() -> Instance {
        Instance::Run { key: "melonds_1".into(), secs: 99, n: 1 }
    }

    fn running(id: &str, key: &str) -> Running {
        Running { id: id.into(), key: key.into(), origin: Origin::Ours, app_id: None, start: 0, seen: false, entry: None }
    }

    /// A game started by shelf, running with a top window and a second window, the second in front.
    fn emulator() -> State {
        let mut s = state();
        s.session.running.push(running("melonds", "melonds_1"));
        s.session.units.push(emu());
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
        s.session.units.push(emu());
        s.apply(Event::Windows(vec![win(2, GAME, "Page one", true, false)]));
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
        assert!(!s.bottom_shows_windows());
        let fx = key(&mut s, DeviceKey::WindowDown);
        assert_eq!(fx[0], Effect::Window(WindowCommand::Down(2)));
        // The title is included, since the emulator has another window with the same app id.
        let rule = WindowRule { owner: "melonds".into(), app_id: "emu".into(), title: Some("Bottom".into()) };
        assert!(matches!(&fx[1], Effect::Save(Save::WindowRules(r)) if r.list() == [rule.clone()]), "{fx:?}");
        assert_eq!(fx[2], Effect::Window(WindowCommand::DownRules(vec![rule.down_rule()])), "for sway");
        assert_eq!(s.bottom().notice().map(|n| &n.what), Some(&NoticeKind::BottomRule(true)));
        // sway reports it down, so it is shown there.
        s.apply(Event::Windows(vec![win(1, GAME, "Top", true, false), win(2, GAME, "Bottom", false, true)]));
        assert!(s.bottom_shows_windows());
        // Home+Select shows ClamShell's view, where Home+Up does nothing, and then the windows again.
        key(&mut s, DeviceKey::BottomView);
        assert!(!s.bottom_shows_windows());
        assert_eq!(key(&mut s, DeviceKey::WindowUp), vec![]);
        key(&mut s, DeviceKey::BottomView);
        assert!(s.bottom_shows_windows());
        // Up moves it back to its workspace, forgets the rule, and shows ClamShell's view below.
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
        // A fresh start with the rules. The game's windows open on top, and the one the rule names goes
        // down, once.
        let mut s = state();
        s.window_rules = WindowRules::from_list(rules.list().to_vec());
        s.session.running.push(running("melonds", "melonds_1"));
        s.session.units.push(emu());
        let fx = s.apply(Event::Windows(vec![win(5, GAME, "Top", true, false), win(6, GAME, "Bottom", false, false)])).1;
        assert!(fx.contains(&Effect::Window(WindowCommand::Down(6))), "{fx:?}");
        assert!(!fx.contains(&Effect::Window(WindowCommand::Down(5))));
        let fx = s.apply(Event::Windows(vec![win(5, GAME, "Top", true, false), win(6, GAME, "Bottom", false, false)])).1;
        assert!(!fx.contains(&Effect::Window(WindowCommand::Down(6))), "asked once");
        // Its window appears before its scope is listed, and is sent down when the scope appears.
        let mut s = state();
        s.window_rules = WindowRules::from_list(rules.list().to_vec());
        let fx = s.apply(Event::Windows(vec![win(8, GAME, "Bottom", true, false)])).1;
        assert!(!fx.iter().any(|e| matches!(e, Effect::Window(WindowCommand::Down(_)))), "no entry yet: {fx:?}");
        s.session.running.push(running("melonds", "melonds_1"));
        s.session.units.push(emu());
        let fx = s.apply(Event::Units(vec![emu()])).1;
        assert!(fx.contains(&Effect::Window(WindowCommand::Down(8))), "{fx:?}");
        let mut s = emulator();
        key(&mut s, DeviceKey::WindowDown);
        // Another game's window with that title doesn't match the rule.
        s.session.running.push(running("other", "other_2"));
        s.session.units.push(Instance::Run { key: "other_2".into(), secs: 5, n: 2 });
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
        assert_eq!(fx, vec![Effect::Window(WindowCommand::ShowDown(2)), Effect::Watch(crate::Watch::Metrics, false)], "its view not seen: not watched");
        assert!(s.bottom_shows_windows());
        // With another program in front, the bar doesn't offer these windows.
        key(&mut s, DeviceKey::BottomView);
        s.apply(Event::Windows(vec![
            win(1, GAME, "Top", false, false),
            win(2, GAME, "Bottom", false, true),
            win(3, "clamshell-run-x_1-1-0.scope", "x", true, false),
        ]));
        assert_eq!(s.hidden_down(), None);
    }

    /// A status reading that says the device is on battery or on the charger.
    fn on_battery(s: &mut State, discharging: bool) -> Vec<Effect> {
        let status = crate::Status { battery: Some(50), charging: !discharging, discharging, ..crate::Status::default() };
        s.apply(Event::Reading(crate::Reading::Status(status))).1
    }

    fn power(fx: &[Effect]) -> Option<bool> {
        fx.iter().find_map(|e| match e {
            Effect::Window(WindowCommand::BottomPower(on)) => Some(*on),
            _ => None,
        })
    }

    #[test]
    fn on_battery_the_untouched_bottom_screen_goes_off_and_a_touch_brings_it_back() {
        let mut s = state();
        on_battery(&mut s, true);
        s.apply(Event::tick(1000));
        assert_eq!(power(&s.apply(Event::tick(1000 + DIM_AFTER + OFF_AFTER - 1)).1), None, "dimmed, not yet off");
        assert_eq!(power(&s.apply(Event::tick(1000 + DIM_AFTER + OFF_AFTER)).1), Some(false), "off by itself");
        assert!(s.bottom_off());
        assert_eq!(power(&s.apply(Event::BottomTouched).1), Some(true), "a touch: on again");
        assert!(!s.bottom_off());
    }

    #[test]
    fn the_wall_clock_set_back_does_not_keep_it_on() {
        let mut s = state();
        on_battery(&mut s, true);
        let year = 365 * 86_400;
        s.apply(Event::Tick { unix_secs: 2 * year, mono_secs: 1000 });
        let at = |mono: u64| Event::Tick { unix_secs: year + mono, mono_secs: mono };
        assert_eq!(power(&s.apply(at(1000 + DIM_AFTER + OFF_AFTER - 1)).1), None);
        assert!(s.bottom_dimmed(), "dimmed on time");
        assert_eq!(power(&s.apply(at(1000 + DIM_AFTER + OFF_AFTER)).1), Some(false), "off on time");
    }

    #[test]
    fn on_the_charger_or_set_to_stay_on_it_only_dims() {
        let mut s = state();
        on_battery(&mut s, false);
        s.apply(Event::tick(1000));
        assert_eq!(power(&s.apply(Event::tick(1000 + DIM_AFTER + OFF_AFTER)).1), None, "on the charger: only dimmed");
        // Unplugged now, but set to Screen on, so it is only dimmed.
        s.settings.global.set(Setting::BottomScreen, BottomWhenIdle::On.value());
        on_battery(&mut s, true);
        assert_eq!(power(&s.apply(Event::tick(1001 + DIM_AFTER + OFF_AFTER)).1), None);
        // Back to the default, it turns off. With the charger plugged in, it turns on.
        s.settings.global.set(Setting::BottomScreen, BottomWhenIdle::OffWhenIdle.value());
        assert_eq!(power(&s.apply(Event::tick(1002 + DIM_AFTER + OFF_AFTER)).1), Some(false));
        assert_eq!(power(&on_battery(&mut s, false)), Some(true), "the charger: on again");
    }

    #[test]
    fn dark_for_a_game_on_battery_it_goes_off_and_a_tap_wakes_it() {
        let mut s = state();
        on_battery(&mut s, true);
        s.settings.global.set(Setting::BottomScreen, BottomWhenIdle::DarkInGame.value());
        crate::testkit::start(&mut s, "zelda-lttp");
        assert!(s.bottom_dark());
        assert!(s.bottom_off(), "dark on battery: off at once");
        assert_eq!(power(&s.apply(Event::Pointer(crate::PointerEvent::Activate(crate::Id::Wake))).1), Some(true), "tapped: on");
    }

    #[test]
    fn windows_moved_down_keep_it_on() {
        let mut s = emulator();
        on_battery(&mut s, true);
        key(&mut s, DeviceKey::WindowDown);
        s.apply(Event::Windows(vec![win(1, GAME, "Top", true, false), win(2, GAME, "Bottom", false, true)]));
        s.apply(Event::tick(1000));
        assert_eq!(power(&s.apply(Event::tick(1000 + DIM_AFTER + OFF_AFTER)).1), None, "a window shown there");
    }

    #[test]
    fn home_start_brings_back_a_screen_off_by_hand_and_by_itself_at_once() {
        let mut s = state();
        on_battery(&mut s, true);
        s.apply(Event::tick(1000));
        key(&mut s, DeviceKey::BottomPower);
        s.apply(Event::tick(1000 + DIM_AFTER + OFF_AFTER));
        assert_eq!(power(&key(&mut s, DeviceKey::BottomPower)), Some(true), "one press");
        assert!(!s.bottom_off(), "on, and not off by itself either");
    }

    #[test]
    fn home_start_brings_back_a_screen_off_by_itself() {
        let mut s = state();
        on_battery(&mut s, true);
        s.apply(Event::tick(1000));
        s.apply(Event::tick(1000 + DIM_AFTER + OFF_AFTER));
        assert_eq!(power(&key(&mut s, DeviceKey::BottomPower)), Some(true), "on, as a touch would");
        assert!(!s.bottom_off());
    }

    #[test]
    fn the_bottom_screen_dims_untouched_and_lights_when_used() {
        let mut s = state();
        s.apply(Event::tick(1000));
        s.apply(Event::tick(1000 + DIM_AFTER - 1));
        assert!(!s.bottom_dimmed());
        let (dirty, _) = s.apply(Event::tick(1000 + DIM_AFTER));
        assert!(s.bottom_dimmed() && dirty.bottom, "untouched a while: dimmed, redrawn");
        // A touch on it lights it.
        let (dirty, _) = s.apply(Event::BottomTouched);
        assert!(!s.bottom_dimmed() && dirty.bottom);
        // The pad's focus coming to one of its targets lights it too, but not shelf's targets on the main
        // screen.
        s.apply(Event::tick(1000 + 3 * DIM_AFTER));
        assert!(s.bottom_dimmed());
        s.apply(Event::Pointer(crate::PointerEvent::Focus(crate::Id::Shelf(crate::ShelfId::Launch))));
        assert!(s.bottom_dimmed(), "the main screen's");
        s.apply(Event::Pointer(crate::PointerEvent::Focus(crate::Id::Tab(crate::Tab::Dock))));
        assert!(!s.bottom_dimmed());
        // Windows moved there are watched without touching, so they are never dimmed.
        let mut s = emulator();
        key(&mut s, DeviceKey::WindowDown);
        s.apply(Event::Windows(vec![win(1, GAME, "Top", true, false), win(2, GAME, "Bottom", false, true)]));
        s.apply(Event::tick(5000));
        s.apply(Event::tick(5000 + 2 * DIM_AFTER));
        assert!(s.bottom_shows_windows() && !s.bottom_dimmed());
    }

    #[test]
    fn the_bottom_screen_turns_off_and_on() {
        let mut s = emulator();
        let off = vec![Effect::Window(WindowCommand::BottomPower(false)), Effect::Watch(crate::Watch::Metrics, false)];
        assert_eq!(key(&mut s, DeviceKey::BottomPower), off, "nothing seen: not watched");
        assert!(s.bottom_off());
        // A window moved down turns it on again.
        let fx = key(&mut s, DeviceKey::WindowDown);
        assert!(fx.contains(&Effect::Window(WindowCommand::BottomPower(true))), "{fx:?}");
        assert!(!s.bottom_off());
    }
}
