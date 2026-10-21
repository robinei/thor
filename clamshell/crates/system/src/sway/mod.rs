//! sway's windows, through its IPC socket.
//!
//! - Launches go through sway's `exec`, aimed at their workspace: sway places the launch's first window
//!   there itself, before it is shown, even after we have switched back (the workspace isn't shown first).
//! - sway's own rules, set when we connect: shelf's window goes to its workspace, kept on the main
//!   screen, and a window that opens on the second screen's workspace (behind the launcher) goes to the
//!   main screen.
//! - The app ids of entries that run in another program's processes ([`Command::LandRules`]: Steam's
//!   games, Waydroid's apps): a rule each, defined when we connect and when a new one comes, which puts
//!   their windows on the landing workspace as they open; ClamShell moves them to their entry's. sway
//!   can't drop a rule: one whose entry is gone lands its windows there still, and ClamShell sends them
//!   to the main screen.
//! - Later windows of a launch (sway only places the first) are moved to their instance's workspace (its
//!   app's, or its game's) by the systemd scope of their process: the kernel's record of which launch
//!   they come from.
//! - The window list after every window event. A workspace in front that empties stays, black.
//! - The windows remembered for the bottom screen ([`Command::DownRules`]): a rule each in sway, which
//!   moves a matching window there as it opens (before it is drawn), switched on and off by a variable
//!   (sway can't drop a rule): defined when we connect and when a new one comes, switched to the rules in
//!   force. A window they put there is left there (ClamShell marks it as one moved down).
//!
//! Without sway there is no window list (other compositors offer no common way).

mod commands;
mod ipc;
mod windows;

use std::collections::{HashMap, HashSet};
use std::convert::Infallible;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::mpsc::Receiver;
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use clamshell_core::Event;
use clamshell_core::bottom_screen::DownRule;

use crate::worker::{self, Ended, Service, Worker, lost_on};
use crate::{Closed, Sink, cgroup};
use ipc::{EventKind, Ipc};

pub use ipc::Error;

/// How long to wait before trying sway again.
const RETRY: Duration = Duration::from_secs(2);
/// Processes' ancestries remembered, at most (by id and start time; emptied now and then).
const LINEAGES_KEPT: usize = 256;

lost_on!(Error: io::Error, serde_json::Error);

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Command {
    /// Show (and create, if new) a workspace.
    Workspace(String),
    /// Run a job (see [`crate::job`]) for this workspace, without showing it.
    Launch { workspace: String, job: PathBuf },
    /// Ask a window to close (as its close button would).
    Close(i64),
    /// Move windows to a workspace (on the main screen), without showing it.
    Move { windows: Vec<i64>, workspace: String },
    /// A window to the bottom screen (marked, so it stays), shown there.
    Down(i64),
    /// A window moved down back up: to this workspace (or the main screen).
    Up { window: i64, workspace: Option<String> },
    /// The bottom screen shows this window moved there.
    ShowDown(i64),
    /// The bottom screen's output on or off.
    Power(bool),
    /// The windows that go to the bottom screen as they open: all the rules in force.
    DownRules(Vec<DownRule>),
    /// The app ids whose windows land on the landing workspace as they open: all of them.
    LandRules(Vec<Arc<str>>),
}

/// The down rules: those in force, and those defined in sway (switched off when no longer in force).
/// Shared by the command thread (new rules) and the window thread (sway connected again: defined anew).
#[derive(Clone, Default)]
struct DownRules(Arc<Mutex<(Vec<DownRule>, Vec<DownRule>)>>);

impl DownRules {
    /// Rules in force from now on (`Some`), or as they were; defined where they aren't, every one
    /// switched; `fresh`: sway has none defined (connected anew).
    fn apply(&self, ipc: &mut Ipc, wanted: Option<Vec<DownRule>>, fresh: bool, bottom: &str) -> Result<(), Error> {
        let mut guard = self.0.lock().unwrap_or_else(PoisonError::into_inner);
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
        self.0.lock().unwrap_or_else(PoisonError::into_inner).0.clone()
    }
}

