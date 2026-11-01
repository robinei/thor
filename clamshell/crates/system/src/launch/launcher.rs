//! Starting entries: games, apps, films. Each launch is started and then left alone. Whether it runs,
//! and when it has ended, comes from systemd's list of our units (see [`crate::launch::units`] and
//! clamshell-state's `running`). The launcher reports only a launch that systemd didn't start. Each start runs on a thread of its own, so launches start side by side.
//!
//! - A program, with its arguments filled in by the library, runs as its own systemd service
//!   ([`crate::launch::job`]), so restarting ClamShell doesn't end it.
//! - A Steam game is handed to Steam and followed in its launch's service. See [`crate::launch::steam`].
//!
//! Where mpv left a video is also read here when its play ends, at the state's request (`ReadPosition`).

use std::fmt;
use std::io;
use std::path::PathBuf;
use std::sync::Arc;

use clamshell_state::{Event, Launch, LaunchEvent, LaunchUnit};
use rustix::process::{Pid, Signal};

use crate::Sink;
use crate::launch::job::{self, Job};
use crate::launch::{steam, units};
use crate::os::worker::{self, Worker};
use crate::os::{cgroup, sys};

/// What the launcher is asked.
pub(crate) enum Command {
    Launch(Request),
    /// Read where mpv left video `file` of `entry`, and send it as `LaunchEvent::Position`.
    ReadPosition {
        entry: Arc<str>,
        file: PathBuf,
    },
}

pub(crate) struct Request {
    pub(crate) entry: Arc<str>,
    /// The launch's unit, from the state.
    pub(crate) unit: LaunchUnit,
    pub(crate) launch: Launch,
    /// The Env tile's variables, expanded on the launch thread with settings and displays.
    pub(crate) env: Vec<(String, String)>,
    pub(crate) settings: clamshell_state::settings::Values,
    pub(crate) ignore: Option<String>,
}

#[derive(Clone)]
pub(crate) struct Config {
    /// Where mpv keeps the positions of videos.
    pub(crate) watch_later: Vec<PathBuf>,
    pub(crate) fex: Option<PathBuf>,
    /// Where the launches' control sockets go, which a command gets as `{socket}`.
    pub(crate) sockets: Option<PathBuf>,
    /// The compositor's displays (`WAYLAND_DISPLAY`, `DISPLAY`), given to every launch after its own
    /// variables.
    pub displays: Vec<(String, String)>,
    /// HOME, where a launch that names no working directory runs.
    pub home: Option<PathBuf>,
}

#[derive(Debug)]
pub(crate) enum Error {
    Io(io::Error),
    Job(job::Error),
    Units(units::Error),
    NoCommand,
    EmptyCommand,
    Panicked,
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::Io(e) => write!(f, "{e}"),
            Error::Job(e) => write!(f, "{e}"),
            Error::Units(e) => write!(f, "{e}"),
            Error::NoCommand => write!(f, "no launch command"),
            Error::EmptyCommand => write!(f, "empty launch command"),
            Error::Panicked => write!(f, "starting it panicked"),
        }
    }
}

impl From<io::Error> for Error {
    fn from(e: io::Error) -> Self {
        Error::Io(e)
    }
}

impl From<job::Error> for Error {
    fn from(e: job::Error) -> Self {
        Error::Job(e)
    }
}

impl From<units::Error> for Error {
    fn from(e: units::Error) -> Self {
        Error::Units(e)
    }
}

pub(crate) fn spawn(config: Config, sink: impl Sink + Clone) -> io::Result<Worker<Command>> {
    Worker::spawn(clamshell_state::Thread::Launcher.name(), move |commands| {
        for command in commands {
            let (config, sink) = (config.clone(), sink.clone());
            let started = worker::detached("launch", move || match command {
                Command::Launch(mut request) => {
                    prepare_environment(&config, &mut request, std::path::Path::new("/"));
                    if let Err(e) = worker::or_on_panic(|| start(&config, &request), || Err(Error::Panicked)) {
                        log::warn!("launcher: {}: {e}", request.entry);
                        let failed = LaunchEvent::LaunchFailed { entry: request.entry.clone(), unit: request.unit.clone() };
                        let _ = sink.send(Event::Launch(failed));
                    }
                }
                Command::ReadPosition { entry, file } => {
                    let position = crate::launch::mpv::position(&config.watch_later, &file).map(|(s, _)| s);
                    let _ = sink.send(Event::Launch(LaunchEvent::Position { entry, position }));
                }
            });
            if let Err(e) = started {
                log::warn!("launcher: {e}");
            }
        }
    })
}

