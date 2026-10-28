//! A program's windows shown together on the top screen. A tap on the dock entry of the program in front,
//! with more than one window on the top screen, opens the overview: its top-level windows in a grid
//! ([`cell`]), each cell showing what the screen would with that window in front, its dialogs over it,
//! scaled down to fit. The bottom screen's window stays out of it.
//!
//! The window in front stays in front underneath. A tap on a window, or A on the chosen one, brings it in
//! front; another tap on the entry, or B, goes back. The D-pad moves the choice through the grid, since the
//! pad goes to ClamShell while the overview is open (see [`crate::nav::pad`]). The compositor draws the
//! cells, and the top screen's overlay the cells' targets and the ring around the chosen one.
//!
//! It closes when another group comes in front, as Home or another dock entry brings it, or when fewer
//! than two windows are left in it.

use crate::Session;
use crate::keymap::Dir;
use crate::session::windows::{Group, Window};

/// The open overview: the group it shows, and the window the pad has chosen in it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Overview {
    group: Group,
    chosen: i64,
}

impl Overview {
    /// The window the pad has chosen, ringed.
    pub fn chosen(&self) -> i64 {
        self.chosen
    }
}

/// What is done to the overview.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OverviewEvent {
    /// Opens it for the group in front, or closes it if it is open, as a tap on the dock entry in front
    /// does.
    Toggle,
    /// The D-pad moves the choice to the next cell that way.
    Step(Dir),
    /// A window was tapped: it comes in front, and the overview closes.
    Pick(i64),
    /// A on the chosen window: as [`OverviewEvent::Pick`] on it.
    Choose,
    /// B: back to the window in front.
    Close,
}

/// The space between the cells and around them, as a share of the screen's height.
const GAP: f32 = 0.03;

/// The grid's columns for `n` windows: two side by side, three or four in two rows, up to nine in three
/// rows of three, and so on.
pub fn columns(n: usize) -> usize {
    (1..n).find(|c| c * c >= n).unwrap_or(n.max(1))
}

/// Cell `i` of `n` on a screen of `size`, as x, y, width and height: the screen in its own shape, as large
/// as fits in its share of the screen and centred there, which is what the compositor draws in it, the
/// ring goes round and a tap picks. The shares split the screen evenly, with a gap between them and around
/// them. A last row with fewer windows is centred.
pub fn cell(i: usize, n: usize, (w, h): (f32, f32)) -> (f32, f32, f32, f32) {
    let columns = columns(n);
    let rows = n.div_ceil(columns).max(1);
    let gap = GAP * h;
    let cell_w = (w - gap * (columns + 1) as f32) / columns as f32;
    let cell_h = (h - gap * (rows + 1) as f32) / rows as f32;
    let (row, column) = (i / columns, i % columns);
    let in_row = if row + 1 == rows { n.saturating_sub(row * columns) } else { columns };
    let indent = columns.saturating_sub(in_row) as f32 * (cell_w + gap) / 2.0;
    let (x, y) = (indent + gap + column as f32 * (cell_w + gap), gap + row as f32 * (cell_h + gap));
    let k = (cell_w / w.max(1.0)).min(cell_h / h.max(1.0));
    let (fit_w, fit_h) = (w * k, h * k);
    (x + (cell_w - fit_w) / 2.0, y + (cell_h - fit_h) / 2.0, fit_w, fit_h)
}

impl Session {
    /// The open overview, if there is one.
    pub fn overview(&self) -> Option<&Overview> {
        self.overview.as_ref()
    }

    /// The windows the open overview shows, in the order they opened, which is the order of its cells.
    /// None while it is closed.
    pub fn overview_windows(&self) -> impl Iterator<Item = &Window> {
        let group = self.overview.as_ref().map(|o| &o.group);
        self.windows.iter().filter(move |w| group.is_some_and(|g| self.top_level_in(w, g)))
    }

