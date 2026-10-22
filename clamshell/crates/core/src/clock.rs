//! The two clocks, as of the last [`crate::Event::Tick`].

/// The wall clock and a monotonic clock, in seconds. Every duration is timed by the monotonic clock:
/// idle, the bottom screen, notices, saves, plays, background grace and handoffs. That way a jump in the
/// wall clock, whether set by hand or by NTP, doesn't count as time passing. The wall clock is only used
/// to date things.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Clock {
    unix: u64,
    mono: u64,
}

impl Clock {
    /// The wall clock in epoch seconds.
    pub fn unix(&self) -> u64 {
        self.unix
    }

    /// The monotonic clock.
    pub fn mono(&self) -> u64 {
        self.mono
    }

    /// Whether the clocks have been read, that is, whether the first tick has come.
    pub fn read(&self) -> bool {
        self.unix != 0
    }

    /// Epoch seconds at monotonic time `mono`, going by the wall clock now. If the wall clock has jumped
    /// since, the result moves with it.
    pub fn unix_at(&self, mono: u64) -> u64 {
        self.unix.saturating_sub(self.mono.saturating_sub(mono))
    }

    /// Sets both clocks. Returns whether the wall clock's minute changed.
    pub(crate) fn tick(&mut self, unix: u64, mono: u64) -> bool {
        let minute_changed = unix / 60 != self.unix / 60;
        *self = Clock { unix, mono };
        minute_changed
    }
}
