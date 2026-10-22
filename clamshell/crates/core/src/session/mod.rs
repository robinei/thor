//! What runs and what is in front: the running entries (see [`crate::session::running`]), our scopes as
//! systemd lists them, the compositor's windows, and the launch to show once its first window appears.

pub mod autostart;
pub mod background;
pub mod dock;
pub mod enqueue;
pub(crate) mod found;
mod front;
pub mod instance;
pub mod program;
pub(crate) mod running;
pub mod tray;
pub mod windows;

use std::sync::Arc;

use crate::effect::{Effect, WindowCommand};
use crate::out::Out;
use crate::{Entry, Library};
use instance::Instance;
use running::{Front, Launching, Running};
use windows::{SHELF_APP_ID, Window};

/// How soon after the window in front closed a new window on its workspace is shown again, in seconds.
const REOPEN_SECS: u64 = 2;

#[derive(Debug, Default)]
pub struct Session {
    /// The entries that run, in the order they started.
    running: Vec<Running>,
    /// The id of the program in force (see [`crate::session::program`]).
    program: Option<Arc<str>>,
    /// Launches asked for that haven't started.
    launching: Vec<Launching>,
    /// The compositor's windows.
    windows: Vec<Window>,
    /// Our running scopes, as systemd lists them.
    units: Vec<Instance>,
    /// Workspaces by when they were last in front, most recent first.
    recent: Vec<String>,
    /// The launch to show when its first window appears.
    pending_show: Option<Instance>,
    /// The workspace whose window in front closed, leaving it empty, and when (see
    /// [`Session::show_reopened`]).
    closed_front: Option<(String, u64)>,
    /// Entries quitting (see [`Quitting`]).
    quitting: Vec<Quitting>,
    /// The counter that numbers new instances' names. Together with the time, it makes them unique across
    /// restarts.
    next_instance: u64,
    /// Whether systemd has listed our scopes at least once, so that what runs is known.
    units_known: bool,
    /// What the last [`crate::State::settle`] saw: the windows' ids and the workspace in front on the
    /// main screen. They tell which windows are new and when the one in front closes.
    followed_windows: Vec<i64>,
    followed_front: Option<String>,
    /// Which running entries have windows, in their order, kept so that each pass reuses it (see
    /// [`Session::end_windowless`]).
    with_windows: Vec<bool>,
    /// Programs' tray icons (see [`crate::session::tray`]).
    tray: Vec<tray::TrayItem>,
    /// Programs paused or ended while hidden (see [`crate::session::background`]).
    background: background::Background,
    /// The entries started with ClamShell, and whether they have been started yet (see
    /// [`crate::session::autostart`]).
    autostart: Vec<Arc<str>>,
    autostarted: bool,
    /// ClamShell is the device's own session (see [`crate::Init::device_session`]).
    device_session: bool,
    /// Counts changes to what the session follows from outside (see [`Session::version`]).
    version: u64,
}

/// An entry by its id, in `library` or else among the `running` (see [`Session::entry`]).
fn entry_in<'a>(library: &'a Library, running: &'a [Running], id: &str) -> Option<&'a Entry> {
    library.entry(id).or_else(|| running.iter().find(|r| &*r.id == id).and_then(|r| r.entry.as_deref()))
}

/// An entry quitting. Its windows are asked to close, and once none is left it is ended if it still runs,
/// which sets `ended`. `stop` is its runner's stop, for an entry known by its app id that ends with its
/// windows.
#[derive(Debug, Clone)]
pub(crate) struct Quitting {
    entry: Arc<str>,
    ended: bool,
    stop: Option<Effect>,
}

impl Session {
    pub(crate) fn new(autostart: Vec<Arc<str>>, device_session: bool) -> Session {
        Session { autostart, device_session, ..Session::default() }
    }

    /// Whether ClamShell is the device's own session (see [`crate::Init::device_session`]).
    pub(crate) fn device_session(&self) -> bool {
        self.device_session
    }

    /// Whether systemd lists a scope of the entry with `key`.
    pub(crate) fn has_units(&self, key: &str) -> bool {
        self.units.iter().any(|u| u.key() == Some(key))
    }

    /// Whether entry `id` is quitting.
    pub(crate) fn is_quitting(&self, id: &str) -> bool {
        self.quitting.iter().any(|q| &*q.entry == id)
    }

    /// The entries that run, in the order they started.
    pub fn running_entries(&self) -> &[Running] {
        &self.running
    }

    /// The running entry whose windows have app id `app`.
    pub(crate) fn running_app(&self, app: &str) -> Option<Arc<str>> {
        self.running.iter().find(|r| r.app_id.as_deref() == Some(app)).map(|r| r.id.clone())
    }

    /// An entry by its id: the library's, or else, for an entry that is still running after leaving the
    /// library because its file was deleted, the entry as it was.
    pub fn entry<'a>(&'a self, library: &'a Library, id: &str) -> Option<&'a Entry> {
        entry_in(library, &self.running, id)
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

    /// The running entry a window belongs to, by the first of these that matches:
    ///
    /// - its app id (see [`crate::session::running::app_id_of`]), as for a Steam game that runs in Steam's
    ///   scope;
    /// - its scope, which also covers windows moved down;
    /// - its process, for a program found by its tray icon;
    /// - its workspace.
    pub fn running_of(&self, w: &Window) -> Option<&Running> {
        let by_key = |k: Option<&str>| self.running.iter().find(|r| k == Some(r.key.as_str()));
        self.running
            .iter()
            .find(|r| r.owns(w))
            .or_else(|| by_key(w.unit.as_deref().and_then(Instance::key_of)))
            .or_else(|| self.running.iter().find(|r| r.fathered(w)))
            .or_else(|| by_key(Instance::key_of(&w.workspace)))
    }

    /// The id of the running entry a window belongs to (see [`Session::running_of`]).
    pub(crate) fn owner_of(&self, w: &Window) -> Option<Arc<str>> {
        self.running_of(w).map(|r| r.id.clone())
    }

    /// The windows moved to the bottom screen, in the compositor's order.
    pub fn windows_down(&self) -> impl Iterator<Item = &Window> {
        self.windows.iter().filter(|w| w.down)
    }

    /// The windows of running entry `id`, including those moved to the bottom screen.
    pub fn windows_of<'a>(&'a self, id: &'a str) -> impl Iterator<Item = &'a Window> + 'a {
        self.windows.iter().filter(move |w| self.running_of(w).is_some_and(|r| &*r.id == id))
    }

