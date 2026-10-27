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

use clamshell_library::Launch;

use crate::{Entry, Library};

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
mod tests;
