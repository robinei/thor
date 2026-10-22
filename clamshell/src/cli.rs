//! The command line: what the program is asked to do, read once.

use std::path::PathBuf;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Command {
    /// No arguments: run the session (the UI and every system thread).
    Run,
    /// `--run FILE`: become a launched job's program. Sway's exec starts us this way (see
    /// clamshell_system::launch::job).
    RunJob(PathBuf),
    /// `--follow-steam <id>`: run in a Steam game's launch scope and move Steam's launcher for the game
    /// into it for as long as the launcher runs (see clamshell_system::launch::steam).
    FollowSteam(u32),
    /// `--mangohud-env`: print the environment that makes a game started by hand report its frame rate
    /// here.
    MangohudEnv,
    /// `--library`: print the tabs and entries found (library.json's content dirs, Steam, desktop files).
    Library,
    /// `--tray`: print the tray icons there are, and their menus (see clamshell_system::session::tray).
    Tray,
    /// `--clipboard get`: print the clipboard's text. `--clipboard set`: set the clipboard to the text read
    /// from stdin (see clamshell_system::session::clipboard).
    ClipboardGet,
    ClipboardSet,
    /// `--wayland-filter`: a Wayland proxy for Waydroid beside the compositor's socket (see
    /// clamshell_system::waydroid).
    WaylandFilter,
    /// `--fetch-metadata`: fetch what the library is missing (covers, screenshots, ...) without the UI and
    /// print what was found (see clamshell_system::library::meta). A running ClamShell shows it from its
    /// next start.
    FetchMetadata,
    /// `--snapshot DIR`: draw both screens at the Thor's size into PNG files, no Wayland needed (built
    /// with the `dev` feature).
    #[cfg(any(test, feature = "dev"))]
    Snapshot(PathBuf),
    /// `ctl COMMAND...`: send a command to the running ClamShell (`volume up`, `screenshot`, ...; see
    /// [`crate::control`]).
    Ctl(Vec<String>),
}

pub const USAGE: &str = "usage: clamshell [ctl COMMAND...]
  ctl COMMAND...       drive the running ClamShell (`clamshell ctl help` lists the commands)
used by ClamShell itself:
  --run FILE           a launch's job, through sway's exec
  --follow-steam ID    a Steam game's follower, in its launch's scope
  --wayland-filter     Waydroid's Wayland filter
  --clipboard get|set  the clipboard's text (Waydroid's clipboard sharing)
for development:
  --snapshot DIR       both screens' views drawn into DIR (built with --features dev)
  --library            the library as it reads, written out
  --tray               the tray icons there are, written out
  --fetch-metadata     the missing covers and details fetched
  --mangohud-env       the environment that has a game started by hand report its frame rate";

/// The command the arguments (without the program's name) ask for.
pub fn parse(args: &[String]) -> Option<Command> {
    Some(match args {
        [] => Command::Run,
        [flag, file] if flag == "--run" => Command::RunJob(file.into()),
        [flag, appid] if flag == "--follow-steam" => Command::FollowSteam(appid.parse().ok()?),
        #[cfg(any(test, feature = "dev"))]
        [flag, dir] if flag == "--snapshot" => Command::Snapshot(dir.into()),
        [flag] if flag == "--mangohud-env" => Command::MangohudEnv,
        [flag] if flag == "--library" => Command::Library,
        [flag] if flag == "--tray" => Command::Tray,
        [flag] if flag == "--wayland-filter" => Command::WaylandFilter,
        [flag, what] if flag == "--clipboard" && what == "get" => Command::ClipboardGet,
        [flag, what] if flag == "--clipboard" && what == "set" => Command::ClipboardSet,
        [flag] if flag == "--fetch-metadata" => Command::FetchMetadata,
        [ctl, rest @ ..] if ctl == "ctl" && !rest.is_empty() => Command::Ctl(rest.to_vec()),
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(line: &str) -> Vec<String> {
        line.split_whitespace().map(Into::into).collect()
    }

    #[test]
    fn commands() {
        assert_eq!(parse(&args("")), Some(Command::Run));
        assert_eq!(parse(&args("--snapshot /tmp/s")), Some(Command::Snapshot("/tmp/s".into())));
        assert_eq!(parse(&args("--run /run/j.json")), Some(Command::RunJob("/run/j.json".into())));
        assert_eq!(parse(&args("--follow-steam 620")), Some(Command::FollowSteam(620)));
        assert_eq!(parse(&args("--library")), Some(Command::Library));
        assert_eq!(parse(&args("--tray")), Some(Command::Tray));
        assert_eq!(parse(&args("--fetch-metadata")), Some(Command::FetchMetadata));
        assert_eq!(parse(&args("--snapshot")), None, "without its dir");
        assert_eq!(parse(&args("--frobnicate")), None);
        assert_eq!(parse(&args("ctl volume up")), Some(Command::Ctl(vec!["volume".into(), "up".into()])));
        assert_eq!(parse(&args("ctl")), None, "without a command");
    }
}
