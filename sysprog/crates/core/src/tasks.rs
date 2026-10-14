//! The launcher as a task switcher. An app's instances are the scopes we started it in, each with its
//! own workspace that all its windows go to (sway arranges several within it). A tap on an app starts it
//! when it isn't running, shows its most recent instance when it is, and goes on to its next instance
//! when one of its own is in front. A long press closes the instance it is on (its windows, politely, as
//! their close buttons would); on the game tile it quits the game. A "+" on running apps that allow
//! several starts another instance.

use crate::apps::{Builtin, SHELF_APP_ID};
use crate::windows::{GAME_WORKSPACE, Window, order};
use crate::{Effect, State};

impl State {
    pub(crate) fn set_windows(&mut self, windows: Vec<Window>) {
        self.windows = windows;
        let present = |name: &String| self.windows.iter().any(|w| w.workspace == *name);
        self.recent.retain(present);
        if let Some(front) = self.windows.iter().find(|w| w.focused).map(|w| w.workspace.clone()) {
            self.recent.retain(|n| *n != front);
            self.recent.insert(0, front);
        }
    }

    /// The instance (workspace) of launcher app `i` that `w` belongs to, if it is the app's: the scope we
    /// started it in; for shelf, its own window's workspace.
    pub fn instance_of<'w>(&self, i: usize, w: &'w Window) -> Option<&'w str> {
        let app = self.apps.get(i)?;
        match app.builtin {
            Some(Builtin::Shelf) => (w.app_id == SHELF_APP_ID).then_some(w.workspace.as_str()),
            None => w.instance().filter(|_| w.from_app(app.key())),
        }
    }

    /// The instance a tile is on: the one in front, else the most recent.
    fn current_instance(&self, i: usize) -> Option<&str> {
        let front = self.windows.iter().filter(|w| w.focused).find_map(|w| self.instance_of(i, w));
        front.or_else(|| {
            self.recent.iter().map(String::as_str).find(|name| self.windows.iter().any(|w| self.instance_of(i, w) == Some(*name)))
        })
    }

    /// Which of app `i`'s instances the tile is on, of how many: (position from 1, count), in the order
    /// they were started. No allocation (for drawing).
    pub fn instance_position(&self, i: usize) -> (usize, usize) {
        let mut count = 0;
        let current = self.current_instance(i).map(order);
        let mut position = 0;
        for (k, w) in self.windows.iter().enumerate() {
            let Some(name) = self.instance_of(i, w) else {
                continue;
            };
            // Each instance once: at its first window.
            let seen = self.windows.iter().take(k).any(|earlier| self.instance_of(i, earlier) == Some(name));
            if !seen {
                count += 1;
                if current.is_some_and(|c| order(name) <= c) {
                    position += 1;
                }
            }
        }
        (position, count)
    }

    /// App `i`'s instances in the order they were started.
    fn instances(&self, i: usize) -> Vec<&str> {
        let mut names: Vec<&str> = self.windows.iter().filter_map(|w| self.instance_of(i, w)).collect();
        names.sort_by_key(|n| order(n));
        names.dedup();
        names
    }

    pub(crate) fn tap_app(&mut self, i: usize, long: bool) -> Vec<Effect> {
        let Some(app) = self.apps.get(i) else {
            return vec![];
        };
        if long {
            let Some(current) = self.current_instance(i) else {
                return vec![];
            };
            let windows = self.windows.iter().filter(|w| self.instance_of(i, w) == Some(current)).map(|w| w.id).collect();
            return vec![Effect::CloseWindows(windows)];
        }
        let instances = self.instances(i);
        if !instances.is_empty() {
            let front = self.windows.iter().filter(|w| w.focused).find_map(|w| self.instance_of(i, w));
            let target = match front {
                // One of its own is in front: the next one, wrapping.
                Some(front) => {
                    let at = instances.iter().position(|n| *n == front).unwrap_or(0);
                    instances.get((at + 1) % instances.len()).copied()
                }
                None => self.current_instance(i),
            };
            return target.map(|t| Effect::FocusWorkspace(t.to_owned())).into_iter().collect();
        }
        match app.builtin {
            Some(Builtin::Shelf) => vec![Effect::ShowShelf],
            // Started and no window yet: wait for it.
            None if self.apps_running.get(i).is_some_and(|n| *n > 0) => vec![],
            None => self.start_app(i),
        }
    }

    /// Whether an app shows a "+": it allows several instances and runs.
    pub fn offers_new(&self, i: usize) -> bool {
        self.apps.get(i).is_some_and(|a| a.multiple && !a.command.is_empty())
            && (self.apps_running.get(i).is_some_and(|n| *n > 0) || self.windows.iter().any(|w| self.instance_of(i, w).is_some()))
    }

    pub(crate) fn new_instance(&mut self, i: usize) -> Vec<Effect> {
        if !self.offers_new(i) {
            return vec![];
        }
        self.start_app(i)
    }

    fn start_app(&mut self, i: usize) -> Vec<Effect> {
        let Some(app) = self.apps.get(i) else {
            return vec![];
        };
        let start = Effect::RunApp { app: i as u16, key: app.key().to_owned(), command: app.command.clone() };
        if let Some(n) = self.apps_running.get_mut(i) {
            *n += 1;
        }
        vec![start]
    }

    /// Whether `w` is the running game's.
    pub fn game_owns(&self, w: &Window) -> bool {
        self.running.is_some() && w.of_game()
    }

    pub(crate) fn tap_game(&mut self, long: bool) -> Vec<Effect> {
        if self.running.is_none() {
            return vec![];
        }
        if long {
            let windows = self.windows.iter().filter(|w| self.game_owns(w)).map(|w| w.id).collect();
            let mut units: Vec<String> = self
                .windows
                .iter()
                .filter(|w| self.game_owns(w))
                .filter_map(|w| w.unit.clone())
                .filter(|u| u.starts_with("sysprog-game-"))
                .collect();
            units.dedup();
            return vec![Effect::QuitGame { windows, units }];
        }
        if self.windows.iter().any(|w| self.game_owns(w)) {
            return vec![Effect::FocusWorkspace(GAME_WORKSPACE.into())];
        }
        vec![]
    }
}

