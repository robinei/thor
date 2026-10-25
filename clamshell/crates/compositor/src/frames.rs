//! Each window's frames as the compositor sees them: a frame is a new buffer committed to the window's
//! surface, counted when a screen first shows it (the page flip's time on the device, the submission's
//! nested). Kept in a fixed ring per window, with no allocation per frame. The window in front on the
//! top screen gives the frame rate the FPS tile and the system info show (`Reading::Frames`), as
//! MangoHud's log does under sway.

use std::time::Duration;

/// Frames shown longer ago than this don't count toward the frame rate.
const SPAN: Duration = Duration::from_secs(1);
/// Frames kept: a second's at 240 Hz.
const RING: usize = 256;
/// How often the frame rate is sent to the state, as MangoHud logs it.
pub(crate) const REPORT: Duration = Duration::from_millis(500);

#[derive(Debug, Clone)]
pub(crate) struct Frames {
    /// When each frame was shown, oldest first from `next`.
    shown: [Duration; RING],
    next: usize,
    len: usize,
    /// A new buffer was committed and not shown yet.
    fresh: bool,
}

impl Default for Frames {
    fn default() -> Self {
        Frames { shown: [Duration::ZERO; RING], next: 0, len: 0, fresh: false }
    }
}

impl Frames {
    /// The window's surface committed a new buffer.
    pub fn committed(&mut self) {
        self.fresh = true;
    }

    /// A screen showed the window at `at`. It counts as a frame if a new buffer came since it was last
    /// shown.
    pub fn shown(&mut self, at: Duration) {
        if !std::mem::take(&mut self.fresh) {
            return;
        }
        if let Some(slot) = self.shown.get_mut(self.next) {
            *slot = at;
        }
        self.next = (self.next + 1) % RING;
        self.len = (self.len + 1).min(RING);
    }

    /// The frames shown in the last [`SPAN`] before `now` per second, and the mean time between them in
    /// milliseconds when there are two or more.
    pub fn rate(&self, now: Duration) -> (f32, Option<f32>) {
        let since = now.saturating_sub(SPAN);
        let recent = (1..=self.len).map(|back| self.shown.get((self.next + RING - back) % RING).copied().unwrap_or_default()).take_while(|at| *at > since);
        let (mut count, mut newest, mut oldest) = (0u32, None, Duration::ZERO);
        for at in recent {
            count += 1;
            newest.get_or_insert(at);
            oldest = at;
        }
        let frame_ms = newest.filter(|_| count > 1).map(|newest: Duration| (newest - oldest).as_secs_f32() * 1000.0 / (count - 1) as f32);
        (count as f32 / SPAN.as_secs_f32(), frame_ms)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ms(n: u64) -> Duration {
        Duration::from_millis(n)
    }

    #[test]
    fn only_new_buffers_count_once_shown() {
        let mut f = Frames::default();
        f.shown(ms(10));
        assert_eq!(f.rate(ms(20)), (0.0, None), "shown with nothing new: no frame");
        f.committed();
        f.committed();
        f.shown(ms(30));
        f.shown(ms(40));
        assert_eq!(f.rate(ms(50)), (1.0, None), "two commits before a flip are one frame shown");
    }

    #[test]
    fn the_rate_is_the_last_seconds_frames_and_their_spacing() {
        let mut f = Frames::default();
        for i in 0..90 {
            f.committed();
            f.shown(ms(i * 20));
        }
        // 50 frames in the last second, 20 ms apart.
        let (fps, frame_ms) = f.rate(ms(89 * 20));
        assert_eq!(fps, 50.0);
        assert!(frame_ms.is_some_and(|ms| (ms - 20.0).abs() < 0.01), "{frame_ms:?}");
        assert_eq!(f.rate(ms(89 * 20 + 2000)), (0.0, None), "a window that stopped drawing");
    }

    #[test]
    fn the_ring_keeps_the_newest_frames() {
        let mut f = Frames::default();
        for i in 0..1000 {
            f.committed();
            f.shown(ms(i * 2));
        }
        let (fps, frame_ms) = f.rate(ms(999 * 2));
        assert_eq!(fps, RING as f32, "at 500 Hz the ring holds half a second");
        assert!(frame_ms.is_some_and(|ms| (ms - 2.0).abs() < 0.01), "{frame_ms:?}");
    }
}
