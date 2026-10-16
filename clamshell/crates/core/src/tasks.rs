//! The dock as a task switcher. An app's instances are the systemd scopes we started it in
//! ([`Instance::App`], as systemd lists them), each with a sway workspace of the same name where its
//! windows are (sway places a launch's first window there, the sway side moves later ones by their
//! scope). A tap on an app starts it when it isn't running, shows its most recent instance when it is,
//! and goes on to its next instance when one of its own is in front. A long press closes the instance it
//! is on (its windows, politely, as their close buttons would); on the game tile it quits the game. A "+"
//! on running apps that allow several starts another instance. A new instance (or game) is shown when its
//! first window appears, not before (its empty workspace never is), also if the user has gone elsewhere
//! meanwhile; a newer start takes its place.

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

/// An instance found by a window's workspace (which may not be listed by systemd yet) or in systemd's
/// list.
#[derive(Debug, Clone, Copy)]
enum Found<'a> {
    Workspace(&'a str),
    Unit(&'a Instance),
}

impl Found<'_> {
    fn order(self) -> (u64, u64) {
        match self {
            Found::Workspace(name) => Instance::parse_order(name),
            Found::Unit(instance) => instance.order(),
        }
    }

    fn is(self, workspace: &str) -> bool {
        match self {
            Found::Workspace(name) => name == workspace,
            Found::Unit(instance) => instance.is(workspace),
        }
    }

    fn name(self) -> String {
        match self {
            Found::Workspace(name) => name.to_owned(),
            Found::Unit(instance) => instance.name(),
        }
    }
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

    /// Dock app `i`'s instance in front, by the focused window's workspace.
    fn front_instance(&self, i: usize) -> Option<&str> {
        self.session.windows.iter().filter(|w| w.focused).map(|w| w.workspace.as_str()).find(|ws| self.is_instance(i, ws))
    }

    /// The instance a tile is on: the one in front, else the most recent, else (no history yet) the
    /// first.
    fn current_instance(&self, i: usize) -> Option<Found<'_>> {
        let session = &self.session;
        let recent = || session.recent.iter().map(String::as_str).find(|ws| session.units.iter().any(|u| u.is(ws)) && self.is_instance(i, ws));
        self.front_instance(i).or_else(recent).map(Found::Workspace).or_else(|| self.instances(i).min_by_key(|u| u.order()).map(Found::Unit))
    }

    /// Which of app `i`'s instances the tile is on, of how many: (position from 1, count), in the order
    /// they were started. No allocation (for drawing).
    fn instance_position(&self, i: usize) -> (usize, usize) {
        let current = self.current_instance(i).map(Found::order);
        let position = current.map_or(0, |c| self.instances(i).filter(|u| u.order() <= c).count());
        (position, self.instances(i).count())
    }

    /// The workspace a window belongs to: the one it is on, or (moved down) its scope's, which has the
    /// same name. Borrowed (for drawing).
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

    /// A program's windows on the top screen (`down`: moved to the bottom one), with their order: by
    /// instance, then as the compositor lists them. No allocation (for drawing).
    fn program_windows(&self, p: Program, down: bool) -> impl Iterator<Item = ((u64, u64, usize), &Window)> {
        self.session.windows.iter().enumerate().filter(move |(_, w)| w.down == down && self.program_has(p, w)).map(|(at, w)| {
            let (secs, n) = Instance::parse_order(Self::place(w));
            ((secs, n, at), w)
        })
    }

    /// Which of a dock program's windows on the top screen its tile is on, of how many (position from 1,
    /// count): the one in front if it is the program's, else the first of the instance the tile is on.
    /// An app with no window listed yet counts its instances. No allocation (for drawing).
    pub fn dock_position(&self, id: DockId) -> (usize, usize) {
        let p = match id {
            DockId::App(i) => Program::App(usize::from(i)),
            DockId::GameTile(i) => Program::Game(usize::from(i)),
            DockId::StartGame | DockId::NewInstance(_) => return (0, 0),
        };
        let count = self.program_windows(p, false).count();
        if count == 0 {
            return match p {
                Program::App(i) => self.instance_position(i),
                Program::Game(_) => (0, 0),
            };
        }
        let focused = self.program_windows(p, false).find(|(_, w)| w.focused).map(|(k, _)| k);
        let current = focused.or_else(|| self.program_windows(p, false).map(|(k, _)| k).min());
        (current.map_or(0, |c| self.program_windows(p, false).filter(|(k, _)| *k <= c).count()), count)
    }

    /// A tap on a dock program: with its window in front, its next window on the top screen; with none
    /// on the top screen and some moved down, the bottom screen shows its next one. None: neither (the
    /// tile's usual tap).
    fn tap_program(&mut self, p: Program) -> Option<Vec<Effect>> {
        let mut top: Vec<((u64, u64, usize), i64, bool)> = self.program_windows(p, false).map(|(k, w)| (k, w.id, w.focused)).collect();
        top.sort();
        if let Some(at) = top.iter().position(|t| t.2) {
            let next = top.get((at + 1) % top.len()).map(|t| t.1).filter(|_| top.len() > 1);
            return Some(next.map(|id| vec![Effect::Window(WindowCommand::FocusWindow(id))]).unwrap_or_default());
        }
        if !top.is_empty() {
            return None;
        }
        let mut down: Vec<((u64, u64, usize), i64)> = self.program_windows(p, true).map(|(k, w)| (k, w.id)).collect();
        down.sort();
        let shown = self.bottom_window();
        let at = shown.and_then(|id| down.iter().position(|d| d.1 == id)).map_or(0, |i| i + 1);
        let next = down.get(at % down.len().max(1)).map(|d| d.1)?;
        self.bottom.screen.shows = Some(next);
        Some(vec![Effect::Window(WindowCommand::ShowDown(next))])
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
        if self.apps.list.get(i).is_none() {
            return vec![];
        }
        if long {
            let Some(current) = self.current_instance(i) else {
                return vec![];
            };
            let windows = self.session.windows.iter().filter(|w| current.is(&w.workspace)).map(|w| w.id).collect();
            return vec![Effect::Window(WindowCommand::Close(windows))];
        }
        let mut instances: Vec<&Instance> = self.instances(i).collect();
        instances.sort_by_key(|u| u.order());
        if instances.is_empty() {
            // Started and not listed yet: wait for it.
            if self.app_running(i) {
                return vec![];
            }
            return self.start_app(i);
        }
        let target = match self.front_instance(i) {
            // One of its own is in front: the next one, wrapping.
            Some(front) => {
                let at = instances.iter().position(|u| u.is(front)).unwrap_or(0);
                instances.get((at + 1) % instances.len()).map(|u| Found::Unit(u))
            }
            None => self.current_instance(i),
        };
        match target {
            Some(t) => vec![Effect::Window(WindowCommand::Focus(t.name()))],
            None => vec![],
        }
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
    fn a_tap_starts_then_goes_to_then_cycles_instances() {
        let mut s = state();
        s.apply(Event::Tick { unix_secs: 100 });
        let app = Id::Dock(DockId::App(TERMINAL));
        let (t1, t2) = (terminal(0), terminal(1));
        let (w1, w2) = (t1.name(), t2.name());
        let shelf = || win(1, apps::SHELF_APP_ID, &Instance::Shelf.name(), true);
        assert_eq!(tap(&mut s, app.clone()), run(t1.clone()));
        assert!(s.app_running(1), "starting counts as running");
        assert_eq!(tap(&mut s, app.clone()), vec![], "starting: no second one");
        // systemd lists it; its window appears on its workspace: shown then, not before.
        s.apply(Event::Units(vec![t1.clone()]));
        assert_eq!(s.apply(Event::Windows(vec![shelf()])).1, vec![]);
        assert_eq!(s.apply(Event::Windows(vec![shelf(), win(10, "foot", &w1, false)])).1, vec![focus(&t1)]);
        // "+": a second; with two, a tap in front goes to the next.
        assert!(s.offers_new(1) && !s.offers_new(0), "Steam allows one");
        assert_eq!(tap(&mut s, Id::Dock(DockId::NewInstance(TERMINAL))), run(t2.clone()));
        s.apply(Event::Units(vec![t1.clone(), t2.clone()]));
        s.apply(Event::Windows(vec![win(10, "foot", &w1, false), win(12, "foot", &w2, false), win(13, "mpv", &w2, true)]));
        assert_eq!(s.dock_position(DockId::App(TERMINAL)), (3, 3), "on window 3 of 3: the windows count, by instance");
        assert_eq!(tap(&mut s, app.clone()), vec![Effect::Window(WindowCommand::FocusWindow(10))], "the next window, wrapping");
        // Something else in front: back to the instance last in front.
        s.apply(Event::Windows(vec![win(10, "foot", &w1, false), win(12, "foot", &w2, false), win(13, "mpv", &w2, false), win(20, "firefox", "1", true)]));
        assert_eq!(tap(&mut s, app.clone()), vec![focus(&t2)]);
        // A long press closes that instance's windows.
        assert_eq!(long(&mut s, app), vec![Effect::Window(WindowCommand::Close(vec![12, 13]))]);
    }

    #[test]
    fn a_program_with_its_windows_moved_down_shows_them_on_the_bottom_screen() {
        let mut s = state();
        s.apply(Event::Tick { unix_secs: 100 });
        let t1 = terminal(0);
        let w1 = t1.name();
        s.apply(Event::Units(vec![t1.clone()]));
        let down = |id| Window { down: true, unit: Some(format!("{w1}.scope")), ..win(id, "foot", &Instance::Bottom.name(), false) };
        // Both its windows down, shelf in front: a tap shows one on the bottom screen, the next tap the
        // other.
        s.apply(Event::Windows(vec![win(1, apps::SHELF_APP_ID, &Instance::Shelf.name(), true), down(10), down(11)]));
        let app = Id::Dock(DockId::App(TERMINAL));
        assert_eq!(tap(&mut s, app.clone()), vec![Effect::Window(WindowCommand::ShowDown(10))]);
        assert_eq!(s.bottom_window(), Some(10));
        assert_eq!(tap(&mut s, app), vec![Effect::Window(WindowCommand::ShowDown(11))]);
        assert_eq!(s.dock_position(DockId::App(TERMINAL)), (1, 1), "none on top: counted by instance");
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
