//! sway's windows, through its IPC socket.
//!
//! - Launches go through sway's `exec`, aimed at their workspace. sway places the launch's first window
//!   there itself before it is shown, even after we have switched back, so the workspace isn't shown
//!   first.
//! - sway's own rules are set when we connect. They put shelf's window on its workspace and keep that
//!   workspace on the main screen. A window that opens on the second screen's workspace, behind the
//!   launcher, is moved to the main screen when the windows are reconciled.
//! - Entries that run in another program's processes, such as Steam's games and Waydroid's apps, are
//!   known by their app ids ([`Command::LandRules`]). Each app id gets a sway rule, defined when we
//!   connect and when a new one comes, which puts its windows on the landing workspace as they open.
//!   ClamShell then moves them to their entry's workspace. sway can't drop a rule, so a rule whose entry
//!   is gone still lands its windows there, and ClamShell sends them to the main screen.
//! - sway only places a launch's first window. Later windows are moved to their instance's workspace,
//!   the app's or the game's, by the systemd scope of their process. The scope is the kernel's record of
//!   which launch they come from.
//! - The window list is sent after every window event. A workspace in front that empties stays, black.
//! - Windows remembered for the bottom screen ([`Command::DownRules`]) each get a sway rule that moves a
//!   matching window there as it opens, before it is drawn. sway can't drop a rule, so each one is
//!   switched on and off by a variable. The rules are defined when we connect and when a new one comes,
//!   and switched to match the rules in force. A window a rule puts there is left there, and ClamShell
//!   marks it as moved down.
//!
//! Without sway there is no window list, because other compositors offer no common way to get one.

mod commands;
mod ipc;
mod windows;

use std::collections::{HashMap, HashSet};
use std::convert::Infallible;
use std::io;
use std::path::PathBuf;
use std::sync::mpsc::Receiver;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use clamshell_core::Event;
use clamshell_core::bottom::screen::DownRule;

use crate::os::cgroup;
use crate::os::worker::{self, Ended, Service, Worker, lost_on};
use crate::{Closed, Sink};
use ipc::{EventKind, Ipc};

pub use ipc::Error;

/// How long to wait before trying sway again.
const RETRY: Duration = Duration::from_secs(2);
/// The most process ancestries to remember, by id and start time. The cache is emptied when it is full.
const LINEAGES_KEPT: usize = 256;

lost_on!(Error: io::Error, serde_json::Error);

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Command {
    /// Show a workspace, creating it if new.
    Workspace(String),
    /// Run a job (see [`crate::launch::job`]) for this workspace, without showing it.
    Launch { workspace: String, job: PathBuf },
    /// Ask a window to close, as its close button would.
    Close(i64),
    /// Move windows to a workspace on the main screen, without showing it.
    Move { windows: Vec<i64>, workspace: String },
    /// Move a window to the bottom screen and show it there. It is marked, so it stays.
    Down(i64),
    /// Move a window that was moved down back up, to this workspace or else to the main screen.
    Up { window: i64, workspace: Option<String> },
    /// Show this window, which was moved to the bottom screen, there.
    ShowDown(i64),
    /// Turn the bottom screen's output on or off.
    Power(bool),
    /// The windows that go to the bottom screen as they open. This is every rule in force.
    DownRules(Vec<DownRule>),
    /// The app ids whose windows land on the landing workspace as they open. This is all of them.
    LandRules(Vec<Arc<str>>),
}

/// The down rules: those in force, and those defined in sway, which are switched off when no longer in
/// force. The command thread sets new rules, and the window thread defines them again when it reconnects
/// to sway.
#[derive(Clone, Default)]
struct DownRules(Arc<Mutex<(Vec<DownRule>, Vec<DownRule>)>>);

impl DownRules {
    /// Applies `wanted` as the rules in force from now on, or keeps the current ones if it is `None`.
    /// Rules not yet defined in sway are defined, and every defined rule is switched on or off. `fresh`
    /// means sway has none defined, because we connected anew.
    fn apply(&self, ipc: &mut Ipc, wanted: Option<Vec<DownRule>>, fresh: bool, bottom: &str) -> Result<(), Error> {
        let mut guard = crate::os::lock(&self.0);
        let (wanted_now, defined) = &mut *guard;
        if let Some(rules) = wanted {
            *wanted_now = rules;
        }
        if fresh {
            defined.clear();
        }
        let mut text = Vec::new();
        for rule in wanted_now.iter() {
            if !defined.contains(rule) {
                match commands::define_down_rule(rule) {
                    Some(define) => text.push(define),
                    None => log::info!("sway: no rule for {rule:?} (sway can't match it): moved down after it opens"),
                }
                defined.push(rule.clone());
            }
        }
        text.extend(defined.iter().map(|r| commands::switch_down_rule(r, wanted_now.contains(r), bottom)));
        if !text.is_empty()
            && let Some(error) = ipc::refused(&ipc.command(&text.join("; "))?)
        {
            log::debug!("sway: down rules: {error}");
        }
        Ok(())
    }

