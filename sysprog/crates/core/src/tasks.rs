//! The launcher as a task switcher. A tap on an app starts it when it isn't running, brings its most
//! recent window to the front when it is, and goes on to its next window when it already is in front. A
//! long press closes the window it is on (politely, as its own close button would); on the game tile it
//! quits the game. A "+" on running apps that allow several starts another instance. Windows are an
//! app's when its scope says we started them, or by app id.

use crate::apps::Builtin;
use crate::windows::Window;
use crate::{Effect, State};

impl State {
    pub(crate) fn set_windows(&mut self, windows: Vec<Window>) {
        self.recent.retain(|id| windows.iter().any(|w| w.id == *id));
        if let Some(front) = windows.iter().find(|w| w.focused) {
            self.recent.retain(|id| *id != front.id);
            self.recent.insert(0, front.id);
        }
        self.windows = windows;
    }

    /// Whether `w` is launcher app `i`'s.
    pub fn app_owns(&self, i: usize, w: &Window) -> bool {
        self.apps.get(i).is_some_and(|app| w.from_app(app.key()) || app.owns(&w.app_id))
    }

    /// Whether `w` is the running game's.
    pub fn game_owns(&self, w: &Window) -> bool {
        let Some(id) = &self.running else {
            return false;
        };
        let steam = self.library.games.iter().find(|g| &g.id == id).and_then(|g| match g.launch {
            crate::Launch::Steam { appid } => Some(appid),
            _ => None,
        });
        w.of_game(steam)
    }

    /// Which of an owner's windows is shown, of how many: (position from 1, count), in a stable order
    /// (by id); the one in front, else the most recent. No allocation (for drawing).
    pub fn window_position(&self, owns: impl Fn(&Window) -> bool) -> (usize, usize) {
        let count = self.windows.iter().filter(|w| owns(w)).count();
        let current = self.windows.iter().filter(|w| owns(w)).find(|w| w.focused).map(|w| w.id).or_else(|| {
            self.recent.iter().copied().find(|id| self.windows.iter().any(|w| w.id == *id && owns(w)))
        });
        let position = current.map_or(0, |c| self.windows.iter().filter(|w| owns(w) && w.id <= c).count());
        (position, count)
    }

    /// The owner's window a tile is on: the one in front, else the most recent.
    fn current_window(&self, owns: impl Fn(&Window) -> bool) -> Option<i64> {
        self.windows.iter().find(|w| w.focused && owns(w)).map(|w| w.id).or_else(|| {
            self.recent.iter().copied().find(|id| self.windows.iter().any(|w| w.id == *id && owns(w)))
        })
    }

    /// The window to go to among an owner's: the next after the one in front (wrapping), else the most
    /// recent, else any.
    fn next_window(&self, owns: impl Fn(&Window) -> bool) -> Option<i64> {
        let mut ids: Vec<i64> = self.windows.iter().filter(|w| owns(w)).map(|w| w.id).collect();
        ids.sort_unstable();
        if let Some(front) = self.windows.iter().find(|w| w.focused && owns(w)) {
            let at = ids.iter().position(|id| *id == front.id).unwrap_or(0);
            return ids.get((at + 1) % ids.len().max(1)).copied();
        }
        self.recent.iter().copied().find(|id| ids.contains(id)).or_else(|| ids.first().copied())
    }

    pub(crate) fn tap_app(&mut self, i: usize, long: bool) -> Vec<Effect> {
        let Some(app) = self.apps.get(i) else {
            return vec![];
        };
        if long {
            return self.current_window(|w| self.app_owns(i, w)).map(Effect::CloseWindow).into_iter().collect();
        }
        if let Some(id) = self.next_window(|w| self.app_owns(i, w)) {
            return vec![Effect::FocusWindow(id)];
        }
        let start = Effect::RunApp { app: i as u16, key: app.key().to_owned(), command: app.command.clone() };
        match app.builtin {
            Some(Builtin::Shelf) => vec![Effect::ShowShelf],
            // Started and no window yet: wait for it.
            None if self.apps_running.get(i).is_some_and(|n| *n > 0) => vec![],
            None => self.started(i, start),
        }
    }

