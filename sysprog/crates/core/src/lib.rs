//! The program's state and its rules. No I/O: every change comes in as an [`Event`], and what the outside
//! world must do comes back as [`Effect`]s. The UI and the policy threads only read the state and send
//! events, so all of this is unit-testable.

pub mod keyboard;
pub mod metrics;
pub mod touch;

use std::collections::HashMap;
use std::sync::Arc;

use keyboard::{Key, KeyPos, Keyboard, Stroke};

pub use sysprog_library::Game;

/// Second-screen modes (docs/plans.md, "Second screen"). Follow is the default: the second screen shows
/// what belongs to the main screen (the selected or running game's settings). The others override it
/// until dismissed; tapping the active override's icon returns to follow.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum Mode {
    #[default]
    Follow,
    Launcher,
    Keyboard,
    Touchpad,
    Sysinfo,
}

impl Mode {
    /// The modes the bar offers; follow has no icon (it is what's left when none is on).
    pub const OVERRIDES: [Mode; 4] = [Mode::Launcher, Mode::Keyboard, Mode::Touchpad, Mode::Sysinfo];
}

/// Power profile per game; a stand-in for the real per-game settings.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum Power {
    Low,
    #[default]
    Balanced,
    High,
}

impl Power {
    pub const ALL: [Power; 3] = [Power::Low, Power::Balanced, Power::High];

    pub fn label(self) -> &'static str {
        match self {
            Power::Low => "Low",
            Power::Balanced => "Balanced",
            Power::High => "High",
        }
    }
}

/// Stable ids of everything that can be focused or tapped, on either screen. Focus is kept by id, so it
/// survives redraws and layout changes.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum Id {
    Game(Arc<str>),
    Mode(Mode),
    Power(Power),
    Launch,
    /// An on-screen keyboard key, by position.
    Key(KeyPos),
    /// The pad mouse toggle.
    PadMouse,
}

/// The system's status as the bar shows it (policy threads read it from the system).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Status {
    /// Battery charge 0-100, if there is a battery.
    pub battery: Option<u8>,
    pub charging: bool,
    /// Wi-Fi link quality 0-100 while connected; `None` when not.
    pub wifi: Option<u8>,
    /// Output volume 0-100, if known, and whether it is muted.
    pub volume: Option<u8>,
    pub muted: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Event {
    /// Wall clock, once a second (policy thread).
    Tick { unix_secs: u64 },
    /// Battery, Wi-Fi, volume (policy thread, when they change).
    Status(Status),
    /// CPU, GPU, memory, temperatures, power (policy thread, once a second).
    Metrics(metrics::Metrics),
    /// A running game's frame rate and frame time in ms (MangoHud), or none.
    Frames(Option<(f32, f32)>),
    /// Tap, click, or the pad's A on a focused item.
    Activate(Id),
    /// A press held longer than a tap (locks a keyboard modifier; otherwise as `Activate`).
    LongPress(Id),
    /// A finger, button or the pad's A went down on / up from a target: shown as pressed meanwhile.
    Down(Id),
    Up(Id),
    /// The pad moved focus (the UI finds the target, since only it knows the layout). Shows the focus.
    Focus(Id),
    /// Touch or mouse input began: the focus ring is hidden until the pad moves focus again.
    HideFocus,
    Quit,
}

/// What the state asks the outside world to do.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Effect {
    Launch { game: Arc<str>, power: Power },
    /// A key press for the focused app (virtual keyboard).
    Key(Stroke),
    /// Touchpad mode started (true) or ended: the input side grabs or releases the touchscreen.
    Touchpad(bool),
    /// The pad mouse turned on or off: the input side grabs the controller and drives a virtual mouse
    /// with it, or releases it to games.
    PadMouse(bool),
    Quit,
}

/// Which screens need a new frame after an event.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Dirty {
    pub top: bool,
    pub bottom: bool,
}

impl Dirty {
    pub const BOTH: Dirty = Dirty { top: true, bottom: true };
    pub const BOTTOM: Dirty = Dirty { top: false, bottom: true };
    pub const NONE: Dirty = Dirty { top: false, bottom: false };
}

#[derive(Debug, Default)]
pub struct State {
    pub games: Vec<Game>,
    /// Id of the selected game (shelf's list on top, its settings below).
    pub selected: Option<Arc<str>>,
    /// Per-game power setting, by game id; absent = default.
    pub power: HashMap<Arc<str>, Power>,
    pub mode: Mode,
    pub focus: Option<Id>,
    /// The focus ring is shown only while navigating with the pad.
    pub show_focus: bool,
    pub unix_secs: u64,
    pub keyboard: Keyboard,
    /// The target being pressed (drawn lit), if any.
    pub pressed: Option<Id>,
    pub status: Status,
    /// The controller drives the pointer (independent of the second screen's mode).
    pub pad_mouse: bool,
    pub metrics: metrics::Metrics,
    pub history: metrics::History,
}

/// What holding a target does once the press lasts beyond a tap.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Hold {
    /// Activate now and again at the repeat rate until released (keyboard keys).
    Repeat,
    /// One long press, then nothing on release (locks a modifier).
    Long,
}

