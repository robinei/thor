//! The command line: what the program is asked to do, read once.

use std::path::PathBuf;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Command {
    /// No arguments: run the session (the UI and every system thread).
    Run,
    /// `--run FILE`: become a launched job's program (sway's exec starts us this way; see
    /// clamshell_system::job).
    RunJob(PathBuf),
    /// `--mangohud-env`: print the environment that makes a game (started by hand) report its frame rate
    /// here.
    MangohudEnv,
    /// `--library`: print the tabs and entries found (library.json's content dirs, Steam, desktop files).
    Library,
    /// `--tray`: print the tray icons there are, and their menus (see clamshell_system::tray).
    Tray,
    /// `--fetch-metadata`: fetch what the library wants (covers, screenshots, ...; see
    /// clamshell_system::meta) without the UI, printing what was found; a running ClamShell shows it from
    /// its next start.
    FetchMetadata,
    /// `--snapshot DIR`: draw both screens at the Thor's size into PNG files, no Wayland needed.
    Snapshot(PathBuf),
    /// `ctl COMMAND...`: send a command to the running ClamShell (`volume up`, `screenshot`, ...; see
    /// [`crate::control`]).
    Ctl(Vec<String>),
}

pub const USAGE: &str = "usage: clamshell [ctl COMMAND... | --snapshot DIR | --library | --tray | --fetch-metadata | --mangohud-env | --run FILE]";

/// The command the arguments (without the program's name) ask for.
pub fn parse(args: &[String]) -> Option<Command> {
    Some(match args {
        [] => Command::Run,
        [flag, file] if flag == "--run" => Command::RunJob(file.into()),
        [flag, dir] if flag == "--snapshot" => Command::Snapshot(dir.into()),
        [flag] if flag == "--mangohud-env" => Command::MangohudEnv,
        [flag] if flag == "--library" => Command::Library,
        [flag] if flag == "--tray" => Command::Tray,
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
        assert_eq!(parse(&args("--library")), Some(Command::Library));
        assert_eq!(parse(&args("--tray")), Some(Command::Tray));
        assert_eq!(parse(&args("--fetch-metadata")), Some(Command::FetchMetadata));
        assert_eq!(parse(&args("--snapshot")), None, "without its dir");
        assert_eq!(parse(&args("--frobnicate")), None);
        assert_eq!(parse(&args("ctl volume up")), Some(Command::Ctl(vec!["volume".into(), "up".into()])));
        assert_eq!(parse(&args("ctl")), None, "without a command");
    }
}