/// Resolve package files and build the environment on the launch thread, so the compositor never
/// walks the package database and an uninstalled alternative cannot leave a stale loader path.
fn prepare_environment(config: &Config, request: &mut Request, root: &std::path::Path) {
    let tile = std::mem::take(&mut request.env);
    let facts = crate::components::launch_facts_at(root, &request.settings, config.fex.as_deref(), request.ignore.as_deref());
    clamshell_state::settings::launch_env(&request.settings, facts, &tile, &mut request.env);
    request.env.extend(config.displays.iter().cloned());
}

/// Starts the game a request is for. It is done once systemd has started the game's service.
fn start(config: &Config, request: &Request) -> Result<(), Error> {
    units::thaw_slice(&request.unit.slice());
    match &request.launch {
        Launch::Program { argv, .. } if argv.is_empty() => Err(Error::EmptyCommand),
        Launch::Program { argv, workdir } => start_program(config, request, argv.clone(), workdir.clone()),
        Launch::Steam { appid } => steam::start(request, *appid),
        Launch::None | Launch::Tool(_) | Launch::Builtin(_) => Err(Error::NoCommand),
    }
}

/// Starts `argv` as the request's unit, in `workdir` or else the home dir.
fn start_program(config: &Config, request: &Request, argv: Vec<String>, workdir: Option<PathBuf>) -> Result<(), Error> {
    // `{socket}` is the entry's control socket. See clamshell_state::session::enqueue.
    let argv = if argv.iter().any(|a| a.contains("{socket}")) {
        let sockets = config.sockets.as_ref().ok_or_else(|| io::Error::other("no XDG_RUNTIME_DIR for its control socket"))?;
        std::fs::create_dir_all(sockets)?;
        let socket = sockets.join(LaunchUnit::socket_of(&request.unit.key));
        argv.iter().map(|a| a.replace("{socket}", &socket.to_string_lossy())).collect()
    } else {
        argv
    };
    // `CLAMSHELL` is this program's path, for runner scripts that call it, such as the Android runner's
    // Wayland filter.
    let mut env = request.env.clone();
    match std::env::current_exe() {
        Ok(exe) => env.push(("CLAMSHELL".to_owned(), exe.to_string_lossy().into_owned())),
        Err(e) => log::warn!("launcher: finding this program's path: {e}"),
    }
    let job = Job {
        unit: request.unit.name(),
        slice: Some(request.unit.slice()),
        argv: argv.into_iter().map(Into::into).collect(),
        env,
        workdir: workdir.or_else(|| config.home.clone()),
        properties: vec![],
    };
    Ok(job::start(&job)?)
}

/// What is done to running programs, by the processes thread (see [`spawn_processes`]).
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum ToProcesses {
    /// End an entry's slice ([`end_run`]).
    EndRun { key: String, force: bool },
    /// Run a runner's `stop`, `pause` or `resume` command ([`runner_command`]).
    Runner(Vec<String>),
    /// Move a program found by its tray icon into `unit` ([`adopt`]).
    Adopt(LaunchUnit, u32),
    /// Ask processes not started by ClamShell to end, or kill them ([`end_processes`]).
    End { pids: Vec<u32>, force: bool },
    /// Give mpv's control socket `socket` these lines ([`crate::launch::mpv::enqueue`]).
    Enqueue { socket: PathBuf, lines: Vec<String> },
}

/// Starts the processes thread, which carries out what is done to running programs one after another,
/// in the order the state asked: a runner's pause and resume, or Waydroid's freeze and thaw, end as the
/// state last asked, and a program is adopted before it is ended. Each may wait (a stop job, a runner's
/// script, a player slow to read), which only holds up those behind it.
pub(crate) fn spawn_processes() -> io::Result<Worker<ToProcesses>> {
    spawn_processes_with(carry_out)
}

fn spawn_processes_with(mut carry_out: impl FnMut(ToProcesses) + Send + 'static) -> io::Result<Worker<ToProcesses>> {
    Worker::spawn(clamshell_state::Thread::Processes.name(), move |commands| {
        for command in commands {
            carry_out(command);
        }
    })
}

