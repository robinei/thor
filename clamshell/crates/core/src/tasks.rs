//! The dock as a task switcher. An app's instances are the systemd scopes we started it in
//! (`clamshell-app-<key>-<secs>-<n>`, as systemd lists them), each with a sway workspace of the same name
//! where its windows are (sway places a launch's first window there, the sway side moves later ones by
//! their scope). A tap on an app starts it when it isn't running, shows its most recent instance when it
//! is, and goes on to its next instance when one of its own is in front. A long press closes the
//! instance it is on (its windows, politely, as their close buttons would); on the game tile it quits the
//! game. A "+" on running apps that allow several starts another instance. A new instance (or game) is
//! shown when its first window appears, not before (its empty workspace never is), also if the user has
//! gone elsewhere meanwhile; a newer start takes its place.

use crate::windows::{Window, order};
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

    /// The instance or game just started, once its first window is there: show it.
    pub(crate) fn show_pending(&mut self) -> Vec<Effect> {
        let arrived = self.pending_show.as_ref().is_some_and(|ws| self.windows.iter().any(|w| w.workspace == *ws));
        match self.pending_show.take() {
            Some(ws) if arrived => vec![Effect::FocusWorkspace(ws)],
            other => {
                self.pending_show = other;
                vec![]
            }
        }
    }

    /// The name prefix of dock app `i`'s instances.
    fn prefix(&self, i: usize) -> Option<(&'static str, &str)> {
        Some(("clamshell-app-", self.apps.get(i)?.key()))
    }

    /// Whether `name` is one of launcher app `i`'s instances (`clamshell-app-<key>-…`).
    fn is_instance(&self, i: usize, name: &str) -> bool {
        self.prefix(i).is_some_and(|(p, key)| {
            name.strip_prefix(p).and_then(|rest| rest.strip_prefix(key)).is_some_and(|rest| rest.starts_with('-'))
        })
    }

    /// The instance (workspace) of dock app `i` that window `w` is in.
    pub fn instance_of<'w>(&self, i: usize, w: &'w Window) -> Option<&'w str> {
        Some(w.workspace.as_str()).filter(|ws| self.is_instance(i, ws))
    }

    /// Whether dock app `i` runs (or is starting).
    pub fn app_running(&self, i: usize) -> bool {
        self.units.iter().any(|u| self.is_instance(i, u)) || self.starting.iter().any(|(n, _)| self.is_instance(i, n))
    }

    /// The instance a tile is on: the one in front, else the most recent, else (no history yet) the first.
    fn current_instance(&self, i: usize) -> Option<&str> {
        let front = self.windows.iter().filter(|w| w.focused).find_map(|w| self.instance_of(i, w));
        front
            .or_else(|| self.recent.iter().map(String::as_str).find(|ws| self.units.iter().any(|u| u == ws) && self.is_instance(i, ws)))
            .or_else(|| self.units.iter().filter(|u| self.is_instance(i, u)).min_by_key(|u| order(u)).map(String::as_str))
    }

    /// Which of app `i`'s instances the tile is on, of how many: (position from 1, count), in the order
    /// they were started. No allocation (for drawing).
    pub fn instance_position(&self, i: usize) -> (usize, usize) {
        let current = self.current_instance(i).map(order);
        let mine = || self.units.iter().filter(|u| self.is_instance(i, u));
        let position = current.map_or(0, |c| mine().filter(|u| order(u) <= c).count());
        (position, mine().count())
    }

    pub(crate) fn tap_app(&mut self, i: usize, long: bool) -> Vec<Effect> {
        if self.apps.get(i).is_none() {
            return vec![];
        }
        if long {
            let Some(current) = self.current_instance(i) else {
                return vec![];
            };
            let windows = self.windows.iter().filter(|w| w.workspace == current).map(|w| w.id).collect();
            return vec![Effect::CloseWindows(windows)];
        }
        let mut instances: Vec<&str> = self.units.iter().filter(|u| self.is_instance(i, u)).map(String::as_str).collect();
        instances.sort_by_key(|n| order(n));
        if instances.is_empty() {
            // Started and not listed yet: wait for it.
            if self.starting.iter().any(|(n, _)| self.is_instance(i, n)) {
                return vec![];
            }
            return self.start_app(i);
        }
        let front = self.windows.iter().filter(|w| w.focused).find_map(|w| self.instance_of(i, w));
        let target = match front {
            // One of its own is in front: the next one, wrapping.
            Some(front) => {
                let at = instances.iter().position(|n| *n == front).unwrap_or(0);
                instances.get((at + 1) % instances.len()).copied()
            }
            None => self.current_instance(i),
        };
        match target.map(str::to_owned) {
            Some(t) => vec![Effect::FocusWorkspace(t)],
            None => vec![],
        }
    }

    /// Whether an app shows a "+": it allows several instances and runs.
    pub fn offers_new(&self, i: usize) -> bool {
        self.apps.get(i).is_some_and(|a| a.multiple && !a.command.is_empty()) && self.app_running(i)
    }

    pub(crate) fn new_instance(&mut self, i: usize) -> Vec<Effect> {
        if !self.offers_new(i) {
            return vec![];
        }
        self.start_app(i)
    }

    /// A new instance: named, remembered as starting, and shown once its first window appears.
    fn start_app(&mut self, i: usize) -> Vec<Effect> {
        let Some(app) = self.apps.get(i) else {
            return vec![];
        };
        let name = format!("clamshell-app-{}-{}-{}", app.key(), self.unix_secs, self.started);
        let command = app.command.clone();
        self.started += 1;
        self.starting.push((name.clone(), self.unix_secs));
        self.pending_show = Some(name.clone());
        vec![Effect::RunApp { name, command }]
    }

    /// A running game's tile: a tap shows it, a long press quits it.
    pub(crate) fn tap_game(&mut self, i: usize, long: bool) -> Vec<Effect> {
        let Some(g) = self.games.get(i) else {
            return vec![];
        };
        if long {
            let id = g.id.clone();
            return self.quit_game(&id);
        }
        let workspace = g.workspace.clone();
        vec![Effect::FocusWorkspace(workspace)]
    }
}

