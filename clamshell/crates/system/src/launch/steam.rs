//! Steam's games. A game is started with `steam -gamepadui -silent -applaunch <id>`. If Steam is running, the
//! command is handed to it. Otherwise Steam is started with the command as a launch of Steam's desktop
//! entry, in that entry's scope, so Steam is on the dock and outlives ClamShell. That start doesn't get
//! the game's environment, because it would become Steam's own environment for every game it starts:
//! only the displays to open (`WAYLAND_DISPLAY`, `DISPLAY`), the session's rather than ClamShell's own
//! (nested, the desktop's).
//!
//! The launch's own scope holds a follower (`clamshell --follow-steam <id>`, [`follow`]). It moves Steam's
//! launcher for the game (`reaper SteamLaunch AppId=<id>`) and what it started into its scope. It does
//! the same for any later launcher, since Steam may run a setup first and then the game. It keeps going
//! as long as a launcher runs or ran within [`STEAM_GAP`], and then it ends. The scope ends with the
//! game's last process. This way the game is counted, frozen and ended separately from Steam for as long
//! as it runs.
//!
//! A game started in Steam itself is launched by the state when its window appears. Since it is found
//! running, it is followed the same way instead of being started.

use std::os::unix::process::CommandExt;
use std::process::{Command as Process, Stdio};
use std::thread;
use std::time::{Duration, Instant};

use clamshell_state::Instance;

use super::job::{self, Job};
use super::launcher::{Error, Request, parents, with_descendants};
use super::scopes;
use crate::os::{cgroup, sys};

/// How long Steam may take to start a game, since it may need to start or update first. The follower
/// ends if no launcher of Steam's for the game appears by then.
const STEAM_START: Duration = Duration::from_secs(120);

/// How long a Steam game counts as running with no launcher of Steam's for it. Steam may first run a
/// setup, such as Proton's install scripts in a fresh Steam, and then the game, each under a launcher of
/// its own.
const STEAM_GAP: Duration = Duration::from_secs(10);

/// Starts a Steam game. Steam is asked to start it, unless its launcher already runs because it was
/// started in Steam itself. Then the launch's scope is started with the follower in it. See the module
/// docs.
pub(super) fn start(request: &Request, appid: u32) -> Result<(), Error> {
    if steam_running(&format!("AppId={appid}")) {
        log::info!("launcher: Steam's AppId={appid} runs: following it");
    } else {
        ask_steam(request, appid)?;
    }
    let follower = std::env::current_exe()?.to_string_lossy().into_owned();
    let argv = vec![follower, "--follow-steam".to_owned(), appid.to_string()];
    let job = Job { unit: request.instance.name(), slice: Some(request.instance.slice()), argv, env: vec![], workdir: None };
    Ok(job::start(&job)?)
}

/// `clamshell --follow-steam <id>`, run in a Steam game's launch scope. It moves Steam's launcher for the
/// game, and what it started, into this scope for as long as a launcher runs or ran within [`STEAM_GAP`].
/// The first launcher must appear within [`STEAM_START`]. Then it ends. See the module docs.
pub fn follow(appid: u32) {
    let marker = format!("AppId={appid}");
    let Some(unit) = cgroup::unit_of(std::process::id()).filter(|u| Instance::is_launch_unit(u)) else {
        log::warn!("follow-steam: not in a launch's unit");
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

/// Asks Steam to start a game, starting Steam for it if it isn't running.
pub(super) fn ask_steam(request: &Request, appid: u32) -> Result<(), Error> {
    let env = &request.env;
    // With `-silent`, a Steam that isn't running starts without its window, so only the game shows.
    let argv: Vec<String> = ["steam", "-gamepadui", "-silent", "-applaunch", &appid.to_string()].map(String::from).to_vec();
    if !steam_client_running() {
        let instance = Instance { key: clamshell_library::key_of(STEAM_ENTRY), secs: crate::time::clock::unix_secs(), n: 0 };
        let displays = env.iter().filter(|(k, _)| k == "WAYLAND_DISPLAY" || k == "DISPLAY").cloned().collect();
        let job = Job { unit: instance.name(), slice: Some(instance.slice()), argv, env: displays, workdir: None };
        log::info!("launcher: Steam isn't running: starting it in {}", job.unit);
        job::start(&job)?;
    } else {
        let mut steam = Process::new("steam");
        steam.args(argv.iter().skip(1)).envs(env.iter().map(|(k, v)| (k, v))).stdin(Stdio::null()).process_group(0);
        job::reap(steam.spawn()?);
    }
    Ok(())
}

/// Moves Steam's launchers for a game (`launchers`, with `marker` in their command line), and what they
/// started, into the launch's scope `unit` if they aren't in it yet.
pub(super) fn gather(marker: &str, unit: &str, slice: Option<&str>, launchers: &[u32]) {
    let outside = |pid: &u32| cgroup::unit_of(*pid).is_none_or(|u| u != unit);
    let pids: Vec<u32> = with_descendants(launchers, &parents()).into_iter().filter(outside).collect();
    if pids.is_empty() {
        return;
    }
    match scopes::adopt(unit, slice, &pids) {
        Ok(()) => log::info!("launcher: Steam's {marker} in {unit} ({} processes)", pids.len()),
        Err(e) => log::warn!("launcher: Steam's {marker}: its own scope: {e}"),
    }
}

/// Steam's desktop entry. A Steam started for a game is one of its launches.
const STEAM_ENTRY: &str = "desktop:steam";

/// Whether Steam's client, or its starter script, is running.
pub(super) fn steam_client_running() -> bool {
    sys::processes().any(|(_, dir)| sys::text(dir.join("comm")).is_some_and(|c| c.trim() == "steam"))
}

/// Whether Steam's launcher for an app (`... SteamLaunch AppId=<id> ...`) is running.
pub(super) fn steam_running(marker: &str) -> bool {
    !steam_launchers(marker).is_empty()
}

/// The processes of Steam's launcher for an app, whose command line is `... SteamLaunch AppId=<id> ...`
/// with `marker` as `AppId=<id>`. The game's processes descend from them.
pub(super) fn steam_launchers(marker: &str) -> Vec<u32> {
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
