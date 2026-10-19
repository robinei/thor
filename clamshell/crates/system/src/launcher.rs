//! Starting entries (games, apps, films) and knowing when they end. Each launch runs on a thread of its
//! own, so they run side by side.
//!
//! - A program (its arguments filled in by the library) runs in its own systemd scope (`systemd-run
//!   --user --scope`), so restarting ClamShell doesn't end it; the thread waits for it.
//! - A Steam game is started with `steam -silent -applaunch <id>`: given to Steam if it runs; else Steam
//!   is started with it as a launch of Steam's desktop entry (its scope, so it is on the dock and outlives
//!   ClamShell; ClamShell finds it by its scope), without the game's environment (Steam's own would be
//!   it, for every game it starts). Once Steam's launcher for it runs, it and what it started are moved
//!   into the launch's own scope (as any launch's, in the entry's slice), and so is any later one for it
//!   (Steam may run a setup first, then the game): the game is counted, frozen and ended apart from
//!   Steam; it counts as running while Steam's launcher process for that app id (`reaper SteamLaunch
//!   AppId=<id>`) exists, or did within [`STEAM_GAP`]. A game started in Steam itself is launched by the
//!   state when its window comes: found running, it is followed the same way, not started.
//!
//! Each launch's start, its end (or that it couldn't start) is reported; whether its entry still runs, and
//! its play, are the state's (by the entry's scopes too: see `clamshell_core::running`).

use std::fmt;
use std::io;
use std::os::unix::process::CommandExt;
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::sync::Arc;
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use clamshell_core::{Event, GameEvent, Instance, Launch};

use crate::job::{self, Job, Runner, Started};
use crate::worker::{self, Worker};
use crate::{Closed, Sink, command, sys, units};

/// How long sway may take to start a game (its scope to appear).
const GAME_START: Duration = Duration::from_secs(30);
/// How long Steam may take to start a game (it may need to start or update first).
const STEAM_START: Duration = Duration::from_secs(120);
/// How long a Steam game counts as running with no launcher of Steam's for it: Steam may run a setup
/// first (Proton's install scripts, in a fresh Steam), then the game, under launchers of their own.
const STEAM_GAP: Duration = Duration::from_secs(10);

pub struct Request {
    pub game: Arc<str>,
    /// Its scope and workspace, from the state.
    pub instance: Instance,
    pub launch: Launch,
    /// A video mpv plays: its file, whose place mpv keeps (read again when it ends).
    pub watch: Option<PathBuf>,
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
    SteamDidNotStart,
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
            Error::SteamDidNotStart => write!(f, "Steam didn't start it"),
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

fn now() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)
}

pub fn spawn(config: Config, sink: impl Sink + Clone) -> io::Result<Worker<Request>> {
    Worker::spawn("launcher", move |requests| {
        let mut games: Vec<JoinHandle<()>> = Vec::new();
        for request in requests {
            let (config, sink) = (config.clone(), sink.clone());
            match thread::Builder::new().name("game".into()).spawn(move || {
                let _ = play(&config, &request, &sink);
            }) {
                Ok(handle) => games.push(handle),
                Err(e) => log::warn!("launcher: {e}"),
            }
            games.retain(|g| !g.is_finished());
        }
        // No more requests: the games still running end on their own; their reports still go out (to
        // no one, ClamShell ending: a ClamShell after it finds them by their scopes).
        for game in games {
            let _ = game.join();
        }
    })
}

/// A game that has started.
enum Running {
    /// Started by sway: it runs while its scope does.
    Scope(String),
    Child(Child),
    /// Steam's launcher for it runs (this marker in its command line); the launch's own scope and slice
    /// its processes are moved to.
    Steam {
        marker: String,
        unit: String,
        slice: Option<String>,
    },
}

