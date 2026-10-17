//! The dock as a task switcher. An app's instances are the systemd scopes we started it in
//! ([`Instance::App`], as systemd lists them); all their windows are tabs on the app's one workspace
//! (sway places a launch's first window there, the sway side moves later ones by their scope). A tap on
//! an app starts it when it isn't running and shows its workspace when it is (its tabs switch its windows).
//! A long press on a running app opens its menu: Open another (an app that allows several: another
//! instance, a tab of the same workspace), Close (with several instances: Close active, the one whose
//! tab is in view, and Close all), closing their windows politely, as their close buttons would; an
//! instance without a window (a program left running in the background) has its scope ended. On a game's
//! tile the menu has Quit. A new instance (or game) is shown when its first
//! window appears, not before, also if the user has gone elsewhere meanwhile; a newer start takes its
//! place. A program whose windows are all on the bottom screen: a tap shows them there.

use crate::State;
use crate::effect::{Effect, WindowCommand};
use crate::id::DockId;
use crate::instance::Instance;
use crate::session::Pending;
use crate::windows::Window;

/// The menu a long press on a dock tile opened: an app's (by position), a running game's.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DockMenu {
    App(u16),
    Game(u16),
}

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

    /// The menu a long press on a dock tile opened, if one is.
    pub fn dock_menu(&self) -> Option<DockMenu> {
        self.bottom.dock_menu
    }

    /// How many instances dock app `i` runs.
    pub fn app_instances(&self, i: usize) -> usize {
        self.instances(i).count()
    }

    /// A tap on a dock tile, a long press (a running app's or game's menu), or a choice in the menu
    /// (which then closes).
    pub(crate) fn on_dock(&mut self, id: DockId, long: bool) -> Vec<Effect> {
        let menu = self.bottom.dock_menu;
        match (id, menu) {
            (DockId::App(i), _) if long => {
                if self.app_running(usize::from(i)) {
                    self.bottom.dock_menu = Some(DockMenu::App(i));
                }
                return vec![];
            }
            (DockId::GameTile(i), _) if long => {
                self.bottom.dock_menu = Some(DockMenu::Game(i));
                return vec![];
            }
            (DockId::App(_) | DockId::GameTile(_) | DockId::StartGame, _) => {}
            (choice, _) => {
                self.bottom.dock_menu = None;
                return match (choice, menu) {
                    (DockId::OpenAnother, Some(DockMenu::App(i))) => self.new_instance(usize::from(i)),
                    (DockId::CloseActive, Some(DockMenu::App(i))) => self.close_tab(usize::from(i)),
                    (DockId::CloseAll, Some(DockMenu::App(i))) => self.close_all(usize::from(i)),
                    (DockId::Quit, Some(DockMenu::Game(i))) => match self.session.games.get(usize::from(i)).map(|g| g.id.clone()) {
                        Some(id) => self.quit_game(&id),
                        None => vec![],
                    },
                    _ => vec![],
                };
            }
        }
        let program = match id {
            DockId::App(i) => Program::App(usize::from(i)),
            DockId::GameTile(i) => Program::Game(usize::from(i)),
            _ => return vec![Effect::Window(WindowCommand::ShowShelf)],
        };
        if let Some(effects) = self.tap_program(program) {
            return effects;
        }
        match program {
            Program::App(i) => self.tap_app(i),
            Program::Game(i) => self.tap_game(i),
        }
    }

    fn tap_app(&mut self, i: usize) -> Vec<Effect> {
        let Some(key) = self.app_key(i) else {
            return vec![];
        };
        if self.instances(i).next().is_none() {
            // Started and not listed yet: wait for it.
            if self.app_running(i) {
                return vec![];
            }
            return self.start_app(i);
        }
        vec![Effect::Window(WindowCommand::Focus(Instance::app_workspace(key)))]
    }

    /// The menu's Close (Close active): the instance whose tab is in view on the top screen (in front, else
    /// shown), else (nothing of the app on top) on the bottom screen, else its latest, closed: its windows
    /// on both screens, politely.
    fn close_tab(&self, i: usize) -> Vec<Effect> {
        let windows = &self.session.windows;
        let top = |w: &&Window| !w.down && self.is_instance(i, &w.workspace);
        let below = |w: &&Window| w.down && self.program_has(Program::App(i), w);
        let active = windows.iter().filter(top).find(|w| w.focused).or_else(|| windows.iter().filter(top).find(|w| w.visible));
        let active = active.or_else(|| windows.iter().find(top).is_none().then(|| windows.iter().filter(below).find(|w| w.visible)).flatten());
        let instance = active.and_then(Window::instance).or_else(|| self.instances(i).max_by_key(|u| u.order()).cloned());
        self.close(instance.into_iter().collect())
    }

    /// The menu's Close all: every instance.
    fn close_all(&self, i: usize) -> Vec<Effect> {
        self.close(self.instances(i).cloned().collect())
    }

    /// App instances closed: their windows, politely; the scopes of those without one ended.
    fn close(&self, instances: Vec<Instance>) -> Vec<Effect> {
        let of = |w: &Window, instance: &Instance| w.unit.is_some() && w.unit == instance.unit();
        let windows: Vec<i64> = self.session.windows.iter().filter(|w| instances.iter().any(|i| of(w, i))).map(|w| w.id).collect();
        let windowless: Vec<Instance> = instances.into_iter().filter(|i| !self.session.windows.iter().any(|w| of(w, i))).collect();
        let mut effects = vec![];
        if !windows.is_empty() {
            effects.push(Effect::Window(WindowCommand::Close(windows)));
        }
        if !windowless.is_empty() {
            effects.push(Effect::EndScopes(windowless));
        }
        effects
    }

    /// Whether an app's menu offers another instance: it allows several and runs.
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

    /// A running game's tile: a tap shows it.
    fn tap_game(&mut self, i: usize) -> Vec<Effect> {
        let Some(g) = self.session.games.get(i) else {
            return vec![];
        };
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
        assert_eq!(long(&mut s, app.clone()), vec![], "its menu");
        assert_eq!(s.dock_menu(), Some(DockMenu::App(TERMINAL)));
        assert_eq!(tap(&mut s, Id::Dock(DockId::OpenAnother)), run(t2.clone()));
        assert_eq!(s.dock_menu(), None, "a choice closes it");
        s.apply(Event::Units(vec![t1.clone(), t2.clone()]));
        assert_eq!(s.apply(Event::Windows(vec![shelf(), of(10, &t1, false)])).1, vec![], "the first's window isn't the second's");
        assert_eq!(s.apply(Event::Windows(vec![of(10, &t1, false), of(12, &t2, false), of(13, &t2, true)])).1, vec![focus(&t2)]);
        // In front, or not: the app's workspace (its tabs switch its windows).
        assert_eq!(tap(&mut s, app.clone()), vec![focus(&t1)]);
        s.apply(Event::Windows(vec![of(10, &t1, false), Window { visible: true, ..of(12, &t2, false) }, of(13, &t2, false), win(20, "firefox", "1", true)]));
        assert_eq!(tap(&mut s, app.clone()), vec![focus(&t1)]);
        // Its menu: Close active closes the instance whose tab is in view (its windows); Close all, every
        // instance's.
        assert_eq!(s.app_instances(1), 2);
        long(&mut s, app.clone());
        assert_eq!(tap(&mut s, Id::Dock(DockId::CloseActive)), vec![Effect::Window(WindowCommand::Close(vec![12, 13]))]);
        long(&mut s, app.clone());
        assert_eq!(tap(&mut s, Id::Dock(DockId::CloseAll)), vec![Effect::Window(WindowCommand::Close(vec![10, 12, 13]))]);
        // All its windows on the bottom screen: active is the tab in view there.
        let down = |id, visible| Window { down: true, visible, ..of(id, &t2, false) };
        s.apply(Event::Windows(vec![win(20, "firefox", "1", true), Window { down: true, visible: false, ..of(10, &t1, false) }, down(12, true)]));
        long(&mut s, app.clone());
        assert_eq!(tap(&mut s, Id::Dock(DockId::CloseActive)), vec![Effect::Window(WindowCommand::Close(vec![12]))]);
        // Instances without a window (left running in the background): their scopes ended.
        s.apply(Event::Windows(vec![win(20, "firefox", "1", true)]));
        long(&mut s, app.clone());
        assert_eq!(tap(&mut s, Id::Dock(DockId::CloseAll)), vec![Effect::EndScopes(vec![t1.clone(), t2.clone()])]);
        // Back, or another tab: closed without a choice.
        long(&mut s, app);
        tap(&mut s, Id::Dock(DockId::Back));
        assert_eq!(s.dock_menu(), None);
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
    fn a_game_tile_goes_to_its_game_and_its_menu_quits_it() {
        let mut s = state();
        let tile = Id::Dock(DockId::GameTile(0));
        assert_eq!(tap(&mut s, tile.clone()), vec![], "no game");
        s.apply(Event::Tick { unix_secs: 77 });
        let game = launch(&mut s, "zelda-lttp");
        assert_eq!(game, Instance::Game { secs: 77, n: 0 });
        s.apply(Event::Units(vec![game.clone()]));
        assert_eq!(s.apply(Event::Windows(vec![win(5, "retroarch", &game.name(), false)])).1, vec![focus(&game)], "shown when its window appears");
        assert_eq!(tap(&mut s, tile.clone()), vec![focus(&game)]);
        assert_eq!(long(&mut s, tile), vec![], "its menu");
        assert_eq!(tap(&mut s, Id::Dock(DockId::Quit)), vec![Effect::QuitGame { windows: vec![5], units: vec![game] }]);
    }
}
