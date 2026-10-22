//! Noticing a wake from sleep through the clocks. CLOCK_BOOTTIME counts time asleep and CLOCK_MONOTONIC
//! doesn't, so their difference grows by however long the system slept. The clock thread checks it every
//! second. D-Bus isn't used, because logind's PrepareForSleep(false) didn't reach us after a resume in
//! testing, and this also sees sleep that didn't go through logind.

use std::time::Duration;

use rustix::time::{ClockId, clock_gettime};

/// Time spent asleep since boot.
pub fn slept() -> Duration {
    let as_duration = |t: rustix::time::Timespec| Duration::new(t.tv_sec.max(0) as u64, t.tv_nsec.clamp(0, 999_999_999) as u32);
    as_duration(clock_gettime(ClockId::Boottime)).saturating_sub(as_duration(clock_gettime(ClockId::Monotonic)))
}

/// Sleep shorter than this isn't counted, since the two clocks are read one after the other.
const AT_LEAST: Duration = Duration::from_secs(1);

/// Notices a sleep between two checks.
#[derive(Debug)]
pub struct Watch {
    last: Duration,
}

impl Watch {
    pub fn new(slept: Duration) -> Self {
        Watch { last: slept }
    }

    /// Whether the system slept since the last check, given the time slept so far.
    pub fn woke(&mut self, slept: Duration) -> bool {
        let woke = slept >= self.last + AT_LEAST;
        if woke || slept < self.last {
            self.last = slept;
        }
        woke
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_sleep_is_noticed_once() {
        let mut watch = Watch::new(Duration::from_secs(100));
        assert!(!watch.woke(Duration::from_millis(100_400)), "jitter");
        assert!(watch.woke(Duration::from_secs(160)));
        assert!(!watch.woke(Duration::from_secs(160)));
        assert!(watch.woke(Duration::from_secs(170)));
    }

    #[test]
    fn the_clocks_can_be_read() {
        assert!(slept() < Duration::from_secs(10 * 365 * 24 * 3600));
    }
}
