//! The last observed visual facts and a coalesced request for the host's next frame.

use crate::{scene, server::Server, windows};
use smithay::input::pointer::CursorImageStatus;

#[cfg(any(feature = "nested", test))]
#[derive(Default)]
pub(crate) struct Redraw(bool);
#[cfg(any(feature = "nested", test))]
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

/// What of the scene changed since a host last looked (see [`Visuals::observe`]).
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Changes {
    /// The parts of the scene (the top screen's, the second's) whose picture changed, but for the cursor.
    pub parts: [bool; 2],
    /// The parts the cursor changed on (moved from or to, or its picture), which a screen showing the
    /// cursor on a plane of its own doesn't compose for.
    pub cursor: [bool; 2],
}

impl Changes {
    /// Whether anything changed.
    #[cfg(any(feature = "nested", test))]
    pub fn any(&self) -> bool {
        self.parts.contains(&true) || self.cursor.contains(&true)
    }
}

/// The scene and presentation facts as a host last looked, compared to find what changed where; this
/// places nothing and decides no policy.
#[derive(Default)]
pub(crate) struct Visuals {
    colour: Option<u64>,
    scene: Vec<scene::Shown>,
    windows: Vec<(i64, windows::Shown)>,
    cursor: Option<CursorImageStatus>,
}
impl Visuals {
    /// What changed in each part of the scene since the last call: the surfaces committed there (see
    /// `Server::surface_committed`), what is shown there, the presentation of a window shown there, the
    /// cursor (apart), and the colour (both).
    pub fn observe(&mut self, server: &mut Server) -> Changes {
        server.place();
        let committed = std::mem::take(&mut server.committed);
        let mut changes = Changes { parts: committed.parts, cursor: [false; 2] };
        let split = server.screens.top_width;
        if self.colour != Some(server.colour.revision) {
            changes.parts = [true; 2];
        }
        let shown = server.scene.shown();
        for (part, changed) in changes.parts.iter_mut().enumerate() {
            let on = |shown: &&scene::Shown| shown.part(split) == Some(part);
            *changed |= !self.scene.iter().filter(on).eq(shown.iter().filter(on));
        }
        let at = |shown: &[scene::Shown]| {
            shown.iter().find_map(|shown| match shown {
                scene::Shown::Cursor { at } => Some(*at),
                _ => None,
            })
        };
        let (was, is) = (at(&self.scene), at(shown));
        let cursor = server.pointer.cursor_activity.visible(std::time::Instant::now()).then(|| server.effective_cursor());
        if was != is || self.cursor != cursor || committed.cursor {
            for at in [was, is].into_iter().flatten() {
                if let Some(changed) = changes.cursor.get_mut(usize::from(server.screens.below(at.x))) {
                    *changed = true;
                }
            }
        }
        let windows = self.windows.len() != server.windows.len()
            || self.windows.iter().zip(&server.windows).any(|((id, shown), window)| *id != window.id || *shown != window.shown);
        if windows {
            for window in &server.windows {
                let same = self.windows.iter().any(|(id, shown)| *id == window.id && *shown == window.shown);
                if let (false, Some(placed)) = (same, server.scene.window(window.id))
                    && let Some(changed) = changes.parts.get_mut(usize::from(placed.below))
                {
                    *changed = true;
                }
            }
            self.windows.clear();
            self.windows.extend(server.windows.iter().map(|window| (window.id, window.shown)));
        }
        if self.scene != shown {
            self.scene.clear();
            self.scene.extend_from_slice(shown);
        }
        self.colour = Some(server.colour.revision);
        self.cursor = cursor;
        changes
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn visual_facts_change_once_and_repeated_turns_stay_idle() {
        let (_display, mut server) = crate::tests::ServerBuilder::new().started();
        let mut visuals = Visuals::default();
        assert!(visuals.observe(&mut server).any());
        for _ in 0..10 {
            server.frame();
            assert!(!visuals.observe(&mut server).any());
        }
        server.windows_changed();
        assert_eq!(visuals.observe(&mut server).parts, [true; 2]);
        assert!(!visuals.observe(&mut server).any());
        server.colour.revision += 1;
        assert_eq!(visuals.observe(&mut server).parts, [true; 2]);
        assert!(!visuals.observe(&mut server).any());
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
