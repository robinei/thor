//! Pad navigation: moving focus by direction between the targets of both screens, as if the bottom screen
//! were directly below the top one. By lines, as on a TV: the nearest line of targets that way (by their
//! edges, so a wide row and a small button in one line are alike), then the one in it nearest across.

use clamshell_core::Id;

use crate::Screen;
use crate::draw::Rect;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Dir {
    Left,
    Right,
    Up,
    Down,
}

/// A target placed in one plane shared by both screens: x in 0..1 on either, y in 0..1 on the top screen
/// and 1..2 on the bottom one (normalized, so panels of different sizes line up).
#[derive(Debug, Clone)]
pub struct NavTarget {
    pub screen: Screen,
    pub rect: Rect,
    pub id: Id,
}

/// Targets whose near edges are within this of the nearest one's (normalized) are in its line.
const LINE: f32 = 0.01;

/// The target to focus after moving `dir` from `current`: in the nearest line of targets beyond it that
/// way, the one nearest across (overlapping it is nearest; then by centres). Up and down may cross to the
/// other screen; left and right stay on the current one. With no current focus (or a focused item no
/// longer drawn), the first target.
pub fn step(targets: &[NavTarget], current: Option<&Id>, dir: Dir) -> Option<Id> {
    let Some(from) = current.and_then(|id| targets.iter().find(|t| &t.id == id)) else {
        return targets.first().map(|t| t.id.clone());
    };
    let f = from.rect;
    let (fx, fy) = f.center();
    // Along: the gap from its far edge to the target's near edge (0 if they overlap); across: the gap
    // between their spans across, and between their centres.
    let gap = |a0: f32, a1: f32, b0: f32, b1: f32| (b0 - a1).max(a0 - b1).max(0.0);
    let candidates: Vec<(f32, f32, f32, &NavTarget)> = targets
        .iter()
        .filter(|t| t.id != from.id)
        .filter(|t| matches!(dir, Dir::Up | Dir::Down) || t.screen == from.screen)
        .filter_map(|t| {
            let r = t.rect;
            let (cx, cy) = r.center();
            let (beyond, along, across, centres) = match dir {
                Dir::Down => (cy > fy + 1e-4, (r.y - f.bottom()).max(0.0), gap(f.x, f.right(), r.x, r.right()), (cx - fx).abs()),
                Dir::Up => (cy < fy - 1e-4, (f.y - r.bottom()).max(0.0), gap(f.x, f.right(), r.x, r.right()), (cx - fx).abs()),
                Dir::Right => (cx > fx + 1e-4, (r.x - f.right()).max(0.0), gap(f.y, f.bottom(), r.y, r.bottom()), (cy - fy).abs()),
                Dir::Left => (cx < fx - 1e-4, (f.x - r.right()).max(0.0), gap(f.y, f.bottom(), r.y, r.bottom()), (cy - fy).abs()),
            };
            beyond.then_some((along, across, centres, t))
        })
        .collect();
    let nearest = candidates.iter().map(|c| c.0).fold(f32::INFINITY, f32::min);
    candidates
        .iter()
        .filter(|c| c.0 <= nearest + LINE)
        .min_by(|a, b| a.1.total_cmp(&b.1).then(a.2.total_cmp(&b.2)))
        .map(|c| c.3.id.clone())
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
            t(Screen::Top, 0.0, 0.0, Id::Shelf(ShelfId::Game("a".into()))),
            t(Screen::Top, 0.0, 0.2, Id::Shelf(ShelfId::Game("b".into()))),
            t(Screen::Bottom, 0.0, 1.0, Id::Tab(Tab::Settings)),
            t(Screen::Bottom, 0.5, 1.0, Id::Tab(Tab::Keyboard)),
            t(Screen::Bottom, 0.0, 1.5, Id::Settings(SettingsId::Tile(Setting::Power))),
        ]
    }

    #[test]
    fn down_walks_the_list_then_crosses_to_the_bottom_screen() {
        let l = layout();
        let b = step(&l, Some(&Id::Shelf(ShelfId::Game("a".into()))), Dir::Down);
        assert_eq!(b, Some(Id::Shelf(ShelfId::Game("b".into()))));
        let bar = step(&l, b.as_ref(), Dir::Down);
        assert_eq!(bar, Some(Id::Tab(Tab::Settings)));
        assert_eq!(step(&l, bar.as_ref(), Dir::Down), Some(Id::Settings(SettingsId::Tile(Setting::Power))));
        assert_eq!(step(&l, bar.as_ref(), Dir::Up), Some(Id::Shelf(ShelfId::Game("b".into()))));
    }

    #[test]
    fn sideways_stays_on_the_screen() {
        let l = layout();
        assert_eq!(step(&l, Some(&Id::Shelf(ShelfId::Game("a".into()))), Dir::Right), Some(Id::Shelf(ShelfId::Game("a".into()))));
        assert_eq!(step(&l, Some(&Id::Tab(Tab::Settings)), Dir::Right), Some(Id::Tab(Tab::Keyboard)));
    }

    #[test]
    fn lost_focus_goes_to_the_first_target() {
        assert_eq!(step(&layout(), Some(&Id::Shelf(ShelfId::Launch)), Dir::Down), Some(Id::Shelf(ShelfId::Game("a".into()))));
    }
}
