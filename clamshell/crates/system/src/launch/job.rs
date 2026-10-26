//! Starting a program as an instance of ours, an app or a game, as a transient systemd service of the
//! user manager: its output in the journal as the unit's, its processes in its cgroup, and systemd
//! starting and stopping it. ClamShell needs a systemd user manager for this.

use std::fmt;
use std::io;
use std::os::unix::process::CommandExt;
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};

use crate::os::worker;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Job {
    /// The service's name (without `.service`).
    pub unit: String,
    /// The slice its service goes in, if any. A dock app's slice holds all its instances.
    pub slice: Option<String>,
    /// Program and arguments.
    pub argv: Vec<String>,
    /// Its environment, beside the user manager's.
    pub env: Vec<(String, String)>,
    pub workdir: Option<PathBuf>,
}

#[derive(Debug)]
pub enum Error {
    Io(io::Error),
    EmptyCommand,
    /// systemd didn't start it, as `systemd-run` said (its error is in ClamShell's log).
    NotStarted(std::process::ExitStatus),
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::Io(e) => write!(f, "{e}"),
            Error::EmptyCommand => write!(f, "empty command"),
            Error::NotStarted(status) => write!(f, "systemd didn't start it ({status})"),
        }
    }
}

impl From<io::Error> for Error {
    fn from(e: io::Error) -> Self {
        Error::Io(e)
    }
}

/// The `systemd-run` that starts a job as the service `<unit>.service`, in its slice if it has one, in
/// its workdir or else HOME, with its environment. `systemd-run` returns once systemd has started it, or
/// failed to.
pub fn command(job: &Job) -> Option<Command> {
    let (program, args) = job.argv.split_first()?;
    let mut cmd = Command::new("systemd-run");
    cmd.args(["--user", "--quiet", "--collect", &format!("--unit={}", job.unit)]);
    // Started once its program runs, so one that can't be run fails the launch; running as long as any
    // of its processes does, as a launcher script's game outlives it; delegated, so what runs in it may
    // move processes into it, as a Steam game's follower moves Steam's launcher.
    cmd.args(["--service-type=exec", "--property=ExitType=cgroup", "--property=Delegate=yes"]);
    // (The arguments as they are: systemd-run would expand `$` in them.)
    cmd.arg("--expand-environment=no");
    if let Some(slice) = &job.slice {
        cmd.arg(format!("--slice={slice}"));
    }
    let workdir = job.workdir.clone().unwrap_or_else(|| std::env::var_os("HOME").map_or_else(|| PathBuf::from("/"), PathBuf::from));
    cmd.arg(format!("--working-directory={}", workdir.display()));
    cmd.args(job.env.iter().map(|(k, v)| format!("--setenv={k}={v}")));
    cmd.arg("--").arg(program).args(args);
    Some(cmd)
}

/// Waits for a child on a thread of its own, so it is reaped whenever it ends.
pub fn reap(mut child: Child) {
    if let Err(e) = worker::detached("reaper", move || drop(child.wait())) {
        log::warn!("reaper: {e}");
    }
}

/// Starts a job's service, returning once systemd has started it.
pub fn start(job: &Job) -> Result<(), Error> {
    let mut cmd = command(job).ok_or(Error::EmptyCommand)?;
    // In its own process group, our signals, such as a Ctrl-C in a terminal, don't reach it.
    let status = cmd.stdin(Stdio::null()).process_group(0).status()?;
    if status.success() { Ok(()) } else { Err(Error::NotStarted(status)) }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    #[test]
    fn a_job_runs_as_its_service_with_its_arguments_and_environment_untouched() {
        let job = Job {
            unit: format!("clamshell-run-jobtest-{}-0", std::process::id()),
            slice: Some("app-graphical-clamshell-run-jobtest.slice".into()),
            argv: vec!["prog".into(), "a b,c;d $HOME".into()],
            env: vec![("K".into(), "v w".into())],
            workdir: Some("/tmp".into()),
        };
        let cmd = command(&job).unwrap();
        let args: Vec<&str> = cmd.get_args().filter_map(|a| a.to_str()).collect();
        assert_eq!(cmd.get_program(), "systemd-run");
        assert_eq!(args.last(), Some(&"a b,c;d $HOME"), "arguments untouched");
        assert!(args.contains(&"--expand-environment=no"), "nor expanded: {args:?}");
        assert!(args.contains(&format!("--unit={}", job.unit).as_str()));
        assert!(args.contains(&"--slice=app-graphical-clamshell-run-jobtest.slice"), "in its slice: {args:?}");
        assert!(args.contains(&"--setenv=K=v w") && args.contains(&"--working-directory=/tmp"), "{args:?}");
        assert!(args.contains(&"--property=ExitType=cgroup"), "running while any of its processes does: {args:?}");
    }

    #[test]
    fn a_job_starts_as_a_service_and_one_that_cant_run_fails() {
        let job = Job { unit: format!("clamshell-run-jobtest-{}-1", std::process::id()), slice: None, argv: vec!["true".into()], env: vec![], workdir: None };
        let empty = Job { argv: vec![], ..job.clone() };
        assert!(matches!(start(&empty), Err(Error::EmptyCommand)));
        // Only where there is a user manager to start the service.
        if std::env::var_os("XDG_RUNTIME_DIR").is_some_and(|d| std::path::Path::new(&d).join("systemd/private").exists()) {
            start(&job).unwrap();
            let missing = Job { unit: format!("clamshell-run-jobtest-{}-2", std::process::id()), argv: vec!["/nonexistent/program".into()], ..job };
            assert!(matches!(start(&missing), Err(Error::NotStarted(_))));
        }
    }
}
