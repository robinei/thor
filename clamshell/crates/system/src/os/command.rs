//! Runs the system's command-line tools (`wpctl`, `pw-dump`, `bsdtar`, ...) with a time limit, so a
//! wedged service can't hang the thread that asked. A program that runs past the limit is killed, with
//! what it started, and counts as failed.

use std::ffi::OsStr;
use std::io::{Read, Write};
use std::os::unix::process::CommandExt;
use std::process::{Command, ExitStatus, Stdio};
use std::sync::mpsc;
use std::time::{Duration, Instant};

/// How long a tool may take, unless a call says otherwise.
pub(crate) const LIMIT: Duration = Duration::from_secs(3);

/// The most of each output kept. The rest is read and dropped, so the program isn't held up by a full
/// pipe. curl's downloads (64 MiB at most) are the largest.
const OUTPUT_MOST: usize = 80 << 20;

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
    // In a process group of its own, so what it started is killed with it past the limit, and ends the
    // threads reading its output.
    let mut child = Command::new(program)
        .args(args)
        .process_group(0)
        .stdin(if input.is_some() { Stdio::piped() } else { Stdio::null() })
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| format!("{program}: {e}"))?;
    // Read the outputs as they come, each on a thread of its own, since a full pipe would stall the program;
    // and write the input on one too, so that a program writing out before it has read all of it can't
    // stall it either.
    let stdout = drain_at_most(program, child.stdout.take(), OUTPUT_MOST);
    let stderr = drain_at_most(program, child.stderr.take(), OUTPUT_MOST);
    if let (Some(input), Some(stdin)) = (input, child.stdin.take()) {
        feed(program, stdin, input.to_vec());
    }
    let deadline = Instant::now() + limit;
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) if Instant::now() < deadline => std::thread::sleep(Duration::from_millis(10)),
            Ok(None) => {
                let how = stop(program, child);
                return Err(format!("{program} {}: no answer in {} s, {how}", shown(args), limit.as_secs()));
            }
            Err(e) => {
                let how = stop(program, child);
                return Err(format!("{program}: {e}; {how}"));
            }
        }
    };
    let ended = Instant::now() + DRAIN;
    let collect = |rx: mpsc::Receiver<Vec<u8>>| {
        rx.recv_timeout(ended.saturating_duration_since(Instant::now())).inspect_err(|e| log::debug!("{program}: its output: {e}")).unwrap_or_default()
    };
    Ok(Ran { status, stdout: collect(stdout), stderr: collect(stderr) })
}

/// Kills `child`'s process group, and says how that went. However it went, the child is waited for on a
/// thread of its own, so none is left a zombie and the caller isn't held: one that can't be killed (a
/// program run as root through pkexec) is still running, and is reaped when it ends.
fn stop(program: &str, mut child: std::process::Child) -> &'static str {
    let group = i32::try_from(child.id()).ok().and_then(rustix::process::Pid::from_raw);
    let killed = match group.map(|g| rustix::process::kill_process_group(g, rustix::process::Signal::KILL)) {
        Some(Ok(())) => true,
        Some(Err(e)) => {
            log::warn!("{program}: killing it: {e}");
            // What can be: the program itself, if it is ours.
            child.kill().is_ok()
        }
        None => child.kill().is_ok(),
    };
    let name = program.to_owned();
    let reaped = super::worker::detached("reaper", move || {
        if let Err(e) = child.wait() {
            log::debug!("{name}: waiting for it: {e}");
        }
    });
    if let Err(e) = reaped {
        log::warn!("{program}: waiting for it: {e}");
    }
    if killed { "stopped" } else { "still running" }
}

/// Arguments as a log line shows them.
fn shown<S: AsRef<OsStr>>(args: &[S]) -> String {
    args.iter().map(|a| a.as_ref().to_string_lossy()).collect::<Vec<_>>().join(" ")
}

