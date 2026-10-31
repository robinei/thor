//! A per-output deadline learned from complete drawings, with no timer while nothing is pending.
use std::time::{Duration, Instant};

#[derive(Clone, Copy)]
struct Plan {
    wake: Instant,
    target: Instant,
}

#[derive(Default)]
pub(crate) struct Schedule {
    costs: [Duration; 32],
    cursor: usize,
    samples: usize,
    lateness: Duration,
    pending: Option<Plan>,
    submitted: Option<Instant>,
}
impl Schedule {
    pub fn observe(&mut self, ready: Duration) {
        if let Some(cost) = self.costs.get_mut(self.cursor) {
            *cost = ready;
        }
        self.cursor = (self.cursor + 1) % self.costs.len();
        self.samples = self.samples.saturating_add(1).min(self.costs.len());
        self.lateness = self.lateness.saturating_sub(self.lateness / 32);
    }
    pub fn budget(&self, interval: Duration) -> Duration {
        if self.samples < 4 {
            return interval;
        }
        let cost = self.costs.iter().copied().max().unwrap_or_default();
        cost.saturating_add((cost / 4).max(interval / 64)).saturating_add(self.lateness).min(interval)
    }
    /// Further damage keeps the same target so a busy client cannot postpone its frame forever.
    pub fn deadline(&mut self, now: Instant, target: Instant, interval: Duration) -> Option<Instant> {
        let budget = self.budget(interval);
        let plan = self.pending.get_or_insert_with(|| Plan { wake: target.checked_sub(budget).unwrap_or(now).max(now), target });
        plan.wake = plan.wake.min(plan.target.checked_sub(budget).unwrap_or(now).max(now));
        (now < plan.wake).then_some(plan.wake)
    }
    pub fn started(&mut self, now: Instant) -> Option<Instant> {
        let plan = self.pending.take()?;
        self.lateness = self.lateness.max(now.saturating_duration_since(plan.wake));
        self.submitted = Some(plan.target);
        Some(plan.target)
    }
    pub fn presented(&mut self, actual: Instant, interval: Duration) -> bool {
        let Some(target) = self.submitted.take() else { return false };
        let missed = actual.saturating_duration_since(target) > interval / 2;
        if missed {
            self.samples = 0;
            self.lateness = self.lateness.max(interval / 2);
        }
        missed
    }
    pub fn cancel(&mut self) {
        self.pending = None;
        self.submitted = None;
    }
    pub fn reset(&mut self) {
        *self = Self::default();
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    const FRAME: Duration = Duration::from_millis(16);
    #[test]
    fn cold_drawings_start_immediately_then_leave_measured_room_before_the_target() {
        let now = Instant::now();
        let mut schedule = Schedule::default();
        assert_eq!(schedule.budget(FRAME), FRAME);
        for _ in 0..4 {
            schedule.observe(Duration::from_millis(2));
        }
        assert_eq!(schedule.budget(FRAME), Duration::from_micros(2500));
        let target = now + FRAME;
        let deadline = schedule.deadline(now, target, FRAME).unwrap();
        assert_eq!(deadline, now + Duration::from_micros(13500));
        assert_eq!(schedule.deadline(now + Duration::from_millis(1), target + FRAME, FRAME), Some(deadline));
        assert_eq!(schedule.deadline(deadline, target + FRAME, FRAME), None);
        assert_eq!(schedule.started(deadline), Some(target));
        assert!(!schedule.presented(target, FRAME));
        assert!(schedule.pending.is_none());
    }
    #[test]
    fn a_slow_drawing_increases_the_budget_at_once_and_ages_out_only_after_a_window() {
        let mut schedule = Schedule::default();
        for _ in 0..32 {
            schedule.observe(Duration::from_millis(1));
        }
        let fast = schedule.budget(FRAME);
        schedule.observe(Duration::from_millis(8));
        assert!(schedule.budget(FRAME) >= Duration::from_millis(10));
        for _ in 0..31 {
            schedule.observe(Duration::from_millis(1));
        }
        assert!(schedule.budget(FRAME) >= Duration::from_millis(10));
        schedule.observe(Duration::from_millis(1));
        assert_eq!(schedule.budget(FRAME), fast);
        schedule.observe(Duration::from_secs(1));
        assert_eq!(schedule.budget(FRAME), FRAME);
    }
    #[test]
    fn wakeup_delay_and_missed_flips_make_future_drawings_more_conservative() {
        let now = Instant::now();
        let mut schedule = Schedule::default();
        for _ in 0..4 {
            schedule.observe(Duration::from_millis(1));
        }
        let target = now + FRAME;
        let deadline = schedule.deadline(now, target, FRAME).unwrap();
        schedule.started(deadline + Duration::from_millis(2));
        assert!(schedule.budget(FRAME) >= Duration::from_millis(3));
        assert!(schedule.presented(target + FRAME, FRAME));
        assert_eq!(schedule.budget(FRAME), FRAME);
        for _ in 0..4 {
            schedule.observe(Duration::from_millis(1));
        }
        assert!(schedule.budget(FRAME) > Duration::from_millis(7));
        schedule.reset();
        assert_eq!(schedule.budget(FRAME), FRAME);
        assert!(schedule.pending.is_none());
    }
    #[test]
    fn outputs_are_independent_and_cancellation_leaves_no_deadline() {
        let now = Instant::now();
        let mut top = Schedule::default();
        let mut bottom = Schedule::default();
        for _ in 0..4 {
            top.observe(Duration::from_millis(1));
        }
        assert!(top.deadline(now, now + FRAME, FRAME).is_some());
        assert_eq!(bottom.budget(FRAME), FRAME);
        assert!(bottom.pending.is_none());
        top.cancel();
        assert!(top.pending.is_none());
        assert_eq!(top.started(now), None);
        bottom.reset();
    }
    #[test]
    fn an_increased_cost_advances_an_armed_wakeup_without_postponing_its_target() {
        let now = Instant::now();
        let mut schedule = Schedule::default();
        for _ in 0..4 {
            schedule.observe(Duration::from_millis(1));
        }
        let old = schedule.deadline(now, now + FRAME, FRAME).unwrap();
        schedule.observe(Duration::from_millis(8));
        let earlier = schedule.deadline(now, now + FRAME * 2, FRAME).unwrap();
        assert!(earlier < old);
        assert_eq!(schedule.started(earlier), Some(now + FRAME));
    }

    #[test]
    fn randomized_turns_keep_budgets_bounded_and_reset_and_cancel_drop_pending_work() {
        let mut schedule = Schedule::default();
        let mut seed = 9127u64;
        let mut now = Instant::now();
        for _ in 0..10000 {
            seed = seed.wrapping_mul(6364136223846793005).wrapping_add(1);
            let interval = Duration::from_micros(8000 + seed % 24000);
            now += Duration::from_micros(seed % 1000);
            match seed % 6 {
                0 => schedule.observe(Duration::from_micros((seed >> 8) % 50000)),
                1 => {
                    schedule.deadline(now, now + interval, interval);
                }
                2 => {
                    schedule.started(now);
                }
                3 => {
                    schedule.presented(now, interval);
                }
                4 => {
                    schedule.cancel();
                    assert!(schedule.pending.is_none() && schedule.submitted.is_none());
                }
                _ => {
                    schedule.reset();
                    assert_eq!(schedule.budget(interval), interval);
                }
            }
            assert!(schedule.budget(interval) <= interval);
            assert!(schedule.samples <= schedule.costs.len());
            assert!(schedule.cursor < schedule.costs.len());
        }
    }
}
