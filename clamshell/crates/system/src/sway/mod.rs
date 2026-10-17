//! sway's windows, through its IPC socket.
//!
//! - Launches go through sway's `exec`, aimed at their workspace: sway places the launch's first window
//!   there itself, before it is shown, even after we have switched back (the workspace isn't shown first).
//! - sway's own rules, set when we connect: shelf's window and Steam's games (`steam_app_…`) go to their
//!   workspaces, those are kept on the main screen, and a window that opens on the second screen's
//!   workspace (behind the launcher) goes to the main screen.
//! - Later windows of a launch (sway only places the first) are moved to their instance's workspace (its
//!   app's, or its game's) by the systemd scope of their process: the kernel's record of which launch
//!   they come from.
//! - The window list after every window event. A workspace in front that empties stays, black.
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

use crate::worker::{self, Ended, Service, Worker, lost_on};
use crate::{Closed, Sink, cgroup};
use ipc::{EventKind, Ipc};

pub use ipc::Error;

/// How long to wait before trying sway again.
const RETRY: Duration = Duration::from_secs(2);
/// Units remembered per process, at most (process ids are reused, so the cache is emptied now and then).
const UNITS_KEPT: usize = 256;

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
    let worker = Worker::spawn("sway commands", move |commands: Receiver<Command>| {
        for command in commands {
            if let Err(e) = carry_out(&commands_socket, &commands_top, &exe, &command) {
                log::warn!("sway: {command:?}: {e}");
            }
        }
    })?;
    let mut windows = Windows { socket, top, sink, units: HashMap::new(), tried: HashSet::new() };
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
    sink: S,
    /// Each window's unit, by process (a process's unit never changes).
    units: HashMap<u32, Option<String>>,
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
        Ok(Connected { events, queries })
    }

    /// Sends the window list after each window event (unless windows were moved, which brings another
    /// event and a fresh list), until sway goes.
    fn run_while_up(&mut self, mut c: Connected, _: &Receiver<Infallible>) -> Result<(), Ended<Error>> {
        loop {
            let tree = c.queries.tree()?;
            if self.units.len() > UNITS_KEPT {
                self.units.clear();
            }
            let units = &mut self.units;
            let list = windows::windows(&tree, &mut |pid| units.entry(pid).or_insert_with(|| cgroup::unit_of(pid)).clone());
            let top = self.top.get(&mut c.queries);
            let mut moved = false;
            for command in windows::reconcile(&list, top.as_deref(), &mut self.tried) {
                moved |= c.queries.command(&command).is_ok();
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