    /// Whether a tap on running entry `id`'s dock tile is for the overview: its group is in front, and it
    /// has more than one window on the top screen, or its overview is open.
    pub(crate) fn overview_of(&self, id: &str) -> bool {
        let Some(group) = self.running(id).map(super::running::Running::group) else { return false };
        if self.top_window().and_then(|w| w.group.as_ref()) != Some(&group) {
            return false;
        }
        self.overview.is_some() || self.windows.iter().filter(|w| self.top_level_in(w, &group)).nth(1).is_some()
    }

    pub(crate) fn on_overview(&mut self, event: OverviewEvent) {
        match event {
            OverviewEvent::Toggle if self.overview.is_some() => self.overview = None,
            OverviewEvent::Toggle => self.open_overview(),
            OverviewEvent::Step(dir) => self.step_overview(dir),
            OverviewEvent::Pick(id) => self.pick(id),
            OverviewEvent::Choose => {
                if let Some(id) = self.overview.as_ref().map(Overview::chosen) {
                    self.pick(id);
                }
            }
            OverviewEvent::Close => self.overview = None,
        }
    }

    /// Opens the overview of the group in front, with the window in front chosen (a dialog's root), if
    /// it has more than one window on the top screen.
    fn open_overview(&mut self) {
        let Some(front) = self.top_window() else { return };
        let Some(group) = front.group.clone() else { return };
        let root = self.root_of(front.id);
        let mut windows = self.windows.iter().filter(|w| self.top_level_in(w, &group));
        let Some(first) = windows.next().map(|w| w.id) else { return };
        if windows.next().is_none() {
            return;
        }
        let in_front = self.windows.iter().any(|w| w.id == root && self.top_level_in(w, &group));
        self.overview = Some(Overview { group, chosen: if in_front { root } else { first } });
    }

    /// Moves the choice one cell `dir`, within the grid. Down from a row above a shorter last row goes to
    /// that row's last window.
    fn step_overview(&mut self, dir: Dir) {
        let Some(chosen) = self.overview.as_ref().map(Overview::chosen) else { return };
        let n = self.overview_windows().count();
        let Some(i) = self.overview_windows().position(|w| w.id == chosen) else { return };
        let columns = columns(n);
        let to = match dir {
            Dir::Left if i % columns > 0 => i - 1,
            Dir::Right if i % columns + 1 < columns && i + 1 < n => i + 1,
            Dir::Up if i >= columns => i - columns,
            Dir::Down if i / columns + 1 < n.div_ceil(columns) => (i + columns).min(n - 1),
            _ => return,
        };
        let to = self.overview_windows().nth(to).map(|w| w.id);
        if let (Some(overview), Some(to)) = (&mut self.overview, to) {
            overview.chosen = to;
        }
    }

    /// Brings window `id` of the overview in front, and closes the overview.
    fn pick(&mut self, id: i64) {
        if !self.overview_windows().any(|w| w.id == id) {
            return;
        }
        self.overview = None;
        self.bring_in_front(id);
    }

    /// Runs after anything changed. Closes the overview when another group is in front, or when fewer
    /// than two of its windows are left; a chosen window that closed or moved down leaves the choice on
    /// the first.
    pub(crate) fn follow_overview(&mut self) {
        let Some(group) = self.overview.as_ref().map(|o| o.group.clone()) else { return };
        let in_front = self.top_window().and_then(|w| w.group.as_ref()) == Some(&group);
        let first = self.overview_windows().next().map(|w| w.id);
        if !in_front || self.overview_windows().nth(1).is_none() {
            self.overview = None;
            return;
        }
        let chosen = self.overview.as_ref().map(Overview::chosen);
        if !self.overview_windows().any(|w| Some(w.id) == chosen)
            && let (Some(overview), Some(first)) = (&mut self.overview, first)
        {
            overview.chosen = first;
        }
    }

    /// Whether the overview, if open, has its group in front with more than one window, and its choice
    /// among them.
    #[cfg(test)]
    pub(crate) fn overview_valid(&self) -> bool {
        let Some(overview) = &self.overview else { return true };
        let in_front = self.top_window().and_then(|w| w.group.as_ref()) == Some(&overview.group);
        in_front && self.overview_windows().nth(1).is_some() && self.overview_windows().any(|w| w.id == overview.chosen)
    }
}

#[cfg(test)]
mod tests;
