//! The short notice under the bar that says what just happened, such as a level changed by a key, a
//! screenshot saved, a launch starting or a file operation done. One shows at a time, for [`NOTICE_FOR`],
//! except the gyro's "hold still", which shows until the calibration answers, for at most [`GYRO_FOR`].

use crate::levels::slider::Control;

/// How long a notice shows, in seconds.
const NOTICE_FOR: u64 = 2;

/// How long the gyro's "hold still" shows at most, in seconds. Its answer replaces it before then.
const GYRO_FOR: u64 = crate::device::ANSWER_WAIT;

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
    /// An editor for the keyboard tab was refused, since another is open there.
    KeyboardBusy,
    /// The library was read again and has problems. Holds the first one and how many there are. All of
    /// them are listed under Rescan library on the Tools tab.
    Problems(String, usize),
}

/// The notice showing, if any. A rule shows one with [`NoticeSlot::show`], the one way to show one.
#[derive(Debug, Clone, Default)]
pub struct NoticeSlot(Option<Notice>);

impl NoticeSlot {
    /// Shows `what` from `now`, in place of any notice showing.
    pub(crate) fn show(&mut self, what: NoticeKind, now: u64) {
        let lasts = if what == NoticeKind::Gyro(None) { GYRO_FOR } else { NOTICE_FOR };
        self.0 = Some(Notice { what, until: now + lasts });
    }

    pub fn get(&self) -> Option<&Notice> {
        self.0.as_ref()
    }

    /// Removes the notice if it has timed out at `now`.
    fn expire(&mut self, now: u64) {
        if self.0.as_ref().is_some_and(|n| now >= n.until) {
            self.0 = None;
        }
    }
}

impl crate::Bottom {
    /// The notice, for a rule to show one.
    pub(crate) fn notice_mut(&mut self) -> &mut NoticeSlot {
        &mut self.notice
    }

    /// Removes the notice if it has timed out at `now`.
    pub(crate) fn expire_notice(&mut self, now: u64) {
        self.notice.expire(now);
    }
}
