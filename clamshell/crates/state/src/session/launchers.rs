//! Game launchers that start games in processes of their own, and how a window is known to be such a
//! game's. ClamShell knows a launch's windows by its unit, but a launcher may start the game elsewhere:
//! Steam under its own launcher process, and Heroic and Lutris, when they already run, by taking a game's
//! launch from its shortcut, which then ends, and starting the game themselves, as they also do for a game
//! started in their own window. Each marks the processes of the games it starts, and that mark, read from
//! a window's process (`WindowFacts::game`), makes the window its game's entry's, as an app id does (see
//! `Running::mark`).
//!
//! The marks, the strongest first:
//!
//! - Steam: `SteamAppId` in the environment, which Steam gives its games. The mark is `steam_app_<id>`,
//!   which is also the X11 class of Steam's Proton games; the entry is Steam's game.
//! - Heroic: `HEROIC_APP_NAME` and `HEROIC_APP_RUNNER` in the environment, which Heroic gives its games
//!   for its users' scripts. The mark is `heroic:<runner>:<name>`; the entry is the desktop file whose
//!   command opens `heroic://launch?appName=<name>&runner=<runner>`, as Heroic's shortcuts do.
//! - Lutris: the `lutris-wrapper` process Lutris runs a game under, whose first argument is the game's
//!   name. The mark is `lutris:<name>`; the entry is the desktop file whose command opens
//!   `lutris:rungameid/` and whose name that is, as Lutris's shortcuts are. The wrapper's arguments are
//!   Lutris's own rather than meant to be read, and names can repeat: a wrapper not like this, or two such
//!   entries of one name, mark nothing, and the game's windows are Lutris's.
//!
//! A game launched from shelf is to run without its launcher being seen, and the launcher, which may go on
//! running after it (Heroic does, syncing saves), is an entry of its own. Steam's games run their own way
//! (`clamshell --follow-steam`), and Lutris's shortcuts already start a game without Lutris's window, Lutris
//! ending with the game. Heroic's shortcut would show Heroic's window and leave Heroic in the game's
//! launch, so a Heroic game is launched as a launch of Heroic's entry, as Heroic's own shortcuts for
//! Steam do it: Heroic's command with `--no-gui --no-sandbox` and the game's URL, which starts Heroic
//! without its window if it doesn't run and hands it the game. The game then runs by its windows (see
//! [`crate::Session::launch`]).

use clamshell_library::{Launch, ids};

use crate::{Entry, Library};

/// A launcher that runs games: the names of its desktop files, whose entry is the launcher's own.
pub(crate) struct Launcher {
    desktop: &'static [&'static str],
    /// The arguments its command takes before a game's URL to start the game without its window, if
    /// the game is to be launched that way (see the module's doc).
    headless: Option<&'static [&'static str]>,
}

const STEAM_LAUNCHER: Launcher = Launcher { desktop: &["steam"], headless: None };
const HEROIC_LAUNCHER: Launcher = Launcher { desktop: &["com.heroicgameslauncher.hgl", "heroic"], headless: Some(&["--no-gui", "--no-sandbox"]) };
const LUTRIS_LAUNCHER: Launcher = Launcher { desktop: &["net.lutris.Lutris", "lutris"], headless: None };

/// The launcher entry `g`'s game runs in, if any.
pub(crate) fn launcher_of(g: &Entry) -> Option<&'static Launcher> {
    match &g.launch {
        Launch::Steam { .. } => Some(&STEAM_LAUNCHER),
        Launch::Program { argv, .. } if argv.iter().any(|a| heroic_game(a).is_some()) => Some(&HEROIC_LAUNCHER),
        Launch::Program { argv, .. } if argv.iter().any(|a| a.starts_with("lutris:rungameid/")) => Some(&LUTRIS_LAUNCHER),
        _ => None,
    }
}

impl Launcher {
    /// Its own entry in `library`: its desktop file's.
    pub(crate) fn entry<'a>(&self, library: &'a Library) -> Option<&'a Entry> {
        library.entries.iter().find(|g| ids::desktop_file(&g.id).is_some_and(|d| self.desktop.contains(&d)))
    }
}

/// How game `g` is launched without its launcher's window, if it is launched as a launch of its launcher
/// (see the module's doc): the launcher's entry, and the command, the launcher's own with its headless
/// arguments and the game's URL.
pub(crate) fn headless<'a>(library: &'a Library, g: &Entry) -> Option<(&'a Entry, Vec<String>)> {
    let launcher = launcher_of(g)?;
    let arguments = launcher.headless?;
    let Launch::Program { argv, .. } = &g.launch else { return None };
    let url = argv.iter().find(|a| heroic_game(a).is_some())?;
    let entry = launcher.entry(library)?;
    let Launch::Program { argv: own, .. } = &entry.launch else { return None };
    let command = own.iter().cloned().chain(arguments.iter().map(|a| (*a).to_owned())).chain([url.clone()]).collect();
    Some((entry, command))
}

