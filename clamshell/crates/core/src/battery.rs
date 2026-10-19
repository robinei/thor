//! What the battery tab shows beyond the bar's status: the charge over the last day (UPower's history)
//! and what is using the machine now (CPU and GPU time per program, read by the system side). The battery
//! reports only its total drain; per program there is no power reading, so programs are ranked by their
//! share of the work instead.

use crate::Effect;
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
    /// Memory its processes have resident (bytes).
    pub memory: u64,
    /// Its processes (another program's: what its Quit and Force kill end).
    pub pids: Vec<u32>,
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

/// A choice in the menu of a row of the list (a tap on it opens it): Quit or Force kill what it is, or
/// back to the list.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum BatteryId {
    /// The list's row, by position.
    Spender(u16),
    Quit,
    ForceKill,
    Back,
}

impl State {
    /// The row whose menu is open: what it is.
    pub fn spender_menu(&self) -> Option<&Group> {
        self.spender_menu.as_ref()
    }

    /// A tap on a row of what is using the machine (its menu opened: not the system's), or a choice in
    /// its menu: an entry quit or killed as from the dock, another program's processes asked to end or
    /// killed.
    pub(crate) fn on_battery(&mut self, id: BatteryId) -> Vec<Effect> {
        match id {
            BatteryId::Spender(i) => {
                self.spender_menu = self.sys.battery.spenders.get(usize::from(i)).map(|s| s.group.clone()).filter(|g| *g != Group::System);
                vec![]
            }
            BatteryId::Back => {
                self.spender_menu = None;
                vec![]
            }
            BatteryId::Quit | BatteryId::ForceKill => {
                let force = id == BatteryId::ForceKill;
                let Some(group) = self.spender_menu.take() else {
                    return vec![];
                };
                match &group {
                    Group::Instance(instance) => {
                        let entry = self.session.running.iter().find(|r| Some(r.key.as_str()) == instance.key()).map(|r| r.id.clone());
                        match entry {
                            Some(id) if force => self.force_kill(&id),
                            Some(id) => self.quit(&id),
                            None => vec![],
                        }
                    }
                    Group::Process(_) => {
                        let pids = self.sys.battery.spenders.iter().find(|s| s.group == group).map(|s| s.pids.clone()).unwrap_or_default();
                        if pids.is_empty() { vec![] } else { vec![Effect::EndProcesses { pids, force }] }
                    }
                    Group::System => vec![],
                }
            }
        }
    }

    /// What a running entry costs now, from the last reading (its launches together): CPU, GPU (of the
    /// machine, 0-1), resident memory (bytes). Read while the battery tab or the entry's dock menu is open.
    pub fn cost_of(&self, entry: &str) -> Option<(f32, f32, u64)> {
        let key = &self.session.running(entry)?.key;
        let mine = self.sys.battery.spenders.iter().filter(|s| matches!(&s.group, Group::Instance(i) if i.key() == Some(key.as_str())));
        mine.fold(None, |sum, s| {
            let (cpu, gpu, memory) = sum.unwrap_or((0.0, 0.0, 0));
            Some((cpu + s.cpu, gpu + s.gpu, memory + s.memory))
        })
    }

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
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    use crate::testkit::*;
    use crate::*;

    fn spender(group: Group) -> Spender {
        Spender { group, cpu: 0.1, gpu: 0.0, memory: 0, pids: vec![] }
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
    fn a_rows_menu_quits_or_kills_what_it_is() {
        let mut s = state();
        let zelda = launch(&mut s, "zelda-lttp");
        let firefox = Spender { pids: vec![40, 41], ..spender(Group::Process("firefox".into())) };
        let spenders = vec![spender(Group::Instance(zelda.clone())), firefox, spender(Group::System)];
        s.apply(Event::Reading(Reading::Battery(Battery { spenders, ..Battery::default() })));
        assert_eq!(s.cost_of("zelda-lttp"), Some((0.1, 0.0, 0)));
        // Another program: its processes.
        tap(&mut s, Id::Battery(BatteryId::Spender(1)));
        assert_eq!(s.spender_menu(), Some(&Group::Process("firefox".into())));
        assert_eq!(tap(&mut s, Id::Battery(BatteryId::ForceKill)), vec![Effect::EndProcesses { pids: vec![40, 41], force: true }]);
        assert_eq!(s.spender_menu(), None);
        // An entry: as from the dock (no window: ended at once).
        tap(&mut s, Id::Battery(BatteryId::Spender(0)));
        assert_eq!(tap(&mut s, Id::Battery(BatteryId::Quit)), vec![Effect::EndRun { key: zelda.key().unwrap().to_owned(), force: false }]);
        // The system's: no menu.
        tap(&mut s, Id::Battery(BatteryId::Spender(2)));
        assert_eq!(s.spender_menu(), None);
    }

    #[test]
    fn the_battery_tab_watches_while_open() {
        let mut s = state();
        assert_eq!(tap(&mut s, Id::Tab(Tab::Battery)), vec![Effect::Watch(Watch::Battery, true)]);
        assert_eq!(tap(&mut s, Id::Tab(Tab::Wifi)), vec![Effect::Watch(Watch::Battery, false), Effect::Watch(Watch::Wifi, true)]);
    }
}