fn carry_out(command: ToProcesses) {
    match command {
        ToProcesses::EndRun { key, force } => end_run(&key, force),
        ToProcesses::Runner(argv) => runner_command(&argv),
        ToProcesses::Adopt(unit, pid) => adopt(&unit, pid),
        ToProcesses::End { pids, force } => end_processes(&pids, force),
        ToProcesses::Enqueue { socket, lines } => {
            if let Err(e) = crate::launch::mpv::enqueue(&socket, &lines) {
                log::warn!("enqueue to {}: {e}", socket.display());
            }
        }
    }
}

/// Ends an entry's slice with all its launches. Normally the slice is stopped: systemd asks its processes
/// to end and kills what is left after its timeout. With `force` they are killed at once. Either is a job
/// systemd carries out after answering.
fn end_run(key: &str, force: bool) {
    let slice = LaunchUnit::slice_of(key);
    log::info!("{slice}: {}", if force { "force kill" } else { "stop" });
    if let Err(e) = units::end(&slice, force) {
        log::warn!("{slice}: {e}");
    }
}

/// Adopts a program not started by ClamShell, found by its tray icon. Its process and the processes it
/// started are moved into `unit`'s scope, in its entry's slice. A program run by a systemd service is left
/// alone, because the service would lose it, and so is one already in a scope of ours.
fn adopt(unit: &LaunchUnit, pid: u32) {
    let scope = unit.scope();
    if !sys::same_process(pid) {
        log::info!("adopt: process {pid} is gone (another has its id)");
        return;
    }
    let group = cgroup::unit_of(pid).unwrap_or_default();
    if LaunchUnit::is_service(&group) || LaunchUnit::from_unit(&group).is_some() {
        log::info!("adopt: process {pid} stays in {group}");
        return;
    }
    let pids = with_descendants(&[pid], &parents());
    match units::adopt(&scope, Some(unit.slice().as_str()), &pids) {
        Ok(()) => log::info!("adopt: process {pid} from {group} in {scope} ({} processes)", pids.len()),
        Err(e) => log::warn!("adopt: process {pid}: {e}"),
    }
}

/// Asks the processes of a program not started by ClamShell to end (SIGTERM), or kills them. Only
/// processes that are still the ones seen are signalled, since a process that is gone may have left its
/// id to another.
fn end_processes(pids: &[u32], force: bool) {
    let pids: Vec<u32> = pids.iter().copied().filter(|&p| sys::same_process(p)).collect();
    if pids.is_empty() {
        return;
    }
    log::info!("{} processes {pids:?}", if force { "killing" } else { "ending" });
    let signal = if force { Signal::KILL } else { Signal::TERM };
    for pid in pids {
        let Some(pid) = i32::try_from(pid).ok().and_then(Pid::from_raw) else { continue };
        if let Err(e) = rustix::process::kill_process(pid, signal) {
            log::warn!("signalling process {pid:?}: {e}");
        }
    }
}

/// How long a runner's command may take.
const RUNNER_LIMIT: std::time::Duration = std::time::Duration::from_secs(20);

/// Runs a runner's `stop`, `pause` or `resume` command, for at most [`RUNNER_LIMIT`]. Waydroid's runner
/// uses these to have Android stop an app or freeze it.
fn runner_command(argv: &[String]) {
    let Some((program, args)) = argv.split_first() else {
        return;
    };
    log::info!("runner command: {}", argv.join(" "));
    match crate::os::command::run(program, args, None, RUNNER_LIMIT) {
        Ok(ran) if ran.status.success() => {}
        Ok(ran) => log::warn!("runner command: {}: {} {}", argv.join(" "), ran.status, ran.said()),
        Err(e) => log::warn!("runner command: {e}"),
    }
}

/// Every process's parent.
pub(super) fn parents() -> Vec<(u32, u32)> {
    sys::processes().filter_map(|(pid, dir)| Some((pid, sys::parent_in(&sys::text(dir.join("stat"))?)?))).collect()
}

