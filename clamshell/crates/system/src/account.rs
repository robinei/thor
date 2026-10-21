//! The account through the image's root helper (tools/device's `thor-account`), run with pkexec (polkit
//! lets the active session's user run it without asking): `status` reads whether the session's user has
//! a password and whether remote login (sshd) is on; `password` sets it (the current one first, when
//! there is one: on its stdin, a line each); `ssh on` / `ssh off`. Off the device (no helper) the account
//! reads as unavailable.

use std::io::Write as _;
use std::path::Path;
use std::process::{Command, Stdio};

use clamshell_core::account::{Account, AccountCommand};

/// The helper.
pub const HELPER: &str = "/usr/lib/thor/thor-account";

/// The account as the helper reads it; unavailable without it (or if it fails).
pub fn read() -> Account {
    if !Path::new(HELPER).exists() {
        return Account::default();
    }
    match run(&["status"], None) {
        Ok(out) => parse(&out),
        Err(e) => {
            log::warn!("account: {e}");
            Account::default()
        }
    }
}

/// What the helper's `status` says.
fn parse(out: &str) -> Account {
    let line = |l: &str| out.lines().any(|x| x.trim() == l);
    Account { available: true, password: line("password set"), ssh: line("ssh on") }
}

/// A change asked of the helper: what it did, or why it didn't. ([`AccountCommand::Read`] is [`read`]'s.)
pub fn change(command: &AccountCommand) -> Result<String, String> {
    if !Path::new(HELPER).exists() {
        return Err("Not on this system".to_owned());
    }
    match command {
        AccountCommand::Read => Ok(String::new()),
        AccountCommand::Password { current, new } => {
            let mut input = String::new();
            if let Some(current) = current {
                input.push_str(&current.0);
                input.push('\n');
            }
            input.push_str(&new.0);
            input.push('\n');
            run(&["password"], Some(&input)).map(|_| "Password set".to_owned())
        }
        AccountCommand::Ssh(on) => {
            run(&["ssh", if *on { "on" } else { "off" }], None).map(|_| if *on { "Remote login on" } else { "Remote login off" }.to_owned())
        }
    }
}

/// Runs the helper through pkexec with `args`, `input` on its stdin: its output, or its message (its
/// first line on stderr) when it fails.
fn run(args: &[&str], input: Option<&str>) -> Result<String, String> {
    let mut child = Command::new("pkexec")
        .arg(HELPER)
        .args(args)
        .stdin(if input.is_some() { Stdio::piped() } else { Stdio::null() })
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| format!("pkexec: {e}"))?;
    if let (Some(text), Some(mut stdin)) = (input, child.stdin.take()) {
        stdin.write_all(text.as_bytes()).map_err(|e| format!("pkexec: {e}"))?;
    }
    let out = child.wait_with_output().map_err(|e| format!("pkexec: {e}"))?;
    if out.status.success() {
        return Ok(String::from_utf8_lossy(&out.stdout).into_owned());
    }
    let err = String::from_utf8_lossy(&out.stderr);
    let first = err.lines().find(|l| !l.trim().is_empty()).unwrap_or("failed").trim();
    Err(capitalised(first))
}

/// "wrong password" as a notice says it: "Wrong password".
fn capitalised(text: &str) -> String {
    let mut chars = text.chars();
    chars.next().map(|c| c.to_uppercase().chain(chars).collect()).unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_helpers_status_and_messages() {
        assert_eq!(parse("password set\nssh off\n"), Account { available: true, password: true, ssh: false });
        assert_eq!(parse("password none\nssh on\n"), Account { available: true, password: false, ssh: true });
        assert_eq!(capitalised("wrong password"), "Wrong password");
        assert_eq!(capitalised(""), "");
    }
}
