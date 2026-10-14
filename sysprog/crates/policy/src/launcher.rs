//! Starting games and knowing when they end. One game at a time, from a queue.
//!
//! - A Pegasus launch command is resolved as Pegasus does (library::launch) and run in its own systemd
//!   scope where there is a user session (`systemd-run --user --scope`), so restarting the system program
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
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::mpsc::Receiver;
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use sysprog_core::{Event, Launch};
use sysprog_library::{launch, stats};

use crate::Sink;

/// How long Steam may take to start a game (it may need to start or update first).
const STEAM_START: Duration = Duration::from_secs(120);

pub struct Request {
    pub game: Arc<str>,
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

pub fn spawn(config: Config, requests: Receiver<Request>, sink: impl Sink) -> io::Result<JoinHandle<()>> {
    thread::Builder::new().name("launcher".into()).spawn(move || {
        let mut count = 0u32;
        for request in requests {
            count += 1;
            let game = request.game.clone();
            let mut env = config.clone();
            env.env.extend(request.env.iter().cloned());
            let played = match &request.launch {
                Launch::Command { command, workdir, basedir } => match request.file.as_deref() {
                    Some(file) => run_command(&env, command, workdir, basedir, file, count, &game, &sink),
                    None => Err(io::Error::other("no file to launch")),
                },
                Launch::Steam { appid } => run_steam(&env, *appid, &game, &sink),
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
            if !sink.send(event) {
                return;
            }
        }
    })
}

#[allow(clippy::too_many_arguments)]
fn run_command(
    config: &Config,
    command: &str,
    workdir: &str,
    basedir: &Path,
    file: &Path,
    count: u32,
    game: &Arc<str>,
    sink: &impl Sink,
) -> io::Result<(u64, u64)> {
    let c = launch::resolve(command, workdir, basedir, file).ok_or_else(|| io::Error::other("empty launch command"))?;
    let mut cmd = if config.scope {
        let mut cmd = Command::new("systemd-run");
        let unit = format!("--unit=sysprog-game-{}-{count}", std::process::id());
        cmd.args(["--user", "--scope", "--quiet", "--collect", &unit, "--"]).arg(&c.program).args(&c.args);
        cmd
    } else {
        let mut cmd = Command::new(&c.program);
        cmd.args(&c.args);
        cmd
    };
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

/// Starts a launcher app (program and arguments, no shell) from the home directory, in its own scope
/// where there is a user session (so it outlives a restart of the system program). Doesn't wait: calls
/// `ended` once it exits, or at once if it couldn't start.
pub fn run_app(command: &[String], scope: bool, ended: impl FnOnce() + Send + 'static) {
    static COUNT: AtomicU32 = AtomicU32::new(0);
    let Some((program, args)) = command.split_first() else {
        return ended();
    };
    let mut cmd = if scope {
        let mut cmd = Command::new("systemd-run");
        let n = COUNT.fetch_add(1, Ordering::Relaxed);
        let unit = format!("--unit=sysprog-app-{}-{n}", std::process::id());
        cmd.args(["--user", "--scope", "--quiet", "--collect", &unit, "--"]).arg(program).args(args);
        cmd
    } else {
        let mut cmd = Command::new(program);
        cmd.args(args);
        cmd
    };
    if let Some(home) = std::env::var_os("HOME") {
        cmd.current_dir(home);
    }
    cmd.stdin(Stdio::null()).process_group(0);
    let mut child = match cmd.spawn() {
        Ok(child) => child,
        Err(e) => {
            log::warn!("{program}: {e}");
            return ended();
        }
    };
    let name = program.clone();
    let waiter = thread::Builder::new().name("app".into()).spawn(move || {
        match child.wait() {
            Ok(status) if !status.success() => log::info!("{name} exited: {status}"),
            Ok(_) => {}
            Err(e) => log::warn!("{name}: {e}"),
        }
        ended();
    });
    if let Err(e) = waiter {
        log::warn!("{program}: no thread to wait for it: {e}");
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    #[test]
    fn runs_a_game_and_records_its_play() {
        let dir = std::env::temp_dir().join(format!("sysprog-launch-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let script = dir.join("game.sh");
        std::fs::write(&script, "#!/bin/sh\nsleep 1\npwd > ran-in\n").unwrap();
        std::fs::set_permissions(&script, std::os::unix::fs::PermissionsExt::from_mode(0o755)).unwrap();
        let db = dir.join("stats.db");

        let events = Arc::new(Mutex::new(Vec::new()));
        let seen = events.clone();
        let (tx, rx) = std::sync::mpsc::channel();
        let config = Config { stats_db: Some(db.clone()), env: vec![], scope: false };
        let handle = spawn(config, rx, move |e| seen.lock().map(|mut v| v.push(e)).is_ok()).unwrap();
        let id: Arc<str> = script.to_string_lossy().into();
        let launch = Launch::Command { command: "\"{file.path}\"".into(), workdir: String::new(), basedir: dir.clone() };
        tx.send(Request { game: id.clone(), launch, file: Some(script.clone()), env: vec![] }).unwrap();
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
    fn apps_report_their_end() {
        let (tx, rx) = std::sync::mpsc::channel();
        let done = tx.clone();
        run_app(&["sh".into(), "-c".into(), "sleep 0.2".into()], false, move || done.send("sleep").unwrap());
        run_app(&["/nonexistent/app".into()], false, move || tx.send("missing").unwrap());
        assert_eq!(rx.recv_timeout(Duration::from_secs(5)).unwrap(), "missing", "at once");
        assert_eq!(rx.recv_timeout(Duration::from_secs(5)).unwrap(), "sleep", "when it exits");
    }

    #[test]
    fn a_missing_program_fails_cleanly() {
        let events = Arc::new(Mutex::new(Vec::new()));
        let seen = events.clone();
        let (tx, rx) = std::sync::mpsc::channel();
        let handle = spawn(Config::default(), rx, move |e| seen.lock().map(|mut v| v.push(e)).is_ok()).unwrap();
        let launch = Launch::Command { command: "/nonexistent/game".into(), workdir: String::new(), basedir: "/".into() };
        tx.send(Request { game: "x".into(), launch, file: Some("/tmp".into()), env: vec![] }).unwrap();
        drop(tx);
        handle.join().unwrap();
        assert_eq!(*events.lock().unwrap(), vec![Event::LaunchFailed("x".into())]);
    }
}
