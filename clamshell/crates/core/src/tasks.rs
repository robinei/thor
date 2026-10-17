//! The dock as a task switcher. An app's instances are the systemd scopes we started it in
//! ([`Instance::App`], as systemd lists them); all their windows are tabs on the app's one workspace
//! (sway places a launch's first window there, the sway side moves later ones by their scope). A tap on
//! an app starts it when it isn't running and shows its workspace when it is (its tabs switch its windows).
//! A long press closes the instance whose tab is in view (its windows, politely, as their
//! close buttons would); on the game tile it quits the game. A "+" on running apps that allow several
//! starts another instance, a tab of the same workspace. A new instance (or game) is shown when its first
//! window appears, not before, also if the user has gone elsewhere meanwhile; a newer start takes its
//! place. A program whose windows are all on the bottom screen: a tap shows them there.

use crate::State;
use crate::effect::{Effect, WindowCommand};
use crate::id::DockId;
use crate::instance::Instance;
use crate::session::Pending;
use crate::windows::Window;

/// A program on the dock: an app (its instances) or a running game.
#[derive(Debug, Clone, Copy)]
enum Program {
    App(usize),
    Game(usize),
}

impl State {
    /// The key of dock app `i`.
    fn app_key(&self, i: usize) -> Option<&str> {
        self.apps.list.get(i).map(|a| a.key())
    }

    /// Whether the workspace `name` is one of dock app `i`'s instances.
    fn is_instance(&self, i: usize, name: &str) -> bool {
        self.app_key(i).is_some_and(|key| Instance::app_key_of(name) == Some(key))
    }

    /// Dock app `i`'s instances that systemd lists.
    fn instances(&self, i: usize) -> impl Iterator<Item = &Instance> {
        let key = self.app_key(i);
        self.session.units.iter().filter(move |u| key.is_some() && u.app_key() == key)
    }

    /// Whether dock app `i` runs (or is starting).
    pub fn app_running(&self, i: usize) -> bool {
        let key = self.app_key(i);
        let mine = |instance: &Instance| key.is_some() && instance.app_key() == key;
        self.session.units.iter().any(mine) || self.session.pending.iter().any(|p| mine(&p.instance))
    }

    /// The workspace a window belongs to: the one it is on, or (moved down) its scope's, which has the
    /// same name.
    fn place(w: &Window) -> &str {
        match (&w.unit, w.down) {
            (Some(unit), true) => unit.trim_end_matches(".scope"),
            _ => &w.workspace,
        }
    }

    /// Whether a window is a dock program's: on (or, moved down, from) one of its workspaces.
    fn program_has(&self, p: Program, w: &Window) -> bool {
        let place = Self::place(w);
        match p {
            Program::App(i) => self.is_instance(i, place),
            Program::Game(i) => self.session.games.get(i).is_some_and(|g| g.instance.is(place)),
        }
    }

    /// A tap on a dock program with all its windows on the bottom screen: they are shown there. None:
    /// otherwise (the tile's usual tap).
    fn tap_program(&mut self, p: Program) -> Option<Vec<Effect>> {
        let windows = &self.session.windows;
        if windows.iter().any(|w| !w.down && self.program_has(p, w)) {
            return None;
        }
        let down = windows.iter().find(|w| w.down && self.program_has(p, w))?.id;
        Some(self.show_program_down(down))
    }

    /// A tap or long press on a dock tile.
    pub(crate) fn on_dock(&mut self, id: DockId, long: bool) -> Vec<Effect> {
        match id {
            DockId::App(i) if !long && let Some(effects) = self.tap_program(Program::App(usize::from(i))) => effects,
            DockId::GameTile(i) if !long && let Some(effects) = self.tap_program(Program::Game(usize::from(i))) => effects,
            DockId::App(i) => self.tap_app(usize::from(i), long),
            DockId::GameTile(i) => self.tap_game(usize::from(i), long),
            DockId::StartGame => vec![Effect::Window(WindowCommand::ShowShelf)],
            DockId::NewInstance(i) => self.new_instance(usize::from(i)),
        }
    }