/// Writes `input` to a program's standard input on a thread of its own, then closes it. A program that
/// ends before reading it all is no error of ours: what it says is in its status and output.
fn feed(program: &str, mut stdin: std::process::ChildStdin, input: Vec<u8>) {
    let name = program.to_owned();
    let fed = super::worker::detached("command input", move || {
        if let Err(e) = stdin.write_all(&input) {
            log::debug!("{name}: its input: {e}");
        }
    });
    if let Err(e) = fed {
        log::warn!("{program}: its input: {e}");
    }
}

/// Reads a pipe to its end on a thread of its own and sends what it held through the channel.
pub(crate) fn drain(program: &str, pipe: Option<impl Read + Send + 'static>) -> mpsc::Receiver<Vec<u8>> {
    drain_at_most(program, pipe, OUTPUT_MOST)
}

/// [`drain`], keeping at most `most` bytes: the rest is read and dropped, which is logged.
fn drain_at_most(program: &str, pipe: Option<impl Read + Send + 'static>, most: usize) -> mpsc::Receiver<Vec<u8>> {
    let (tx, rx) = mpsc::channel();
    // If the thread can't start, its sender is dropped and the output reads as empty.
    if let Some(mut pipe) = pipe {
        let name = program.to_owned();
        let started = crate::os::worker::detached("output", move || {
            let mut out = Vec::new();
            let most = u64::try_from(most).unwrap_or(u64::MAX);
            if let Err(e) = (&mut pipe).take(most).read_to_end(&mut out) {
                log::warn!("{name}: reading its output: {e}");
            }
            match std::io::copy(&mut pipe, &mut std::io::sink()) {
                Ok(0) => {}
                Ok(n) => log::warn!("{name}: {n} bytes of its output past {most} dropped"),
                Err(e) => log::debug!("{name}: reading its output: {e}"),
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
#[allow(clippy::unwrap_used)]
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

    /// A big input to a program that writes as it reads, and one that ends without reading it.
    #[test]
    fn a_big_input_doesnt_stall_and_one_not_read_isnt_an_error() {
        let big = vec![b'a'; 1 << 20];
        let ran = run("cat", &[] as &[&str], Some(&big), LIMIT);
        assert_eq!(ran.map(|r| r.stdout.len()), Ok(big.len()));
        let ran = run("true", &[] as &[&str], Some(&big), LIMIT);
        assert!(ran.is_ok_and(|r| r.status.success()));
    }

    #[test]
    fn past_the_limit_it_is_stopped() {
        let start = Instant::now();
        assert!(run("sleep", &["5"], None, Duration::from_millis(200)).is_err());
        assert!(start.elapsed() < Duration::from_secs(2));
    }

    /// Past the limit, what the program started is killed with it: nothing is left running, holding its
    /// output's reader.
    #[test]
    fn past_the_limit_what_it_started_is_stopped_too() {
        let dir = tempfile::tempdir().unwrap();
        let pid_file = dir.path().join("pid");
        let script = format!("sleep 30 & echo $! > {}; wait", pid_file.display());
        let ran = run("sh", &["-c", script.as_str()], None, Duration::from_millis(300));
        assert!(ran.is_err_and(|e| e.ends_with("stopped")));
        let pid = std::fs::read_to_string(&pid_file).unwrap().trim().to_owned();
        let gone = (0..100).any(|_| {
            let state = std::fs::read_to_string(format!("/proc/{pid}/stat")).unwrap_or_default();
            // Gone, or a zombie waiting for its new parent to reap it.
            let running = state.rsplit_once(") ").is_some_and(|(_, rest)| !rest.starts_with('Z'));
            if running {
                std::thread::sleep(Duration::from_millis(20));
            }
            !running
        });
        assert!(gone, "the sleep it started was killed");
    }

    /// Output past what is kept is read and dropped, so the program ends rather than waiting on a full
    /// pipe.
    #[test]
    fn output_past_what_is_kept_is_dropped() {
        let rx = drain_at_most("head", Some(&b"0123456789"[..]), 4);
        assert_eq!(rx.recv_timeout(Duration::from_secs(2)).unwrap(), b"0123");
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
