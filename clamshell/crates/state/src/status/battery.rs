//! What the battery tab shows beyond the bar's status: the charge over the last day, from UPower's
//! history, and what is using the machine now, as CPU and GPU time per program read by the system side.
//! The battery reports only its total drain. There is no power reading per program, so programs are
//! ranked by their share of the work instead.

use crate::out::Out;
use crate::session::launch_unit::LaunchUnit;
use crate::status::Sys;
use crate::{BatteryId, Effect, EntryAct, Event, Library, Running, Session};

/// Who a spender is. It is a launch of ours, an app or a game, known by its unit. Or an
/// app run in another program's processes, known by its app id, such as an Android app in Waydroid's
/// container. Or another of the user's programs, known by process name. Or everything the system runs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Group {
    Launch(LaunchUnit),
    App(String),
    Process(String),
    System,
}

impl Group {
    /// Whether its GPU time can be read. That is only possible for our user's processes. An app run in
    /// another program's processes, such as Android's, and the system's processes belong to another
    /// user.
    pub fn gpu_known(&self) -> bool {
        matches!(self, Group::Launch(_) | Group::Process(_))
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Spender {
    pub group: Group,
    /// CPU in use, as a share of all cores (0-1), and GPU busy, as a share of the GPU's time (0-1).
    pub cpu: f32,
    pub gpu: f32,
    /// Memory its processes have resident, in bytes.
    pub memory: u64,
    /// Its processes. For another program, these are what Quit and Force kill end.
    pub pids: Vec<u32>,
    /// Whether it may be ended from the list. The session's own processes can't be, meaning ClamShell and
    /// what it starts, PipeWire and the bus, and neither can the system.
    pub endable: bool,
}

impl Spender {
    /// How much of the machine's work it is doing, for ranking and the bars.
    pub fn load(&self) -> f32 {
        self.cpu + self.gpu
    }
}

/// A point of the charge history: when, in epoch seconds, the level (0-100), and whether it was
/// charging.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Charge {
    pub at: u64,
    pub level: u8,
    pub charging: bool,
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct Battery {
    /// The charge over the last day, oldest first. Empty without UPower.
    pub history: Vec<Charge>,
    /// What is using the machine, busiest first.
    pub spenders: Vec<Spender>,
    /// Whether the history could be read, meaning UPower is running and has a battery.
    pub has_history: bool,
}

/// What a running entry costs: its share of the machine's CPU and GPU (0-1), and its resident memory in
/// bytes. The GPU share is unknown if any of it can't be read (see [`Group::gpu_known`]).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Cost {
    pub cpu: f32,
    pub gpu: Option<f32>,
    pub memory: u64,
}

impl Spender {
    /// Its name: the entry its unit is a launch of, the program's name, or "System".
    pub fn name<'a>(&'a self, library: &'a Library) -> &'a str {
        match &self.group {
            Group::Launch(unit) => {
                let entry = library.entries.iter().find(|g| g.key == unit.key);
                entry.map_or(unit.key.as_str(), |g| g.title.as_str())
            }
            Group::App(app) => {
                let entry = library.entries.iter().find(|g| clamshell_library::ids::desktop_file(&g.id) == Some(app.as_str()));
                entry.map_or(app.as_str(), |g| g.title.as_str())
            }
            Group::Process(name) => name,
            Group::System => "System",
        }
    }
}

impl Sys {
    /// What the row whose menu is open is.
    pub fn spender_menu(&self) -> Option<&Group> {
        self.spender_menu.as_ref()
    }

    /// A tap on a row of what is using the machine, or a choice in a row's menu. A tap opens the row's
    /// menu, except for rows that can't be ended, such as the system's. In the menu, an entry is quit or
    /// killed as from the dock, and another program's processes are asked to end or killed.
    pub(crate) fn on_battery(&mut self, id: BatteryId, session: &Session, out: &mut Out) {
        match id {
            BatteryId::Spender(i) => {
                // An app is ended as its entry, so only one that is running gets a menu.
                let endable = |s: &&Spender| s.endable && !matches!(&s.group, Group::App(app) if session.running_app(app).is_none());
                self.spender_menu = self.battery.spenders.get(usize::from(i)).filter(endable).map(|s| s.group.clone());
            }
            BatteryId::Back => self.spender_menu = None,
            BatteryId::Quit | BatteryId::ForceKill => {
                let force = id == BatteryId::ForceKill;
                let Some(group) = self.spender_menu.take() else {
                    return;
                };
                let entry = match &group {
                    Group::Launch(unit) => session.running_entries().iter().find(|r| r.key == unit.key).map(|r| r.id.clone()),
                    Group::App(app) => session.running_app(app),
                    Group::Process(_) => {
                        let pids = self.battery.spenders.iter().find(|s| s.group == group).map(|s| s.pids.clone()).unwrap_or_default();
                        if !pids.is_empty() {
                            out.effects.push(Effect::EndProcesses { pids, force });
                        }
                        return;
                    }
                    Group::System => None,
                };
                let act = if force { EntryAct::ForceKill } else { EntryAct::Quit };
                out.events.extend(entry.map(|id| Event::Entry(id, act)));
            }
        }
    }

