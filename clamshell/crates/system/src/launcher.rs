//! Starting entries (games, apps, films) and knowing when they end. Each launch runs on a thread of its
//! own, so they run side by side.
//!
//! - A program (its arguments filled in by the library) runs in its own systemd scope where there is a
//!   user session (`systemd-run --user --scope`), so restarting ClamShell doesn't end it; the thread
//!   waits for it.
//! - A Steam game is started with `steam -silent -applaunch <id>`: given to Steam if it runs; else Steam
//!   is started with it as a launch of Steam's desktop entry (its scope, so it is on the dock and outlives
//!   ClamShell; ClamShell finds it by its scope), without the game's environment (Steam's own would be
//!   it, for every game it starts). Once Steam's launcher for it runs, it and what it started are moved
//!   into the launch's own scope (as any launch's, in the entry's slice): the game is counted, frozen and
//!   ended apart from Steam; it counts as running while Steam's launcher
//!   process for that app id (`reaper SteamLaunch AppId=<id>`) exists.
//!
//! Every play is added to the database (see [`crate::store`]) and reported, with its start and duration.

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
use crate::store::Store;
use crate::worker::{self, Worker};
use crate::{Closed, Sink, command, sys, units};

/// How long sway may take to start a game (its scope to appear).
const GAME_START: Duration = Duration::from_secs(30);
/// How long Steam may take to start a game (it may need to start or update first).
const STEAM_START: Duration = Duration::from_secs(120);

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
    /// The database plays are recorded in; `None`: not recorded.
    pub db: Option<PathBuf>,
    /// Run games in their own systemd scope.
    pub scope: bool,
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
        // No more requests: the games still running end on their own; their reports still go out.
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
    /// Steam's launcher for it runs (this marker in its command line).
    Steam(String),
}

impl Running {
    /// Waits for the game to end.
    fn wait(self) -> Result<(), Error> {
        match self {
            Running::Scope(unit) => Ok(units::wait(&unit, GAME_START)?),
            Running::Child(mut child) => child.wait().map(drop).map_err(Error::from),
            Running::Steam(marker) => {
                while steam_running(&marker) {
                    thread::sleep(Duration::from_secs(2));
                }
                Ok(())
            }
        }
    }
}

/// Starts one game, waits until it ends, and reports and records its play (or that it couldn't start);
/// ends early if the UI is gone.
fn play(config: &Config, request: &Request, sink: &impl Sink) -> Result<(), Closed> {
    let game = &request.game;
    let failed = |e: Error| {
        log::warn!("launcher: {game}: {e}");
        GameEvent::LaunchFailed(game.clone())
    };
    let event = match start(config, request) {
        Err(e) => failed(e),
        Ok(running) => {
            let (start, started) = (now(), Instant::now());
            sink.send(GameEvent::Started(game.clone()).into())?;
            match running.wait() {
                Err(e) => failed(e),
                Ok(()) => {
                    let duration = started.elapsed().as_secs();
                    log::info!("launcher: {game} ran {duration} s");
                    if let Some(file) = &request.watch {
                        let position = crate::mpv::position(&config.watch_later, file).map(|(s, _)| s);
                        sink.send(Event::Game(GameEvent::Position { game: game.clone(), position }))?;
                    }
                    if let Some(db) = &config.db
                        && let Err(e) = Store::open(db).and_then(|s| s.add_play(game, start, duration))
                    {
                        log::warn!("launcher: recording the play: {e}");
                    }
                    GameEvent::Played { game: game.clone(), start, duration }
                }
            }
        }
    };
    sink.send(Event::Game(event))
}

/// Starts the game a request is for.
fn start(config: &Config, request: &Request) -> Result<Running, Error> {
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
    let job = Job {
        unit: request.instance.name(),
        workspace: request.instance.workspace(),
        slice: request.instance.slice(),
        argv,
        env: request.env.clone(),
        workdir,
        scope: config.scope,
    };
    // Through sway only in a scope: the scope is how the game is followed then.
    let jobs = if config.scope { config.jobs.clone() } else { config.jobs.without_sway() };
    Ok(match jobs.start(&job)? {
        Started::Sway => Running::Scope(request.instance.unit().ok_or(Error::NoScope)?),
        Started::Child(child) => Running::Child(child),
    })
}

