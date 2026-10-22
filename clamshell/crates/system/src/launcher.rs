//! Starting entries (games, apps, films). Each launch is started and left: whether it runs, and when it
//! has ended, systemd's list of our scopes says (see [`crate::units`] and clamshell-core's `running`);
//! the launcher reports only a launch that couldn't start (its scope never came). Starting runs on a
//! thread of its own, so launches start side by side.
//!
//! - A program (its arguments filled in by the library) runs in its own systemd scope (`systemd-run
//!   --user --scope`, through sway's exec: its first window opens on its workspace), so restarting
//!   ClamShell doesn't end it.
//! - A Steam game is started with `steam -silent -applaunch <id>`: given to Steam if it runs; else Steam
//!   is started with it as a launch of Steam's desktop entry (its scope, so it is on the dock and outlives
//!   ClamShell), without the game's environment (Steam's own would be it, for every game it starts). The
//!   launch's own scope holds a follower (`clamshell --follow-steam <id>`, [`follow_steam`]): it moves
//!   Steam's launcher for the game (`reaper SteamLaunch AppId=<id>`) and what it started into its scope,
//!   and any later one (Steam may run a setup first, then the game), as long as one runs or did within
//!   [`STEAM_GAP`]; then it ends, and the scope with the game's last process: the game is counted,
//!   frozen and ended apart from Steam, as long as it runs. A game started in Steam itself is launched by
//!   the state when its window comes: found running, it is followed the same way, not started.
//!
//! Where mpv left a video is read here too, as its play ends (the state asks: `ReadPosition`).

use std::fmt;
use std::io;
use std::os::unix::process::CommandExt;
use std::path::PathBuf;
use std::process::{Command as Process, Stdio};
use std::sync::Arc;
use std::thread;
use std::time::{Duration, Instant};

use clamshell_core::{Event, Instance, Launch, LaunchEvent};
use rustix::process::{Pid, Signal};

use crate::job::{self, Job, Runner, Started};
use crate::worker::{self, Worker};
use crate::{Sink, cgroup, sys, units};

/// How long sway may take to start a game (its scope to appear).
const GAME_START: Duration = Duration::from_secs(30);
/// How long Steam may take to start a game (it may need to start or update first): the follower ends
/// if no launcher of Steam's for it comes by then.
const STEAM_START: Duration = Duration::from_secs(120);
/// How long a Steam game counts as running with no launcher of Steam's for it: Steam may run a setup
/// first (Proton's install scripts, in a fresh Steam), then the game, under launchers of their own.
const STEAM_GAP: Duration = Duration::from_secs(10);

/// What the launcher is asked.
pub enum Command {
    Launch(Request),
    /// Where mpv left video `file` (entry `game`'s): read, and told (`LaunchEvent::Position`).
    ReadPosition {
        game: Arc<str>,
        file: PathBuf,
    },
}

pub struct Request {
    pub game: Arc<str>,
    /// Its scope and workspace, from the state.
    pub instance: Instance,
    pub launch: Launch,
    /// The game's environment (its MangoHud config, its own variables).
    pub env: Vec<(String, String)>,
}

#[derive(Clone)]
pub struct Config {
    /// Where mpv keeps videos' places.
    pub watch_later: Vec<PathBuf>,
    /// How jobs start: under sway, through it (see [`crate::sway`]), so the game's window opens on the
    /// game's workspace.
    pub jobs: Runner,
    /// Where launches' control sockets go (a command's `{socket}`).
    pub sockets: PathBuf,
}

