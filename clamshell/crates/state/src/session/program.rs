//! The program in force: the running entry most recently in front. It stays in force while shelf, or a
//! window that isn't ours, is in front, as long as it runs. Once it stops, the entry started last that
//! still runs takes over. Its settings are in force, its keymap is the pad's, and its pad mouse setting
//! decides the pad mouse.

use crate::Session;

impl Session {
    /// Runs after any event. The program in force follows the one in front, or, when that is gone, the
    /// latest that runs. Returns whether it changed, and so its settings are to be applied.
    pub(crate) fn follow_program(&mut self, now: u64) -> bool {
        let front = self.top_window().and_then(|w| self.owner_of(w));
        let kept = self.program.clone().filter(|p| self.running(p).is_some());
        let program = front.or(kept).or_else(|| self.running.last().map(|r| r.id.clone()));
        let changed = crate::kit::replace(&mut self.program, program);
        let launch = self.refresh_launch.take().filter(|(e, at)| {
            let waiting = self.is_launching(e) || (self.running(e).is_some() && self.windows_of(e).next().is_none());
            !changed && waiting && now.saturating_sub(*at) < crate::session::running::LAUNCH_WAIT
        });
        self.refresh_launch = launch;
        changed
    }
}
