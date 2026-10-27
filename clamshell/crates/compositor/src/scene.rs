//! What is shown where, front first: the cursor, the software surfaces (the UI's overlays and screens,
//! see `software`), each at its place with the part of it shown, then the windows, each with its
//! geometry. A window's popups and subsurfaces are its own, placed and scaled with it. One
//! function places everything ([`Server::place`]); drawing makes its elements from the list, and input,
//! frame pacing and popups read the windows' places from it, so that a window is touched where it is
//! drawn. The list is kept and refilled: placing allocates nothing once it has held the most there have
//! been.
//!
//! It is placed again after anything that may move what is shown: the state changing
//! ([`Server::sync_windows`]), a commit (a window's size or picture), and before drawing. In debug builds
//! each read of a window's place is checked against a fresh placement, so that a missed one fails the
//! tests rather than leaving a window drawn where it isn't touched.

use clamshell_state::presentation::{WindowLayout, WindowPresentation, WindowScale};
use clamshell_ui::{Bounds, Layer, Screen};
use smithay::input::pointer::CursorImageStatus;
use smithay::utils::{Logical, Point, Rectangle};

use crate::geometry::Geometry;
use crate::server::Server;
use crate::windows::ClientWindow;

/// One thing shown.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) enum Shown {
    /// The cursor, its hotspot `at` the pointer (see `cursor`).
    Cursor {
        at: Point<f64, Logical>,
    },
    /// The software surface of the UI's `layer`, at `x` in the scene: the whole of it, or only `part`.
    Software {
        layer: Layer,
        x: i32,
        part: Option<Bounds>,
    },
    Window(Placed),
}

/// A window shown: on the second screen or the top one, where and how it is drawn, and whether input
/// reaches it (not the overview's windows on the top screen, which are shown to be chosen).
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct Placed {
    pub id: i64,
    pub below: bool,
    pub geometry: Geometry,
    pub input: bool,
}

#[derive(Debug, Default)]
pub(crate) struct Scene {
    shown: Vec<Shown>,
}

impl Scene {
    /// What is shown, front first.
    pub fn shown(&self) -> &[Shown] {
        &self.shown
    }

    /// The windows shown, front first.
    pub fn windows(&self) -> impl Iterator<Item = &Placed> {
        self.shown.iter().filter_map(|shown| match shown {
            Shown::Window(placed) => Some(placed),
            Shown::Software { .. } | Shown::Cursor { .. } => None,
        })
    }

    pub fn window(&self, id: i64) -> Option<&Placed> {
        self.windows().find(|placed| placed.id == id)
    }

    /// Whether a window covers the top screen, whose view is then neither shown nor drawn.
    pub fn covers_top(&self) -> bool {
        self.windows().any(|placed| !placed.below)
    }
}

impl Server {
    /// Places what is shown (see the module's doc): the windows, the frontmost last in [`Server::windows`]
    /// first here, then in front of them the UI: the second screen's view (only above its windows' area
    /// while it shows them), the top screen's while no window covers it, and the overlays where they draw;
    /// and in front of all the cursor, while it shows.
    pub(crate) fn place(&mut self) {
        let mut shown = std::mem::take(&mut self.scene.shown);
        shown.clear();
        shown.extend(self.windows.iter().rev().filter_map(|client| self.placed(client)).map(Shown::Window));
        let covered = shown.iter().any(|s| matches!(s, Shown::Window(placed) if !placed.below));
        if !covered {
            shown.insert(0, Shown::Software { layer: Layer::Screen(Screen::Top), x: 0, part: None });
        }
        let state = self.core.state();
        let above_windows = state.bottom().shows_windows(state.session()).then(|| Bounds::new(0, 0, i32::MAX, self.area(true).loc.y.max(1)));
        shown.insert(0, Shown::Software { layer: Layer::Screen(Screen::Bottom), x: self.screens.top_width, part: above_windows });
        for (screen, x) in [(Screen::Top, 0), (Screen::Bottom, self.screens.top_width)] {
            if let Some(area) = self.core.overlay_area(screen) {
                shown.insert(0, Shown::Software { layer: Layer::Overlay(screen), x, part: Some(area) });
            }
        }
        self.scene.shown = shown;
        // Once the windows are placed: what the cursor shows is the client's under it.
        if self.pointer.cursor_activity.visible(std::time::Instant::now()) && self.effective_cursor() != CursorImageStatus::Hidden {
            self.scene.shown.insert(0, Shown::Cursor { at: self.pointer.location });
        }
    }

    /// Where `client`'s window is shown, if it is: the top screen's window, the second screen's while that
    /// shows its window, or one of the overview's, fitted into its cell whatever its presentation (it
    /// keeps the size it was configured to). It is shown once it has a buffer and a size.
    fn placed(&self, client: &ClientWindow) -> Option<Placed> {
        if !client.mapped {
            return None;
        }
        let state = self.core.state();
        let session = state.session();
        let id = client.id;
        let below = session.below(id);
        let cell = self.overview_cell(id);
        let shown = session.top_window().is_some_and(|w| w.id == id) || (below && state.bottom().shows_windows(session)) || cell.is_some();
        if !shown {
            return None;
        }
        let (area, policy) = match cell {
            Some(cell) => (cell, WindowPresentation { layout: WindowLayout::Fullscreen, scale: WindowScale::Fit }),
            None => (self.area(below), client.shown.presentation?),
        };
        let (size, place) = client.window.picture().unzip();
        let picture = crate::geometry::Picture { size, place };
        let geometry = Geometry::new(area, client.window.geometry(), policy, picture)?;
        Some(Placed { id, below, geometry, input: cell.is_none() })
    }

    /// Where window `id` is shown, if it is (see [`Server::placed`]).
    pub(crate) fn shown(&self, id: i64) -> Option<&Placed> {
        let placed = self.scene.window(id);
        #[cfg(debug_assertions)]
        {
            let fresh = self.windows.iter().find(|client| client.id == id).and_then(|client| self.placed(client));
            debug_assert_eq!(placed.copied(), fresh, "the scene wasn't placed again after window {id} moved");
        }
        placed
    }

    /// Whether window `id` is shown.
    pub fn visible(&self, id: i64) -> bool {
        self.shown(id).is_some()
    }

    /// Where and how window `id` is drawn, if it is shown.
    pub fn geometry(&self, id: i64) -> Option<Geometry> {
        self.shown(id).map(|placed| placed.geometry)
    }

    /// Window `id`'s cell in the overview on the top screen, if it is one of the overview's.
    pub(crate) fn overview_cell(&self, id: i64) -> Option<Rectangle<i32, Logical>> {
        let session = self.core.state().session();
        let i = session.overview_windows().position(|w| w.id == id)?;
        let area = self.area(false);
        let (x, y, w, h) = clamshell_state::session::overview::cell(i, session.overview_windows().count(), (area.size.w as f32, area.size.h as f32));
        Some(Rectangle::new(
            (area.loc.x + x.round() as i32, area.loc.y + y.round() as i32).into(),
            ((w.round() as i32).max(1), (h.round() as i32).max(1)).into(),
        ))
    }
}