    /// What a running entry costs now, from the last reading, with all its launches together. It is read
    /// while the battery tab or the entry's dock menu is open.
    pub fn cost_of(&self, r: &Running) -> Option<Cost> {
        let mine = self.battery.spenders.iter().filter(|s| match &s.group {
            Group::Launch(i) => i.key == r.key.as_str(),
            Group::App(app) => r.app_id.as_deref() == Some(app.as_str()),
            _ => false,
        });
        mine.fold(None, |sum, s| {
            let Cost { cpu, gpu, memory } = sum.unwrap_or(Cost { cpu: 0.0, gpu: Some(0.0), memory: 0 });
            let gpu = gpu.zip(s.group.gpu_known().then_some(s.gpu)).map(|(a, b)| a + b);
            Some(Cost { cpu: cpu + s.cpu, gpu, memory: memory + s.memory })
        })
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::Group;
    use super::*;
    use crate::Compositor;
    use crate::id::BatteryId;
    use crate::testkit::*;
    use crate::*;

    fn spender(group: Group) -> Spender {
        let endable = group != Group::System;
        Spender { group, cpu: 0.1, gpu: 0.0, memory: 0, pids: vec![], endable }
    }

    #[test]
    fn spenders_are_named_after_their_app_or_game() {
        let mut s = state();
        s.applied(Event::tick(1792010377));
        let first = s.shelf().selected().cloned().unwrap_or_default();
        let zelda = launch(&mut s, &first);
        // Each unit keeps its own entry's name.
        let ff6 = launch(&mut s, "ff6");
        crate::testkit::start(&mut s, "ff6");
        let terminal = LaunchUnit { key: clamshell_library::key_of("desktop:foot"), secs: 1, n: 9 };
        assert_eq!(spender(Group::Launch(terminal)).name(s.shelf().library()), "Terminal");
        let title = |id: &str| s.entry(id).map(|g| g.title.clone()).unwrap_or_default();
        assert_eq!(spender(Group::Launch(zelda)).name(s.shelf().library()), title(&first));
        assert_eq!(spender(Group::Launch(ff6)).name(s.shelf().library()), title("ff6"));
        assert_eq!(spender(Group::Process("firefox".into())).name(s.shelf().library()), "firefox");
        assert_eq!(spender(Group::System).name(s.shelf().library()), "System");
    }

    #[test]
    fn a_rows_menu_quits_or_kills_what_it_is() {
        let mut s = state();
        let zelda = launch(&mut s, "zelda-lttp");
        let firefox = Spender { pids: vec![40, 41], ..spender(Group::Process("firefox".into())) };
        let spenders = vec![spender(Group::Launch(zelda.clone())), firefox, spender(Group::System)];
        s.applied(Event::Reading(Reading::Battery(Battery { spenders, ..Battery::default() })));
        assert_eq!(s.session().running("zelda-lttp").and_then(|r| s.sys().cost_of(r)), Some(Cost { cpu: 0.1, gpu: Some(0.0), memory: 0 }));
        // Another program ends its processes.
        tap(&mut s, Id::Battery(BatteryId::Spender(1)));
        assert_eq!(s.sys().spender_menu(), Some(&Group::Process("firefox".into())));
        assert_eq!(tap(&mut s, Id::Battery(BatteryId::ForceKill)), vec![Effect::EndProcesses { pids: vec![40, 41], force: true }]);
        assert_eq!(s.sys().spender_menu(), None);
        // An entry ends as from the dock. With no window, it is ended at once.
        tap(&mut s, Id::Battery(BatteryId::Spender(0)));
        assert_eq!(tap(&mut s, Id::Battery(BatteryId::Quit)), vec![Effect::EndRun { key: zelda.key.clone(), force: false }]);
        // The system's row, or the session's own, has no menu.
        tap(&mut s, Id::Battery(BatteryId::Spender(2)));
        assert_eq!(s.sys().spender_menu(), None);
        let own = Spender { endable: false, ..spender(Group::Process("pipewire".into())) };
        s.applied(Event::Reading(Reading::Battery(Battery { spenders: vec![own], ..Battery::default() })));
        tap(&mut s, Id::Battery(BatteryId::Spender(0)));
        assert_eq!(s.sys().spender_menu(), None);
    }

    #[test]
    fn an_app_handed_over_is_known_by_its_app_id_and_edited_without_what_it_cant_get() {
        let mut library = clamshell_library::demo::library();
        let firefox = library.entries.iter_mut().find(|g| &*g.id == "desktop:firefox").unwrap();
        firefox.another = true;
        firefox.runner = Some(clamshell_library::RunnerOf { handover: true, ..clamshell_library::RunnerOf::new("android", "Android", true) });
        let mut s = state_with(library);
        launch_now(&mut s, "desktop:firefox");
        crate::testkit::start(&mut s, "desktop:firefox");
        crate::testkit::end(&mut s, "desktop:firefox");
        s.observed(vec![win(6, "firefox")]);
        assert!(!s.opens_another("desktop:firefox"), "a launch would show the one it runs");
        // Its processes belong to another program, as Android apps do. The entry gets their cost, and
        // the row is named and ended as the entry.
        let spenders = vec![spender(Group::App("firefox".into())), spender(Group::App("org.other".into()))];
        s.applied(Event::Reading(Reading::Battery(Battery { spenders, ..Battery::default() })));
        assert_eq!(
            s.session().running("desktop:firefox").and_then(|r| s.sys().cost_of(r)),
            Some(Cost { cpu: 0.1, gpu: None, memory: 0 }),
            "its GPU time: another user's"
        );
        assert_eq!(spender(Group::App("firefox".into())).name(s.shelf().library()), "Firefox");
        tap(&mut s, Id::Battery(BatteryId::Spender(1)));
        assert_eq!(s.sys().spender_menu(), None, "an app that isn't an entry running: no menu");
        tap(&mut s, Id::Battery(BatteryId::Spender(0)));
        assert_eq!(tap(&mut s, Id::Battery(BatteryId::Quit)), vec![Effect::Compositor(Compositor::Close(vec![6]))]);
        // Settings that go through the launch's own process don't reach it either.
        tap(&mut s, Id::Settings(SettingsId::Scope(SettingsLevel::Global)));
        assert!(!s.panel().handed_over(), "All");
        tap(&mut s, Id::Settings(SettingsId::Scope(SettingsLevel::Runner)));
        assert!(s.panel().handed_over());
    }

    #[test]
    fn a_rows_menu_closes_when_the_tab_is_left_or_the_row_goes() {
        let mut s = state();
        tap(&mut s, Id::Tab(Tab::Battery));
        let firefox = || Battery { spenders: vec![spender(Group::Process("firefox".into()))], ..Battery::default() };
        s.applied(Event::Reading(Reading::Battery(firefox())));
        tap(&mut s, Id::Battery(BatteryId::Spender(0)));
        assert!(s.sys().spender_menu().is_some());
        tap(&mut s, Id::Tab(Tab::Wifi));
        assert_eq!(s.sys().spender_menu(), None, "the tab left");
        tap(&mut s, Id::Tab(Tab::Battery));
        s.applied(Event::Reading(Reading::Battery(firefox())));
        tap(&mut s, Id::Battery(BatteryId::Spender(0)));
        s.applied(Event::Reading(Reading::Battery(firefox())));
        assert!(s.sys().spender_menu().is_some(), "the row is still there");
        s.applied(Event::Reading(Reading::Battery(Battery::default())));
        assert_eq!(s.sys().spender_menu(), None, "the row gone");
    }

    #[test]
    fn the_battery_tab_watches_while_open() {
        let mut s = state();
        assert_eq!(tap(&mut s, Id::Tab(Tab::Battery)), vec![Effect::Watch(Watch::Metrics, false), Effect::Watch(Watch::Battery, true)]);
        assert_eq!(tap(&mut s, Id::Tab(Tab::Wifi)), vec![Effect::Watch(Watch::Battery, false), Effect::Watch(Watch::Wifi, true)]);
    }
}
