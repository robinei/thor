//! Game launchers that start games in processes of their own, and how a window is known to be such a
//! game's. ClamShell knows a launch's windows by its unit, but a launcher may start the game elsewhere:
//! Steam under its own launcher process, and Heroic and Lutris, when they already run, by taking a game's
//! launch from its shortcut, which then ends, and starting the game themselves, as they also do for a game
//! started in their own window. Each marks the processes of the games it starts, and that mark, read from
//! a window's process (`WindowFacts::game`), makes the window its game's entry's, as an app id does (see
//! `Running::mark`).
//!
//! The marks, and how an entry is known by one, are the library's ([`clamshell_library::marks`]).
//!
//! A game launched from the Library is to run without its launcher being seen, and the launcher, which may go on
//! running after it (Heroic does, syncing saves), is an entry of its own. Steam's games run their own way
//! (`clamshell --follow-steam`), and Lutris's shortcuts already start a game without Lutris's window, Lutris
//! ending with the game. Heroic's shortcut would show Heroic's window and leave Heroic in the game's
//! launch, so a Heroic game is launched as a launch of Heroic's entry, as Heroic's own shortcuts for
//! Steam do it: Heroic's command with `--no-gui --no-sandbox` and the game's URL, which starts Heroic
//! without its window if it doesn't run and hands it the game. The game then runs by its windows (see
//! [`crate::Session::launch`]).

use clamshell_library::Launch;
use clamshell_library::marks::{HEROIC, LUTRIS, STEAM, heroic_game, is_lutris};

use crate::{Entry, Library};

/// A launcher that runs games: its name, as the Library says a game starts in it, and the names of its desktop
/// files, whose entry is the launcher's own.
pub(crate) struct Launcher {
    pub(crate) name: &'static str,
    desktop: &'static [&'static str],
    /// The arguments its command takes before a game's URL to start the game without its window, if
    /// the game is to be launched that way (see the module's doc).
    headless: Option<&'static [&'static str]>,
    /// Whether its games get their own launch-time settings: Steam's, by the launch hook Steam runs for
    /// each (docs/runners.md). Another launcher's game runs in the launcher's processes, which a running
    /// launcher started with its own settings and environment, so the game's never reach it.
    hooked: bool,
}

const STEAM_LAUNCHER: Launcher = Launcher { name: "Steam", desktop: &["steam"], headless: None, hooked: true };
const HEROIC_LAUNCHER: Launcher =
    Launcher { name: "Heroic", desktop: &["com.heroicgameslauncher.hgl", "heroic"], headless: Some(&["--no-gui", "--no-sandbox"]), hooked: false };
const LUTRIS_LAUNCHER: Launcher = Launcher { name: "Lutris", desktop: &["net.lutris.Lutris", "lutris"], headless: None, hooked: false };

/// The launcher entry `g`'s game runs in, if any.
pub(crate) fn launcher_of(g: &Entry) -> Option<&'static Launcher> {
    match &g.launch {
        Launch::Steam { .. } => Some(&STEAM_LAUNCHER),
        Launch::Program { argv, .. } if argv.iter().any(|a| heroic_game(a).is_some()) => Some(&HEROIC_LAUNCHER),
        Launch::Program { .. } if is_lutris(g) => Some(&LUTRIS_LAUNCHER),
        _ => None,
    }
}

/// Whether entry `g` is a launcher's game whose own launch-time settings don't reach it (see
/// [`Launcher::hooked`]): as for an app handed over, the settings panel dims them.
pub(crate) fn without_launch_settings(g: &Entry) -> bool {
    launcher_of(g).is_some_and(|l| !l.hooked)
}

impl Launcher {
    /// Its own entry in `library`: its desktop file's.
    pub(crate) fn entry<'a>(&self, library: &'a Library) -> Option<&'a Entry> {
        library.entry_of_desktop_file(self.desktop)
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

/// The mark of a game's process (see [`clamshell_library::marks`]), from its environment as `/proc/<pid>/environ`
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

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests;
