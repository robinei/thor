//! What the battery tab shows beyond the bar's status: the charge over the last day (UPower's history)
//! and what is using the machine now (CPU and GPU time per program, read by the system side). The battery
//! reports only its total drain; per program there is no power reading, so programs are ranked by their
//! share of the work instead.

use crate::State;
use crate::instance::Instance;

/// Who a spender is: one of our instances or a game shelf started (by its scope), another program of
/// the user's (by process name), or everything the system runs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Group {
    Instance(Instance),
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
pub struct Battery {
    /// The charge over the last day, oldest first; empty without UPower.
    pub history: Vec<Charge>,
    /// The busiest first.
    pub spenders: Vec<Spender>,
    /// The history could be read (UPower runs and has a battery).
    pub has_history: bool,
}

impl State {
    /// A spender's name: the entry its scope is a launch of, the program's name, or "System".
    pub fn spender_name<'a>(&'a self, spender: &'a Spender) -> &'a str {
        match &spender.group {
            Group::Instance(instance) => {
                let key = instance.key();
                let entry = self.shelf.library().games.iter().find(|g| Some(g.key.as_str()) == key);
                entry.map_or_else(|| key.unwrap_or("?"), |g| g.title.as_str())
            }
            Group::Process(name) => name,
            Group::System => "System",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testkit::*;
    use crate::*;

    fn spender(group: Group) -> Spender {
        Spender { group, cpu: 0.1, gpu: 0.0 }
    }

    #[test]
    fn spenders_are_named_after_their_app_or_game() {
        let mut s = state();
        s.apply(Event::Tick { unix_secs: 1792010377 });
        let first = s.shelf().selected().cloned().unwrap_or_default();
        let zelda = launch(&mut s, &first);
        // Each scope keeps its own entry's name.
        let ff6 = launch(&mut s, "ff6");
        s.apply(GameEvent::Started("ff6".into()).into());
        let terminal = Instance::Run { key: clamshell_library::key_of("desktop:foot"), secs: 1, n: 9 };
        assert_eq!(s.spender_name(&spender(Group::Instance(terminal))), "Terminal");
        let title = |id: &str| s.game(id).map(|g| g.title.clone()).unwrap_or_default();
        assert_eq!(s.spender_name(&spender(Group::Instance(zelda))), title(&first));
        assert_eq!(s.spender_name(&spender(Group::Instance(ff6))), title("ff6"));
        assert_eq!(s.spender_name(&spender(Group::Process("firefox".into()))), "firefox");
        assert_eq!(s.spender_name(&spender(Group::System)), "System");
    }

    #[test]
    fn the_battery_tab_watches_while_open() {
        let mut s = state();
        assert_eq!(tap(&mut s, Id::Tab(Tab::Battery)), vec![Effect::Watch(Watch::Battery, true)]);
        assert_eq!(tap(&mut s, Id::Tab(Tab::Wifi)), vec![Effect::Watch(Watch::Battery, false), Effect::Watch(Watch::Wifi, true)]);
    }
}
