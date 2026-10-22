//! The short notice under the bar that says what just happened, such as a level changed by a key, a
//! screenshot saved, a launch starting or a file operation done. One shows at a time, for [`NOTICE_FOR`].

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
    /// A level changed by a key.
    Level(Control),
    /// A screenshot was saved, with its file, or wasn't, with the reason.
    Screenshot(Result<String, String>),
    /// The gyro is calibrating (None, meaning hold still), or is done (whether it worked).
    Gyro(Option<bool>),
    /// A window moved down is now remembered for its program (true), or was moved up and forgotten
    /// (false).
    BottomRule(bool),
    /// A file operation ended: what was done, or what went wrong.
    Files(Result<String, String>),
    /// Entries were enqueued: how many, and the title of the running entry they went to.
    Enqueued(usize, String),
    /// A running entry that was asked for has no window to show. Holds its title.
    NoWindow(String),
    /// A launch didn't start. Holds the entry's title; the reason is in the log.
    NotStarted(String),
    /// A requested launch is starting. Holds the entry's title.
    Starting(String),
    /// An account change is done, with what was done, or failed, with the reason (see
    /// [`crate::device::account`]).
    Account(Result<String, String>),
    /// The library was read again and has problems. Holds the first one and how many there are. All of
    /// them are listed under Rescan library on the Tools tab.
    Problems(String, usize),
}

impl Notice {
    /// A notice shown from `now`.
    pub(crate) fn new(what: NoticeKind, now: u64) -> Notice {
        Notice { what, until: now + NOTICE_FOR }
    }
}

impl crate::Bottom {
    /// Shows `what` from `now`, in place of any notice showing.
    pub(crate) fn notify(&mut self, what: NoticeKind, now: u64) {
        self.notice = Some(Notice::new(what, now));
    }

    /// The notice, for another area's rule to show one.
    pub(crate) fn notice_mut(&mut self) -> &mut Option<Notice> {
        &mut self.notice
    }

    /// Removes the notice if it has timed out at `now`. Returns whether it did.
    pub(crate) fn expire_notice(&mut self, now: u64) -> bool {
        let expired = self.notice.as_ref().is_some_and(|n| now >= n.until);
        if expired {
            self.notice = None;
        }
        expired
    }
}