/// Which screen a target is drawn on.
fn screen_of(id: &Id) -> Dirty {
    match id {
        Id::Game(_) => Dirty::BOTH,
        Id::Mode(_) | Id::Power(_) | Id::Launch | Id::Key(_) | Id::PadMouse => Dirty::BOTTOM,
    }
}

impl State {
    pub fn new(games: Vec<Game>) -> Self {
        let selected = games.first().map(|g| g.id.clone());
        let focus = selected.clone().map(Id::Game);
        State { games, selected, focus, ..State::default() }
    }

    pub fn selected_game(&self) -> Option<&Game> {
        let id = self.selected.as_ref()?;
        self.games.iter().find(|g| &g.id == id)
    }

    pub fn power_of(&self, game: &str) -> Power {
        self.power.get(game).copied().unwrap_or_default()
    }

    /// Applies one event; returns which screens changed and what must happen outside.
    pub fn apply(&mut self, event: Event) -> (Dirty, Vec<Effect>) {
        match event {
            Event::Tick { unix_secs } => {
                // Once a minute: the bar's clock, and both screens' burn-in orbit step.
                let minute_changed = unix_secs / 60 != self.unix_secs / 60;
                self.unix_secs = unix_secs;
                (if minute_changed { Dirty::BOTH } else { Dirty::NONE }, vec![])
            }
            Event::Status(status) => {
                let changed = status != self.status;
                self.status = status;
                (if changed { Dirty::BOTTOM } else { Dirty::NONE }, vec![])
            }
            Event::Metrics(mut m) => {
                // The frame rate comes separately (Frames); keep it.
                m.fps = self.metrics.fps;
                m.frame_ms = self.metrics.frame_ms;
                self.metrics = m;
                self.history.push(&m);
                // Only the sysinfo mode shows them.
                (if self.mode == Mode::Sysinfo { Dirty::BOTTOM } else { Dirty::NONE }, vec![])
            }
            Event::Frames(frames) => {
                self.metrics.fps = frames.map(|f| f.0);
                self.metrics.frame_ms = frames.map(|f| f.1);
                (if self.mode == Mode::Sysinfo { Dirty::BOTTOM } else { Dirty::NONE }, vec![])
            }
            Event::Focus(id) => {
                self.focus = Some(id);
                self.show_focus = true;
                (Dirty::BOTH, vec![])
            }
            Event::HideFocus => {
                let was = std::mem::replace(&mut self.show_focus, false);
                (if was { Dirty::BOTH } else { Dirty::NONE }, vec![])
            }
            Event::Activate(id) => {
                self.focus = Some(id.clone());
                self.activate(id, false)
            }
            Event::LongPress(id) => {
                self.focus = Some(id.clone());
                self.activate(id, true)
            }
            Event::Down(id) => {
                let dirty = screen_of(&id);
                self.pressed = Some(id);
                (dirty, vec![])
            }
            Event::Up(id) => {
                if self.pressed.as_ref() == Some(&id) {
                    self.pressed = None;
                }
                (screen_of(&id), vec![])
            }
            Event::Quit => (Dirty::NONE, vec![Effect::Quit]),
        }
    }

    /// What holding `id` does: keyboard keys repeat, modifiers and everything else get one long press.
    pub fn hold(&self, id: &Id) -> Hold {
        match id {
            Id::Key(pos) => match self.keyboard.effective(*pos) {
                Some(Key::Mod(_)) | None => Hold::Long,
                Some(_) => Hold::Repeat,
            },
            _ => Hold::Long,
        }
    }

    fn activate(&mut self, id: Id, long: bool) -> (Dirty, Vec<Effect>) {
        match id {
            Id::Game(game) => {
                // Activating the selected game again launches it, as A on a list entry does in shelf.
                if self.selected.as_ref() == Some(&game) {
                    return (Dirty::BOTH, vec![self.launch_effect(game)]);
                }
                self.selected = Some(game);
                (Dirty::BOTH, vec![])
            }
            Id::Mode(mode) => {
                // Tapping the current mode's icon returns to follow (docs/plans.md, "Switching modes").
                let before = self.mode;
                self.mode = if self.mode == mode { Mode::Follow } else { mode };
                let touchpad = |m: Mode| m == Mode::Touchpad;
                let effects = if touchpad(before) != touchpad(self.mode) {
                    vec![Effect::Touchpad(touchpad(self.mode))]
                } else {
                    vec![]
                };
                (Dirty::BOTH, effects)
            }
            Id::Power(power) => {
                if let Some(game) = self.selected.clone() {
                    self.power.insert(game, power);
                }
                (Dirty::BOTH, vec![])
            }
            Id::Launch => match self.selected.clone() {
                Some(game) => (Dirty::BOTH, vec![self.launch_effect(game)]),
                None => (Dirty::BOTH, vec![]),
            },
            Id::PadMouse => {
                self.pad_mouse = !self.pad_mouse;
                (Dirty::BOTTOM, vec![Effect::PadMouse(self.pad_mouse)])
            }
            Id::Key(pos) => {
                let effects = self.keyboard.press(pos, long).map(Effect::Key).into_iter().collect();
                (Dirty::BOTTOM, effects)
            }
        }
    }

