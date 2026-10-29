//! What can be done to what the Library's details show (see docs/runners.md). Each action comes from one source:
//!
//! - Open (A) comes from the entry's runner, and so does Enqueue, for a runner that takes more while it
//!   plays.
//! - Continue (Start) comes from the entry's content, such as a folder of videos.
//! - The file actions come from the entry's place, a browsed directory (see [`crate::library_view::files`]).
//! - Favourite (Y) and Pin (X) come from an identity that outlives a rescan, which a browsed file doesn't
//!   have.
//!
//! Where Favourite and Pin are missing, Y and X take the place's Copy and Paste, and Start takes its
//! Select. The details draw these rows in order, and the pad's buttons act on them. Holding the selected
//! entry, by touch or with A, takes the focus to the rows themselves (see [`LibraryView::in_actions`]).

use crate::library_view::LibraryView;
use crate::library_view::files::FileAction;
use crate::out::Out;
use crate::{EntryAct, LibraryViewId, Pointer};

/// A row of the details' actions.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    Open,
    Continue,
    Enqueue,
    Favourite,
    Pin,
    File(FileAction),
    /// The pick's answer (see [`super::pick`]).
    Choose,
}

/// Room for every action once: the five of an entry and the fourteen file actions.
const ROOM: usize = 5 + 14;

/// The details' actions, in order, with at most one of each. It doesn't allocate, because the Library draws
/// them every frame.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Actions {
    items: [Action; ROOM],
    len: usize,
}

impl Default for Actions {
    fn default() -> Actions {
        Actions { items: [Action::Open; ROOM], len: 0 }
    }
}

impl Actions {
    pub fn push(&mut self, action: Action) {
        debug_assert!(!self.contains(&action) && self.len < ROOM, "each action once: {action:?} in {self:?}");
        if let Some(slot) = self.items.get_mut(self.len) {
            *slot = action;
            self.len += 1;
        }
    }

    pub fn as_slice(&self) -> &[Action] {
        self.items.get(..self.len).unwrap_or(&[])
    }

    pub fn contains(&self, action: &Action) -> bool {
        self.as_slice().contains(action)
    }
}

/// The pad's buttons for the details' actions (besides A).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Shortcut {
    Y,
    X,
    Start,
}

impl Action {
    /// Its target in the details.
    pub fn id(self) -> LibraryViewId {
        match self {
            Action::Open => LibraryViewId::Launch,
            Action::Continue => LibraryViewId::Continue,
            Action::Enqueue => LibraryViewId::Enqueue,
            Action::Favourite => LibraryViewId::Favourite,
            Action::Pin => LibraryViewId::Pin,
            Action::File(a) => LibraryViewId::File(a),
            Action::Choose => LibraryViewId::Choose,
        }
    }
}

impl LibraryView {
    /// The details' actions, in order (see the module's doc).
    pub fn actions(&self) -> Actions {
        let mut actions = Actions::default();
        let Some((entry, selected)) = self.file_target() else {
            return actions;
        };
        // Picking, the one action is Choose, on what the pick takes; folders open as they do.
        if let Some(pick) = self.pick() {
            if selected && pick.takes(entry) {
                actions.push(Action::Choose);
            }
            return actions;
        }
        if selected && !entry.is_empty_folder() {
            actions.push(Action::Open);
        }
        let next = self.selected_index().filter(|_| selected && entry.is_group()).and_then(|g| self.library().to_continue(g));
        if next.is_some() {
            actions.push(Action::Continue);
        }
        if selected && entry.runner.as_ref().is_some_and(|r| r.enqueue.is_some()) {
            actions.push(Action::Enqueue);
        }
        if selected && entry.browsed.is_none() {
            actions.push(Action::Favourite);
            // An app always pinned can't be unpinned (see `crate::builtin_apps`).
            if !crate::builtin_apps::always_pinned(&entry.id) {
                actions.push(Action::Pin);
            }
        }
        for &a in self.file_actions().as_slice() {
            actions.push(a);
        }
        actions
    }

    /// Whether the focus is on the details' actions, which holding the selected entry does. This is a
    /// mode of its own: the face buttons' shortcuts are off, the focus stays on the actions, A confirms
    /// and B goes back.
    pub fn in_actions(&self, pointer: &Pointer) -> bool {
        pointer.show_focus() && self.actions().as_slice().iter().any(|a| pointer.focus() == Some(&crate::Id::LibraryView(a.id())))
    }