/// The landing rules: the app ids wanted, and those defined in sway. Shared as [`DownRules`] are.
#[derive(Clone, Default)]
struct LandRules(Arc<Mutex<Landing>>);

/// The app ids wanted, and those defined in sway.
type Landing = (Vec<Arc<str>>, HashSet<Arc<str>>);

impl LandRules {
    /// App ids wanted from now on (`Some`), or as they were; defined where they aren't; `fresh`: sway has
    /// none defined (connected anew).
    fn apply(&self, ipc: &mut Ipc, wanted: Option<Vec<Arc<str>>>, fresh: bool) -> Result<(), Error> {
        let mut guard = self.0.lock().unwrap_or_else(PoisonError::into_inner);
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
        self.0.lock().unwrap_or_else(PoisonError::into_inner).0.clone()
    }
}

/// The main screen's output: the first active one that isn't the second screen's. Kept by the window
/// thread (read when it connects and at every output event), used by the command thread.
#[derive(Clone)]
struct TopOutput {
    bottom: String,
    known: Arc<Mutex<Option<String>>>,
}

impl TopOutput {
    fn read(&self, ipc: &mut Ipc) -> Option<String> {
        let outputs = ipc.outputs().map_err(|e| log::debug!("sway: outputs: {e}")).ok()?;
        let top = outputs.into_iter().find(|o| o.active && o.name != self.bottom).map(|o| o.name);
        *self.known.lock().unwrap_or_else(PoisonError::into_inner) = top.clone();
        top
    }

    /// The output as last read, else read now through `ipc`.
    fn get(&self, ipc: &mut Ipc) -> Option<String> {
        let known = self.known.lock().unwrap_or_else(PoisonError::into_inner).clone();
        known.or_else(|| self.read(ipc))
    }
}

/// Follows sway's windows (sending [`Event::Windows`] after each change) and carries out commands;
/// connects again if sway goes away. `None` without sway (`socket`). `bottom`: the second screen's
/// output.
pub fn spawn(socket: Option<PathBuf>, bottom: String, sink: impl Sink) -> io::Result<Option<Worker<Command>>> {
    let Some(socket) = socket else {
        log::info!("windows: not on sway, no window list");
        return Ok(None);
    };
    let top = TopOutput { bottom, known: Arc::default() };
    let exe = std::env::current_exe()?;
    let commands_socket = socket.clone();
    let commands_top = top.clone();
    let (down, land) = (DownRules::default(), LandRules::default());
    let (commands_down, commands_land) = (down.clone(), land.clone());
    let worker = Worker::spawn("sway commands", move |commands: Receiver<Command>| {
        for command in commands {
            let rules = match &command {
                Command::DownRules(rules) => Some((
                    "down",
                    Ipc::connect(&commands_socket)
                        .map_err(Error::from)
                        .and_then(|mut ipc| commands_down.apply(&mut ipc, Some(rules.clone()), false, &commands_top.bottom)),
                )),
                Command::LandRules(ids) => Some((
                    "landing",
                    Ipc::connect(&commands_socket).map_err(Error::from).and_then(|mut ipc| commands_land.apply(&mut ipc, Some(ids.clone()), false)),
                )),
                _ => None,
            };
            if let Some((what, applied)) = rules {
                if let Err(e) = applied {
                    log::warn!("sway: {what} rules: {e}");
                }
                continue;
            }
            if let Err(e) = carry_out(&commands_socket, &commands_top, &exe, &command) {
                log::warn!("sway: {command:?}: {e}");
            }
        }
    })?;
    let mut windows = Windows { socket, top, down, land, sink, lineages: HashMap::new(), tried: HashSet::new() };
    worker::detached("sway windows", move || worker::supervise_alone(&mut windows, RETRY))?;
    Ok(Some(worker))
}

