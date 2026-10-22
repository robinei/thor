//! Pad navigation. Focus moves by direction between the targets on both screens, as if the bottom screen
//! were directly below the top one (see [`step`]).

use clamshell_core::Id;

use crate::Screen;
use crate::gfx::Rect;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Dir {
    Left,
    Right,
    Up,
    Down,
}

/// A target placed in one plane shared by both screens. x is 0..1 on either screen; y is 0..1 on the top
/// screen and 1..2 on the bottom one. The values are normalized so that panels of different sizes line up.
#[derive(Debug, Clone)]
pub struct NavTarget {
    pub screen: Screen,
    pub rect: Rect,
    pub id: Id,
}

/// How much more the distance in the direction of travel weighs than the distance across it. Both
/// distances are squared, as in Android's focus finder.
const ALONG: f32 = 13.0;

/// The target to focus after moving `dir` from `current`. Of the targets beyond it in that direction, it
/// picks the nearest, weighting the gap in the direction of travel far above the offset across it. The
/// gap is measured between edges, so it is 0 for a target alongside; the offset is measured between
/// centres. So the next item in a row wins over one alongside but far across, and a small control just
/// past an edge wins over a wide row further on.
///
/// Up and down may cross to the other screen; left and right stay on the current one. With no current
/// focus, or a focused item that is no longer drawn, it returns the first target.
pub fn step(targets: &[NavTarget], current: Option<&Id>, dir: Dir) -> Option<Id> {
    let Some(from) = current.and_then(|id| targets.iter().find(|t| &t.id == id)) else {
        return targets.first().map(|t| t.id.clone());
    };
    let f = from.rect;
    let (fx, fy) = f.center();
    targets
        .iter()
        .filter(|t| t.id != from.id)
        .filter(|t| matches!(dir, Dir::Up | Dir::Down) || t.screen == from.screen)
        .filter_map(|t| {
            let r = t.rect;
            let (cx, cy) = r.center();
            let (beyond, along, across) = match dir {
                Dir::Down => (cy > fy + 1e-4, r.y - f.bottom(), cx - fx),
                Dir::Up => (cy < fy - 1e-4, f.y - r.bottom(), cx - fx),
                Dir::Right => (cx > fx + 1e-4, r.x - f.right(), cy - fy),
                Dir::Left => (cx < fx - 1e-4, f.x - r.right(), cy - fy),
            };
            let along = along.max(0.0);
            beyond.then_some((ALONG * along * along + across * across, t))
        })
        .min_by(|a, b| a.0.total_cmp(&b.0))
        .map(|(_, t)| t.id.clone())
        .or_else(|| Some(from.id.clone()))
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::indexing_slicing)]
mod tests {
    use super::*;
    use clamshell_core::Tab;
    use clamshell_core::settings::Setting;
    use clamshell_core::{SettingsId, ShelfId};

    fn t(screen: Screen, x: f32, y: f32, id: Id) -> NavTarget {
        NavTarget { screen, rect: Rect::new(x, y, 0.1, 0.1), id }
    }

    fn layout() -> Vec<NavTarget> {
        vec![
            t(Screen::Top, 0.0, 0.0, Id::Shelf(ShelfId::Entry("a".into()))),
            t(Screen::Top, 0.0, 0.2, Id::Shelf(ShelfId::Entry("b".into()))),
            t(Screen::Bottom, 0.0, 1.0, Id::Tab(Tab::Settings)),
            t(Screen::Bottom, 0.5, 1.0, Id::Tab(Tab::Keyboard)),
            t(Screen::Bottom, 0.0, 1.5, Id::Settings(SettingsId::Tile(Setting::Power))),
        ]
    }

    #[test]
    fn down_walks_the_list_then_crosses_to_the_bottom_screen() {
        let l = layout();
        let b = step(&l, Some(&Id::Shelf(ShelfId::Entry("a".into()))), Dir::Down);
        assert_eq!(b, Some(Id::Shelf(ShelfId::Entry("b".into()))));
        let bar = step(&l, b.as_ref(), Dir::Down);
        assert_eq!(bar, Some(Id::Tab(Tab::Settings)));
        assert_eq!(step(&l, bar.as_ref(), Dir::Down), Some(Id::Settings(SettingsId::Tile(Setting::Power))));
        assert_eq!(step(&l, bar.as_ref(), Dir::Up), Some(Id::Shelf(ShelfId::Entry("b".into()))));
    }

    #[test]
    fn sideways_stays_on_the_screen() {
        let l = layout();
        assert_eq!(step(&l, Some(&Id::Shelf(ShelfId::Entry("a".into()))), Dir::Right), Some(Id::Shelf(ShelfId::Entry("a".into()))));
        assert_eq!(step(&l, Some(&Id::Tab(Tab::Settings)), Dir::Right), Some(Id::Tab(Tab::Keyboard)));
    }

    #[test]
    fn lost_focus_goes_to_the_first_target() {
        assert_eq!(step(&layout(), Some(&Id::Shelf(ShelfId::Launch)), Dir::Down), Some(Id::Shelf(ShelfId::Entry("a".into()))));
    }

    #[test]
    fn right_goes_to_the_next_in_the_row_not_one_alongside_far_above() {
        use clamshell_core::DockId;
        let r = |x: f32, y: f32, w: f32, h: f32, id: Id| NavTarget { screen: Screen::Bottom, rect: Rect::new(x, y, w, h), id };
        let l = vec![
            // The bar: tabs along the top, one just right of the dock icon's left edge.
            r(0.10, 1.0, 0.08, 0.07, Id::Tab(Tab::Settings)),
            r(0.18, 1.0, 0.08, 0.07, Id::Tab(Tab::Dock)),
            // The dock's icons, in a row well below.
            r(0.05, 1.4, 0.12, 0.15, Id::Dock(DockId::Entry(0))),
            r(0.19, 1.4, 0.12, 0.15, Id::Dock(DockId::Entry(1))),
        ];
        assert_eq!(step(&l, Some(&Id::Dock(DockId::Entry(0))), Dir::Right), Some(Id::Dock(DockId::Entry(1))));
    }
}
