//! Buffers needed only now and then are made when needed and dropped a while after their last use, rather
//! than kept: a screen's scene (`colour::render`), its overlay plane's buffers (`plane_layers`), ClamShell's
//! UI layers as composed (`render::UiFrame`), a screen's buffers to compose into (`presenter`). In the
//! common states, ClamShell alone on a screen (shown from its own buffers) or a game alone (shown from its
//! own, or drawn straight), most of them hold nothing. Buffers used all the time are made once and kept.
//!
//! A [`Hold`] is when one was last used; its owner drops it once [`HOLD`] has passed since, long enough that
//! something coming and going (a dialog, a notice) doesn't have it made again each time, and the loop wakes
//! then to do so. While one is in use (shown, say) it counts as used now, so that it is kept for [`HOLD`]
//! after, without waking the loop meanwhile. The owners' trims each turn report to one [`Trims`], which then
//! hands the memory freed back to the system, once ([`give_back`]): the allocator would otherwise keep it
//! for later, and ClamShell's memory would stay as it was.

use std::time::{Duration, Instant};

/// How long a buffer is kept after its last use.
pub(crate) const HOLD: Duration = Duration::from_secs(5);

/// Hands memory the allocator holds free back to the system, after buffers were dropped (glibc keeps what
/// was freed for reuse otherwise).
fn give_back() {
    #[cfg(target_env = "gnu")]
    // SAFETY: malloc_trim only returns free memory to the system.
    #[allow(unsafe_code)]
    unsafe {
        libc::malloc_trim(0);
    }
}

/// When something made on demand was last used.
#[derive(Debug, Default, Clone, Copy)]
pub(crate) struct Hold {
    used: Option<Instant>,
}

impl Hold {
    /// It is used at `now`.
    pub(crate) fn used(&mut self, now: Instant) {
        self.used = Some(now);
    }

    /// Whether its time is up at `now`, for its owner to drop it: then it is forgotten, until used again.
    pub(crate) fn expired(&mut self, now: Instant) -> bool {
        let expired = self.used.is_some_and(|used| now >= used + HOLD);
        if expired {
            self.used = None;
        }
        expired
    }

    /// When its time will be up, if it is held.
    pub(crate) fn due(&self) -> Option<Instant> {
        self.used.map(|used| used + HOLD)
    }
}

/// What the trims in one turn of the loop found: when the next thing held runs out, and whether any was
/// dropped.
#[derive(Debug, Default)]
pub(crate) struct Trims {
    due: Option<Instant>,
    dropped: bool,
}

impl Trims {
    /// `hold` keeps something: the loop wakes when it runs out.
    pub(crate) fn held(&mut self, hold: &Hold) {
        if let Some(due) = hold.due() {
            self.due = Some(self.due.map_or(due, |next| next.min(due)));
        }
    }

    /// Something was dropped.
    pub(crate) fn dropped(&mut self) {
        self.dropped = true;
    }

    /// Hands back what was dropped. Returns when the trims are next due.
    pub(crate) fn finish(self) -> Option<Instant> {
        if self.dropped {
            give_back();
        }
        self.due
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A hold runs out [`HOLD`] after its last use, is forgotten once it has, and holds again when used.
    #[test]
    fn a_hold_runs_out_after_its_last_use() {
        let start = Instant::now();
        let mut hold = Hold::default();
        assert!(!hold.expired(start + HOLD * 9) && hold.due().is_none(), "never used");
        hold.used(start);
        hold.used(start + HOLD / 2);
        assert_eq!(hold.due(), Some(start + HOLD + HOLD / 2), "from its last use");
        assert!(!hold.expired(start + HOLD));
        assert!(hold.expired(start + HOLD + HOLD / 2));
        assert!(!hold.expired(start + HOLD * 9) && hold.due().is_none(), "forgotten once it ran out");
        hold.used(start + HOLD * 9);
        assert_eq!(hold.due(), Some(start + HOLD * 10));
    }

    /// The trims wake the loop when the first of the holds runs out, and only for those still holding.
    #[test]
    fn the_trims_are_next_due_when_the_first_hold_runs_out() {
        let start = Instant::now();
        let (mut early, mut late, idle) = (Hold::default(), Hold::default(), Hold::default());
        late.used(start + HOLD);
        early.used(start);
        let mut trims = Trims::default();
        trims.held(&late);
        trims.held(&idle);
        trims.held(&early);
        assert_eq!(trims.finish(), Some(start + HOLD));
        assert_eq!(Trims::default().finish(), None);
    }
}
