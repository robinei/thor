//! What the battery mode shows beyond the bar's status: the charge over the last day (UPower's history)
//! and what is using the machine now (CPU and GPU time per program, read by the policy side). The battery
//! reports only its total drain; per program there is no power reading, so programs are ranked by their
//! share of the work instead.

use crate::State;

/// Who a spender is: one of our instances or the game (a systemd scope, by unit name without
/// `.scope`), another program of the user's (by process name), or everything the system runs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Group {
    Unit(String),
    Process(String),
    System,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Spender {
    pub group: Group,
    /// CPU in use, of all cores (0-1); GPU busy, of the GPU's time (0-1).
    pub cpu: f32,
    pub gpu: f32,
}

impl Spender {
    /// How much of the machine's work it is doing, for ranking and the bars.
    pub fn load(&self) -> f32 {
        self.cpu + self.gpu
    }
}

/// A point of the charge history: when (epoch seconds), the level (0-100), and whether charging.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Charge {
    pub at: u64,
    pub level: u8,
    pub charging: bool,
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct Power {
    /// The charge over the last day, oldest first; empty without UPower.
    pub history: Vec<Charge>,
    /// The busiest first.
    pub spenders: Vec<Spender>,
    /// The history could be read (UPower runs and has a battery).
    pub has_history: bool,
}

impl State {
    /// A spender's name: the app or game its scope belongs to, the program's name, or "System".
    pub fn spender_name<'a>(&'a self, spender: &'a Spender) -> &'a str {
        match &spender.group {
            Group::Unit(unit) if unit.starts_with("clamshell-game-") => {
                self.running.as_ref().and_then(|id| self.library.games.iter().find(|g| &g.id == id)).map_or("Game", |g| g.title.as_str())
            }
            Group::Unit(unit) => {
                let key = unit.strip_prefix("clamshell-app-").and_then(|rest| rest.rsplitn(3, '-').nth(2));
                key.and_then(|key| self.apps.iter().find(|a| a.key() == key)).map_or(unit.as_str(), |a| a.name.as_str())
            }
            Group::Process(name) => name,
            Group::System => "System",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tests::state;
    use crate::{Event, Id, Mode};

    fn spender(group: Group) -> Spender {
        Spender { group, cpu: 0.1, gpu: 0.0 }
    }

    #[test]
    fn spenders_are_named_after_their_app_or_game() {
        let mut s = state();
        s.apply(Event::Started("zelda-lttp".into()));
        let terminal = s.apps.iter().find(|a| a.multiple).map(|a| a.key().to_owned()).unwrap_or_default();
        let unit = format!("clamshell-app-{terminal}-1792010377-0");
        assert_eq!(s.spender_name(&spender(Group::Unit(unit))), "Terminal");
        assert_eq!(s.spender_name(&spender(Group::Unit("clamshell-game-1792010377-0".into()))), "A Link to the Past");
        assert_eq!(s.spender_name(&spender(Group::Process("firefox".into()))), "firefox");
        assert_eq!(s.spender_name(&spender(Group::System)), "System");
    }

    #[test]
    fn the_battery_mode_watches_while_open() {
        let mut s = state();
        let (_, fx) = s.apply(Event::Activate(Id::Mode(Mode::Battery)));
        assert_eq!(fx, vec![crate::Effect::WatchPower(true)]);
        let (_, fx) = s.apply(Event::Activate(Id::Mode(Mode::Wifi)));
        assert_eq!(fx, vec![crate::Effect::WatchPower(false), crate::Effect::Wifi(crate::wifi::Command::Watch(true))]);
        let (_, fx) = s.apply(Event::Activate(Id::Mode(Mode::Wifi)));
        assert_eq!((s.mode, fx), (Mode::Sysinfo, vec![crate::Effect::Wifi(crate::wifi::Command::Watch(false))]), "back to the bar's mode");
    }
}