/// Carries out one command on a connection of its own (sway may have restarted since the last).
fn carry_out(socket: &Path, top: &TopOutput, exe: &Path, command: &Command) -> Result<(), Error> {
    let mut ipc = Ipc::connect(socket)?;
    let bottom = top.bottom.clone();
    let top = top.get(&mut ipc);
    let top = top.as_deref();
    let text = match command {
        Command::Workspace(name) => commands::show(name, top),
        Command::Launch { workspace, job } => commands::launch(workspace, job, exe, top)?,
        Command::Close(id) => commands::close(*id),
        Command::Move { windows, workspace } => commands::move_windows(windows, workspace, top),
        Command::Down(id) => commands::down(*id, top),
        Command::Up { window, workspace } => commands::up(*window, workspace.as_deref(), top),
        Command::ShowDown(id) => commands::show_down(*id, top),
        Command::Power(on) => commands::power(&bottom, *on),
        Command::DownRules(_) | Command::LandRules(_) => return Ok(()),
    };
    if let Some(error) = ipc::refused(&ipc.command(&text)?) {
        log::debug!("sway: {text}: {error}");
    }
    Ok(())
}

/// The thread following sway's windows.
struct Windows<S> {
    socket: PathBuf,
    top: TopOutput,
    down: DownRules,
    land: LandRules,
    sink: S,
    /// Each window's unit, by process (a process's unit never changes).
    lineages: HashMap<(u32, Option<u64>), Vec<u32>>,
    /// Moves asked for, so a window sway won't move isn't asked again (and again, by its events).
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

    /// Subscribes to window and output events, and gives sway our rules.
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
            // Parts of a rule may fail as things are (shelf's window not open yet): only a rule that
            // failed entirely is worth a warning.
            let outcomes = queries.command(&rule)?;
            if !outcomes.iter().any(|o| o.success) {
                log::warn!("sway: {rule}: {}", ipc::refused(&outcomes).unwrap_or_default());
            } else if let Some(error) = ipc::refused(&outcomes) {
                log::debug!("sway: {rule}: {error}");
            }
        }
        // (A new connection may be a new sway, without them.)
        if let Err(e) = self.down.apply(&mut queries, None, true, &self.top.bottom) {
            log::warn!("sway: down rules: {e}");
        }
        if let Err(e) = self.land.apply(&mut queries, None, true) {
            log::warn!("sway: landing rules: {e}");
        }
        Ok(Connected { events, queries })
    }

    /// Sends the window list after each window event (unless windows were moved, which brings another
    /// event and a fresh list), until sway goes.
    fn run_while_up(&mut self, mut c: Connected, _: &Receiver<Infallible>) -> Result<(), Ended<Error>> {
        loop {
            let tree = c.queries.tree()?;
            if self.lineages.len() > LINEAGES_KEPT {
                self.lineages.clear();
            }
            // (A process's unit read each time: adopted, it changes; its ancestry kept, by its id and when
            // it started, so a later process with the same id isn't taken for it.)
            let lineages = &mut self.lineages;
            let list = windows::windows(&tree, &mut |pid| {
                let start = crate::sys::saw(pid);
                (cgroup::unit_of(pid), lineages.entry((pid, start)).or_insert_with(|| crate::sys::lineage(pid)).clone())
            });
            let top = self.top.get(&mut c.queries);
            let mut moved = false;
            for command in windows::reconcile(&list, top.as_deref(), &self.down.wanted(), &self.land.wanted(), &mut self.tried) {
                // (Only a move sway carried out brings the event; a refused one, none.)
                moved |= c.queries.command(&command).is_ok_and(|outcomes| !outcomes.is_empty() && outcomes.iter().all(|o| o.success));
            }
            if !moved {
                self.sink.send(Event::Windows(list))?;
            }
            // Wait for the next window event (and any that came with it).
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
