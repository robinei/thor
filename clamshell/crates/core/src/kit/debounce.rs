//! Saving a moment after the last of a run of changes, rather than after each one.

/// A change waiting to be saved. It holds the time of the last change in seconds on the monotonic clock,
/// if there is one.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(crate) struct Debounce {
    since: Option<u64>,
}

impl Debounce {
    /// Records a change at `now`. It is saved once nothing has changed for a moment.
    pub(crate) fn touch(&mut self, now: u64) {
        self.since = Some(now);
    }

    /// Whether it is time to save, which is `after` seconds after the last change. Once this returns true,
    /// nothing is due until the next change.
    pub(crate) fn due(&mut self, now: u64, after: u64) -> bool {
        let due = self.since.is_some_and(|since| now >= since + after);
        if due {
            self.since = None;
        }
        due
    }
}