impl Running {
    /// Waits for the game to end.
    fn wait(self) -> Result<(), Error> {
        match self {
            Running::Scope(unit) => Ok(units::wait(&unit)?),
            Running::Child(mut child) => child.wait().map(drop).map_err(Error::from),
            Running::Steam { marker, unit, slice } => {
                let mut seen = Instant::now();
                loop {
                    let launchers = steam_launchers(&marker);
                    if !launchers.is_empty() {
                        seen = Instant::now();
                        gather(&marker, &unit, slice.as_deref(), &launchers);
                    } else if seen.elapsed() > STEAM_GAP {
                        return Ok(());
                    }
                    thread::sleep(Duration::from_secs(2));
                }
            }
        }
    }
}

/// Starts one game, waits until it ends, and reports that (or that it couldn't start: before it is
/// reported started, so it is one or the other); ends early if the UI is gone.
fn play(config: &Config, request: &Request, sink: &impl Sink) -> Result<(), Closed> {
    let game = &request.game;
    let failed = |e: Error| {
        log::warn!("launcher: {game}: {e}");
        GameEvent::LaunchFailed(game.clone())
    };
    let event = match start(config, request) {
        Err(e) => failed(e),
        Ok(running) => {
            let started = Instant::now();
            sink.send(GameEvent::Started(game.clone()).into())?;
            match running.wait() {
                // (Not followed to its end: ended as far as the launcher goes; its scope, if any, says.)
                Err(e) => {
                    log::warn!("launcher: {game}: {e}");
                    GameEvent::Ended(game.clone())
                }
                Ok(()) => {
                    log::info!("launcher: {game} ran {} s", started.elapsed().as_secs());
                    if let Some(file) = &request.watch {
                        let position = crate::mpv::position(&config.watch_later, file).map(|(s, _)| s);
                        sink.send(Event::Game(GameEvent::Position { game: game.clone(), position }))?;
                    }
                    GameEvent::Ended(game.clone())
                }
            }
        }
    };
    sink.send(Event::Game(event))
}

/// Starts the game a request is for.
fn start(config: &Config, request: &Request) -> Result<Running, Error> {
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
fn start_program(config: &Config, request: &Request, argv: Vec<String>, workdir: Option<PathBuf>) -> Result<Running, Error> {
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
    Ok(match config.jobs.start(&job)? {
        Started::Sway => {
            // (Started once its scope is there: sway may not start it.)
            let unit = request.instance.unit().ok_or(Error::NoScope)?;
            units::appear(&unit, GAME_START)?;
            Running::Scope(unit)
        }
        Started::Child(child) => Running::Child(child),
    })
}

fn start_steam(config: &Config, request: &Request, appid: u32) -> Result<Running, Error> {
    let marker = format!("AppId={appid}");
    // Started in Steam (found by its window): followed as it is.
    if steam_running(&marker) {
        log::info!("launcher: Steam's {marker} runs: following it");
    } else {
        ask_steam(config, request, appid)?;
        let asked = Instant::now();
        while !steam_running(&marker) {
            if asked.elapsed() > STEAM_START {
                return Err(Error::SteamDidNotStart);
            }
            thread::sleep(Duration::from_secs(1));
        }
    }
    // The game in the launch's own scope, out of Steam's (see the module's doc), as long as it runs.
    let (unit, slice) = (request.instance.unit().ok_or(Error::NoScope)?, request.instance.slice());
    gather(&marker, &unit, slice.as_deref(), &steam_launchers(&marker));
    Ok(Running::Steam { marker, unit, slice })
}

/// Asks Steam to start a game: Steam started for it if it isn't running.
fn ask_steam(config: &Config, request: &Request, appid: u32) -> Result<(), Error> {
    let env = &request.env;
    // (`-silent`: if Steam isn't running, it starts without its window, only the game showing.)
    let argv: Vec<String> = ["steam", "-silent", "-applaunch", &appid.to_string()].map(String::from).to_vec();
    if !steam_client_running() {
        let instance = Instance::Run { key: clamshell_library::key_of(STEAM_ENTRY), secs: now(), n: 0 };
        let job = Job { unit: instance.name(), workspace: instance.workspace(), slice: instance.slice(), argv, env: vec![], workdir: None };
        log::info!("launcher: Steam isn't running: starting it in {}", job.unit);
        if let Started::Child(child) = config.jobs.without_sway().start(&job)? {
            job::reap(child);
        }
    } else {
        let mut steam = Command::new("steam");
        steam.args(argv.iter().skip(1)).envs(env.iter().map(|(k, v)| (k, v))).stdin(Stdio::null()).process_group(0);
        job::reap(steam.spawn()?);
    }
    Ok(())
}

/// Steam's launchers for a game (`launchers`, `marker` in their command line) and what they started,
/// those not yet in the launch's scope `unit`: moved there.
fn gather(marker: &str, unit: &str, slice: Option<&str>, launchers: &[u32]) {
    let outside = |pid: &u32| !sys::text(format!("/proc/{pid}/cgroup")).is_some_and(|c| c.trim_end().ends_with(&format!("/{unit}")));
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
        if force {
            command::run("systemctl", &["--user", "kill", "--signal=SIGKILL", &slice]);
        } else {
            command::run("systemctl", &["--user", "stop", &slice]);
        }
    });
    if let Err(e) = ended {
        log::warn!("end: {e}");
    }
}

