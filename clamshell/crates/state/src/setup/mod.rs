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
    /// While the account reads Failed: when it is read again (monotonic seconds), and the wait before that.
    reread: Option<Reread>,
}

/// A read of the account again after a failed one, at `at`, `wait` seconds after the failure.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Reread {
    at: u64,
    wait: u64,
}

/// The wait before the account is read again after its first failed read, in seconds. It doubles with each
/// failure in a row, up to [`REREAD_MOST`].
pub const REREAD_FIRST: u64 = 2;
/// The longest wait between reads of an account that keeps failing to read, in seconds.
pub const REREAD_MOST: u64 = 60;

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

    /// A reading of the account at `now`. A password set answers what went wrong with the one before. A
    /// failed read is read again ([`Setup::reread_due`]): polkit can refuse pkexec for a while, as when
    /// ClamShell starts while the console's login is being replaced, and the watch that read it stays on.
    pub(crate) fn read_account(&mut self, account: Account, now: u64) {
        if account.password() {
            self.password_problem = None;
        }
        self.reread = match (&account, self.reread) {
            (Account::Failed(_), None) => Some(Reread { at: now + REREAD_FIRST, wait: REREAD_FIRST }),
            (Account::Failed(_), Some(last)) => {
                let wait = (last.wait * 2).min(REREAD_MOST);
                Some(Reread { at: now + wait, wait })
            }
            _ => None,
        };
        self.account = account;
    }

    /// Whether the account, which failed to read, is to be read again at `now`: [`REREAD_FIRST`] seconds
    /// after the first failure, the wait doubling with each failure in a row up to [`REREAD_MOST`]. The
    /// next is timed from that read's answer, or, if none comes, [`REREAD_MOST`] seconds after it was asked.
    pub(crate) fn reread_due(&mut self, now: u64) -> bool {
        match &mut self.reread {
            Some(reread) if now >= reread.at => {
                reread.at = now + REREAD_MOST;
                true
            }
            _ => false,
        }
    }

    /// When the account, which failed to read, is read again (monotonic seconds). None unless it reads
    /// Failed.
    pub fn reread_at(&self) -> Option<u64> {
        self.reread.map(|r| r.at)
    }

    /// The account helper answered a change with `result`, or can't any more (its thread was started
    /// again). Its reading follows. A failure is kept for Setup to show.
    pub(crate) fn account_changed(&mut self, result: &Result<String, String>) {
        self.password_problem = result.as_ref().err().cloned();
    }
}