#[cfg(test)]
mod tests {
    use crate::tests::state;
    use crate::windows::{GAME_WORKSPACE, Window};
    use crate::*;

    fn win(id: i64, app_id: &str, unit: &str, workspace: &str, focused: bool) -> Window {
        Window {
            id,
            app_id: app_id.into(),
            title: String::new(),
            focused,
            unit: (!unit.is_empty()).then(|| unit.into()),
            workspace: workspace.into(),
        }
    }

    const T1: &str = "sysprog-app-terminal-1-1";
    const T2: &str = "sysprog-app-terminal-1-2";

    #[test]
    fn a_tap_starts_then_goes_to_then_cycles_instances() {
        let mut s = state();
        let term: u16 = 2;
        let (_, fx) = s.apply(Event::Activate(Id::App(term)));
        assert!(matches!(fx.as_slice(), [Effect::RunApp { key, .. }] if key == "terminal"));
        assert_eq!(s.apply(Event::Activate(Id::App(term))).1, vec![], "starting: no second one");
        // Two instances; the second has two windows (one it started: mpv), and is in front.
        s.apply(Event::Windows(vec![
            win(10, "foot", &format!("{T1}.scope"), T1, false),
            win(12, "foot", &format!("{T2}.scope"), T2, false),
            win(13, "mpv", &format!("{T2}.scope"), T2, true),
        ]));
        assert_eq!(s.instance_position(usize::from(term)), (2, 2), "on instance 2 of 2: windows don't count, instances do");
        assert_eq!(s.apply(Event::Activate(Id::App(term))).1, vec![Effect::FocusWorkspace(T1.into())], "the next, wrapping");
        // Something else in front: back to the instance last in front.
        s.apply(Event::Windows(vec![
            win(10, "foot", &format!("{T1}.scope"), T1, false),
            win(12, "foot", &format!("{T2}.scope"), T2, false),
            win(13, "mpv", &format!("{T2}.scope"), T2, false),
            win(20, "firefox", "app-firefox.scope", "1", true),
        ]));
        assert_eq!(s.apply(Event::Activate(Id::App(term))).1, vec![Effect::FocusWorkspace(T2.into())]);
        // A long press closes that instance's windows; "+" starts another (the terminal allows several).
        assert_eq!(s.apply(Event::LongPress(Id::App(term))).1, vec![Effect::CloseWindows(vec![12, 13])]);
        assert!(s.offers_new(2) && !s.offers_new(1), "Steam allows one");
        assert!(matches!(s.apply(Event::Activate(Id::NewInstance(term))).1.as_slice(), [Effect::RunApp { .. }]));
        assert_eq!(s.apply(Event::Activate(Id::NewInstance(1))).1, vec![]);
        // Shelf: its window's workspace.
        s.apply(Event::Windows(vec![win(1, apps::SHELF_APP_ID, "sysprog.service", "1", false)]));
        assert_eq!(s.apply(Event::Activate(Id::App(0))).1, vec![Effect::FocusWorkspace("1".into())]);
    }

    #[test]
    fn the_game_tile_goes_to_the_game_and_quits_it() {
        let mut s = state();
        assert_eq!(s.apply(Event::Activate(Id::GameTile)).1, vec![], "no game: a placeholder");
        s.apply(Event::Started("zelda-lttp".into()));
        s.apply(Event::Windows(vec![win(5, "retroarch", "sysprog-game-77-0.scope", GAME_WORKSPACE, false)]));
        assert_eq!(s.apply(Event::Activate(Id::GameTile)).1, vec![Effect::FocusWorkspace(GAME_WORKSPACE.into())]);
        assert_eq!(
            s.apply(Event::LongPress(Id::GameTile)).1,
            vec![Effect::QuitGame { windows: vec![5], units: vec!["sysprog-game-77-0.scope".into()] }]
        );
    }
}
