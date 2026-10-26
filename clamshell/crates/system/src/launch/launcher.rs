//! Starting entries: games, apps, films. Each launch is started and then left alone. Whether it runs,
//! and when it has ended, comes from systemd's list of our scopes (see [`crate::launch::scopes`] and
//! clamshell-state's `running`). The launcher reports only a launch that couldn't start because its scope
//! never appeared. Each start runs on a thread of its own, so launches start side by side.
//!
//! - A program, with its arguments filled in by the library, runs in its own systemd scope (`systemd-run
//!   --user --scope`), so restarting ClamShell doesn't end it.
//! - A Steam game is handed to Steam and followed in its launch's scope. See [`crate::launch::steam`].
//!
//! Where mpv left a video is also read here when its play ends, at the state's request (`ReadPosition`).

use std::fmt;
use std::io;
use std::os::unix::process::CommandExt;
use std::path::PathBuf;
use std::process::{Command as Process, Stdio};
use std::sync::Arc;

use clamshell_state::{Event, Instance, Launch, LaunchEvent};
use rustix::process::{Pid, Signal};

use crate::Sink;
use crate::launch::job::{self, Job};
use crate::launch::{scopes, steam};
use crate::os::worker::{self, Worker};
use crate::os::{cgroup, sys};

/// What the launcher is asked.
pub enum Command {
    Launch(Request),
    /// Read where mpv left video `file` of `entry`, and send it as `LaunchEvent::Position`.
    ReadPosition {
        entry: Arc<str>,
        file: PathBuf,
    },
}

pub struct Request {
    pub entry: Arc<str>,
    /// The launch's scope, from the state.
    pub instance: Instance,
    pub launch: Launch,
    /// The game's environment: the cap's and its own variables.
    pub env: Vec<(String, String)>,
}

#[derive(Clone)]
pub struct Config {
    /// Where mpv keeps the positions of videos.
    pub watch_later: Vec<PathBuf>,
    /// Where the launches' control sockets go, which a command gets as `{socket}`.
    pub sockets: PathBuf,
}

