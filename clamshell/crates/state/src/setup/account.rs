//! The device's account. The session's user starts without a password, which means it is locked, with
//! no sudo and no ssh. A password is set in the Device view ([`crate::setup`]), and once there is one,
//! remote login (ssh) can be turned on.
//!
//! The system side is the image's root helper, `thor-account`, which polkit lets the session's user run.
//! The account is read while the Device view is open and after each change. Off the device there is no
//! helper, and all of it shows as unavailable.

use crate::bottom::editor::{Editor, Editors, Purpose};
use crate::effect::Effect;
use crate::kit::secret::Secret;
use crate::out::Out;

/// The account, as last read. Launches and Setup are functions of it (see [`crate::setup::wizard`]).
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum Account {
    /// Not read yet since ClamShell started.
    #[default]
    Unread,
    /// There is no helper, as off the device: nothing to set.
    NoHelper,
    /// The helper is there but couldn't read it: why. It is read again, after a wait that grows with each
    /// failure in a row (see [`crate::setup::Setup::reread_due`]).
    Failed(String),
    /// Read: whether the session's user has a password, and whether remote login (sshd) is on.
    Read { password: bool, ssh: bool },
}

impl Account {
    /// Whether the helper read it, and so can change it.
    pub fn available(&self) -> bool {
        matches!(self, Account::Read { .. })
    }

    pub fn password(&self) -> bool {
        matches!(self, Account::Read { password: true, .. })
    }

    pub fn ssh(&self) -> bool {
        matches!(self, Account::Read { ssh: true, .. })
    }

    /// Whether the user's programs may run: the account has a password, or there is no helper to give
    /// it one. Unread or unreadable, it may have none, which any program of the user could then set.
    pub fn lets_programs_run(&self) -> bool {
        matches!(self, Account::NoHelper | Account::Read { password: true, .. })
    }
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
    /// The new one again, to be sure of it.
    Again { current: Option<Secret>, new: Secret },
}

/// What is said when the new password typed again differs.
pub const DIFFERENT: &str = "The two passwords differ";

/// The password editor was saved with `text`. After the current password it asks for the new one, after
/// the new one for it again, and if that is the same it sets the password. If not, that is answered as the
/// helper's failures are ([`crate::Event::AccountChanged`]).
pub(crate) fn password_typed(step: PasswordStep, text: String, editors: &mut Editors, out: &mut Out) {
    let next = match step {
        PasswordStep::Current => PasswordStep::New { current: Some(Secret(text)) },
        PasswordStep::New { current } => PasswordStep::Again { current, new: Secret(text) },
        PasswordStep::Again { current, new } if new.0 == text => {
            out.effects.push(Effect::Account(AccountCommand::Password { current, new }));
            return;
        }
        PasswordStep::Again { .. } => {
            out.events.push(crate::Event::AccountChanged(Err(DIFFERENT.into())));
            return;
        }
    };
    editors.open(Editor { purpose: Purpose::AccountPassword(next), text: String::new() });
}
