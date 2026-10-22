//! Focus and presses. This holds the target the pad's focus is on, whose ring is shown only while
//! navigating with the pad, and the target a finger, the mouse or the pad's A is pressing, which is drawn
//! lit. For a press it also holds which list item the press came down on (see [`crate::nav::items`]).

use std::sync::Arc;

use crate::id::{Id, ShelfId};
use crate::kit::list::ItemKey;

/// A press on a list item, kept by what the item is rather than where it is, so a list that changes
/// under the finger doesn't redirect it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Press {
    pub id: Id,
    pub item: ItemKey,
}

#[derive(Debug, Clone, Default)]
pub struct Pointer {
    pub(crate) focus: Option<Id>,
    /// The focus ring is shown only while navigating with the pad.
    pub(crate) show_focus: bool,
    /// The target being pressed (drawn lit), if any.
    pub(crate) pressed: Option<Id>,
    pub(crate) press: Option<Press>,
}

impl Pointer {
    /// The focus goes to shelf's selected entry, or nowhere without one.
    pub(crate) fn focus_shelf(&mut self, selected: Option<&Arc<str>>) {
        self.focus = selected.cloned().map(|g| Id::Shelf(ShelfId::Entry(g)));
    }

    pub fn focus(&self) -> Option<&Id> {
        self.focus.as_ref()
    }

    pub fn show_focus(&self) -> bool {
        self.show_focus
    }

    pub fn pressed(&self) -> Option<&Id> {
        self.pressed.as_ref()
    }

    /// Whether the focus ring is on `id`.
    pub fn focused(&self, id: &Id) -> bool {
        self.show_focus && self.focus.as_ref() == Some(id)
    }

    /// Hides the focus ring. Returns whether it was shown.
    pub(crate) fn hide_focus(&mut self) -> bool {
        std::mem::replace(&mut self.show_focus, false)
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use crate::bottom::keyboard::{Key, Modifier, find};
    use crate::testkit::*;
    use crate::*;

    #[test]
    fn keys_repeat_and_modifiers_lock_when_held() {
        let s = state();
        assert_eq!(s.hold(&Id::Key(find(Key::Char('a')).unwrap())), Hold::Repeat);
        assert_eq!(s.hold(&Id::Key(find(Key::Mod(Modifier::Shift)).unwrap())), Hold::Long);
        assert_eq!(s.hold(&Id::Shelf(ShelfId::Launch)), Hold::Long);
    }

    #[test]
    fn down_and_up_light_a_target_on_its_screen() {
        let mut s = state();
        let launch = Id::Shelf(ShelfId::Launch);
        assert_eq!(s.apply(PointerEvent::Down(launch.clone()).into()).0, Dirty::BOTH, "shelf's: drawn on top");
        assert_eq!(s.pointer().pressed(), Some(&launch));
        s.apply(PointerEvent::Up(launch).into());
        assert_eq!(s.pointer().pressed(), None);
        assert_eq!(s.apply(PointerEvent::Down(Id::PadMouse).into()).0, Dirty::BOTTOM);
    }

    #[test]
    fn the_focus_ring_shows_for_the_pad_only() {
        let mut s = state();
        assert!(!s.pointer().show_focus());
        s.apply(PointerEvent::Focus(Id::Shelf(ShelfId::Launch)).into());
        assert!(s.pointer().focused(&Id::Shelf(ShelfId::Launch)));
        assert_eq!(s.apply(PointerEvent::HideFocus.into()).0, Dirty::BOTH);
        assert!(!s.pointer().show_focus());
        assert_eq!(s.apply(PointerEvent::HideFocus.into()).0, Dirty::NONE);
    }
}
