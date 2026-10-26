//! Starting a program as a transient systemd service of the user manager: a launch of ours, an app or a
//! game, and an archive's mount (see `files::archives`). Its output is in the journal as the
//! unit's, its processes in its cgroup, and systemd starts and stops it. ClamShell needs a systemd user
//! manager for this. (A program found running is moved into a scope over D-Bus instead, see
//! [`crate::launch::units::adopt`]: `systemd-run` only starts new processes.)

use std::ffi::OsString;
use std::fmt;
use std::io;
use std::os::unix::process::CommandExt;
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};

use crate::os::worker;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Job {
    /// The service's name (without `.service`).
    pub(crate) unit: String,
    /// The slice its service goes in, if any. A dock app's slice holds all its instances.
    pub(crate) slice: Option<String>,
    /// Program and arguments.
    pub(crate) argv: Vec<OsString>,
    /// Its environment, beside ClamShell's own.
    pub(crate) env: Vec<(String, String)>,
    pub(crate) workdir: Option<PathBuf>,
    /// More of the service's properties, as `Name=value`.
    pub(crate) properties: Vec<String>,
}

#[derive(Debug)]
pub(crate) enum Error {
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

/// What systemd sets for ClamShell's own unit, which a launch's unit doesn't take from it.
const UNIT_OWN: [&str; 13] = [
    "NOTIFY_SOCKET",
    "WATCHDOG_USEC",
    "WATCHDOG_PID",
    "JOURNAL_STREAM",
    "INVOCATION_ID",
    "MANAGERPID",
    "MANAGERPIDFDID",
    "SYSTEMD_EXEC_PID",
    "LISTEN_FDS",
    "LISTEN_PID",
    "LISTEN_FDNAMES",
    "MEMORY_PRESSURE_WATCH",
    "MEMORY_PRESSURE_WRITE",
];

/// The `systemd-run` that starts a job as the service `<unit>.service`, in its slice if it has one, in
/// its workdir or else HOME, with its environment. `systemd-run` returns once systemd has started it, or
/// failed to.
pub(crate) fn command(job: &Job) -> Option<Command> {
    let (program, args) = job.argv.split_first()?;
    let mut cmd = Command::new("systemd-run");
    cmd.args(["--user", "--quiet", "--collect", &format!("--unit={}", job.unit)]);
    // Started once its program runs, so one that can't be run fails the launch; running as long as any
    // of its processes does, as a launcher script's game outlives it; delegated, so what runs in it may
    // move processes into it, as a Steam game's follower moves Steam's launcher.
    cmd.args(["--service-type=exec", "--property=ExitType=cgroup", "--property=Delegate=yes"]);
    cmd.args(job.properties.iter().map(|p| format!("--property={p}")));
    // (The arguments as they are: systemd-run would expand `$` in them.)
    cmd.arg("--expand-environment=no");
    if let Some(slice) = &job.slice {
        cmd.arg(format!("--slice={slice}"));
    }
    let workdir = job.workdir.clone().unwrap_or_else(|| std::env::var_os("HOME").map_or_else(|| PathBuf::from("/"), PathBuf::from));
    cmd.arg(format!("--working-directory={}", workdir.display()));
    // ClamShell's own environment, as a program it started itself would have it (a service starts with
    // the user manager's: a ClamShell run with its own runtime or config dirs, nested or in a test, would
    // send its games to another compositor), then the job's own, which go before it.
    let own = std::env::vars().filter(|(k, _)| !UNIT_OWN.contains(&k.as_str()));
    cmd.args(own.chain(job.env.iter().cloned()).map(|(k, v)| format!("--setenv={k}={v}")));
    cmd.arg("--").arg(program).args(args);
    Some(cmd)
}

/// Waits for a child on a thread of its own, so it is reaped whenever it ends.
pub(crate) fn reap(mut child: Child) {
    if let Err(e) = worker::detached("reaper", move || drop(child.wait())) {
        log::warn!("reaper: {e}");
    }
}

/// Starts a job's service, returning once systemd has started it.
pub(crate) fn start(job: &Job) -> Result<(), Error> {
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
            properties: vec!["ExecStopPost=/bin/true".into()],
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
        assert!(args.contains(&"--service-type=exec"), "started once its program runs: {args:?}");
        assert!(args.contains(&"--property=Delegate=yes"), "delegated, for the Steam follower: {args:?}");
        assert!(args.contains(&"--property=ExecStopPost=/bin/true"), "its own properties: {args:?}");
    }

    /// Starts services on this machine's user manager. Run it by hand with `--ignored`.
    #[test]
    #[ignore = "needs a user manager; starts services"]
    fn a_job_starts_as_a_service_and_one_that_cant_run_fails() {
        let job = Job {
            unit: format!("clamshell-run-jobtest-{}-1", std::process::id()),
            slice: None,
            argv: vec!["true".into()],
            env: vec![],
            workdir: None,
            properties: vec![],
        };
        let empty = Job { argv: vec![], ..job.clone() };
        assert!(matches!(start(&empty), Err(Error::EmptyCommand)));
        start(&job).unwrap();
        let missing = Job { unit: format!("clamshell-run-jobtest-{}-2", std::process::id()), argv: vec!["/nonexistent/program".into()], ..job };
        assert!(matches!(start(&missing), Err(Error::NotStarted(_))));
    }
}
