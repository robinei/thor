//! Focus and presses. This holds the target the pad's focus is on, whose ring is shown only while
//! navigating with the pad, and the target a finger, the mouse or the pad's A is pressing, which is drawn
//! lit. For a press it also holds which list item the press came down on (see [`crate::nav::items`]).

use std::sync::Arc;

use crate::id::{Id, ShelfId};
use crate::kit::list::ItemKey;

/// A press on a list item, kept by what the item is rather than where it is, so a list that changes
/// under the finger doesn't redirect it.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Press {
    id: Id,
    item: ItemKey,
}

#[derive(Debug, Clone, Default)]
pub struct Pointer {
    focus: Option<Id>,
    /// The focus ring is shown only while navigating with the pad.
    show_focus: bool,
    /// The target being pressed (drawn lit), if any.
    pressed: Option<Id>,
    press: Option<Press>,
}

impl Pointer {
    /// The focus goes to `id`. The ring stays as it was.
    pub(crate) fn focus_on(&mut self, id: Id) {
        self.focus = Some(id);
    }

    /// The focus goes to shelf's selected entry, or nowhere without one.
    pub(crate) fn focus_shelf(&mut self, selected: Option<&Arc<str>>) {
        self.focus = selected.cloned().map(|g| Id::Shelf(ShelfId::Entry(g)));
    }

    /// Shelf's selection moved. A focus in shelf's list follows it.
    pub(crate) fn shelf_moved(&mut self, selected: Option<&Arc<str>>) {
        if matches!(self.focus, Some(Id::Shelf(ShelfId::Entry(_) | ShelfId::Collection(_)))) {
            self.focus_shelf(selected);
        }
    }

    /// Shows the focus ring, as the pad navigates.
    pub(crate) fn show(&mut self) {
        self.show_focus = true;
    }

    /// `id` is pressed, on the list item `item` if it is one.
    pub(crate) fn down(&mut self, id: Id, item: Option<ItemKey>) {
        self.press = item.map(|item| Press { id: id.clone(), item });
        self.pressed = Some(id);
    }

    /// The press on `id` ended.
    pub(crate) fn up(&mut self, id: &Id) {
        if self.pressed.as_ref() == Some(id) {
            self.pressed = None;
        }
        if self.press.as_ref().is_some_and(|p| p.id == *id) {
            self.press = None;
        }
    }

    /// The list item the press on `id` came down on, if `id` is pressed and is a list item.
    pub(crate) fn pressed_item(&self, id: &Id) -> Option<&ItemKey> {
        self.press.as_ref().filter(|p| p.id == *id).map(|p| &p.item)
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
        assert_eq!(s.keyboard().hold(&Id::Key(find(Key::Char('a')).unwrap())), Hold::Repeat);
        assert_eq!(s.keyboard().hold(&Id::Key(find(Key::Mod(Modifier::Shift)).unwrap())), Hold::Long);
        assert_eq!(s.keyboard().hold(&Id::Shelf(ShelfId::Launch)), Hold::Long);
    }

    #[test]
    fn down_and_up_light_a_target() {
        let mut s = state();
        let launch = Id::Shelf(ShelfId::Launch);
        s.applied(PointerEvent::Down(launch.clone(), None).into());
        assert_eq!(s.pointer().pressed(), Some(&launch));
        s.applied(PointerEvent::Up(launch).into());
        assert_eq!(s.pointer().pressed(), None);
    }

    /// A Down without its Up (the UI lost the finger) leaves the target lit and its item held, until the
    /// next Down replaces it or its own Up comes; an Up of another target leaves it. The UI sends an Up for
    /// every Down it sends, including on a cancelled touch, so this is how a lost one recovers.
    #[test]
    fn a_down_without_its_up_lasts_until_the_next_down() {
        let mut s = state();
        let launch = Id::Shelf(ShelfId::Launch);
        s.applied(PointerEvent::Down(launch.clone(), None).into());
        s.applied(PointerEvent::Up(Id::Shelf(ShelfId::Up)).into());
        assert_eq!(s.pointer().pressed(), Some(&launch), "another's Up");
        let tab = Id::Tab(crate::Tab::Dock);
        s.applied(PointerEvent::Down(tab.clone(), None).into());
        assert_eq!(s.pointer().pressed(), Some(&tab), "replaced");
        s.applied(PointerEvent::Up(tab).into());
        assert_eq!(s.pointer().pressed(), None);
    }

    #[test]
    fn the_focus_ring_shows_for_the_pad_only() {
        let mut s = state();
        assert!(!s.pointer().show_focus());
        s.applied(PointerEvent::Focus(Id::Shelf(ShelfId::Launch)).into());
        assert!(s.pointer().focused(&Id::Shelf(ShelfId::Launch)));
        s.applied(PointerEvent::HideFocus.into());
        assert!(!s.pointer().show_focus());
    }
}