/// Processes asked to end (SIGTERM), or killed: a program not started by ClamShell.
pub fn end_processes(pids: &[u32], force: bool) {
    log::info!("{} processes {pids:?}", if force { "killing" } else { "ending" });
    let mut args = vec![if force { "-KILL".to_owned() } else { "-TERM".to_owned() }];
    args.extend(pids.iter().map(u32::to_string));
    command::run("kill", &args.iter().map(String::as_str).collect::<Vec<_>>());
}

/// Runs a runner's command (its `stop`, `pause` or `resume`: Waydroid's, Android stopping an app or
/// frozen), not waited for.
pub fn runner_command(argv: &[String]) {
    let Some((program, args)) = argv.split_first() else {
        return;
    };
    log::info!("runner command: {}", argv.join(" "));
    match Command::new(program).args(args).stdin(Stdio::null()).process_group(0).spawn() {
        Ok(child) => job::reap(child),
        Err(e) => log::warn!("runner command: {program}: {e}"),
    }
}

/// Every process's parent, from /proc/<pid>/stat (its fourth field, after the name in parentheses).
fn parents() -> Vec<(u32, u32)> {
    crate::sys::processes()
        .filter_map(|(pid, dir)| {
            let stat = crate::sys::text(dir.join("stat"))?;
            let ppid = stat.rsplit_once(')')?.1.split_whitespace().nth(1)?.parse().ok()?;
            Some((pid, ppid))
        })
        .collect()
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
    fn runs_a_game_in_its_scope_and_reports_its_end() {
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
        launcher.send(Request { game: id.clone(), instance, launch, watch: None, env: vec![] });
        drop(launcher);

        let events: Vec<Event> = events.iter().collect();
        assert_eq!(events, vec![Event::Game(GameEvent::Started(id.clone())), Event::Game(GameEvent::Ended(id))]);
        assert_eq!(std::fs::read_to_string(dir.join("ran-in")).unwrap().trim(), dir.to_string_lossy(), "in its own dir");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn no_command_fails_cleanly() {
        let (tx, events) = mpsc::channel();
        let launcher = spawn(config(), tx).unwrap();
        let instance = Instance::Run { key: "test_1".into(), secs: u64::from(std::process::id()), n: 1 };
        launcher.send(Request { game: "x".into(), instance, launch: Launch::program(vec![]), watch: None, env: vec![] });
        drop(launcher);
        assert_eq!(events.iter().collect::<Vec<_>>(), vec![Event::Game(GameEvent::LaunchFailed("x".into()))]);
    }
}
