//! The account, managed through thor-system's root helper `/usr/lib/thor/thor-account`. It is run with
//! pkexec, and polkit lets the active session's user run it without asking.
//!
//! - `status` reads whether the session's user has a password and whether remote login (sshd) is on.
//! - `password` sets the password. It reads the current password first, if there is one, then the new
//!   one, each on a line of its stdin.
//! - `ssh on` and `ssh off` turn remote login on and off.
//!
//! Off the device, where there is no helper, the account reads as unavailable.

use std::path::Path;
use std::time::Duration;

use clamshell_state::device::account::{Account, AccountCommand};

use crate::os::command;

/// The helper.
const HELPER: &str = "/usr/lib/thor/thor-account";

/// How long the helper may take. Setting a password hashes it, and turning remote login on starts sshd.
const HELPER_LIMIT: Duration = Duration::from_secs(30);

/// The account as the helper reads it. It is unavailable if there is no helper or it fails.
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

/// Asks the helper for a change. Returns what it did, or why it didn't. [`AccountCommand::Read`] is
/// handled by [`read`].
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

/// Runs the helper through pkexec with `args` and `input` on its stdin. Returns its output, or its
/// message from stderr when it fails.
fn run(args: &[&str], input: Option<&str>) -> Result<String, String> {
    let out = command::run("pkexec", &[&[HELPER], args].concat(), input.map(str::as_bytes), HELPER_LIMIT)?;
    if out.status.success() {
        return Ok(String::from_utf8_lossy(&out.stdout).into_owned());
    }
    let said = out.said();
    Err(capitalised(if said.is_empty() { "failed" } else { &said }))
}

/// A message capitalised for a notice: "wrong password" becomes "Wrong password".
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
