//! Running the system's command-line tools (`wpctl`, `pw-dump`, `systemctl`) with a time limit, so a
//! wedged service can't hang the thread that asked: past it, the program is killed and counts as failed.

use std::io::Read;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

/// How long a tool may take.
pub const LIMIT: Duration = Duration::from_secs(3);

/// Runs `program` with `args`; its standard output if it succeeded within the limit (failures logged).
pub fn output(program: &str, args: &[&str]) -> Option<Vec<u8>> {
    output_with(program, args, true)
}

/// Whether `program` with `args` succeeds within the limit, for a question it answers with its exit
/// status (a failure is an answer, not logged).
pub fn succeeds(program: &str, args: &[&str]) -> bool {
    output_with(program, args, false).is_some()
}

fn output_with(program: &str, args: &[&str], log_failure: bool) -> Option<Vec<u8>> {
    let mut child = match Command::new(program).args(args).stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::null()).spawn() {
        Ok(child) => child,
        Err(e) => {
            log::debug!("{program}: {e}");
            return None;
        }
    };
    // Read as it comes (a full pipe would stall it), on a thread of its own.
    let mut stdout = child.stdout.take()?;
    let reader = std::thread::spawn(move || {
        let mut out = Vec::new();
        stdout.read_to_end(&mut out).map(|_| out)
    });
    let deadline = Instant::now() + LIMIT;
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break Some(status),
            Ok(None) if Instant::now() < deadline => std::thread::sleep(Duration::from_millis(10)),
            Ok(None) => {
                log::warn!("{program} {}: no answer in {} s, stopped", args.join(" "), LIMIT.as_secs());
                let _ = child.kill();
                let _ = child.wait();
                break None;
            }
            Err(e) => {
                log::warn!("{program}: {e}");
                break None;
            }
        }
    };
    let out = reader.join().ok().and_then(Result::ok);
    match status {
        Some(s) if s.success() => out,
        Some(s) => {
            if log_failure {
                log::warn!("{program} {}: {s}", args.join(" "));
            }
            None
        }
        None => None,
    }
}

/// Runs `program` with `args`; whether it succeeded within the limit (failures logged).
pub fn run(program: &str, args: &[&str]) -> bool {
    output(program, args).is_some()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn outputs_and_failures() {
        assert_eq!(output("echo", &["hi"]).as_deref(), Some(&b"hi\n"[..]));
        assert!(!run("false", &[]) && !succeeds("false", &[]) && succeeds("true", &[]));
        assert!(!run("clamshell-no-such-program", &[]));
    }
}