    /// The `awake` command of the runner of a shown window, if it has one.
    pub(crate) fn awake_command(&self) -> Option<Arc<[String]>> {
        self.windows.iter().filter(|w| w.visible).find_map(|w| {
            let runner = self.running_of(w)?.entry.as_ref()?.runner.as_ref()?;
            (!runner.awake.is_empty()).then(|| runner.awake.clone())
        })
    }

    /// The window Home with B acts on. It is the one in front on the top screen, other than shelf's. If
    /// there is none, it is the one in view on the bottom screen while it shows the windows moved there
    /// (`shows_windows`).
    pub(crate) fn home_b_window(&self, shows_windows: bool) -> Option<&Window> {
        let windows = &self.windows;
        windows
            .iter()
            .find(|w| w.focused && !w.down && w.app_id != SHELF_APP_ID)
            .or_else(|| shows_windows.then(|| windows.iter().filter(|w| w.down).max_by_key(|w| w.visible)).flatten())
    }

    /// Sets the icon of found entry `entry`, once it was found (see [`Effect::FindIcon`]). Returns whether
    /// it changed.
    pub(crate) fn set_icon(&mut self, entry: &str, path: std::path::PathBuf) -> bool {
        let running = self.running.iter_mut().find(|r| &*r.id == entry);
        running.and_then(|r| r.entry.as_mut()).is_some_and(|g| crate::kit::replace(&mut g.icon, Some(path)))
    }

    /// Has `running` run in scope `unit`, as if it had started and systemd listed the scope.
    #[cfg(test)]
    pub(crate) fn add_run(&mut self, running: Running, unit: Instance) {
        self.running.push(running);
        self.units.push(unit);
    }

    /// The launches asked for that haven't started.
    #[cfg(test)]
    pub(crate) fn launching(&self) -> &[Launching] {
        &self.launching
    }

    /// The launch to show when its first window appears.
    #[cfg(test)]
    pub(crate) fn pending_show(&self) -> Option<&Instance> {
        self.pending_show.as_ref()
    }

    /// The counter for a new instance's name (see [`crate::session::instance::Instance::Run`]).
    pub(crate) fn next_instance(&mut self) -> u64 {
        let n = self.next_instance;
        self.next_instance += 1;
        n
    }

    /// Counts changes to what the session follows from outside: the windows, systemd's scopes, the tray,
    /// the launcher's messages, the library and the clock. The steps that follow them run when it
    /// changed (see [`crate::State::settle`]).
    pub(crate) fn version(&self) -> u64 {
        self.version
    }

    /// Something the session follows from outside changed (see [`Session::version`]).
    pub(crate) fn changed(&mut self) {
        self.version += 1;
    }

    pub(crate) fn set_windows(&mut self, windows: Vec<Window>) {
        self.changed();
        self.windows = windows;
        let present = |name: &String| self.windows.iter().any(|w| w.workspace == *name);
        self.recent.retain(present);
        // Already first, it stays.
        if let Some(front) = self.windows.iter().find(|w| w.focused).map(|w| &w.workspace).filter(|f| self.recent.first() != Some(*f)) {
            let front = front.clone();
            self.recent.retain(|n| *n != front);
            self.recent.insert(0, front);
        }
    }

    pub(crate) fn set_units(&mut self, units: Vec<Instance>) {
        self.changed();
        self.units = units;
        self.units_known = true;
    }

    /// If a window appears within [`REOPEN_SECS`] on the workspace whose window in front closed, shows
    /// that workspace again. This is for a program that replaces its first window, such as a splash
    /// screen, or Waydroid on a cold start.
    pub(crate) fn show_reopened(&mut self, now: u64, out: &mut Out) {
        let Some((ws, at)) = self.closed_front.take() else {
            return;
        };
        if now > at + REOPEN_SECS {
            return;
        }
        if self.windows.iter().any(|w| w.workspace == ws && !w.down) {
            out.effects.push(Effect::Window(WindowCommand::Focus(ws)));
            return;
        }
        self.closed_front = Some((ws, at));
    }

    /// Shows the workspace of the launch just started, once its own first window is there. The entry's
    /// workspace may already have windows of its other launches. An entry known by its app id has no scope
    /// of ours, so any of its windows counts.
    pub(crate) fn show_pending(&mut self, out: &mut Out) {
        let arrived = self.pending_show.as_ref().is_some_and(|i| {
            let unit = i.unit();
            let outside = self.running.iter().find(|r| Some(r.key.as_str()) == i.key()).is_some_and(Running::outside);
            self.windows.iter().any(|w| i.on(&w.workspace) && (outside || w.unit == unit))
        });
        match self.pending_show.take() {
            Some(instance) if arrived => out.effects.push(Effect::Window(WindowCommand::Focus(instance.workspace()))),
            other => self.pending_show = other,
        }
    }
}
