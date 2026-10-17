//! What runs and what is in front: the games, the dock apps' instances (systemd's scopes), the
//! compositor's windows, and what was started and hasn't shown up yet. The rules for games are in
//! [`crate::game`], for the dock's apps in [`crate::tasks`].

use std::sync::Arc;

use crate::apps::SHELF_APP_ID;
use crate::effect::{Effect, WindowCommand};
use crate::game::{Front, Launching, Queued, RunningGame};
use crate::instance::Instance;
use crate::windows::Window;

/// Seconds an instance may take to show up in systemd's list before its start counts as failed.
const STARTING_FOR: u64 = 15;

/// Something started that hasn't shown up yet: its instance, and when it was asked for (epoch seconds).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Pending {
    pub instance: Instance,
    pub since: u64,
}

#[derive(Debug, Default)]
pub struct Session {
    /// The games that run, in the order they started.
    pub(crate) games: Vec<RunningGame>,
    /// The game: of the games running, the one most recently in front (its settings are in force).
    pub(crate) current: Option<Arc<str>>,
    /// Launches asked for that haven't started.
    pub(crate) launching: Vec<Launching>,
    /// The game to start once the one closed for it has ended.
    pub(crate) queued: Option<Queued>,
    /// The compositor's windows.
    pub(crate) windows: Vec<Window>,
    /// Windows sent to the bottom screen by a rule (each once; dropped when it closes).
    pub(crate) sent_down: Vec<i64>,
    /// Our running scopes (systemd's list).
    pub(crate) units: Vec<Instance>,
    /// Workspaces by when they were last in front, most recent first.
    pub(crate) recent: Vec<String>,
    /// Dock app instances started and not yet listed by systemd, so a second tap doesn't start another;
    /// dropped once listed, or after a while if the start failed.
    pub(crate) pending: Vec<Pending>,
    /// The instance to show when its first window appears: the instance or game just started.
    pub(crate) pending_show: Option<Instance>,
    /// Dock apps quitting (by key): their windows asked to close; once none is left, their slice is
    /// stopped (then true) if it still runs.
    pub(crate) quitting: Vec<(String, bool)>,
    /// Numbers new instances' names (with the time, unique across restarts).
    pub(crate) next_instance: u64,
}

impl Session {
    pub fn games(&self) -> &[RunningGame] {
        &self.games
    }

    /// The game: of the games running, the one most recently in front.
    pub fn current_game(&self) -> Option<&Arc<str>> {
        self.current.as_ref()
    }

    pub fn windows(&self) -> &[Window] {
        &self.windows
    }

    pub fn units(&self) -> &[Instance] {
        &self.units
    }

    /// What is in front, from the compositor's focused window; without one, the game if one runs, else
    /// shelf.
    pub fn front(&self) -> Front {
        match self.windows.iter().find(|w| w.focused) {
            Some(w) if w.app_id == SHELF_APP_ID => Front::Shelf,
            Some(w) if w.of_game() || self.game_of(w).is_some() => Front::Game,
            Some(_) => Front::App,
            None if self.current.is_some() => Front::Game,
            None => Front::Shelf,
        }
    }

    /// The running game a window belongs to: on its workspace, or Steam's window of it.
    pub fn game_of(&self, w: &Window) -> Option<&RunningGame> {
        self.games.iter().find(|g| g.instance.is(&w.workspace) || g.appid().is_some_and(|id| w.steam_id() == Some(id)))
    }

    pub fn running_game(&self, id: &str) -> Option<&RunningGame> {
        self.games.iter().find(|g| &*g.id == id)
    }

    /// The start time and counter for a new instance's name.
    pub(crate) fn new_name(&mut self, now: u64) -> (u64, u64) {
        let n = self.next_instance;
        self.next_instance += 1;
        (now, n)
    }

    pub(crate) fn set_windows(&mut self, windows: Vec<Window>) {
        self.windows = windows;
        let present = |name: &String| self.windows.iter().any(|w| w.workspace == *name);
        self.recent.retain(present);
        if let Some(front) = self.windows.iter().find(|w| w.focused).map(|w| w.workspace.clone()) {
            self.recent.retain(|n| *n != front);
            self.recent.insert(0, front);
        }
    }

    pub(crate) fn set_units(&mut self, units: Vec<Instance>) {
        self.units = units;
        let units = &self.units;
        self.pending.retain(|p| !units.contains(&p.instance));
    }

    /// Starts systemd never listed failed: forgotten.
    pub(crate) fn forget_failed_starts(&mut self, now: u64) {
        self.pending.retain(|p| now < p.since + STARTING_FOR);
    }

    /// The instance or game just started, once its first window is there: show it.
    pub(crate) fn show_pending(&mut self) -> Vec<Effect> {
        // (An app's: its own window, as its workspace may have its other instances' already.)
        let arrived = self.pending_show.as_ref().is_some_and(|i| {
            let unit = i.app_key().and(i.unit());
            self.windows.iter().any(|w| i.on(&w.workspace) && (unit.is_none() || w.unit == unit))
        });
        match self.pending_show.take() {
            Some(instance) if arrived => vec![Effect::Window(WindowCommand::Focus(instance.workspace()))],
            other => {
                self.pending_show = other;
                vec![]
            }
        }
    }
}
