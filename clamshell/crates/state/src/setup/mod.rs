//! Setup, the first start's steps: a built-in app ([`clamshell_library::BuiltinApp::Setup`]) shown while
//! the account or the first start needs it (see [`wizard`]). Its area holds the account as last read (see
//! [`account`]), which also says whether the user's programs may run.

pub mod account;
pub mod wizard;

use account::Account;

/// Setup's state, and the account.
#[derive(Debug, Clone, Default)]
pub struct Setup {
    /// ClamShell's first start (see [`crate::Init::first_start`]): Setup isn't done.
    first_start: bool,
    /// The step of Setup reached (see [`wizard`]).
    step: wizard::Step,
    /// The account, as last read (see [`account`]).
    account: Account,
    /// Why the password last typed in Setup wasn't set, until another is (see [`wizard`]).
    password_problem: Option<String>,
}

impl Setup {
    /// Setup as ClamShell starts: shown at the first start (see [`wizard`]).
    pub(crate) fn new(first_start: bool, account: Account) -> Setup {
        Setup { first_start, account, ..Setup::default() }
    }

    /// Whether the account is read for itself: at start, so that one without a password brings Setup back
    /// (see [`wizard`]), and again until it reads. Setup and the Device view have it read while they run,
    /// as their definitions say (see [`crate::builtin_apps`]).
    pub fn reads_account(&self) -> bool {
        matches!(self.account, Account::Unread | Account::Failed(_))
    }

    /// Whether the user's programs are held: launches and autostart wait (see
    /// [`Account::lets_programs_run`]).
    pub fn holds_programs(&self) -> bool {
        !self.account.lets_programs_run()
    }

    /// Whether this is ClamShell's first start, Setup not being done.
    pub fn first_start(&self) -> bool {
        self.first_start
    }

    pub fn account(&self) -> &Account {
        &self.account
    }

    /// A reading of the account. A password set answers what went wrong with the one before.
    pub(crate) fn read_account(&mut self, account: Account) {
        if account.password() {
            self.password_problem = None;
        }
        self.account = account;
    }

    /// The account helper answered a change with `result`, or can't any more (its thread was started
    /// again). Its reading follows. A failure is kept for Setup to show.
    pub(crate) fn account_changed(&mut self, result: &Result<String, String>) {
        self.password_problem = result.as_ref().err().cloned();
    }
}
