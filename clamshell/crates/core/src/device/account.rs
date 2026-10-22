//! The device's account. The session's user starts without a password, which means it is locked, with
//! no sudo and no ssh. A password is set in the Device view ([`crate::device`]), and once there is one,
//! remote login (ssh) can be turned on.
//!
//! The system side is the image's root helper, `thor-account`, which polkit lets the session's user run.
//! The account is read while the Device view is open and after each change. Off the device there is no
//! helper, and all of it shows as unavailable.

use crate::bottom::editor::{Editor, Editors, Purpose};
use crate::effect::Effect;
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

/// The password editor was saved with `text`. After the current password it asks for the new one, and
/// after the new one it sets the password.
pub(crate) fn password_typed(step: PasswordStep, text: String, editors: &mut Editors) -> Vec<Effect> {
    match step {
        PasswordStep::Current => {
            let step = PasswordStep::New { current: Some(Secret(text)) };
            editors.open(Editor { purpose: Purpose::AccountPassword(step), text: String::new() });
            vec![]
        }
        PasswordStep::New { current } => vec![Effect::Account(AccountCommand::Password { current, new: Secret(text) })],
    }
}
