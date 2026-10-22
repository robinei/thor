//! Saving a moment after the last of a run of changes, not on each.

/// A change waiting to be saved: since when (the monotonic clock's seconds), if any.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(crate) struct Debounce {
    since: Option<u64>,
}

impl Debounce {
    /// Something changed at `now`: saved once nothing has for a moment.
    pub(crate) fn touch(&mut self, now: u64) {
        self.since = Some(now);
    }

    /// Whether it is time to save (`after` seconds since the last change); then it is no longer due.
    pub(crate) fn due(&mut self, now: u64, after: u64) -> bool {
        let due = self.since.is_some_and(|since| now >= since + after);
        if due {
            self.since = None;
        }
        due
    }
}
