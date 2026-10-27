//! What is shown where, front first: the cursor, the software surfaces (the UI's overlays and screens,
//! see `software`), each at its place with the part of it shown, then the windows, each with its
//! geometry. A screen shows a window and its dialogs (`Session::stack`): the topmost takes the input, with
//! a black layer behind it darkening the rest. The overview shows each of its windows' stacks in a cell,
//! placed as on the screen and brought into the cell. A window's popups and subsurfaces are its own,
//! placed and scaled with it. One
//! function places everything ([`Server::place`]); drawing makes its elements from the list, and input,
//! frame pacing and popups read the windows' places from it, so that a window is touched where it is
//! drawn. The list is kept and refilled: placing allocates nothing once it has held the most there have
//! been.
//!
//! It is placed again after anything that may move what is shown: the state changing
//! ([`Server::sync_windows`]), a commit (a window's size or picture), and before drawing. In debug builds
//! each read of a window's place is checked against a fresh placement, so that a missed one fails the
//! tests rather than leaving a window drawn where it isn't touched.

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
    /// A black layer over `area`, darkening what is under it: the windows under their topmost dialog.
    Dim {
        area: Rectangle<i32, Logical>,
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
    /// The element ids of the dim layers, the `n`th for the `n`th in the list, kept so that each keeps
    /// its damage from frame to frame.
    dims: Vec<smithay::backend::renderer::element::Id>,
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
            Shown::Software { .. } | Shown::Cursor { .. } | Shown::Dim { .. } => None,
        })
    }

    pub fn window(&self, id: i64) -> Option<&Placed> {
        self.windows().find(|placed| placed.id == id)
    }

    /// The element id of the `n`th dim layer (see [`Scene::dims`]), asked for in order from the first.
    pub fn dim_id(&mut self, n: usize) -> smithay::backend::renderer::element::Id {
        if let Some(id) = self.dims.get(n) {
            return id.clone();
        }
        let id = smithay::backend::renderer::element::Id::new();
        self.dims.push(id.clone());
        id
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
        self.place_windows(&mut shown);
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

    /// Places the windows shown, front first, into `shown`: the top screen's stack, or while the overview
    /// is open its windows' stacks in their cells (shown to be chosen, so input doesn't reach them), and the
    /// bottom screen's stack while it shows the windows.
    fn place_windows(&self, shown: &mut Vec<Shown>) {
        let state = self.core.state();
        let session = state.session();
        if session.overview().is_some() {
            for w in session.overview_windows() {
                self.place_stack(shown, w.id, false, self.overview_cell(w.id));
            }
        } else if let Some(top) = session.top_window() {
            self.place_stack(shown, top.id, false, None);
        }
        if state.bottom().shows_windows(session)
            && let Some(bottom) = session.bottom_window()
        {
            self.place_stack(shown, bottom.id, true, None);
        }
    }

    /// Places window `id`'s stack (see `Session::stack`) on the bottom screen if `below`, else the top,
    /// front first, at the end of `shown`: each window as on its screen, brought into `cell` if given; the
    /// topmost takes the input unless in a cell, with a black layer behind it if there are others.
    fn place_stack(&self, shown: &mut Vec<Shown>, id: i64, below: bool, cell: Option<Rectangle<i32, Logical>>) {
        let session = self.core.state().session();
        let area = self.area(below);
        let start = shown.len();
        for w in session.stack_of(id) {
            let Some(client) = self.windows.iter().find(|client| client.id == w.id) else { continue };
            let Some(geometry) = Self::geometry_in(client, area) else { continue };
            let geometry = cell.map_or(geometry, |cell| geometry.mapped(area, cell));
            shown.insert(start, Shown::Window(Placed { id: w.id, below, geometry, input: false }));
        }
        if let Some(Shown::Window(topmost)) = shown.get_mut(start) {
            topmost.input = cell.is_none();
        }
        if shown.len() > start + 1 {
            let full = Geometry { viewport: area, native: area, origin: area.loc.to_f64(), scale: (1.0, 1.0).into() };
            let area = cell.map_or(area, |cell| full.mapped(area, cell).viewport);
            shown.insert(start + 1, Shown::Dim { area });
        }
    }

    /// Where `client`'s window is shown in `area` by its presentation, once it has a buffer and a size. A
    /// window keeps the size it was configured to.
    fn geometry_in(client: &ClientWindow, area: Rectangle<i32, Logical>) -> Option<Geometry> {
        if !client.mapped {
            return None;
        }
        let (size, place) = client.window.picture().unzip();
        let picture = crate::geometry::Picture { size, place };
        Geometry::new(area, client.window.geometry(), client.shown.presentation?, picture)
    }

    /// Where window `id` is shown, if it is (see [`Server::place_windows`]).
    pub(crate) fn shown(&self, id: i64) -> Option<&Placed> {
        let placed = self.scene.window(id);
        #[cfg(debug_assertions)]
        {
            let mut fresh = Vec::new();
            self.place_windows(&mut fresh);
            let fresh = fresh.iter().find_map(|shown| match shown {
                Shown::Window(placed) if placed.id == id => Some(*placed),
                _ => None,
            });
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