    /// Whether an app shows a "+": it allows several instances and runs.
    pub fn offers_new(&self, i: usize) -> bool {
        self.apps.get(i).is_some_and(|a| a.multiple && !a.command.is_empty())
            && (self.apps_running.get(i).is_some_and(|n| *n > 0) || self.windows.iter().any(|w| self.app_owns(i, w)))
    }

    pub(crate) fn new_instance(&mut self, i: usize) -> Vec<Effect> {
        let Some(app) = self.apps.get(i).filter(|_| self.offers_new(i)) else {
            return vec![];
        };
        let start = Effect::RunApp { app: i as u16, key: app.key().to_owned(), command: app.command.clone() };
        self.started(i, start)
    }

    fn started(&mut self, i: usize, start: Effect) -> Vec<Effect> {
        if let Some(n) = self.apps_running.get_mut(i) {
            *n += 1;
        }
        vec![start]
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
        self.next_window(|w| self.game_owns(w)).map(Effect::FocusWindow).into_iter().collect()
    }
}

#[cfg(test)]
mod tests {
    use crate::tests::state;
    use crate::windows::Window;
    use crate::*;

    fn win(id: i64, app_id: &str, unit: &str, focused: bool) -> Window {
        Window { id, app_id: app_id.into(), title: String::new(), focused, unit: (!unit.is_empty()).then(|| unit.into()) }
    }

    #[test]
    fn a_tap_starts_then_goes_to_then_cycles() {
        let mut s = state();
        let foot = 2;
        let (_, fx) = s.apply(Event::Activate(Id::App(foot)));
        assert!(matches!(fx.as_slice(), [Effect::RunApp { key, .. }] if key == "terminal"));
        assert_eq!(s.apply(Event::Activate(Id::App(foot))).1, vec![], "starting: no second one");
        // Two terminals, the second in front, the first by its scope, the second by its app id.
        s.apply(Event::Windows(vec![win(10, "foot", "sysprog-app-terminal-1-0.scope", false), win(12, "foot", "", true)]));
        assert_eq!(s.window_position(|w| s.app_owns(2, w)), (2, 2), "viewing 2 of 2");
        assert_eq!(s.apply(Event::Activate(Id::App(foot))).1, vec![Effect::FocusWindow(10)], "in front: the next, wrapping");
        // Something else in front: back to the terminal last in front.
        s.apply(Event::Windows(vec![win(10, "foot", "", false), win(12, "foot", "", false), win(20, "firefox", "", true)]));
        assert_eq!(s.apply(Event::Activate(Id::App(foot))).1, vec![Effect::FocusWindow(12)]);
        assert_eq!(s.window_position(|w| s.app_owns(2, w)), (2, 2));
        // A long press closes the one it is on; "+" starts another (the terminal allows several).
        assert_eq!(s.apply(Event::LongPress(Id::App(foot))).1, vec![Effect::CloseWindow(12)]);
        assert!(s.offers_new(2) && !s.offers_new(1), "Steam allows one");
        assert!(matches!(s.apply(Event::Activate(Id::NewInstance(foot))).1.as_slice(), [Effect::RunApp { .. }]));
        assert_eq!(s.apply(Event::Activate(Id::NewInstance(1))).1, vec![]);
        // Shelf: its window, by app id.
        s.apply(Event::Windows(vec![win(1, apps::SHELF_APP_ID, "", false)]));
        assert_eq!(s.apply(Event::Activate(Id::App(0))).1, vec![Effect::FocusWindow(1)]);
    }

    #[test]
    fn the_game_tile_goes_to_the_game_and_quits_it() {
        let mut s = state();
        assert_eq!(s.apply(Event::Activate(Id::GameTile)).1, vec![], "no game: a placeholder");
        s.apply(Event::Started("zelda-lttp".into()));
        s.apply(Event::Windows(vec![win(5, "retroarch", "sysprog-game-77-0.scope", false), win(6, "foot", "", true)]));
        assert_eq!(s.apply(Event::Activate(Id::GameTile)).1, vec![Effect::FocusWindow(5)]);
        assert_eq!(
            s.apply(Event::LongPress(Id::GameTile)).1,
            vec![Effect::QuitGame { windows: vec![5], units: vec!["sysprog-game-77-0.scope".into()] }]
        );
    }
}