    /// The rules in force.
    fn wanted(&self) -> Vec<DownRule> {
        crate::os::lock(&self.0).0.clone()
    }
}

/// The landing rules: the app ids wanted, and those defined in sway. They are shared the same way as
/// [`DownRules`].
#[derive(Clone, Default)]
struct LandRules(Arc<Mutex<Landing>>);

/// The app ids wanted, and those defined in sway.
type Landing = (Vec<Arc<str>>, HashSet<Arc<str>>);

impl LandRules {
    /// Applies `wanted` as the app ids wanted from now on, or keeps the current ones if it is `None`. App
    /// ids not yet defined in sway are defined. `fresh` means sway has none defined, because we connected
    /// anew.
    fn apply(&self, ipc: &mut Ipc, wanted: Option<Vec<Arc<str>>>, fresh: bool) -> Result<(), Error> {
        let mut guard = crate::os::lock(&self.0);
        let (wanted_now, defined) = &mut *guard;
        if let Some(ids) = wanted {
            *wanted_now = ids;
        }
        if fresh {
            defined.clear();
        }
        let mut text = Vec::new();
        for id in wanted_now.iter() {
            if defined.insert(id.clone()) {
                match commands::define_land_rule(id) {
                    Some(define) => text.push(define),
                    None => log::info!("sway: no landing rule for {id} (sway can't match it)"),
                }
            }
        }
        if !text.is_empty()
            && let Some(error) = ipc::refused(&ipc.command(&text.join("; "))?)
        {
            log::debug!("sway: landing rules: {error}");
        }
        Ok(())
    }

    /// The app ids wanted.
    fn wanted(&self) -> Vec<Arc<str>> {
        crate::os::lock(&self.0).0.clone()
    }
}

/// The main screen's output: the first active output that isn't the second screen's. The window thread
/// reads it when it connects and at every output event, and the command thread uses it.
#[derive(Clone)]
struct TopOutput {
    bottom: String,
    known: Arc<Mutex<Option<String>>>,
}

impl TopOutput {
    fn read(&self, ipc: &mut Ipc) -> Option<String> {
        let outputs = ipc.outputs().map_err(|e| log::debug!("sway: outputs: {e}")).ok()?;
        let top = outputs.into_iter().find(|o| o.active && o.name != self.bottom).map(|o| o.name);
        *crate::os::lock(&self.known) = top.clone();
        top
    }

    /// The output as last read, or else read now through `ipc`.
    fn get(&self, ipc: &mut Ipc) -> Option<String> {
        let known = crate::os::lock(&self.known).clone();
        known.or_else(|| self.read(ipc))
    }
}

/// Follows sway's windows, sending [`Event::Windows`] after each change, and carries out commands. It
/// connects again if sway goes away. Returns `None` without sway, when there is no `socket`. `bottom` is
/// the second screen's output.
pub fn spawn(socket: Option<PathBuf>, bottom: String, sink: impl Sink) -> io::Result<Option<Worker<Command>>> {
    let Some(socket) = socket else {
        log::info!("windows: not on sway, no window list");
        return Ok(None);
    };
    let top = TopOutput { bottom, known: Arc::default() };
    let (down, land) = (DownRules::default(), LandRules::default());
    let carrier = Carrier { socket: socket.clone(), top: top.clone(), exe: std::env::current_exe()?, down: down.clone(), land: land.clone() };
    let worker = Worker::spawn("sway commands", move |commands: Receiver<Command>| {
        for command in commands {
            if let Err(e) = carrier.carry_out(&command) {
                log::warn!("sway: {command:?}: {e}");
            }
        }
    })?;
    let mut windows = Windows { socket, top, down, land, sink, lineages: HashMap::new(), tried: HashSet::new() };
    worker::detached("sway windows", move || worker::supervise_alone(&mut windows, RETRY))?;
    Ok(Some(worker))
}

/// What the command thread needs to carry out commands. The rules are shared with the window thread,
/// which applies them again when sway restarts.
struct Carrier {
    socket: PathBuf,
    top: TopOutput,
    exe: PathBuf,
    down: DownRules,
    land: LandRules,
}

