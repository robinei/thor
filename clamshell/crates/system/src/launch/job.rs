//! Starting a program as an instance of ours, an app or a game, in its systemd scope. ClamShell needs a
//! systemd user manager for this. Under sway the program starts through sway's `exec`, aimed at the
//! instance's workspace, so its first window opens there. Elsewhere it starts as a child process.
//!
//! A launch is handed to sway's `exec` without its arguments going through sway's command parser, which
//! splits at ';' and ','. Instead the job is written to a file and sway runs `clamshell --run <file>`.
//! That reads the file, deletes it and execs the program inside the job's systemd scope. Because exec
//! keeps the same process, sway's launch tracking and its startup token carry over.

use std::fmt;
use std::io;
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};

use serde::{Deserialize, Serialize};

use crate::os::worker::{self, Worker};
use crate::sway;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Job {
    /// The scope's name (without `.scope`).
    pub unit: String,
    /// The workspace its first window opens on, under sway.
    pub workspace: String,
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
    Json(serde_json::Error),
    EmptyCommand,
    SwayGone,
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::Io(e) => write!(f, "{e}"),
            Error::Json(e) => write!(f, "{e}"),
            Error::EmptyCommand => write!(f, "empty command"),
            Error::SwayGone => write!(f, "the sway thread is gone"),
        }
    }
}

impl From<io::Error> for Error {
    fn from(e: io::Error) -> Self {
        Error::Io(e)
    }
}

impl From<serde_json::Error> for Error {
    fn from(e: serde_json::Error) -> Self {
        Error::Json(e)
    }
}

/// Writes the job into `dir`, where `--run` will read it, and returns its path.
pub fn write(dir: &Path, job: &Job) -> Result<PathBuf, Error> {
    std::fs::create_dir_all(dir)?;
    let path = dir.join(format!("{}.json", job.unit));
    std::fs::write(&path, serde_json::to_vec(job)?)?;
    Ok(path)
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

/// Handles `--run <file>` by becoming the job's program. Returns only if that fails.
pub fn run(path: &Path) -> Error {
    let job: Job = match std::fs::read(path).map_err(Error::from).and_then(|b| Ok(serde_json::from_slice(&b)?)) {
        Ok(job) => job,
        Err(e) => return e,
    };
    if let Err(e) = std::fs::remove_file(path) {
        log::warn!("removing {}: {e}", path.display());
    }
    match command(&job) {
        Some(mut cmd) => cmd.exec().into(),
        None => Error::EmptyCommand,
    }
}

/// Waits for a child on a thread of its own, so it is reaped whenever it ends. Whether it runs is for
/// systemd's list, or the caller, to say.
pub fn reap(mut child: Child) {
    if let Err(e) = worker::detached("reaper", move || drop(child.wait())) {
        log::warn!("reaper: {e}");
    }
}

/// How a job was started.
#[derive(Debug)]
pub enum Started {
    /// Started by sway. It runs once its scope is there.
    Sway,
    Child(Child),
}

/// Starts jobs through sway where there is sway, writing them to `jobs`, and otherwise as child
/// processes.
#[derive(Clone)]
pub struct Runner {
    pub jobs: PathBuf,
    pub sway: Option<Worker<sway::Command>>,
}

impl Runner {
    pub fn start(&self, job: &Job) -> Result<Started, Error> {
        if let Some(sway) = &self.sway {
            let path = write(&self.jobs, job)?;
            sway.try_send(sway::Command::Launch { workspace: job.workspace.clone(), job: path }).map_err(|_| Error::SwayGone)?;
            return Ok(Started::Sway);
        }
        let mut cmd = command(job).ok_or(Error::EmptyCommand)?;
        // In its own process group, our signals, such as a Ctrl-C in a terminal, don't reach it.
        Ok(Started::Child(cmd.stdin(Stdio::null()).process_group(0).spawn()?))
    }

    /// The same, without sway: jobs start as child processes.
    pub fn without_sway(&self) -> Runner {
        Runner { jobs: self.jobs.clone(), sway: None }
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    #[test]
    fn a_job_round_trips_and_runs_in_its_scope() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path();
        let job = Job {
            unit: format!("clamshell-run-jobtest-{}-0", std::process::id()),
            workspace: "clamshell-run-jobtest".into(),
            slice: Some("app-graphical-clamshell-run-jobtest.slice".into()),
            argv: vec!["prog".into(), "a b,c;d".into()],
            env: vec![("K".into(), "v".into())],
            workdir: Some("/tmp".into()),
        };
        let path = write(dir, &job).unwrap();
        let back: Job = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
        assert_eq!(back, job);
        let cmd = command(&job).unwrap();
        let args: Vec<&str> = cmd.get_args().filter_map(|a| a.to_str()).collect();
        assert_eq!(cmd.get_program(), "systemd-run");
        assert_eq!(args.last(), Some(&"a b,c;d"), "arguments untouched");
        assert!(args.contains(&format!("--unit={}", job.unit).as_str()));
        assert!(args.contains(&"--slice=app-graphical-clamshell-run-jobtest.slice"), "in its slice: {args:?}");
    }

    #[test]
    fn without_sway_a_job_is_a_child_in_its_scope() {
        let runner = Runner { jobs: std::env::temp_dir(), sway: None };
        let unit = format!("clamshell-run-jobtest-{}-1", std::process::id());
        let job = Job { unit, workspace: "u".into(), slice: None, argv: vec!["true".into()], env: vec![], workdir: None };
        let empty = Job { argv: vec![], ..job.clone() };
        assert!(matches!(runner.start(&empty), Err(Error::EmptyCommand)));
        // Only where there is a user manager to start the scope.
        if std::env::var_os("XDG_RUNTIME_DIR").is_some_and(|d| std::path::Path::new(&d).join("systemd/private").exists()) {
            let started = runner.start(&job).unwrap();
            assert!(matches!(&started, Started::Child(_)));
            if let Started::Child(mut child) = started {
                assert!(child.wait().unwrap().success());
            }
        }
    }
}