    fn tap_app(&mut self, i: usize, long: bool) -> Vec<Effect> {
        let Some(key) = self.app_key(i) else {
            return vec![];
        };
        if long {
            return self.close_tab(i);
        }
        if self.instances(i).next().is_none() {
            // Started and not listed yet: wait for it.
            if self.app_running(i) {
                return vec![];
            }
            return self.start_app(i);
        }
        vec![Effect::Window(WindowCommand::Focus(Instance::app_workspace(key)))]
    }

    /// A long press on a running app: the instance whose tab is in view (in front, else shown), else its
    /// latest, closed: its windows, politely.
    fn close_tab(&self, i: usize) -> Vec<Effect> {
        let mine = |w: &&Window| !w.down && self.is_instance(i, &w.workspace);
        let windows = &self.session.windows;
        let unit = windows.iter().filter(mine).find(|w| w.focused).or_else(|| windows.iter().filter(mine).find(|w| w.visible)).and_then(|w| w.unit.clone());
        let unit = unit.or_else(|| self.instances(i).max_by_key(|u| u.order()).and_then(Instance::unit));
        let Some(unit) = unit else {
            return vec![];
        };
        let closing: Vec<i64> = windows.iter().filter(|w| w.unit.as_deref() == Some(unit.as_str())).map(|w| w.id).collect();
        if closing.is_empty() { vec![] } else { vec![Effect::Window(WindowCommand::Close(closing))] }
    }

    /// Whether an app shows a "+": it allows several instances and runs.
    pub fn offers_new(&self, i: usize) -> bool {
        self.apps.list.get(i).is_some_and(|a| a.multiple && !a.command.is_empty()) && self.app_running(i)
    }

    fn new_instance(&mut self, i: usize) -> Vec<Effect> {
        if !self.offers_new(i) {
            return vec![];
        }
        self.start_app(i)
    }

    /// A new instance: named, remembered as starting, and shown once its first window appears.
    fn start_app(&mut self, i: usize) -> Vec<Effect> {
        let Some(app) = self.apps.list.get(i) else {
            return vec![];
        };
        let (key, command) = (app.key().to_owned(), app.command.clone());
        let (secs, n) = self.session.new_name(self.unix_secs);
        let instance = Instance::App { key, secs, n };
        self.session.pending.push(Pending { instance: instance.clone(), since: self.unix_secs });
        self.session.pending_show = Some(instance.clone());
        vec![Effect::RunApp { instance, command }]
    }

    /// A running game's tile: a tap shows it, a long press quits it.
    fn tap_game(&mut self, i: usize, long: bool) -> Vec<Effect> {
        let Some(g) = self.session.games.get(i) else {
            return vec![];
        };
        if long {
            let id = g.id.clone();
            return self.quit_game(&id);
        }
        vec![Effect::Window(WindowCommand::Focus(g.instance.name()))]
    }
}

#[cfg(test)]
mod tests {
    use crate::testkit::*;
    use crate::*;

    const TERMINAL: u16 = 1;

    fn terminal(n: u64) -> Instance {
        Instance::App { key: "terminal".into(), secs: 100, n }
    }

    fn run(instance: Instance) -> Vec<Effect> {
        vec![Effect::RunApp { instance, command: vec!["foot".into()] }]
    }

