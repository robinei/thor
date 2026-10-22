//! Steam's games. A game is started with `steam -silent -applaunch <id>`: given to Steam if it runs; else
//! Steam is started with it as a launch of Steam's desktop entry (its scope, so it is on the dock and
//! outlives ClamShell), without the game's environment (Steam's own would be it, for every game it
//! starts). The launch's own scope holds a follower (`clamshell --follow-steam <id>`, [`follow`]): it
//! moves Steam's launcher for the game (`reaper SteamLaunch AppId=<id>`) and what it started into its
//! scope, and any later one (Steam may run a setup first, then the game), as long as one runs or did
//! within [`STEAM_GAP`]; then it ends, and the scope with the game's last process: the game is counted,
//! frozen and ended apart from Steam, as long as it runs. A game started in Steam itself is launched by
//! the state when its window comes: found running, it is followed the same way, not started.

use std::os::unix::process::CommandExt;
use std::process::{Command as Process, Stdio};
use std::thread;
use std::time::{Duration, Instant};

use clamshell_core::Instance;

use super::job::{self, Job, Started};
use super::launcher::{Config, Error, Request, parents, started, with_descendants};
use super::scopes;
use crate::os::{cgroup, sys};

/// How long Steam may take to start a game (it may need to start or update first): the follower ends
/// if no launcher of Steam's for it comes by then.
const STEAM_START: Duration = Duration::from_secs(120);

/// How long a Steam game counts as running with no launcher of Steam's for it: Steam may run a setup
/// first (Proton's install scripts, in a fresh Steam), then the game, under launchers of their own.
const STEAM_GAP: Duration = Duration::from_secs(10);

/// A Steam game: asked of Steam (unless its launcher runs: started in Steam itself), and the launch's
/// scope started with its follower in it (see the module's doc).
pub(super) fn start(config: &Config, request: &Request, appid: u32) -> Result<(), Error> {
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
pub fn follow(appid: u32) {
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
pub(super) fn ask_steam(config: &Config, request: &Request, appid: u32) -> Result<(), Error> {
    let env = &request.env;
    // (`-silent`: if Steam isn't running, it starts without its window, only the game showing.)
    let argv: Vec<String> = ["steam", "-silent", "-applaunch", &appid.to_string()].map(String::from).to_vec();
    if !steam_client_running() {
        let instance = Instance::Run { key: clamshell_library::key_of(STEAM_ENTRY), secs: crate::time::clock::unix_secs(), n: 0 };
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

/// Steam's desktop entry, whose launches Steam started for a game is one of.
const STEAM_ENTRY: &str = "desktop:steam";

/// Whether Steam's client (or its starter script) runs.
pub(super) fn steam_client_running() -> bool {
    sys::processes().any(|(_, dir)| sys::text(dir.join("comm")).is_some_and(|c| c.trim() == "steam"))
}

/// Whether Steam's launcher for an app (`... SteamLaunch AppId=<id> ...`) is running.
pub(super) fn steam_running(marker: &str) -> bool {
    !steam_launchers(marker).is_empty()
}

/// The processes of Steam's launcher for an app (`... SteamLaunch AppId=<id> ...`: `marker`), which the
/// game's processes descend from.
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
