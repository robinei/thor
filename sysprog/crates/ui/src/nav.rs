//! Pad navigation: moving focus by direction between the targets of both screens, as if the bottom screen
//! were directly below the top one.

use sysprog_core::Id;

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

/// The target to focus after moving `dir` from `current`. Up and down may cross to the other screen;
/// left and right stay on the current one. With no current focus (or a focused item no longer drawn),
/// the first target.
pub fn step(targets: &[NavTarget], current: Option<&Id>, dir: Dir) -> Option<Id> {
    let Some(from) = current.and_then(|id| targets.iter().find(|t| &t.id == id)) else {
        return targets.first().map(|t| t.id.clone());
    };
    let (ox, oy) = from.rect.center();
    targets
        .iter()
        .filter(|t| t.id != from.id)
        .filter(|t| matches!(dir, Dir::Up | Dir::Down) || t.screen == from.screen)
        .filter_map(|t| {
            let (cx, cy) = t.rect.center();
            let (dx, dy) = (cx - ox, cy - oy);
            let (along, across) = match dir {
                Dir::Left => (-dx, dy),
                Dir::Right => (dx, dy),
                Dir::Up => (-dy, dx),
                Dir::Down => (dy, dx),
            };
            (along > 1e-4).then_some((along + 2.0 * across.abs(), t))
        })
        .min_by(|a, b| a.0.total_cmp(&b.0))
        .map(|(_, t)| t.id.clone())
        .or_else(|| Some(from.id.clone()))
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::indexing_slicing)]
mod tests {
    use super::*;
    use sysprog_core::{Mode, Power};

    fn t(screen: Screen, x: f32, y: f32, id: Id) -> NavTarget {
        NavTarget { screen, rect: Rect::new(x, y, 0.1, 0.1), id }
    }

    fn layout() -> Vec<NavTarget> {
        vec![
            t(Screen::Top, 0.0, 0.0, Id::Game("a".into())),
            t(Screen::Top, 0.0, 0.2, Id::Game("b".into())),
            t(Screen::Bottom, 0.0, 1.0, Id::Mode(Mode::Follow)),
            t(Screen::Bottom, 0.5, 1.0, Id::Mode(Mode::Keyboard)),
            t(Screen::Bottom, 0.0, 1.5, Id::Power(Power::Low)),
        ]
    }

    #[test]
    fn down_walks_the_list_then_crosses_to_the_bottom_screen() {
        let l = layout();
        let b = step(&l, Some(&Id::Game("a".into())), Dir::Down);
        assert_eq!(b, Some(Id::Game("b".into())));
        let bar = step(&l, b.as_ref(), Dir::Down);
        assert_eq!(bar, Some(Id::Mode(Mode::Follow)));
        assert_eq!(step(&l, bar.as_ref(), Dir::Down), Some(Id::Power(Power::Low)));
        assert_eq!(step(&l, bar.as_ref(), Dir::Up), Some(Id::Game("b".into())));
    }

    #[test]
    fn sideways_stays_on_the_screen() {
        let l = layout();
        assert_eq!(step(&l, Some(&Id::Game("a".into())), Dir::Right), Some(Id::Game("a".into())));
        assert_eq!(step(&l, Some(&Id::Mode(Mode::Follow)), Dir::Right), Some(Id::Mode(Mode::Keyboard)));
    }

    #[test]
    fn lost_focus_goes_to_the_first_target() {
        assert_eq!(step(&layout(), Some(&Id::Launch), Dir::Down), Some(Id::Game("a".into())));
    }
}