/// `pids` and all their descendants, found through `parents` as (pid, parent) pairs.
pub(super) fn with_descendants(pids: &[u32], parents: &[(u32, u32)]) -> Vec<u32> {
    let mut out: Vec<u32> = pids.to_vec();
    let mut at = 0;
    while let Some(&pid) = out.get(at) {
        out.extend(parents.iter().filter(|(child, parent)| *parent == pid && !out.contains(child)).map(|(child, _)| *child).collect::<Vec<_>>());
        at += 1;
    }
    out
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    /// What is done to programs is done in the order asked, one at a time, whatever each takes: a
    /// runner's pause and then resume leave it resumed.
    #[test]
    fn what_is_done_to_programs_is_done_in_the_order_asked() {
        let (done, order) = std::sync::mpsc::channel();
        let processes = super::spawn_processes_with(move |c| {
            if c == super::ToProcesses::Runner(vec!["pause".into()]) {
                std::thread::sleep(std::time::Duration::from_millis(100));
            }
            done.send(c).unwrap();
        })
        .unwrap();
        let asked = [
            super::ToProcesses::Runner(vec!["pause".into()]),
            super::ToProcesses::Runner(vec!["resume".into()]),
            super::ToProcesses::EndRun { key: "k".into(), force: false },
        ];
        for c in asked.clone() {
            processes.send(c);
        }
        let got: Vec<_> = (0..asked.len()).map(|_| order.recv_timeout(std::time::Duration::from_secs(2)).unwrap()).collect();
        assert_eq!(got, asked);
    }

    #[test]
    fn a_process_tree_is_its_pids_and_their_descendants() {
        let parents = [(10, 1), (11, 10), (12, 11), (13, 10), (20, 1), (21, 20)];
        let mut tree = super::with_descendants(&[10], &parents);
        tree.sort_unstable();
        assert_eq!(tree, vec![10, 11, 12, 13], "not 20's");
    }

    use super::*;
    use std::sync::mpsc;
    use std::time::Duration;

    fn config() -> Config {
        Config { watch_later: Vec::new(), sockets: Some(std::env::temp_dir()), displays: Vec::new(), home: None, fex: None }
    }

    /// Starts a service on this machine's user manager. Run it by hand with `--ignored`.
    #[test]
    #[ignore = "needs a user manager; starts a service"]
    fn runs_a_game_as_its_service_and_reports_nothing_more() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path();
        let script = dir.join("game.sh");
        std::fs::write(&script, "#!/bin/sh\nsleep 1\npwd > ran-in\n").unwrap();
        std::fs::set_permissions(&script, std::os::unix::fs::PermissionsExt::from_mode(0o755)).unwrap();

        let (tx, events) = mpsc::channel();
        let launcher = spawn(config(), tx).unwrap();
        let id: Arc<str> = script.to_string_lossy().into();
        let launch = Launch::Program { argv: vec![script.to_string_lossy().into()], workdir: Some(dir.to_path_buf()) };
        let unit = LaunchUnit { key: "test_1".into(), secs: u64::from(std::process::id()), n: 0 };
        launcher.send(Command::Launch(Request { entry: id, unit, launch, env: vec![], settings: Default::default(), ignore: None }));
        drop(launcher);

        // systemd started it, so nothing is sent. Whether it runs and ends is for systemd's list to say.
        let events: Vec<Event> = events.iter().collect();
        assert_eq!(events, vec![]);
        let ran = dir.join("ran-in");
        let started = std::time::Instant::now();
        while !ran.exists() && started.elapsed() < Duration::from_secs(10) {
            std::thread::sleep(Duration::from_millis(50));
        }
        assert_eq!(std::fs::read_to_string(ran).unwrap().trim(), dir.to_string_lossy(), "in its own dir");
    }

    #[test]
    fn no_command_fails_cleanly() {
        let (tx, events) = mpsc::channel();
        let launcher = spawn(config(), tx).unwrap();
        let unit = LaunchUnit { key: "test_1".into(), secs: u64::from(std::process::id()), n: 1 };
        launcher.send(Command::Launch(Request {
            entry: "x".into(),
            unit: unit.clone(),
            launch: Launch::program(vec![]),
            env: vec![],
            settings: Default::default(),
            ignore: None,
        }));
        drop(launcher);
        assert_eq!(events.iter().collect::<Vec<_>>(), vec![Event::Launch(LaunchEvent::LaunchFailed { entry: "x".into(), unit })]);
    }
}

