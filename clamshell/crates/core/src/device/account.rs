//! The device's account. The session's user starts without a password, which means it is locked, with
//! no sudo and no ssh. A password is set in the Device view ([`crate::device`]), and once there is one,
//! remote login (ssh) can be turned on.
//!
//! The system side is the image's root helper, `thor-account`, which polkit lets the session's user run.
//! The account is read while the Device view is open and after each change. Off the device there is no
//! helper, and all of it shows as unavailable.

use crate::State;
use crate::bottom::notice::NoticeKind;
use crate::kit::secret::Secret;

/// The account as the helper read it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Account {
    /// The helper is there, as on the device's image, and answered.
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
    /// Set the password. `current` is given when there is one, and is `None` for the first password.
    Password { current: Option<Secret>, new: Secret },
    /// Turn remote login on or off.
    Ssh(bool),
}

/// Which of the password's two texts the editor asks for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PasswordStep {
    /// The current password, when there is one.
    Current,
    /// The new one, after the current one if there is one.
    New { current: Option<Secret> },
}

impl State {
    pub fn account(&self) -> Account {
        self.sys.account
    }

    /// Whether this is ClamShell's first start, which opens the Device view ([`crate::device`]).
    pub fn first_start(&self) -> bool {
        self.first_start
    }

    /// A change requested of the helper is done, with what it did, or failed, with the reason. It is
    /// shown for a moment.
    pub(crate) fn on_account_changed(&mut self, result: Result<String, String>) {
        self.notify(NoticeKind::Account(result));
    }
}