#[cfg(test)]
mod tests {
    use crate::tests::state;
    use crate::apps::SHELF_APP_ID;
    use crate::windows::{SHELF_WORKSPACE, Window};
    use crate::*;

    fn win(id: i64, app_id: &str, workspace: &str, focused: bool) -> Window {
        Window { id, app_id: app_id.into(), title: String::new(), focused, unit: None, workspace: workspace.into() }
    }

    const T1: &str = "clamshell-app-terminal-100-0";
    const T2: &str = "clamshell-app-terminal-100-1";

    #[test]
    fn a_tap_starts_then_goes_to_then_cycles_instances() {
        let mut s = state();
        s.apply(Event::Tick { unix_secs: 100 });
        let term: u16 = 1;
        let (_, fx) = s.apply(Event::Activate(Id::App(term)));
        assert_eq!(fx, vec![Effect::RunApp { name: T1.into(), command: vec!["foot".into()] }]);
        assert!(s.app_running(1), "starting counts as running");
        assert_eq!(s.apply(Event::Activate(Id::App(term))).1, vec![], "starting: no second one");
        // systemd lists it; its window appears on its workspace: shown then, not before.
        s.apply(Event::Units(vec![T1.into()]));
        assert_eq!(s.apply(Event::Windows(vec![win(1, SHELF_APP_ID, SHELF_WORKSPACE, true)])).1, vec![]);
        assert_eq!(
            s.apply(Event::Windows(vec![win(1, SHELF_APP_ID, SHELF_WORKSPACE, true), win(10, "foot", T1, false)])).1,
            vec![Effect::FocusWorkspace(T1.into())]
        );
        // "+": a second; with two, a tap in front goes to the next.
        assert!(s.offers_new(1) && !s.offers_new(0), "Steam allows one");
        assert_eq!(s.apply(Event::Activate(Id::NewInstance(term))).1, vec![Effect::RunApp { name: T2.into(), command: vec!["foot".into()] }]);
        s.apply(Event::Units(vec![T1.into(), T2.into()]));
        s.apply(Event::Windows(vec![win(10, "foot", T1, false), win(12, "foot", T2, false), win(13, "mpv", T2, true)]));
        assert_eq!(s.instance_position(1), (2, 2), "on instance 2 of 2: windows don't count, instances do");
        assert_eq!(s.apply(Event::Activate(Id::App(term))).1, vec![Effect::FocusWorkspace(T1.into())], "the next, wrapping");
        // Something else in front: back to the instance last in front.
        s.apply(Event::Windows(vec![win(10, "foot", T1, false), win(12, "foot", T2, false), win(13, "mpv", T2, false), win(20, "firefox", "1", true)]));
        assert_eq!(s.apply(Event::Activate(Id::App(term))).1, vec![Effect::FocusWorkspace(T2.into())]);
        // A long press closes that instance's windows.
        assert_eq!(s.apply(Event::LongPress(Id::App(term))).1, vec![Effect::CloseWindows(vec![12, 13])]);
    }

    #[test]
    fn a_failed_start_is_forgotten() {
        let mut s = state();
        s.apply(Event::Tick { unix_secs: 100 });
        s.apply(Event::Activate(Id::App(1)));
        s.apply(Event::Tick { unix_secs: 120 });
        assert!(!s.app_running(1));
        assert!(matches!(s.apply(Event::Activate(Id::App(1))).1.as_slice(), [Effect::RunApp { .. }]), "tapping starts it again");
    }

    #[test]
    fn a_game_tile_goes_to_its_game_and_quits_it() {
        let mut s = state();
        assert_eq!(s.apply(Event::Activate(Id::GameTile(0))).1, vec![], "no game");
        s.apply(Event::Tick { unix_secs: 77 });
        s.apply(Event::Activate(Id::Launch));
        s.apply(Event::Started("zelda-lttp".into()));
        let ws = s.games.first().map(|g| g.workspace.clone()).unwrap_or_default();
        assert_eq!(ws, "clamshell-game-77-0");
        s.apply(Event::Units(vec![ws.clone()]));
        assert_eq!(
            s.apply(Event::Windows(vec![win(5, "retroarch", &ws, false)])).1,
            vec![Effect::FocusWorkspace(ws.clone())],
            "shown when its window appears"
        );
        assert_eq!(s.apply(Event::Activate(Id::GameTile(0))).1, vec![Effect::FocusWorkspace(ws.clone())]);
        assert_eq!(s.apply(Event::LongPress(Id::GameTile(0))).1, vec![Effect::QuitGame { windows: vec![5], units: vec![format!("{ws}.scope")] }]);
    }
}
