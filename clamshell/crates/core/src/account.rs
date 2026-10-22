//! The device's account. The session's user starts without a password (locked: no sudo, no ssh); one is
//! set in the Device view ([`crate::device`]), and remote login (ssh) turned on once there is one. The
//! system's side is the image's root helper (`thor-account`, which polkit lets the session's user run):
//! read while the Device view is open and after each change. Off the device (no helper) all of it shows
//! as unavailable.

use crate::State;
use crate::hardware::NoticeKind;
use crate::secret::Secret;

/// The account as the helper read it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Account {
    /// The helper is there (the device's image), and answered.
    pub available: bool,
    /// The session's user has a password.
    pub password: bool,
    /// Remote login (sshd) is on.
    pub ssh: bool,
}

/// What the helper is asked to do.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AccountCommand {
    /// Read the account.
    Read,
    /// Set the password: `current` when there is one (none: the first).
    Password { current: Option<Secret>, new: Secret },
    /// Remote login on or off.
    Ssh(bool),
}

/// Which of the password's two texts the editor asks for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PasswordStep {
    /// The current one (there is one).
    Current,
    /// The new one, after the current one if there is one.
    New { current: Option<Secret> },
}

impl State {
    pub fn account(&self) -> Account {
        self.sys.account
    }

    /// Whether this is the first start: the setup not done yet (the Device view: [`crate::device`]).
    pub fn first_run(&self) -> bool {
        self.first_run
    }

    /// A change asked of the helper is done (what it did), or failed (why): shown for a moment.
    pub(crate) fn on_account_changed(&mut self, result: Result<String, String>) {
        self.notify(NoticeKind::Account(result));
    }
}
