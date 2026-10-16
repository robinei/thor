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
    pub fn instance_position(&self, i: usize) -> (usize, usize) {
        let current = self.current_instance(i).map(Found::order);
        let position = current.map_or(0, |c| self.instances(i).filter(|u| u.order() <= c).count());
        (position, self.instances(i).count())
    }

    /// A tap or long press on a dock tile.
    pub(crate) fn on_dock(&mut self, id: DockId, long: bool) -> Vec<Effect> {
        match id {
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
        assert_eq!(s.instance_position(1), (2, 2), "on instance 2 of 2: windows don't count, instances do");
        assert_eq!(tap(&mut s, app.clone()), vec![focus(&t1)], "the next, wrapping");
        // Something else in front: back to the instance last in front.
        s.apply(Event::Windows(vec![win(10, "foot", &w1, false), win(12, "foot", &w2, false), win(13, "mpv", &w2, false), win(20, "firefox", "1", true)]));
        assert_eq!(tap(&mut s, app.clone()), vec![focus(&t2)]);
        // A long press closes that instance's windows.
        assert_eq!(long(&mut s, app), vec![Effect::Window(WindowCommand::Close(vec![12, 13]))]);
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