fn start_steam(config: &Config, request: &Request, appid: u32) -> Result<Running, Error> {
    let env = &request.env;
    // (`-silent`: if Steam isn't running, it starts without its window, only the game showing.)
    let argv: Vec<String> = ["steam", "-silent", "-applaunch", &appid.to_string()].map(String::from).to_vec();
    if config.scope && !steam_client_running() {
        let instance = Instance::Run { key: clamshell_library::key_of(STEAM_ENTRY), secs: now(), n: 0 };
        let job = Job { unit: instance.name(), workspace: instance.workspace(), slice: instance.slice(), argv, env: vec![], workdir: None, scope: true };
        log::info!("launcher: Steam isn't running: starting it in {}", job.unit);
        if let Started::Child(child) = config.jobs.without_sway().start(&job)? {
            job::reap(child);
        }
    } else {
        let mut steam = Command::new("steam");
        steam.args(argv.iter().skip(1)).envs(env.iter().map(|(k, v)| (k, v))).stdin(Stdio::null()).process_group(0);
        // (Without a scope, if Steam wasn't running, this process becomes Steam itself.)
        job::reap(steam.spawn()?);
    }
    let marker = format!("AppId={appid}");
    let asked = Instant::now();
    while !steam_running(&marker) {
        if asked.elapsed() > STEAM_START {
            return Err(Error::SteamDidNotStart);
        }
        thread::sleep(Duration::from_secs(1));
    }
    // The game in the launch's own scope, out of Steam's (see the module's doc).
    if config.scope
        && let Some(unit) = request.instance.unit()
    {
        let pids = with_descendants(&steam_launchers(&marker), &parents());
        match crate::units::adopt(&unit, request.instance.slice().as_deref(), &pids) {
            Ok(()) => log::info!("launcher: Steam app {appid} in {unit} ({} processes)", pids.len()),
            Err(e) => log::warn!("launcher: Steam app {appid}: its own scope: {e}"),
        }
    }
    Ok(Running::Steam(marker))
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

/// Ends a game Steam runs (in Steam's scope, not ours): Steam's launcher for its app id and every process
/// under it (Proton's helpers too, which no window belongs to), killed.
pub fn end_steam(appid: u32) {
    let launchers = steam_launchers(&format!("AppId={appid}"));
    let tree = with_descendants(&launchers, &parents());
    if tree.is_empty() {
        return;
    }
    log::info!("Steam app {appid}: killing processes {tree:?}");
    let args: Vec<String> = std::iter::once("-KILL".to_owned()).chain(tree.iter().map(u32::to_string)).collect();
    let args: Vec<&str> = args.iter().map(String::as_str).collect();
    command::run("kill", &args);
}

/// Ends an entry that runs in another program's processes by its runner's `stop` command (Waydroid's
/// apps: Android stops them), not waited for.
pub fn stop(argv: &[String]) {
    let Some((program, args)) = argv.split_first() else {
        return;
    };
    log::info!("stop: {}", argv.join(" "));
    match Command::new(program).args(args).stdin(Stdio::null()).process_group(0).spawn() {
        Ok(child) => job::reap(child),
        Err(e) => log::warn!("stop: {program}: {e}"),
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

    fn config(db: Option<PathBuf>) -> Config {
        Config { db, scope: false, watch_later: Vec::new(), jobs: Runner { jobs: std::env::temp_dir(), sway: None }, sockets: std::env::temp_dir() }
    }

    #[test]
    fn runs_a_game_and_records_its_play() {
        let dir = std::env::temp_dir().join(format!("clamshell-launch-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let script = dir.join("game.sh");
        std::fs::write(&script, "#!/bin/sh\nsleep 1\npwd > ran-in\n").unwrap();
        std::fs::set_permissions(&script, std::os::unix::fs::PermissionsExt::from_mode(0o755)).unwrap();
        let db = dir.join("clamshell.db");

        let (tx, events) = mpsc::channel();
        let launcher = spawn(config(Some(db.clone())), tx).unwrap();
        let id: Arc<str> = script.to_string_lossy().into();
        let launch = Launch::Program { argv: vec![script.to_string_lossy().into()], workdir: Some(dir.clone()) };
        let instance = Instance::Run { key: "test_1".into(), secs: u64::from(std::process::id()), n: 0 };
        launcher.send(Request { game: id.clone(), instance, launch, watch: None, env: vec![] });
        drop(launcher);

        let events: Vec<Event> = events.iter().collect();
        assert_eq!(events.first(), Some(&Event::Game(GameEvent::Started(id.clone()))));
        assert!(matches!(events.get(1), Some(Event::Game(GameEvent::Played { game, duration, .. })) if *game == id && *duration >= 1));
        assert_eq!(std::fs::read_to_string(dir.join("ran-in")).unwrap().trim(), dir.to_string_lossy(), "in its own dir");
        assert_eq!(Store::open(&db).unwrap().plays().unwrap().get(&*id).map(|p| p.count), Some(1));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_missing_program_fails_cleanly() {
        let (tx, events) = mpsc::channel();
        let launcher = spawn(config(None), tx).unwrap();
        let launch = Launch::program(vec!["/nonexistent/game".into()]);
        let instance = Instance::Run { key: "test_1".into(), secs: u64::from(std::process::id()), n: 1 };
        launcher.send(Request { game: "x".into(), instance, launch, watch: None, env: vec![] });
        drop(launcher);
        assert_eq!(events.iter().collect::<Vec<_>>(), vec![Event::Game(GameEvent::LaunchFailed("x".into()))]);
    }
}
