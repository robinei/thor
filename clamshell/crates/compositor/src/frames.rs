//! Each window's frames as the compositor sees them: a frame is a new buffer committed to the window's
//! surface, counted when a screen first shows it (the page flip's time on the device, the submission's
//! nested). Kept in a fixed ring per window, with no allocation per frame. The window in front on the
//! top screen gives the frame rate the FPS tile and the system info show (`Reading::Frames`), as
//! MangoHud's log does under sway.
//!
//! A window with a frame rate cap gets its frame callbacks at the cap's pace ([`Frames::pace`]), which
//! clients presenting in FIFO follow: Mesa's Vulkan WSI, EGL with a swap interval, Xwayland's Present.

use std::time::{Duration, Instant};

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
    /// Under a cap: when the next frame callback is due.
    paced: Option<Instant>,
}

impl Default for Frames {
    fn default() -> Self {
        Frames { shown: [Duration::ZERO; RING], next: 0, len: 0, fresh: false, paced: None }
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

    /// Whether the window waiting for a frame callback gets it at `now`, under a cap of `cap` frames a second (None:
    /// always) on a screen showing a frame every `refresh`. A capped window gets them at the cap's
    /// period on average: on a 60 Hz screen, a 40 fps cap gives two refreshes, then one, and so on. They
    /// come a quarter of a refresh early, since flips are when they are sent. Otherwise, the time they
    /// are due.
    pub fn pace(&mut self, cap: Option<u16>, refresh: Duration, now: Instant) -> Result<(), Instant> {
        let Some(cap) = cap else {
            self.paced = None;
            return Ok(());
        };
        let period = Duration::from_secs(1) / u32::from(cap.max(1));
        let due = self.paced.unwrap_or(now);
        if now + refresh / 4 < due {
            return Err(due);
        }
        // Behind by more than a period (the window was hidden, or the cap just set): start again from now.
        self.paced = Some(if due + period < now { now + period } else { due + period });
        Ok(())
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

    /// The refreshes at which a cap sends frame callbacks, over `n` refreshes from flips every `refresh`.
    fn paced(cap: u16, refresh: Duration, n: u32) -> Vec<u32> {
        let (mut f, start) = (Frames::default(), Instant::now());
        (0..n).filter(|i| f.pace(Some(cap), refresh, start + refresh * *i).is_ok()).collect()
    }

    #[test]
    fn a_cap_paces_frame_callbacks_on_the_screens_refreshes() {
        let hz60 = Duration::from_secs(1) / 60;
        assert_eq!(paced(30, hz60, 8), [0, 2, 4, 6]);
        assert_eq!(paced(60, hz60, 4), [0, 1, 2, 3]);
        assert_eq!(paced(120, hz60, 3), [0, 1, 2], "a cap over the refresh: every refresh");
        assert_eq!(paced(40, hz60, 7), [0, 2, 3, 5, 6], "40 on 60 Hz: two refreshes then one");
        assert_eq!(paced(40, Duration::from_secs(1) / 120, 7), [0, 3, 6], "40 on 120 Hz: every third");
        let (mut f, now) = (Frames::default(), Instant::now());
        assert_eq!(f.pace(None, hz60, now), Ok(()), "no cap");
        assert_eq!(f.pace(Some(30), hz60, now), Ok(()));
        assert_eq!(f.pace(Some(30), hz60, now + hz60), Err(now + Duration::from_secs(1) / 30), "held until due");
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