/// Steam's app id for a game's windows, and the mark of its processes: `steam_app_<id>`.
pub(crate) const STEAM: &str = "steam_app_";
const HEROIC: &str = "heroic:";
const LUTRIS: &str = "lutris:";

/// The mark of a game's process (see the module's doc), from its environment as `/proc/<pid>/environ`
/// has it and the command lines of it and its ancestors, its own first, as `/proc/<pid>/cmdline` has them.
/// The command lines are read only as far as needed.
pub fn of_process(environ: &[u8], cmdlines: impl IntoIterator<Item = impl AsRef<[u8]>>) -> Option<String> {
    let var = |name: &[u8]| environ.split(|b| *b == 0).find_map(|v| v.strip_prefix(name)?.strip_prefix(b"=")).and_then(|v| std::str::from_utf8(v).ok());
    // Steam's own client has 0.
    if let Some(id) = var(b"SteamAppId").and_then(|v| v.parse::<u32>().ok()).filter(|id| *id != 0) {
        return Some(format!("{STEAM}{id}"));
    }
    if let (Some(runner), Some(name)) = (var(b"HEROIC_APP_RUNNER"), var(b"HEROIC_APP_NAME")) {
        return Some(format!("{HEROIC}{runner}:{name}"));
    }
    cmdlines.into_iter().find_map(|cmdline| lutris_wrapper(cmdline.as_ref()).map(|name| format!("{LUTRIS}{name}")))
}

/// The game's name in the command line of Lutris's wrapper: `[python3] …/lutris-wrapper <name> <count>
/// <count> <command>…`.
fn lutris_wrapper(cmdline: &[u8]) -> Option<&str> {
    let mut args = cmdline.split(|b| *b == 0);
    args.find(|a| a.ends_with(b"/lutris-wrapper") || *a == b"lutris-wrapper")?;
    let name = std::str::from_utf8(args.next()?).ok().filter(|name| !name.is_empty())?;
    let count = |a: Option<&[u8]>| a.is_some_and(|a| !a.is_empty() && a.iter().all(u8::is_ascii_digit));
    (count(args.next()) && count(args.next())).then_some(name)
}

/// The mark of entry `g`'s game, if a launcher starts it (see the module's doc).
pub(crate) fn of_entry(library: &Library, g: &Entry) -> Option<String> {
    match &g.launch {
        Launch::Steam { appid } => Some(format!("{STEAM}{appid}")),
        Launch::Program { argv, .. } => match argv.iter().find_map(|a| heroic_game(a)) {
            Some((runner, name)) => Some(format!("{HEROIC}{runner}:{name}")),
            None => lutris_unique(library, g).then(|| format!("{LUTRIS}{}", g.title)),
        },
        Launch::None | Launch::Tool(_) => None,
    }
}

/// Whether [`of_entry`] of `g` is `mark`, without building it, since this is asked for every window and
/// entry.
pub(crate) fn is_marked(library: &Library, g: &Entry, mark: &str) -> bool {
    match &g.launch {
        Launch::Steam { appid } => mark.strip_prefix(STEAM).and_then(|n| n.parse::<u32>().ok()) == Some(*appid),
        Launch::Program { argv, .. } => match mark.strip_prefix(HEROIC) {
            Some(game) => argv.iter().filter_map(|a| heroic_game(a)).any(|game_of| game.split_once(':') == Some(game_of)),
            None => mark.strip_prefix(LUTRIS) == Some(g.title.as_str()) && lutris_unique(library, g),
        },
        Launch::None | Launch::Tool(_) => false,
    }
}

/// The runner and the game's name in a Heroic launch URL: `heroic://launch?appName=<name>&runner=<runner>`.
fn heroic_game(arg: &str) -> Option<(&str, &str)> {
    let query = arg.strip_prefix("heroic://launch?")?;
    let value = |key: &str| query.split('&').find_map(|pair| pair.strip_prefix(key)?.strip_prefix('='));
    Some((value("runner")?, value("appName")?))
}

/// Whether `g` is a Lutris shortcut, and the only one of its name.
fn lutris_unique(library: &Library, g: &Entry) -> bool {
    let lutris = |g: &Entry| matches!(&g.launch, Launch::Program { argv, .. } if argv.iter().any(|a| a.starts_with("lutris:rungameid/")));
    lutris(g) && !library.entries.iter().any(|other| other.id != g.id && other.title == g.title && lutris(other))
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests;