#[derive(Debug)]
pub enum Error {
    Io(io::Error),
    Job(job::Error),
    Units(units::Error),
    NoCommand,
    /// The instance has no scope to follow the game by (only apps' and games' instances have one).
    NoScope,
    EmptyCommand,
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::Io(e) => write!(f, "{e}"),
            Error::Job(e) => write!(f, "{e}"),
            Error::Units(e) => write!(f, "{e}"),
            Error::NoCommand => write!(f, "no launch command"),
            Error::NoScope => write!(f, "the instance has no scope"),
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

impl From<units::Error> for Error {
    fn from(e: units::Error) -> Self {
        Error::Units(e)
    }
}

pub fn spawn(config: Config, sink: impl Sink + Clone) -> io::Result<Worker<Command>> {
    Worker::spawn("launcher", move |commands| {
        for command in commands {
            let (config, sink) = (config.clone(), sink.clone());
            let started = thread::Builder::new().name("launch".into()).spawn(move || match command {
                Command::Launch(request) => {
                    if let Err(e) = start(&config, &request) {
                        log::warn!("launcher: {}: {e}", request.game);
                        let _ = sink.send(Event::Launch(LaunchEvent::LaunchFailed(request.game.clone())));
                    }
                }
                Command::ReadPosition { game, file } => {
                    let position = crate::mpv::position(&config.watch_later, &file).map(|(s, _)| s);
                    let _ = sink.send(Event::Launch(LaunchEvent::Position { game, position }));
                }
            });
            if let Err(e) = started {
                log::warn!("launcher: {e}");
            }
        }
    })
}

/// Starts the game a request is for: done once its scope is there.
fn start(config: &Config, request: &Request) -> Result<(), Error> {
    if let Some(slice) = request.instance.slice() {
        units::thaw_slice(&slice);
    }
    match &request.launch {
        Launch::Program { argv, .. } if argv.is_empty() => Err(Error::EmptyCommand),
        Launch::Program { argv, workdir } => start_program(config, request, argv.clone(), workdir.clone()),
        Launch::Steam { appid } => start_steam(config, request, *appid),
        Launch::None | Launch::Tool(_) => Err(Error::NoCommand),
    }
}

/// Starts `argv` in `workdir` (else the home dir) as the request's instance.
fn start_program(config: &Config, request: &Request, argv: Vec<String>, workdir: Option<PathBuf>) -> Result<(), Error> {
    // (`{socket}`: its entry's control socket, see clamshell_core::enqueue.)
    let argv = match &request.instance {
        Instance::Run { key, .. } if argv.iter().any(|a| a.contains("{socket}")) => {
            std::fs::create_dir_all(&config.sockets)?;
            let socket = config.sockets.join(Instance::socket_of(key));
            argv.iter().map(|a| a.replace("{socket}", &socket.to_string_lossy())).collect()
        }
        _ => argv,
    };
    // (`CLAMSHELL`: this program, for what a runner's script asks of it: the Android runner's Wayland
    // filter.)
    let mut env = request.env.clone();
    if let Ok(exe) = std::env::current_exe() {
        env.push(("CLAMSHELL".to_owned(), exe.to_string_lossy().into_owned()));
    }
    let job = Job { unit: request.instance.name(), workspace: request.instance.workspace(), slice: request.instance.slice(), argv, env, workdir };
    started(config.jobs.start(&job)?, request)
}

/// A job started for `request`: done once its scope is there (sway may not start it).
fn started(started: Started, request: &Request) -> Result<(), Error> {
    if let Started::Child(child) = started {
        job::reap(child);
    }
    let unit = request.instance.unit().ok_or(Error::NoScope)?;
    units::appear(&unit, GAME_START)?;
    Ok(())
}

/// A Steam game: asked of Steam (unless its launcher runs: started in Steam itself), and the launch's
/// scope started with its follower in it (see the module's doc).
fn start_steam(config: &Config, request: &Request, appid: u32) -> Result<(), Error> {
    if steam_running(&format!("AppId={appid}")) {
        log::info!("launcher: Steam's AppId={appid} runs: following it");
    } else {
        ask_steam(config, request, appid)?;
    }
    let follower = std::env::current_exe()?.to_string_lossy().into_owned();
    let argv = vec![follower, "--follow-steam".to_owned(), appid.to_string()];
    let job = Job { unit: request.instance.name(), workspace: request.instance.workspace(), slice: request.instance.slice(), argv, env: vec![], workdir: None };
    started(config.jobs.without_sway().start(&job)?, request)
}

/// `clamshell --follow-steam <id>`, in a Steam game's launch scope: Steam's launcher for the game, and
/// what it started, moved into this scope as long as one runs (or did within [`STEAM_GAP`]; the first
/// within [`STEAM_START`]); then it ends (see the module's doc).
pub fn follow_steam(appid: u32) {
    let marker = format!("AppId={appid}");
    let Some(unit) = cgroup::unit_of(std::process::id()).filter(|u| u.ends_with(".scope")) else {
        log::warn!("follow-steam: not in a scope of its own");
        return;
    };
    let mut seen = Instant::now();
    let mut gap = STEAM_START;
    loop {
        let launchers = steam_launchers(&marker);
        if !launchers.is_empty() {
            (seen, gap) = (Instant::now(), STEAM_GAP);
            gather(&marker, &unit, None, &launchers);
        } else if seen.elapsed() > gap {
            log::info!("follow-steam: no launcher of Steam's for {marker}: done");
            return;
        }
        thread::sleep(Duration::from_secs(2));
    }
}

/// Asks Steam to start a game: Steam started for it if it isn't running.
fn ask_steam(config: &Config, request: &Request, appid: u32) -> Result<(), Error> {
    let env = &request.env;
    // (`-silent`: if Steam isn't running, it starts without its window, only the game showing.)
    let argv: Vec<String> = ["steam", "-silent", "-applaunch", &appid.to_string()].map(String::from).to_vec();
    if !steam_client_running() {
        let instance = Instance::Run { key: clamshell_library::key_of(STEAM_ENTRY), secs: clamshell_core::unix_secs(), n: 0 };
        let job = Job { unit: instance.name(), workspace: instance.workspace(), slice: instance.slice(), argv, env: vec![], workdir: None };
        log::info!("launcher: Steam isn't running: starting it in {}", job.unit);
        if let Started::Child(child) = config.jobs.without_sway().start(&job)? {
            job::reap(child);
        }
    } else {
        let mut steam = Process::new("steam");
        steam.args(argv.iter().skip(1)).envs(env.iter().map(|(k, v)| (k, v))).stdin(Stdio::null()).process_group(0);
        job::reap(steam.spawn()?);
    }
    Ok(())
}

/// Steam's launchers for a game (`launchers`, `marker` in their command line) and what they started,
/// those not yet in the launch's scope `unit`: moved there.
fn gather(marker: &str, unit: &str, slice: Option<&str>, launchers: &[u32]) {
    let outside = |pid: &u32| cgroup::unit_of(*pid).is_none_or(|u| u != unit);
    let pids: Vec<u32> = with_descendants(launchers, &parents()).into_iter().filter(outside).collect();
    if pids.is_empty() {
        return;
    }
    match crate::units::adopt(unit, slice, &pids) {
        Ok(()) => log::info!("launcher: Steam's {marker} in {unit} ({} processes)", pids.len()),
        Err(e) => log::warn!("launcher: Steam's {marker}: its own scope: {e}"),
    }
}

/// Steam's desktop entry, whose launches Steam started for a game is one of.
const STEAM_ENTRY: &str = "desktop:steam";

/// Whether Steam's client (or its starter script) runs.
fn steam_client_running() -> bool {
    sys::processes().any(|(_, dir)| sys::text(dir.join("comm")).is_some_and(|c| c.trim() == "steam"))
}

/// Whether Steam's launcher for an app (`... SteamLaunch AppId=<id> ...`) is running.
fn steam_running(marker: &str) -> bool {
    !steam_launchers(marker).is_empty()
}

/// The processes of Steam's launcher for an app (`... SteamLaunch AppId=<id> ...`: `marker`), which the
/// game's processes descend from.
fn steam_launchers(marker: &str) -> Vec<u32> {
    sys::processes()
        .filter(|(_, dir)| {
            std::fs::read(dir.join("cmdline")).is_ok_and(|c| {
                let mut args = c.split(|&b| b == 0);
                args.any(|a| a == b"SteamLaunch") && c.split(|&b| b == 0).any(|a| a == marker.as_bytes())
            })
        })
        .map(|(pid, _)| pid)
        .collect()
}

/// Ends an entry's slice, all its launches: stopped (systemd asks its processes to end, then kills what
/// is left after its timeout), or with `force` killed at once. On a thread of its own (stopping waits
/// for them).
pub fn end_run(key: &str, force: bool) {
    let slice = Instance::slice_of(key);
    let ended = worker::detached("end", move || {
        log::info!("{slice}: {}", if force { "force kill" } else { "stop" });
        if let Err(e) = crate::units::end(&slice, force) {
            log::warn!("{slice}: {e}");
        }
    });
    if let Err(e) = ended {
        log::warn!("end: {e}");
    }
}

/// A program not started by ClamShell (found by its tray icon): its process and those it started moved
/// into `instance`'s scope (in its entry's slice), on a thread of its own. Not one a systemd service runs
/// (moved out, the service would lose it), nor one in a scope of ours already.
pub fn adopt(instance: Instance, pid: u32) {
    let done = worker::detached("adopt", move || {
        let Some(unit) = instance.unit() else {
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
        match units::adopt(&unit, instance.slice().as_deref(), &pids) {
            Ok(()) => log::info!("adopt: process {pid} from {group} in {unit} ({} processes)", pids.len()),
            Err(e) => log::warn!("adopt: process {pid}: {e}"),
        }
    });
    if let Err(e) = done {
        log::warn!("adopt: {e}");
    }
}

/// Processes asked to end (SIGTERM), or killed: a program not started by ClamShell.
pub fn end_processes(pids: &[u32], force: bool) {
    // (Only those still the processes seen: a process gone may have left its id to another.)
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

/// Runs a runner's command (its `stop`, `pause` or `resume`: Waydroid's, Android stopping an app or
/// frozen), not waited for.
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
fn parents() -> Vec<(u32, u32)> {
    sys::processes().filter_map(|(pid, dir)| Some((pid, sys::parent_in(&sys::text(dir.join("stat"))?)?))).collect()
}

/// `pids` and all their descendants, by `parents` (pid, parent).
fn with_descendants(pids: &[u32], parents: &[(u32, u32)]) -> Vec<u32> {
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

    fn config() -> Config {
        Config { watch_later: Vec::new(), jobs: Runner { jobs: std::env::temp_dir(), sway: None }, sockets: std::env::temp_dir() }
    }

    /// (Only where there is a user manager to start its scope.)
    #[test]
    fn runs_a_game_in_its_scope_and_reports_nothing_more() {
        if !std::env::var_os("XDG_RUNTIME_DIR").is_some_and(|d| std::path::Path::new(&d).join("systemd/private").exists()) {
            return;
        }
        let dir = std::env::temp_dir().join(format!("clamshell-launch-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let script = dir.join("game.sh");
        std::fs::write(&script, "#!/bin/sh\nsleep 1\npwd > ran-in\n").unwrap();
        std::fs::set_permissions(&script, std::os::unix::fs::PermissionsExt::from_mode(0o755)).unwrap();

        let (tx, events) = mpsc::channel();
        let launcher = spawn(config(), tx).unwrap();
        let id: Arc<str> = script.to_string_lossy().into();
        let launch = Launch::Program { argv: vec![script.to_string_lossy().into()], workdir: Some(dir.clone()) };
        let instance = Instance::Run { key: "test_1".into(), secs: u64::from(std::process::id()), n: 0 };
        launcher.send(Command::Launch(Request { game: id, instance, launch, env: vec![] }));
        drop(launcher);

        // (Its scope came: nothing told; that it runs and ends is systemd's list's.)
        let events: Vec<Event> = events.iter().collect();
        assert_eq!(events, vec![]);
        std::thread::sleep(Duration::from_secs(2));
        assert_eq!(std::fs::read_to_string(dir.join("ran-in")).unwrap().trim(), dir.to_string_lossy(), "in its own dir");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn no_command_fails_cleanly() {
        let (tx, events) = mpsc::channel();
        let launcher = spawn(config(), tx).unwrap();
        let instance = Instance::Run { key: "test_1".into(), secs: u64::from(std::process::id()), n: 1 };
        launcher.send(Command::Launch(Request { game: "x".into(), instance, launch: Launch::program(vec![]), env: vec![] }));
        drop(launcher);
        assert_eq!(events.iter().collect::<Vec<_>>(), vec![Event::Launch(LaunchEvent::LaunchFailed("x".into()))]);
    }
}

#[cfg(test)]
mod live {
    use super::*;

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

    /// A program in a scope not ours (a desktop's app scope) is moved into one of ours, with what it
    /// started; one a service runs stays (run by hand: `--ignored`).
    #[test]
    #[ignore]
    #[allow(clippy::unwrap_used)]
    fn a_program_is_adopted_unless_a_service_runs_it() {
        let id = std::process::id();
        let mut child = Process::new("systemd-run")
            .args(["--user", "--scope", "--quiet", "--collect", &format!("--unit=app-adopttest-{id}"), "sh", "-c", "sleep 30 & wait"])
            .spawn()
            .unwrap();
        let pid = child.id();
        // (The scope's process: systemd-run runs the shell as itself.)
        assert!(wait_for(|| group_of(pid).starts_with("app-adopttest")), "{}", group_of(pid));
        assert!(wait_for(|| with_descendants(&[pid], &parents()).len() == 2), "its sleep");
        let instance = Instance::Run { key: format!("adopttest_{id}"), secs: u64::from(id), n: 0 };
        adopt(instance.clone(), pid);
        let unit = instance.unit().unwrap();
        assert!(wait_for(|| with_descendants(&[pid], &parents()).iter().all(|p| group_of(*p) == unit)), "moved, the sleep too");
        let _ = Process::new("systemctl").args(["--user", "stop", &instance.slice().unwrap()]).status();
        let _ = child.wait();
        // A service's: stays.
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
