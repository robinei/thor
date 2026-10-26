//! Runs the system's command-line tools (`wpctl`, `pw-dump`, `bsdtar`, ...) with a time limit, so a
//! wedged service can't hang the thread that asked. A program that runs past the limit is killed and
//! counts as failed.

use std::ffi::OsStr;
use std::io::{Read, Write};
use std::process::{Command, ExitStatus, Stdio};
use std::sync::mpsc;
use std::time::{Duration, Instant};

/// How long a tool may take, unless a call says otherwise.
pub(crate) const LIMIT: Duration = Duration::from_secs(3);

/// How long the output may still take to arrive once the program has ended. A process it started may
/// hold the pipes open, so after this the output is given up rather than waited for.
const DRAIN: Duration = Duration::from_secs(1);

/// How a program ended.
pub(crate) struct Ran {
    pub(crate) status: ExitStatus,
    pub(crate) stdout: Vec<u8>,
    pub(crate) stderr: Vec<u8>,
}

impl Ran {
    /// The first non-empty line of standard error, trimmed. It usually says what went wrong.
    pub(crate) fn said(&self) -> String {
        let err = String::from_utf8_lossy(&self.stderr);
        err.lines().map(str::trim).find(|l| !l.is_empty()).unwrap_or_default().to_owned()
    }
}

/// Runs `program` with `args` and `input` on its standard input, for at most `limit`. Returns how it
/// ended, or an error if it couldn't start or ran past the limit and was killed.
pub(crate) fn run<S: AsRef<OsStr>>(program: &str, args: &[S], input: Option<&[u8]>, limit: Duration) -> Result<Ran, String> {
    let mut child = Command::new(program)
        .args(args)
        .stdin(if input.is_some() { Stdio::piped() } else { Stdio::null() })
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| format!("{program}: {e}"))?;
    if let (Some(input), Some(mut stdin)) = (input, child.stdin.take()) {
        stdin.write_all(input).map_err(|e| format!("{program}: {e}"))?;
    }
    // Read the outputs as they come, each on a thread of its own, since a full pipe would stall the program.
    let stdout = drain(program, child.stdout.take());
    let stderr = drain(program, child.stderr.take());
    let deadline = Instant::now() + limit;
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) if Instant::now() < deadline => std::thread::sleep(Duration::from_millis(10)),
            Ok(None) => {
                if let Err(e) = child.kill().and_then(|()| child.wait().map(drop)) {
                    log::debug!("{program}: stopping it: {e}");
                }
                return Err(format!("{program} {}: no answer in {} s, stopped", shown(args), limit.as_secs()));
            }
            Err(e) => return Err(format!("{program}: {e}")),
        }
    };
    let ended = Instant::now() + DRAIN;
    let collect = |rx: mpsc::Receiver<Vec<u8>>| {
        rx.recv_timeout(ended.saturating_duration_since(Instant::now())).inspect_err(|e| log::debug!("{program}: its output: {e}")).unwrap_or_default()
    };
    Ok(Ran { status, stdout: collect(stdout), stderr: collect(stderr) })
}

/// Arguments as a log line shows them.
fn shown<S: AsRef<OsStr>>(args: &[S]) -> String {
    args.iter().map(|a| a.as_ref().to_string_lossy()).collect::<Vec<_>>().join(" ")
}

/// Reads a pipe to its end on a thread of its own and sends what it held through the channel.
pub(crate) fn drain(program: &str, pipe: Option<impl Read + Send + 'static>) -> mpsc::Receiver<Vec<u8>> {
    let (tx, rx) = mpsc::channel();
    // If the thread can't start, its sender is dropped and the output reads as empty.
    if let Some(mut pipe) = pipe {
        let name = program.to_owned();
        let started = crate::os::worker::detached("output", move || {
            let mut out = Vec::new();
            if let Err(e) = pipe.read_to_end(&mut out) {
                log::warn!("{name}: reading its output: {e}");
            }
            if tx.send(out).is_err() {
                log::debug!("{name}: its output came after it was given up");
            }
        });
        if let Err(e) = started {
            log::warn!("{program}: reading its output: {e}");
        }
    }
    rx
}

/// Runs `program` with `args` within [`LIMIT`] and returns its standard output if it succeeded. Failures
/// are logged.
pub(crate) fn output(program: &str, args: &[&str]) -> Option<Vec<u8>> {
    match run(program, args, None, LIMIT) {
        Ok(ran) if ran.status.success() => Some(ran.stdout),
        Ok(ran) => {
            log::warn!("{program} {}: {}", args.join(" "), ran.status);
            None
        }
        Err(e) => {
            log::warn!("{e}");
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn outputs_and_failures() {
        assert_eq!(output("echo", &["hi"]).as_deref(), Some(&b"hi\n"[..]));
        assert!(output("false", &[]).is_none());
        assert!(output("clamshell-no-such-program", &[]).is_none());
    }

    #[test]
    fn input_and_what_it_said() {
        let ran = run("sh", &["-c", "cat; echo '  oops  ' >&2; exit 3"], Some(b"in"), LIMIT).ok();
        assert_eq!(ran.as_ref().map(|r| r.stdout.as_slice()), Some(&b"in"[..]));
        assert_eq!(ran.as_ref().map(|r| (r.status.code(), r.said())), Some((Some(3), "oops".to_owned())));
    }

    #[test]
    fn past_the_limit_it_is_stopped() {
        let start = Instant::now();
        assert!(run("sleep", &["5"], None, Duration::from_millis(200)).is_err());
        assert!(start.elapsed() < Duration::from_secs(2));
    }

    #[test]
    fn a_process_left_holding_the_output_does_not_hold_it_up() {
        // The shell ends at once, but the sleep it started keeps stdout open.
        let start = Instant::now();
        let ran = run("sh", &["-c", "echo out; sleep 5 &"], None, LIMIT);
        assert!(ran.is_ok_and(|r| r.status.success()));
        assert!(start.elapsed() < Duration::from_secs(3));
    }
}
