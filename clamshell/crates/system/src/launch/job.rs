//! Starting a program as an instance of ours, an app or a game, in its systemd scope, as a child process.
//! ClamShell needs a systemd user manager for this.

use std::fmt;
use std::io;
use std::os::unix::process::CommandExt;
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};

use crate::os::worker;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Job {
    /// The scope's name (without `.scope`).
    pub unit: String,
    /// The slice its scope goes in, if any. A dock app's slice holds all its instances.
    pub slice: Option<String>,
    /// Program and arguments.
    pub argv: Vec<String>,
    /// Extra environment.
    pub env: Vec<(String, String)>,
    pub workdir: Option<PathBuf>,
}

#[derive(Debug)]
pub enum Error {
    Io(io::Error),
    EmptyCommand,
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::Io(e) => write!(f, "{e}"),
            Error::EmptyCommand => write!(f, "empty command"),
        }
    }
}

impl From<io::Error> for Error {
    fn from(e: io::Error) -> Self {
        Error::Io(e)
    }
}

/// A command running `program` with `args` in a user systemd scope named `unit`, in `slice` if given. The
/// scope is collected when it ends.
fn scoped(unit: &str, slice: Option<&str>, program: impl AsRef<std::ffi::OsStr>, args: &[impl AsRef<std::ffi::OsStr>]) -> Command {
    let mut cmd = Command::new("systemd-run");
    // The scope is delegated, so what runs in it may move processes into it. For example, a Steam game's
    // follower moves Steam's launcher.
    cmd.args(["--user", "--scope", "--quiet", "--collect", "--property=Delegate=yes", &format!("--unit={unit}")]);
    if let Some(slice) = slice {
        cmd.arg(format!("--slice={slice}"));
    }
    cmd.arg("--").arg(program).args(args);
    cmd
}

/// The command a job runs as, in its workdir or else in HOME.
pub fn command(job: &Job) -> Option<Command> {
    let (program, args) = job.argv.split_first()?;
    let mut cmd = scoped(&job.unit, job.slice.as_deref(), program, args);
    match &job.workdir {
        Some(dir) => cmd.current_dir(dir),
        None => cmd.current_dir(std::env::var_os("HOME").map_or_else(|| PathBuf::from("/"), PathBuf::from)),
    };
    cmd.envs(job.env.iter().map(|(k, v)| (k, v)));
    Some(cmd)
}

/// Waits for a child on a thread of its own, so it is reaped whenever it ends. Whether it runs is for
/// systemd's list, or the caller, to say.
pub fn reap(mut child: Child) {
    if let Err(e) = worker::detached("reaper", move || drop(child.wait())) {
        log::warn!("reaper: {e}");
    }
}

/// Starts a job as a child process.
pub fn start(job: &Job) -> Result<Child, Error> {
    let mut cmd = command(job).ok_or(Error::EmptyCommand)?;
    // In its own process group, our signals, such as a Ctrl-C in a terminal, don't reach it.
    Ok(cmd.stdin(Stdio::null()).process_group(0).spawn()?)
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    #[test]
    fn a_job_runs_in_its_scope_with_its_arguments_untouched() {
        let job = Job {
            unit: format!("clamshell-run-jobtest-{}-0", std::process::id()),
            slice: Some("app-graphical-clamshell-run-jobtest.slice".into()),
            argv: vec!["prog".into(), "a b,c;d".into()],
            env: vec![("K".into(), "v".into())],
            workdir: Some("/tmp".into()),
        };
        let cmd = command(&job).unwrap();
        let args: Vec<&str> = cmd.get_args().filter_map(|a| a.to_str()).collect();
        assert_eq!(cmd.get_program(), "systemd-run");
        assert_eq!(args.last(), Some(&"a b,c;d"), "arguments untouched");
        assert!(args.contains(&format!("--unit={}", job.unit).as_str()));
        assert!(args.contains(&"--slice=app-graphical-clamshell-run-jobtest.slice"), "in its slice: {args:?}");
    }

    #[test]
    fn a_job_is_a_child_in_its_scope() {
        let unit = format!("clamshell-run-jobtest-{}-1", std::process::id());
        let job = Job { unit, slice: None, argv: vec!["true".into()], env: vec![], workdir: None };
        let empty = Job { argv: vec![], ..job.clone() };
        assert!(matches!(start(&empty), Err(Error::EmptyCommand)));
        // Only where there is a user manager to start the scope.
        if std::env::var_os("XDG_RUNTIME_DIR").is_some_and(|d| std::path::Path::new(&d).join("systemd/private").exists()) {
            assert!(start(&job).unwrap().wait().unwrap().success());
        }
    }
}
