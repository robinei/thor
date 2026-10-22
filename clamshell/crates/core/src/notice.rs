//! The short notice under the bar that says what just happened: a level changed by a key, a screenshot
//! saved, a launch starting, a file operation done. One shows at a time, for [`NOTICE_FOR`].

use crate::State;
use crate::levels::slider::Control;

/// How long a notice shows, in seconds.
const NOTICE_FOR: u64 = 2;

/// A notice under the bar, until it times out.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Notice {
    pub what: NoticeKind,
    until: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NoticeKind {
    /// A level changed by a key: show it.
    Level(Control),
    /// A screenshot was saved (its file), or wasn't (why).
    Screenshot(Result<String, String>),
    /// The gyro calibrating (none: hold still), or done (whether it took).
    Gyro(Option<bool>),
    /// A window moved down is remembered for its program (true), or moved up and forgotten (false).
    BottomRule(bool),
    /// A file operation ended: what was done, or what went wrong.
    Files(Result<String, String>),
    /// Entries enqueued: how many, and to what (the running entry's title).
    Enqueued(usize, String),
    /// A running entry asked for has no window to show (its title).
    NoWindow(String),
    /// A launch didn't start (its entry's title; why is in the log).
    NotStarted(String),
    /// A launch asked for (its entry's title): it is starting.
    Starting(String),
    /// A change of the account is done (what), or failed (why) (see [`crate::device::account`]).
    Account(Result<String, String>),
    /// The library read anew has problems (the first, and how many: all are listed on the Tools tab's
    /// Rescan library).
    Problems(String, usize),
}

impl State {
    pub(crate) fn notify(&mut self, what: NoticeKind) {
        self.bottom.notice = Some(Notice { what, until: self.mono_secs + NOTICE_FOR });
    }

    /// The notice timed out at `now`: whether it went.
    pub(crate) fn expire_notice(&mut self, now: u64) -> bool {
        let expired = self.bottom.notice.as_ref().is_some_and(|n| now >= n.until);
        if expired {
            self.bottom.notice = None;
        }
        expired
    }
}