impl Carrier {
    /// Carries out one command on a connection of its own, since sway may have restarted since the last.
    fn carry_out(&self, command: &Command) -> Result<(), Error> {
        let mut ipc = Ipc::connect(&self.socket)?;
        let (bottom, exe) = (&self.top.bottom, &self.exe);
        let top = self.top.get(&mut ipc);
        let top = top.as_deref();
        let text = match command {
            Command::DownRules(rules) => return self.down.apply(&mut ipc, Some(rules.clone()), false, bottom),
            Command::LandRules(ids) => return self.land.apply(&mut ipc, Some(ids.clone()), false),
            Command::Workspace(name) => commands::show(name, top),
            Command::Launch { workspace, job } => commands::launch(workspace, job, exe, top)?,
            Command::Close(id) => commands::close(*id),
            Command::Move { windows, workspace } => commands::move_windows(windows, workspace, top),
            Command::Down(id) => commands::down(*id, top),
            Command::Up { window, workspace } => commands::up(*window, workspace.as_deref(), top),
            Command::ShowDown(id) => commands::show_down(*id, top),
            Command::Power(on) => commands::power(bottom, *on),
        };
        if let Some(error) = ipc::refused(&ipc.command(&text)?) {
            log::debug!("sway: {text}: {error}");
        }
        Ok(())
    }
}

/// The thread following sway's windows.
struct Windows<S> {
    socket: PathBuf,
    top: TopOutput,
    down: DownRules,
    land: LandRules,
    sink: S,
    /// Each window process's ancestry (its pid, its parent's, and so on), keyed by its pid and start time.
    lineages: HashMap<(u32, Option<u64>), Vec<u32>>,
    /// Moves already asked for, so a window sway won't move isn't asked again at each of its events.
    tried: HashSet<(i64, String)>,
}

/// Two connections: one for the events, one for the questions asked meanwhile.
struct Connected {
    events: Ipc,
    queries: Ipc,
}

impl<S: Sink> Service for Windows<S> {
    type Command = Infallible;
    type Session = Connected;
    type Error = Error;
    const NAME: &'static str = "sway";

    /// Subscribes to window and output events and gives sway our rules.
    fn connect(&mut self) -> Result<Connected, Error> {
        let mut events = Ipc::connect(&self.socket)?;
        let mut queries = Ipc::connect(&self.socket)?;
        events.subscribe("window")?;
        // Without output events the main screen is read once, here.
        if let Err(e) = events.subscribe("output") {
            log::warn!("sway: {e}");
        }
        let top = self.top.read(&mut queries);
        for rule in commands::rules(top.as_deref(), &self.top.bottom) {
            // Parts of a rule may fail in the current state, for example when shelf's window isn't open
            // yet. Only a rule that failed entirely is worth a warning.
            let outcomes = queries.command(&rule)?;
            if !outcomes.iter().any(|o| o.success) {
                log::warn!("sway: {rule}: {}", ipc::refused(&outcomes).unwrap_or_default());
            } else if let Some(error) = ipc::refused(&outcomes) {
                log::debug!("sway: {rule}: {error}");
            }
        }
        // A new connection may be to a new sway that doesn't have them.
        if let Err(e) = self.down.apply(&mut queries, None, true, &self.top.bottom) {
            log::warn!("sway: down rules: {e}");
        }
        if let Err(e) = self.land.apply(&mut queries, None, true) {
            log::warn!("sway: landing rules: {e}");
        }
        Ok(Connected { events, queries })
    }

    /// Sends the window list after each window event until sway goes. If windows were moved, the list
    /// isn't sent, because the move brings another event and a fresh list.
    fn run_while_up(&mut self, mut c: Connected, _: &Receiver<Infallible>) -> Result<(), Ended<Error>> {
        loop {
            let tree = c.queries.tree()?;
            if self.lineages.len() > LINEAGES_KEPT {
                self.lineages.clear();
            }
            // A process's unit is read each time, because it changes when the process is adopted. Its
            // ancestry is kept by its id and start time, so a later process with the same id isn't taken
            // for it.
            let lineages = &mut self.lineages;
            let list = windows::windows(&tree, &mut |pid| {
                let start = crate::os::sys::saw(pid);
                (cgroup::unit_of(pid), lineages.entry((pid, start)).or_insert_with(|| crate::os::sys::lineage(pid)).clone())
            });
            let top = self.top.get(&mut c.queries);
            let mut moved = false;
            for command in windows::reconcile(&list, top.as_deref(), &self.down.wanted(), &self.land.wanted(), &mut self.tried) {
                // Only a move sway carried out brings an event. A refused one brings none.
                match c.queries.command(&command) {
                    Ok(outcomes) => {
                        if let Some(error) = ipc::refused(&outcomes) {
                            log::debug!("sway: {command}: {error}");
                        }
                        moved |= !outcomes.is_empty() && outcomes.iter().all(|o| o.success);
                    }
                    Err(e) => log::warn!("sway: {command}: {e}"),
                }
            }
            if !moved {
                self.sink.send(Event::Windows(list))?;
            }
            // Wait for the next window event, and any that came with it.
            loop {
                match c.events.next_event()? {
                    Some(EventKind::Window) => break,
                    Some(EventKind::Output) => {
                        self.top.read(&mut c.queries);
                    }
                    None => {}
                }
            }
        }
    }

    fn command_while_down(&mut self, command: Infallible) -> Result<(), Closed> {
        match command {}
    }
}