#[derive(Debug)]
pub enum Error {
    Io(io::Error),
    Job(job::Error),
    Units(scopes::Error),
    NoCommand,
    EmptyCommand,
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::Io(e) => write!(f, "{e}"),
            Error::Job(e) => write!(f, "{e}"),
            Error::Units(e) => write!(f, "{e}"),
            Error::NoCommand => write!(f, "no launch command"),
            Error::EmptyCommand => write!(f, "empty launch command"),
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

impl From<scopes::Error> for Error {
    fn from(e: scopes::Error) -> Self {
        Error::Units(e)
    }
}

pub fn spawn(config: Config, sink: impl Sink + Clone) -> io::Result<Worker<Command>> {
    Worker::spawn("launcher", move |commands| {
        for command in commands {
            let (config, sink) = (config.clone(), sink.clone());
            let started = worker::detached("launch", move || match command {
                Command::Launch(request) => {
                    if let Err(e) = start(&config, &request) {
                        log::warn!("launcher: {}: {e}", request.entry);
                        let _ = sink.send(Event::Launch(LaunchEvent::LaunchFailed(request.entry.clone())));
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

/// Starts the game a request is for. It is done once the game's scope is there.
fn start(config: &Config, request: &Request) -> Result<(), Error> {
    if let Some(slice) = request.instance.slice() {
        scopes::thaw_slice(&slice);
    }
    match &request.launch {
        Launch::Program { argv, .. } if argv.is_empty() => Err(Error::EmptyCommand),
        Launch::Program { argv, workdir } => start_program(config, request, argv.clone(), workdir.clone()),
        Launch::Steam { appid } => steam::start(request, *appid),
        Launch::None | Launch::Tool(_) => Err(Error::NoCommand),
    }
}

/// Starts `argv` as the request's instance, in `workdir` or else the home dir.
fn start_program(config: &Config, request: &Request, argv: Vec<String>, workdir: Option<PathBuf>) -> Result<(), Error> {
    // `{socket}` is the entry's control socket. See clamshell_state::session::enqueue.
    let argv = match &request.instance {
        Instance::Run { key, .. } if argv.iter().any(|a| a.contains("{socket}")) => {
            std::fs::create_dir_all(&config.sockets)?;
            let socket = config.sockets.join(Instance::socket_of(key));
            argv.iter().map(|a| a.replace("{socket}", &socket.to_string_lossy())).collect()
        }
        _ => argv,
    };
    // `CLAMSHELL` is this program's path, for runner scripts that call it, such as the Android runner's
    // Wayland filter.
    let mut env = request.env.clone();
    match std::env::current_exe() {
        Ok(exe) => env.push(("CLAMSHELL".to_owned(), exe.to_string_lossy().into_owned())),
        Err(e) => log::warn!("launcher: finding this program's path: {e}"),
    }
    let job = Job { unit: request.instance.name(), slice: request.instance.slice(), argv, env, workdir };
    Ok(job::start(&job)?)
}

/// Ends an entry's slice with all its launches. Normally the slice is stopped: systemd asks its processes
/// to end and kills what is left after its timeout. With `force` they are killed at once. This runs on a
/// thread of its own, because stopping waits for the processes.
pub fn end_run(key: &str, force: bool) {
    let slice = Instance::slice_of(key);
    let ended = worker::detached("end", move || {
        log::info!("{slice}: {}", if force { "force kill" } else { "stop" });
        if let Err(e) = scopes::end(&slice, force) {
            log::warn!("{slice}: {e}");
        }
    });
    if let Err(e) = ended {
        log::warn!("end: {e}");
    }
}

/// Adopts a program not started by ClamShell, found by its tray icon. Its process and the processes it
/// started are moved into `instance`'s scope, in its entry's slice, on a thread of its own. A program run
/// by a systemd service is left alone, because the service would lose it, and so is one already in a
/// scope of ours.
pub fn adopt(instance: Instance, pid: u32) {
    let done = worker::detached("adopt", move || {
        let Some(unit) = instance.scope() else {
            return;
        };
        if !sys::same_process(pid) {
            log::info!("adopt: process {pid} is gone (another has its id)");
            return;
        }
        let group = cgroup::unit_of(pid).unwrap_or_default();
        if Instance::is_service(&group) || Instance::from_unit(&group).is_some() {
            log::info!("adopt: process {pid} stays in {group}");
            return;
        }
        let pids = with_descendants(&[pid], &parents());
        match scopes::adopt(&unit, instance.slice().as_deref(), &pids) {
            Ok(()) => log::info!("adopt: process {pid} from {group} in {unit} ({} processes)", pids.len()),
            Err(e) => log::warn!("adopt: process {pid}: {e}"),
        }
    });
    if let Err(e) = done {
        log::warn!("adopt: {e}");
    }
}

/// Asks the processes of a program not started by ClamShell to end (SIGTERM), or kills them.
pub fn end_processes(pids: &[u32], force: bool) {
    // Only signal processes that are still the ones seen, since a process that is gone may have left its
    // id to another.
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

/// Runs a runner's `stop`, `pause` or `resume` command without waiting for it. Waydroid's runner uses
/// these to have Android stop an app or freeze it.
pub fn runner_command(argv: &[String]) {
    let Some((program, args)) = argv.split_first() else {
        return;
    };
    log::info!("runner command: {}", argv.join(" "));
    match Process::new(program).args(args).stdin(Stdio::null()).process_group(0).spawn() {
        Ok(child) => job::reap(child),
        Err(e) => log::warn!("runner command: {program}: {e}"),
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
        Config { watch_later: Vec::new(), sockets: std::env::temp_dir() }
    }

    /// Runs only where there is a user manager to start the scope.
    #[test]
    fn runs_a_game_in_its_scope_and_reports_nothing_more() {
        if !std::env::var_os("XDG_RUNTIME_DIR").is_some_and(|d| std::path::Path::new(&d).join("systemd/private").exists()) {
            return;
        }
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path();
        let script = dir.join("game.sh");
        std::fs::write(&script, "#!/bin/sh\nsleep 1\npwd > ran-in\n").unwrap();
        std::fs::set_permissions(&script, std::os::unix::fs::PermissionsExt::from_mode(0o755)).unwrap();

        let (tx, events) = mpsc::channel();
        let launcher = spawn(config(), tx).unwrap();
        let id: Arc<str> = script.to_string_lossy().into();
        let launch = Launch::Program { argv: vec![script.to_string_lossy().into()], workdir: Some(dir.to_path_buf()) };
        let instance = Instance::Run { key: "test_1".into(), secs: u64::from(std::process::id()), n: 0 };
        launcher.send(Command::Launch(Request { entry: id, instance, launch, env: vec![] }));
        drop(launcher);

        // Its scope appeared, so nothing is sent. Whether it runs and ends is for systemd's list to say.
        let events: Vec<Event> = events.iter().collect();
        assert_eq!(events, vec![]);
        std::thread::sleep(Duration::from_secs(2));
        assert_eq!(std::fs::read_to_string(dir.join("ran-in")).unwrap().trim(), dir.to_string_lossy(), "in its own dir");
    }

    #[test]
    fn no_command_fails_cleanly() {
        let (tx, events) = mpsc::channel();
        let launcher = spawn(config(), tx).unwrap();
        let instance = Instance::Run { key: "test_1".into(), secs: u64::from(std::process::id()), n: 1 };
        launcher.send(Command::Launch(Request { entry: "x".into(), instance, launch: Launch::program(vec![]), env: vec![] }));
        drop(launcher);
        assert_eq!(events.iter().collect::<Vec<_>>(), vec![Event::Launch(LaunchEvent::LaunchFailed("x".into()))]);
    }
}

#[cfg(test)]
mod live {
    use super::*;
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
        let instance = Instance::Run { key: format!("adopttest_{id}"), secs: u64::from(id), n: 0 };
        adopt(instance.clone(), pid);
        let unit = instance.unit().unwrap();
        assert!(wait_for(|| with_descendants(&[pid], &parents()).iter().all(|p| group_of(*p) == unit)), "moved, the sleep too");
        let _ = Process::new("systemctl").args(["--user", "stop", &instance.slice().unwrap()]).status();
        let _ = child.wait();
        // A service's process stays.
        let service = format!("adopttest-{id}");
        Process::new("systemd-run").args(["--user", "--quiet", "--collect", &format!("--unit={service}"), "sleep", "30"]).status().unwrap();
        let pid: u32 = String::from_utf8(Process::new("systemctl").args(["--user", "show", "-p", "MainPID", "--value", &service]).output().unwrap().stdout)
            .unwrap()
            .trim()
            .parse()
            .unwrap();
        adopt(Instance::Run { key: format!("adopttest_{id}"), secs: u64::from(id), n: 1 }, pid);
        thread::sleep(Duration::from_secs(1));
        assert_eq!(group_of(pid), format!("{service}.service"));
        let _ = Process::new("systemctl").args(["--user", "stop", &service]).status();
    }
}