#[cfg(test)]
mod live {
    use super::*;
    use std::process::Command as Process;
    use std::thread;
    use std::time::Duration;
    use std::time::Instant;

    fn group_of(pid: u32) -> String {
        cgroup::unit_of(pid).unwrap_or_default()
    }

    fn wait_for(what: impl Fn() -> bool) -> bool {
        let start = Instant::now();
        while !what() {
            if start.elapsed() > Duration::from_secs(5) {
                return false;
            }
            thread::sleep(Duration::from_millis(100));
        }
        true
    }

    /// A program in a scope that isn't ours, such as a desktop's app scope, is moved into one of ours
    /// together with what it started. A program run by a service stays. Run it by hand with `--ignored`.
    #[test]
    #[ignore = "needs a user session; moves processes"]
    #[allow(clippy::unwrap_used)]
    fn a_program_is_adopted_unless_a_service_runs_it() {
        let id = std::process::id();
        let mut child = Process::new("systemd-run")
            .args(["--user", "--scope", "--quiet", "--collect", &format!("--unit=app-adopttest-{id}"), "sh", "-c", "sleep 30 & wait"])
            .spawn()
            .unwrap();
        let pid = child.id();
        // This is the scope's process, since systemd-run runs the shell as itself.
        assert!(wait_for(|| group_of(pid).starts_with("app-adopttest")), "{}", group_of(pid));
        assert!(wait_for(|| with_descendants(&[pid], &parents()).len() == 2), "its sleep");
        let unit = LaunchUnit { key: format!("adopttest_{id}"), secs: u64::from(id), n: 0 };
        adopt(&unit, pid);
        let scope = unit.scope();
        assert!(wait_for(|| with_descendants(&[pid], &parents()).iter().all(|p| group_of(*p) == scope)), "moved, the sleep too");
        let _ = Process::new("systemctl").args(["--user", "stop", &unit.slice()]).status();
        let _ = child.wait();
        // A service's process stays.
        let service = format!("adopttest-{id}");
        Process::new("systemd-run").args(["--user", "--quiet", "--collect", &format!("--unit={service}"), "sleep", "30"]).status().unwrap();
        let pid: u32 = String::from_utf8(Process::new("systemctl").args(["--user", "show", "-p", "MainPID", "--value", &service]).output().unwrap().stdout)
            .unwrap()
            .trim()
            .parse()
            .unwrap();
        adopt(&LaunchUnit { key: format!("adopttest_{id}"), secs: u64::from(id), n: 1 }, pid);
        thread::sleep(Duration::from_secs(1));
        assert_eq!(group_of(pid), format!("{service}.service"));
        let _ = Process::new("systemctl").args(["--user", "stop", &service]).status();
    }
}

#[cfg(test)]
mod environment_tests {
    use super::*;
    #[test]
    fn the_launch_thread_keeps_setting_then_user_then_display_order() {
        use clamshell_state::settings::{Setting, Values, X86};
        let mut values = Values::default();
        values.set(Setting::X86, X86::Fast as u16);
        values.set(Setting::FpsCap, 30);
        let config = Config {
            fex: Some("/cache/fex".into()),
            watch_later: Vec::new(),
            sockets: None,
            home: None,
            displays: vec![("WAYLAND_DISPLAY".into(), "private".into())],
        };
        let mut request = Request {
            entry: "test".into(),
            unit: LaunchUnit { key: "test".into(), secs: 1, n: 0 },
            launch: Launch::program(vec!["game".into()]),
            settings: values,
            ignore: Some("pad".into()),
            env: vec![("vblank_mode".into(), "0".into()), ("WAYLAND_DISPLAY".into(), "user".into())],
        };
        prepare_environment(&config, &mut request, std::path::Path::new("/unused-for-system-driver"));
        assert_eq!(
            request.env,
            vec![
                ("MESA_VK_WSI_PRESENT_MODE".into(), "fifo".into()),
                ("vblank_mode".into(), "3".into()),
                ("FEX_APP_CONFIG".into(), "/cache/fex/fast.json".into()),
                ("SDL_GAMECONTROLLER_IGNORE_DEVICES".into(), "pad".into()),
                ("vblank_mode".into(), "0".into()),
                ("WAYLAND_DISPLAY".into(), "user".into()),
                ("WAYLAND_DISPLAY".into(), "private".into())
            ]
        );
    }
}
