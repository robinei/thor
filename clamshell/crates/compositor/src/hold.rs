//! Buffers needed only now and then are made when needed and dropped a while after their last use, rather
//! than kept: a screen's scene (`colour::render`), its overlay plane's buffers (`plane_layers`), ClamShell's
//! UI layers as composed (`render::UiFrame`), a screen's buffers to compose into (`presenter`). In the
//! common states, ClamShell alone on a screen (shown from its own buffers) or a game alone (shown from its
//! own, or drawn straight), most of them hold nothing. Buffers used all the time are made once and kept.
//!
//! A [`Hold`] is when one was last used; its owner drops it once [`HOLD`] has passed since, long enough that
//! something coming and going (a dialog, a notice) doesn't have it made again each time, and the loop wakes
//! then to do so. Having dropped one, it hands the memory freed back to the system ([`give_back`]): the
//! allocator would otherwise keep it for later, and ClamShell's memory would stay as it was.

use std::time::{Duration, Instant};

/// How long a buffer is kept after its last use.
pub(crate) const HOLD: Duration = Duration::from_secs(5);

/// Hands memory the allocator holds free back to the system, after buffers were dropped (glibc keeps what
/// was freed for reuse otherwise).
pub(crate) fn give_back() {
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
