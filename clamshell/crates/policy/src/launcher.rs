//! Starting games and knowing when they end. Each request runs on a thread of its own, so games run side
//! by side.
//!
//! - A Pegasus launch command is resolved as Pegasus does (library::launch) and run in its own systemd
//!   scope where there is a user session (`systemd-run --user --scope`), so restarting ClamShell
//!   doesn't end the game; the thread waits for it.
//! - A Steam game is started with `steam -applaunch <id>`; it counts as running while Steam's launcher
//!   process for that app id (`reaper SteamLaunch AppId=<id>`) exists.
//!
//! Every play is added to stats.db (Pegasus' format) and reported, with its start and duration.

use std::io;
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::Arc;
use std::sync::mpsc::Receiver;
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use clamshell_core::{Event, Launch};
use clamshell_library::{launch, stats};

use crate::{Sink, job, sway, units};
use std::sync::mpsc::Sender;

/// How long sway may take to start a game (its scope to appear).
const GAME_START: Duration = Duration::from_secs(30);

/// How long Steam may take to start a game (it may need to start or update first).
const STEAM_START: Duration = Duration::from_secs(120);

pub struct Request {
    pub game: Arc<str>,
    /// The name for its scope and workspace (`clamshell-game-<secs>-<n>`), from the state.
    pub workspace: String,
    pub launch: Launch,
    pub file: Option<PathBuf>,
    /// This game's own environment (its MangoHud config), on top of the config's.
    pub env: Vec<(String, String)>,
}

#[derive(Debug, Clone, Default)]
pub struct Config {
    /// Where plays are recorded; `None`: not recorded.
    pub stats_db: Option<PathBuf>,
    /// Added to every game's environment (MangoHud).
    pub env: Vec<(String, String)>,
    /// Run games in their own systemd scope.
    pub scope: bool,
    /// Under sway: launches go through it (see [`crate::sway`]), so the game's window opens on the
    /// game's workspace.
    pub sway: Option<Sender<sway::Command>>,
}

impl Config {
    /// Whether there is a systemd user session to put games in scopes of.
    pub fn has_user_systemd() -> bool {
        std::env::var_os("XDG_RUNTIME_DIR").is_some_and(|d| Path::new(&d).join("systemd/private").exists())
    }
}

fn now() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)
}

