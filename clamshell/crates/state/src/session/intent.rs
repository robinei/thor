//! What is to come in front next: a launch's first window, or the next window of an entry that runs (a
//! tray Activate, a program whose window in front closed while it runs on, a launcher's game). Until it
//! comes, or the intent is given up, other programs' new windows open behind (see
//! [`super::Session::opening_focus`]). Every intent has a deadline, and one for a launch names the unit
//! that launch asked for, so that neither a made-up unit nor a forgotten one can hold the front.

use crate::LaunchUnit;

/// How long a launch started in front waits for its first window, in seconds from when it was asked for:
/// a game may load for a while before it shows one (a first Proton start sets up its prefix and compiles
/// shaders first, for minutes). Waiting long costs little: a new launch or Home replaces the intent, and
/// windows nobody asked for stay behind meanwhile. A launch the launcher never answers is given up sooner
/// ([`super::running::LAUNCH_WAIT`]).
pub(crate) const WINDOW_WAIT: u64 = 600;

/// What is waited for.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Awaited {
    /// A launch of ours, by the unit it asked for: its windows, not those of the entry's other launches.
    Launch(LaunchUnit),
    /// An entry that runs, by its key: any window of it.
    Entry(String),
}

/// The intent to show what comes next in front, until `until` (monotonic seconds).
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct FrontIntent {
    awaited: Awaited,
    until: u64,
    /// The launcher's dialogs that brought its group in front, while any is open. It is not remembered as
    /// having been in front, so that what comes after the game is what would have come without it.
    pub(super) promoted: Vec<i64>,
}

impl FrontIntent {
    /// Launch `unit`, asked for at `now`: its first window, for [`WINDOW_WAIT`].
    pub(crate) fn launch(unit: LaunchUnit, now: u64) -> FrontIntent {
        FrontIntent { awaited: Awaited::Launch(unit), until: now + WINDOW_WAIT, promoted: Vec::new() }
    }

    /// The next window of the entry with `key`, which runs, until `until`.
    pub(crate) fn entry(key: &str, until: u64) -> FrontIntent {
        FrontIntent { awaited: Awaited::Entry(key.to_owned()), until, promoted: Vec::new() }
    }

    /// The key of the entry whose window is waited for.
    pub(crate) fn key(&self) -> &str {
        match &self.awaited {
            Awaited::Launch(unit) => &unit.key,
            Awaited::Entry(key) => key,
        }
    }

    /// The launch waited for, if it is one.
    pub(crate) fn unit(&self) -> Option<&LaunchUnit> {
        match &self.awaited {
            Awaited::Launch(unit) => Some(unit),
            Awaited::Entry(_) => None,
        }
    }

    /// Whether a window in systemd unit `unit` is waited for by its unit: the launch's own, or any of the
    /// entry's.
    pub(crate) fn takes_unit(&self, unit: Option<&str>) -> bool {
        match &self.awaited {
            Awaited::Launch(launch) => unit.and_then(LaunchUnit::unit_name).is_some_and(|name| launch.is(name)),
            Awaited::Entry(key) => unit.and_then(LaunchUnit::key_of) == Some(key.as_str()),
        }
    }

    /// When it is given up, in monotonic seconds.
    #[cfg(test)]
    pub(crate) fn until(&self) -> u64 {
        self.until
    }

    /// Whether it is given up at `now`.
    pub(crate) fn expired(&self, now: u64) -> bool {
        now > self.until
    }
}
