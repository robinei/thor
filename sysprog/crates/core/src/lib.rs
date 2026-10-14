//! The program's state and its rules. No I/O: every change comes in as an [`Event`], and what the outside
//! world must do comes back as [`Effect`]s. The UI and the policy threads only read the state and send
//! events, so all of this is unit-testable.

use std::collections::HashMap;

pub use sysprog_library::Game;

/// Second-screen modes (docs/plans.md, "Second screen"). Follow shows the current game's settings.
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
    pub const ALL: [Mode; 5] = [Mode::Follow, Mode::Launcher, Mode::Keyboard, Mode::Touchpad, Mode::Sysinfo];

    pub fn label(self) -> &'static str {
        match self {
            Mode::Follow => "Game",
            Mode::Launcher => "Apps",
            Mode::Keyboard => "Keys",
            Mode::Touchpad => "Pad",
            Mode::Sysinfo => "Info",
        }
    }
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
    Game(String),
    Mode(Mode),
    Power(Power),
    Launch,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Event {
    /// Wall clock, once a second (policy thread).
    Tick { unix_secs: u64 },
    /// Tap, click, or the pad's A on a focused item.
    Activate(Id),
    /// The pad moved focus (the UI finds the target, since only it knows the layout).
    Focus(Id),
    Quit,
}

/// What the state asks the outside world to do.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Effect {
    Launch { game: String, power: Power },
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
    pub selected: Option<String>,
    /// Per-game power setting, by game id; absent = default.
    pub power: HashMap<String, Power>,
    pub mode: Mode,
    pub focus: Option<Id>,
    pub unix_secs: u64,
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
                self.unix_secs = unix_secs;
                (Dirty::BOTTOM, vec![])
            }
            Event::Focus(id) => {
                self.focus = Some(id);
                (Dirty::BOTH, vec![])
            }
            Event::Activate(id) => {
                self.focus = Some(id.clone());
                self.activate(id)
            }
            Event::Quit => (Dirty::NONE, vec![Effect::Quit]),
        }
    }

    fn activate(&mut self, id: Id) -> (Dirty, Vec<Effect>) {
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
                self.mode = if self.mode == mode { Mode::Follow } else { mode };
                (Dirty::BOTH, vec![])
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
        }
    }

    fn launch_effect(&self, game: String) -> Effect {
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
    fn tick_redraws_only_the_bottom() {
        let mut s = state();
        let (dirty, _) = s.apply(Event::Tick { unix_secs: 60 });
        assert_eq!(dirty, Dirty::BOTTOM);
    }
}