    fn launch_effect(&self, game: Arc<str>) -> Effect {
        let power = self.power_of(&game);
        Effect::Launch { game, power }
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::indexing_slicing)]
mod tests {
    use super::*;

    fn state() -> State {
        State::new(sysprog_library::demo_games())
    }

    #[test]
    fn starts_with_first_game_selected_and_focused() {
        let s = state();
        assert_eq!(s.selected.as_deref(), Some("zelda-lttp"));
        assert_eq!(s.focus, Some(Id::Game("zelda-lttp".into())));
    }

    #[test]
    fn selecting_then_activating_again_launches() {
        let mut s = state();
        let (_, fx) = s.apply(Event::Activate(Id::Game("ff6".into())));
        assert!(fx.is_empty());
        let (_, fx) = s.apply(Event::Activate(Id::Game("ff6".into())));
        assert_eq!(fx, vec![Effect::Launch { game: "ff6".into(), power: Power::Balanced }]);
    }

    #[test]
    fn power_is_per_game_and_used_at_launch() {
        let mut s = state();
        s.apply(Event::Activate(Id::Power(Power::High)));
        s.apply(Event::Activate(Id::Game("ff6".into())));
        assert_eq!(s.power_of("zelda-lttp"), Power::High);
        assert_eq!(s.power_of("ff6"), Power::Balanced);
        let (_, fx) = s.apply(Event::Activate(Id::Launch));
        assert_eq!(fx, vec![Effect::Launch { game: "ff6".into(), power: Power::Balanced }]);
    }

    #[test]
    fn current_mode_toggles_back_to_follow() {
        let mut s = state();
        s.apply(Event::Activate(Id::Mode(Mode::Keyboard)));
        assert_eq!(s.mode, Mode::Keyboard);
        s.apply(Event::Activate(Id::Mode(Mode::Keyboard)));
        assert_eq!(s.mode, Mode::Follow);
    }

    #[test]
    fn keys_repeat_and_modifiers_lock_when_held() {
        use keyboard::{Modifier, find};
        let s = state();
        assert_eq!(s.hold(&Id::Key(find(Key::Char('a')).unwrap())), Hold::Repeat);
        assert_eq!(s.hold(&Id::Key(find(Key::Mod(Modifier::Shift)).unwrap())), Hold::Long);
        assert_eq!(s.hold(&Id::Launch), Hold::Long);
    }

    #[test]
    fn down_and_up_light_a_target() {
        let mut s = state();
        s.apply(Event::Down(Id::Launch));
        assert_eq!(s.pressed, Some(Id::Launch));
        s.apply(Event::Up(Id::Launch));
        assert_eq!(s.pressed, None);
    }

    #[test]
    fn status_redraws_the_bottom_only_when_it_changes() {
        let mut s = state();
        let status = Status { battery: Some(50), ..Status::default() };
        assert_eq!(s.apply(Event::Status(status)).0, Dirty::BOTTOM);
        assert_eq!(s.apply(Event::Status(status)).0, Dirty::NONE);
    }

    #[test]
    fn touchpad_mode_switches_the_touchscreen() {
        let mut s = state();
        let (_, fx) = s.apply(Event::Activate(Id::Mode(Mode::Touchpad)));
        assert_eq!(fx, vec![Effect::Touchpad(true)]);
        let (_, fx) = s.apply(Event::Activate(Id::Mode(Mode::Keyboard)));
        assert_eq!(fx, vec![Effect::Touchpad(false)]);
        let (_, fx) = s.apply(Event::Activate(Id::Mode(Mode::Keyboard)));
        assert!(fx.is_empty());
    }

    #[test]
    fn pad_mouse_toggles_independently_of_the_mode() {
        let mut s = state();
        s.apply(Event::Activate(Id::Mode(Mode::Keyboard)));
        let (_, fx) = s.apply(Event::Activate(Id::PadMouse));
        assert_eq!(fx, vec![Effect::PadMouse(true)]);
        assert_eq!(s.mode, Mode::Keyboard);
        let (_, fx) = s.apply(Event::Activate(Id::PadMouse));
        assert_eq!(fx, vec![Effect::PadMouse(false)]);
    }

    #[test]
    fn the_focus_ring_shows_for_the_pad_only() {
        let mut s = state();
        assert!(!s.show_focus);
        s.apply(Event::Focus(Id::Launch));
        assert!(s.show_focus);
        assert_eq!(s.apply(Event::HideFocus).0, Dirty::BOTH);
        assert!(!s.show_focus);
        assert_eq!(s.apply(Event::HideFocus).0, Dirty::NONE);
    }

    #[test]
    fn tick_redraws_once_a_minute() {
        let mut s = state();
        let (dirty, _) = s.apply(Event::Tick { unix_secs: 60 });
        assert_eq!(dirty, Dirty::BOTH);
        let (dirty, _) = s.apply(Event::Tick { unix_secs: 119 });
        assert_eq!(dirty, Dirty::NONE);
    }
}
