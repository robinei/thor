//! A launch handed to sway's `exec` without its arguments going through sway's command parser (which
//! splits at ';' and ','): the job is written to a file, sway runs `sysprog --run <file>`, and that reads
//! it, deletes it and becomes the program (exec: the same process, so sway's launch tracking and its
//! startup token carry over), inside the job's systemd scope.

use std::io;
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::Command;

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Job {
    /// The scope's name (without `.scope`), also the instance's workspace for apps.
    pub unit: String,
    /// Program and arguments.
    pub argv: Vec<String>,
    /// Extra environment.
    pub env: Vec<(String, String)>,
    pub workdir: Option<PathBuf>,
    /// In a systemd scope (where there is a user session).
    pub scope: bool,
}

fn dir() -> PathBuf {
    std::env::var_os("XDG_RUNTIME_DIR").map_or_else(std::env::temp_dir, PathBuf::from).join("sysprog/jobs")
}

/// Writes the job where `--run` will read it; returns its path.
pub fn write(job: &Job) -> io::Result<PathBuf> {
    let dir = dir();
    std::fs::create_dir_all(&dir)?;
    let path = dir.join(format!("{}.json", job.unit));
    std::fs::write(&path, serde_json::to_vec(job).map_err(io::Error::other)?)?;
    Ok(path)
}

/// The command a job runs as.
pub fn command(job: &Job) -> Option<Command> {
    let (program, args) = job.argv.split_first()?;
    let mut cmd = if job.scope {
        let mut cmd = Command::new("systemd-run");
        cmd.args(["--user", "--scope", "--quiet", "--collect", &format!("--unit={}", job.unit), "--"]).arg(program).args(args);
        cmd
    } else {
        let mut cmd = Command::new(program);
        cmd.args(args);
        cmd
    };
    match &job.workdir {
        Some(dir) => cmd.current_dir(dir),
        None => cmd.current_dir(std::env::var_os("HOME").map_or_else(|| PathBuf::from("/"), PathBuf::from)),
    };
    cmd.envs(job.env.iter().map(|(k, v)| (k, v)));
    Some(cmd)
}

/// `--run <file>`: becomes the job's program. Returns only if that fails.
pub fn run(path: &Path) -> io::Error {
    let job: Job = match std::fs::read(path).map_err(io::Error::other).and_then(|b| serde_json::from_slice(&b).map_err(io::Error::other)) {
        Ok(job) => job,
        Err(e) => return e,
    };
    let _ = std::fs::remove_file(path);
    match command(&job) {
        Some(mut cmd) => cmd.exec(),
        None => io::Error::other("empty command"),
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    #[test]
    fn a_job_round_trips_and_runs_in_its_scope() {
        let job = Job {
            unit: format!("sysprog-app-jobtest-{}-0", std::process::id()),
            argv: vec!["prog".into(), "a b,c;d".into()],
            env: vec![("K".into(), "v".into())],
            workdir: Some("/tmp".into()),
            scope: true,
        };
        let path = write(&job).unwrap();
        let back: Job = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
        assert_eq!(back, job);
        let _ = std::fs::remove_file(&path);
        let cmd = command(&job).unwrap();
        let args: Vec<&str> = cmd.get_args().filter_map(|a| a.to_str()).collect();
        assert_eq!(cmd.get_program(), "systemd-run");
        assert_eq!(args.last(), Some(&"a b,c;d"), "arguments untouched");
        assert!(args.contains(&format!("--unit={}", job.unit).as_str()));
    }
}
