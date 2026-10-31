//! The last observed visual facts and a coalesced request for the host's next frame.

use crate::{scene, server::Server, windows};
use smithay::input::pointer::CursorImageStatus;

#[derive(Default)]
pub(super) struct Redraw(bool);
impl Redraw {
    pub fn changed(&mut self) {
        self.0 = true;
    }
    pub fn pending(&self) -> bool {
        self.0
    }
    pub fn submitted(&mut self) {
        self.0 = false;
    }
}

/// Snapshots existing scene/presentation facts for comparison; this computes no placement or policy.
#[derive(Default)]
pub(super) struct Visuals {
    epoch: Option<u64>,
    colour: Option<u64>,
    scene: Vec<scene::Shown>,
    windows: Vec<(i64, windows::Shown)>,
    cursor: Option<CursorImageStatus>,
}
impl Visuals {
    pub fn observe(&mut self, server: &mut Server) -> bool {
        server.place();
        let cursor = server.pointer.cursor_activity.visible(std::time::Instant::now()).then(|| server.effective_cursor());
        let mut changed = self.epoch != Some(server.windows_epoch)
            || self.colour != Some(server.colour.revision)
            || self.cursor != cursor
            || self.scene != server.scene.shown();
        let windows = self.windows.len() != server.windows.len()
            || self.windows.iter().zip(&server.windows).any(|((id, shown), window)| *id != window.id || *shown != window.shown);
        changed |= windows;
        if windows {
            self.windows.clear();
            self.windows.extend(server.windows.iter().map(|window| (window.id, window.shown)));
        }
        if self.scene != server.scene.shown() {
            self.scene.clear();
            self.scene.extend_from_slice(server.scene.shown());
        }
        self.epoch = Some(server.windows_epoch);
        self.colour = Some(server.colour.revision);
        self.cursor = cursor;
        changed
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn visual_facts_change_once_and_repeated_turns_stay_idle() {
        let (_display, mut server) = crate::tests::ServerBuilder::new().started();
        let mut visuals = Visuals::default();
        assert!(visuals.observe(&mut server));
        for _ in 0..10 {
            server.frame();
            assert!(!visuals.observe(&mut server));
        }
        server.windows_changed();
        assert!(visuals.observe(&mut server));
        assert!(!visuals.observe(&mut server));
        server.colour.revision += 1;
        assert!(visuals.observe(&mut server));
        assert!(!visuals.observe(&mut server));
    }

    #[test]
    fn idle_never_requests_a_frame_and_changes_coalesce_until_submission() {
        let mut redraw = Redraw::default();
        assert!(!redraw.pending());
        redraw.changed();
        assert!(redraw.pending());
        redraw.changed();
        assert!(redraw.pending(), "changes before the presentation share one drawing");
        redraw.submitted();
        for _ in 0..10 {
            assert!(!redraw.pending(), "submission creates no demand for another frame");
        }
        redraw.changed();
        assert!(redraw.pending(), "new input wakes the idle window");
    }
}