    /// The action a pad button stands for: Start for Continue (or else Select), Y for Favourite (or else
    /// Copy), and X for Pin (or else Paste). There is none while the pad is on the actions themselves (see
    /// [`LibraryView::button_of`]).
    pub fn shortcut(&self, key: Shortcut, pointer: &Pointer) -> Option<Action> {
        if self.in_actions(pointer) {
            return None;
        }
        self.button_of(key)
    }

    /// The action that pad button `key` is the shortcut for, whether or not the pad is on the actions. The
    /// actions' badges use this.
    pub fn button_of(&self, key: Shortcut) -> Option<Action> {
        let wanted: &[Action] = match key {
            Shortcut::Y => &[Action::Favourite, Action::File(FileAction::Copy)],
            Shortcut::X => &[Action::Pin, Action::File(FileAction::Paste)],
            Shortcut::Start => &[Action::Continue, Action::File(FileAction::Select)],
        };
        let actions = self.actions();
        wanted.iter().copied().find(|a| actions.contains(a))
    }

    /// Does a pad button's action, if the details have it.
    pub(crate) fn act(&mut self, key: Shortcut, pointer: &mut Pointer, out: &mut Out) {
        let Some(action) = self.shortcut(key, pointer) else {
            return;
        };
        match action {
            Action::Open => self.activate(out),
            Action::Continue => self.continue_selected(out),
            Action::Enqueue => self.ask_enqueue(out),
            Action::Favourite => self.toggle_favourite(out),
            Action::Pin => self.ask(EntryAct::TogglePin, out),
            Action::File(a) => self.on_file(a, out),
            Action::Choose => self.choose(out),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testkit::*;
    use crate::{Effect, Event, Id, LibraryViewEvent};

    #[test]
    fn an_entrys_actions_come_from_its_runner_content_and_identity() {
        let mut s = state();
        assert_eq!(s.library_view().actions().as_slice(), [Action::Open, Action::Favourite, Action::Pin]);
        assert_eq!(s.library_view().shortcut(Shortcut::Y, s.pointer()), Some(Action::Favourite));
        assert_eq!(s.library_view().shortcut(Shortcut::Start, s.pointer()), None, "a game: nothing to continue");
        tap(&mut s, Id::LibraryView(LibraryViewId::Collection(2)));
        assert_eq!(s.library_view().actions().as_slice(), [Action::Open, Action::Continue, Action::Favourite, Action::Pin], "a series: Continue");
        let fx = s.applied(LibraryViewEvent::Continue.into());
        assert!(fx.iter().any(|e| matches!(e, Effect::Launch { .. })), "{fx:?}");
        s.applied(Event::tick(1));
    }

    /// While the pad is on the details' actions, the face buttons' shortcuts are off; with the focus back
    /// on the entry they work again.
    #[test]
    fn the_shortcuts_are_off_on_the_actions() {
        use crate::PointerEvent;
        let mut s = state();
        let g = entry("zelda-lttp");
        s.applied(PointerEvent::Focus(g.clone()).into());
        s.applied(PointerEvent::LongPress(g.clone()).into());
        assert!(s.library_view().in_actions(s.pointer()));
        assert_eq!(s.library_view().shortcut(Shortcut::Y, s.pointer()), None);
        assert_eq!(s.applied(LibraryViewEvent::ToggleFavourite.into()), vec![], "Y does nothing");
        assert_eq!(s.applied(LibraryViewEvent::TogglePin.into()), vec![], "nor X");
        s.applied(PointerEvent::Focus(g).into());
        assert!(!s.library_view().in_actions(s.pointer()));
        assert_eq!(s.applied(LibraryViewEvent::ToggleFavourite.into()), vec![Effect::Save(crate::Save::Favourites(vec!["zelda-lttp".into()]))]);
    }

    #[test]
    fn a_held_on_the_selected_entry_opens_its_actions() {
        use crate::PointerEvent;
        let mut s = state();
        let g = Id::LibraryView(LibraryViewId::Entry(s.library_view().selected().cloned().unwrap_or_else(|| "?".into())));
        // By touch, with no focus shown, the actions open: nothing plays, and the ring is on the first.
        assert!(!s.pointer().show_focus());
        assert_eq!(s.applied(PointerEvent::LongPress(g.clone()).into()), vec![], "not played");
        assert!(s.pointer().focused(&Id::LibraryView(LibraryViewId::Launch)));
        assert!(s.library_view().in_actions(s.pointer()));
        // With the pad, the same.
        s.applied(PointerEvent::Focus(g.clone()).into());
        s.applied(PointerEvent::LongPress(g).into());
        assert!(s.pointer().focused(&Id::LibraryView(LibraryViewId::Launch)));
    }
}