    #[test]
    fn a_tap_starts_then_shows_the_apps_workspace_with_all_instances_as_tabs() {
        let mut s = state();
        s.apply(Event::Tick { unix_secs: 100 });
        let app = Id::Dock(DockId::App(TERMINAL));
        let (t1, t2) = (terminal(0), terminal(1));
        let space = t1.workspace();
        // A window of an instance: on the app's workspace, by its scope.
        let of = |id, t: &Instance, focused| Window { unit: t.unit(), ..win(id, "foot", &space, focused) };
        let shelf = || win(1, apps::SHELF_APP_ID, &Instance::Shelf.name(), true);
        assert_eq!(tap(&mut s, app.clone()), run(t1.clone()));
        assert!(s.app_running(1), "starting counts as running");
        assert_eq!(tap(&mut s, app.clone()), vec![], "starting: no second one");
        // systemd lists it; its window appears on the app's workspace: shown then, not before.
        s.apply(Event::Units(vec![t1.clone()]));
        assert_eq!(s.apply(Event::Windows(vec![shelf()])).1, vec![]);
        assert_eq!(s.apply(Event::Windows(vec![shelf(), of(10, &t1, false)])).1, vec![focus(&t1)]);
        // "+": a second, a tab of the same workspace, shown when its own window is there.
        assert!(s.offers_new(1) && !s.offers_new(0), "Steam allows one");
        assert_eq!(tap(&mut s, Id::Dock(DockId::NewInstance(TERMINAL))), run(t2.clone()));
        s.apply(Event::Units(vec![t1.clone(), t2.clone()]));
        assert_eq!(s.apply(Event::Windows(vec![shelf(), of(10, &t1, false)])).1, vec![], "the first's window isn't the second's");
        assert_eq!(s.apply(Event::Windows(vec![of(10, &t1, false), of(12, &t2, false), of(13, &t2, true)])).1, vec![focus(&t2)]);
        // In front, or not: the app's workspace (its tabs switch its windows).
        assert_eq!(tap(&mut s, app.clone()), vec![focus(&t1)]);
        s.apply(Event::Windows(vec![of(10, &t1, false), Window { visible: true, ..of(12, &t2, false) }, of(13, &t2, false), win(20, "firefox", "1", true)]));
        assert_eq!(tap(&mut s, app.clone()), vec![focus(&t1)]);
        // A long press closes the instance whose tab is in view: its windows.
        assert_eq!(long(&mut s, app), vec![Effect::Window(WindowCommand::Close(vec![12, 13]))]);
    }

    #[test]
    fn a_program_with_its_windows_moved_down_shows_them_on_the_bottom_screen() {
        let mut s = state();
        s.apply(Event::Tick { unix_secs: 100 });
        let t1 = terminal(0);
        s.apply(Event::Units(vec![t1.clone()]));
        let down = |id| Window { down: true, unit: t1.unit(), ..win(id, "foot", &Instance::Bottom.name(), false) };
        // Both its windows down, ClamShell's view below, shelf in front: a tap shows them there.
        s.apply(Event::Windows(vec![win(1, apps::SHELF_APP_ID, &Instance::Shelf.name(), true), down(10), down(11)]));
        assert!(!s.bottom_shows_windows());
        let app = Id::Dock(DockId::App(TERMINAL));
        assert_eq!(tap(&mut s, app), vec![Effect::Window(WindowCommand::ShowDown(10))]);
        assert!(s.bottom_shows_windows());
    }

    #[test]
    fn a_failed_start_is_forgotten() {
        let mut s = state();
        s.apply(Event::Tick { unix_secs: 100 });
        tap(&mut s, Id::Dock(DockId::App(TERMINAL)));
        s.apply(Event::Tick { unix_secs: 120 });
        assert!(!s.app_running(1));
        assert!(matches!(tap(&mut s, Id::Dock(DockId::App(TERMINAL))).as_slice(), [Effect::RunApp { .. }]), "tapping starts it again");
    }

    #[test]
    fn a_game_tile_goes_to_its_game_and_quits_it() {
        let mut s = state();
        let tile = Id::Dock(DockId::GameTile(0));
        assert_eq!(tap(&mut s, tile.clone()), vec![], "no game");
        s.apply(Event::Tick { unix_secs: 77 });
        let game = launch(&mut s, "zelda-lttp");
        assert_eq!(game, Instance::Game { secs: 77, n: 0 });
        s.apply(Event::Units(vec![game.clone()]));
        assert_eq!(s.apply(Event::Windows(vec![win(5, "retroarch", &game.name(), false)])).1, vec![focus(&game)], "shown when its window appears");
        assert_eq!(tap(&mut s, tile.clone()), vec![focus(&game)]);
        assert_eq!(long(&mut s, tile), vec![Effect::QuitGame { windows: vec![5], units: vec![game] }]);
    }
}