pub fn spawn(config: Config, requests: Receiver<Request>, sink: impl Sink + Clone) -> io::Result<JoinHandle<()>> {
    thread::Builder::new().name("launcher".into()).spawn(move || {
        let mut games = Vec::new();
        for request in requests {
            let (config, sink) = (config.clone(), sink.clone());
            match thread::Builder::new().name("game".into()).spawn(move || run(&config, &request, &sink)) {
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

/// Runs one game until it ends; reports and records its play.
fn run(config: &Config, request: &Request, sink: &impl Sink) {
    let game = request.game.clone();
    let mut env = config.clone();
    env.env.extend(request.env.iter().cloned());
    let played = match &request.launch {
        Launch::Command { command, workdir, basedir } => match request.file.as_deref() {
            Some(file) => run_command(&env, command, workdir, basedir, file, &request.workspace, &game, sink),
            None => Err(io::Error::other("no file to launch")),
        },
        Launch::Steam { appid } => run_steam(&env, *appid, &game, sink),
        Launch::None => Err(io::Error::other("no launch command")),
    };
    let event = match played {
        Ok((start, duration)) => {
            log::info!("launcher: {game} ran {duration} s");
            if let Some(db) = &config.stats_db
                && let Err(e) = stats::record(db, &game, start, duration)
            {
                log::warn!("launcher: recording the play: {e}");
            }
            Event::Played { game, start, duration }
        }
        Err(e) => {
            log::warn!("launcher: {game}: {e}");
            Event::LaunchFailed(game)
        }
    };
    sink.send(event);
}

#[allow(clippy::too_many_arguments)]
fn run_command(
    config: &Config,
    command: &str,
    workdir: &str,
    basedir: &Path,
    file: &Path,
    workspace: &str,
    game: &Arc<str>,
    sink: &impl Sink,
) -> io::Result<(u64, u64)> {
    let c = launch::resolve(command, workdir, basedir, file).ok_or_else(|| io::Error::other("empty launch command"))?;
    let unit = workspace.to_owned();
    // Under sway (and systemd): sway starts it for the game's workspace; it has ended when its scope has.
    if let (Some(sway), true) = (&config.sway, config.scope) {
        let job = job::Job {
            unit: unit.clone(),
            argv: std::iter::once(c.program.to_string_lossy().into_owned()).chain(c.args.iter().cloned()).collect(),
            env: config.env.clone(),
            workdir: Some(c.workdir.clone()),
            scope: true,
        };
        let path = job::write(&job)?;
        sway.send(sway::Command::Launch { workspace: unit.clone(), job: path })
            .map_err(|_| io::Error::other("the sway thread is gone"))?;
        let start = now();
        let started = Instant::now();
        if !sink.send(Event::Started(game.clone())) {
            return Err(io::Error::other("shutting down"));
        }
        units::wait(&format!("{unit}.scope"), GAME_START)?;
        return Ok((start, started.elapsed().as_secs()));
    }
    let mut cmd = crate::job::scoped(config.scope, &unit, &c.program, &c.args);
    // Its own process group: our signals (a Ctrl-C in a terminal) don't reach it.
    cmd.current_dir(&c.workdir).envs(config.env.iter().map(|(k, v)| (k, v))).stdin(Stdio::null()).process_group(0);
    let mut child = cmd.spawn()?;
    let start = now();
    let started = Instant::now();
    if !sink.send(Event::Started(game.clone())) {
        return Err(io::Error::other("shutting down"));
    }
    child.wait()?;
    Ok((start, started.elapsed().as_secs()))
}

fn run_steam(config: &Config, appid: u32, game: &Arc<str>, sink: &impl Sink) -> io::Result<(u64, u64)> {
    let mut steam = Command::new("steam");
    steam.args(["-applaunch", &appid.to_string()]).envs(config.env.iter().map(|(k, v)| (k, v))).stdin(Stdio::null()).process_group(0);
    let mut child = steam.spawn()?;
    // If Steam wasn't running, this process becomes Steam itself: reap it whenever it ends.
    thread::spawn(move || child.wait());
    let marker = format!("AppId={appid}");
    let asked = Instant::now();
    while !steam_running(&marker) {
        if asked.elapsed() > STEAM_START {
            return Err(io::Error::other("Steam didn't start it"));
        }
        thread::sleep(Duration::from_secs(1));
    }
    let start = now();
    let started = Instant::now();
    if !sink.send(Event::Started(game.clone())) {
        return Err(io::Error::other("shutting down"));
    }
    while steam_running(&marker) {
        thread::sleep(Duration::from_secs(2));
    }
    Ok((start, started.elapsed().as_secs()))
}

/// Whether Steam's launcher for an app (`... SteamLaunch AppId=<id> ...`) is running.
fn steam_running(marker: &str) -> bool {
    let Ok(procs) = std::fs::read_dir("/proc") else {
        return false;
    };
    procs.flatten().any(|p| {
        std::fs::read(p.path().join("cmdline")).is_ok_and(|c| {
            let mut args = c.split(|&b| b == 0);
            args.any(|a| a == b"SteamLaunch") && c.split(|&b| b == 0).any(|a| a == marker.as_bytes())
        })
    })
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    #[test]
    fn runs_a_game_and_records_its_play() {
        let dir = std::env::temp_dir().join(format!("clamshell-launch-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let script = dir.join("game.sh");
        std::fs::write(&script, "#!/bin/sh\nsleep 1\npwd > ran-in\n").unwrap();
        std::fs::set_permissions(&script, std::os::unix::fs::PermissionsExt::from_mode(0o755)).unwrap();
        let db = dir.join("stats.db");

        let events = Arc::new(Mutex::new(Vec::new()));
        let seen = events.clone();
        let (tx, rx) = std::sync::mpsc::channel();
        let config = Config { stats_db: Some(db.clone()), env: vec![], scope: false, sway: None };
        let handle = spawn(config, rx, move |e| seen.lock().map(|mut v| v.push(e)).is_ok()).unwrap();
        let id: Arc<str> = script.to_string_lossy().into();
        let launch = Launch::Command { command: "\"{file.path}\"".into(), workdir: String::new(), basedir: dir.clone() };
        tx.send(Request { game: id.clone(), workspace: "clamshell-game-test-0".into(), launch, file: Some(script.clone()), env: vec![] }).unwrap();
        drop(tx);
        handle.join().unwrap();

        let events = events.lock().unwrap();
        assert_eq!(events.first(), Some(&Event::Started(id.clone())));
        assert!(matches!(events.get(1), Some(Event::Played { game, duration, .. }) if *game == id && *duration >= 1));
        assert_eq!(std::fs::read_to_string(dir.join("ran-in")).unwrap().trim(), dir.to_string_lossy(), "in its own dir");
        assert_eq!(stats::read(&db).get(&*id).map(|p| p.count), Some(1));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_missing_program_fails_cleanly() {
        let events = Arc::new(Mutex::new(Vec::new()));
        let seen = events.clone();
        let (tx, rx) = std::sync::mpsc::channel();
        let handle = spawn(Config::default(), rx, move |e| seen.lock().map(|mut v| v.push(e)).is_ok()).unwrap();
        let launch = Launch::Command { command: "/nonexistent/game".into(), workdir: String::new(), basedir: "/".into() };
        tx.send(Request { game: "x".into(), workspace: "clamshell-game-test-1".into(), launch, file: Some("/tmp".into()), env: vec![] }).unwrap();
        drop(tx);
        handle.join().unwrap();
        assert_eq!(*events.lock().unwrap(), vec![Event::LaunchFailed("x".into())]);
    }
}
