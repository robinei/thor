//! What runs and what is in front: the running entries (see [`crate::running`]), our scopes as systemd
//! lists them, the compositor's windows, and the launch to show once its first window appears.

use std::sync::Arc;

use crate::effect::{Effect, WindowCommand};
use crate::instance::Instance;
use crate::running::{Front, Launching, Running};
use crate::windows::{SHELF_APP_ID, Window};

/// How long after the window in front closed a new one on its workspace is shown again (seconds).
const REOPEN_SECS: u64 = 2;

#[derive(Debug, Default)]
pub struct Session {
    /// The entries that run, in the order they started.
    pub(crate) running: Vec<Running>,
    /// The program in force (see [`crate::program`]): an entry's id.
    pub(crate) program: Option<Arc<str>>,
    /// Launches asked for that haven't started.
    pub(crate) launching: Vec<Launching>,
    /// The compositor's windows.
    pub(crate) windows: Vec<Window>,
    /// Windows sent to the bottom screen by a rule (each once; dropped when it closes).
    pub(crate) sent_down: Vec<i64>,
    /// Our running scopes (systemd's list).
    pub(crate) units: Vec<Instance>,
    /// Workspaces by when they were last in front, most recent first.
    pub(crate) recent: Vec<String>,
    /// The launch to show when its first window appears.
    pub(crate) pending_show: Option<Instance>,
    /// The workspace whose window in front closed, leaving it empty, and when (see
    /// [`Session::show_reopened`]).
    pub(crate) closed_front: Option<(String, u64)>,
    /// Entries quitting: their windows asked to close; once none is left, ended (then true) if they still
    /// run; and their runner's stop, for one that ends with its windows (known by its app id).
    pub(crate) quitting: Vec<(Arc<str>, bool, Option<Effect>)>,
    /// Numbers new instances' names (with the time, unique across restarts).
    pub(crate) next_instance: u64,
}

impl Session {
    /// The entries that run, in the order they started.
    pub fn running_entries(&self) -> &[Running] {
        &self.running
    }

    /// A running entry, by its id.
    pub fn running(&self, id: &str) -> Option<&Running> {
        self.running.iter().find(|r| &*r.id == id)
    }

    /// The program in force: an entry's id.
    pub fn program(&self) -> Option<&Arc<str>> {
        self.program.as_ref()
    }

    pub fn windows(&self) -> &[Window] {
        &self.windows
    }

    pub fn units(&self) -> &[Instance] {
        &self.units
    }

    /// What is in front on the main screen: shelf, a running entry's window, or another.
    pub fn front(&self) -> Front {
        match self.windows.iter().find(|w| w.focused && !w.down) {
            Some(w) if w.app_id == SHELF_APP_ID => Front::Shelf,
            Some(w) if self.running_of(w).is_some() => Front::Program,
            Some(_) => Front::Other,
            None if self.program.is_some() => Front::Program,
            None => Front::Shelf,
        }
    }

    /// The running entry a window belongs to, by the first that says: its app id (see
    /// [`crate::running::app_id_of`]: a Steam game's, though it runs in Steam's scope), its scope (moved
    /// down too), its process (a program found by its tray icon), its workspace.
    pub fn running_of(&self, w: &Window) -> Option<&Running> {
        let by_key = |k: Option<&str>| self.running.iter().find(|r| k == Some(r.key.as_str()));
        self.running
            .iter()
            .find(|r| r.owns(w))
            .or_else(|| by_key(w.unit.as_deref().and_then(Instance::key_of)))
            .or_else(|| self.running.iter().find(|r| r.fathered(w)))
            .or_else(|| by_key(Instance::key_of(&w.workspace)))
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
    }

    /// A window on the workspace whose window in front closed, within [`REOPEN_SECS`] (a program that
    /// replaces its first window: a splash's, Waydroid's on a cold start): that workspace shown again.
    pub(crate) fn show_reopened(&mut self, now: u64) -> Vec<Effect> {
        let Some((ws, at)) = self.closed_front.take() else {
            return vec![];
        };
        if now > at + REOPEN_SECS {
            return vec![];
        }
        if self.windows.iter().any(|w| w.workspace == ws && !w.down) {
            return vec![Effect::Window(WindowCommand::Focus(ws))];
        }
        self.closed_front = Some((ws, at));
        vec![]
    }

    /// The launch just started, once its own first window is there (the entry's workspace may have its
    /// other launches' already; one known by its app id has no scope of ours: any of its windows): its
    /// workspace shown.
    pub(crate) fn show_pending(&mut self) -> Vec<Effect> {
        let arrived = self.pending_show.as_ref().is_some_and(|i| {
            let unit = i.unit();
            let outside = self.running.iter().find(|r| Some(r.key.as_str()) == i.key()).is_some_and(Running::outside);
            self.windows.iter().any(|w| i.on(&w.workspace) && (outside || w.unit == unit))
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
