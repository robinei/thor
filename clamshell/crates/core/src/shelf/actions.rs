//! What can be done to what shelf's details show (see docs/runners.md). Each action comes from one source:
//!
//! - Open (A) comes from the entry's runner, and so does Enqueue, for a runner that takes more while it
//!   plays.
//! - Continue (Start) comes from the entry's content, such as a folder of videos.
//! - The file actions come from the entry's place, a browsed directory (see [`crate::shelf::files`]).
//! - Favourite (Y) and Pin (X) come from an identity that outlives a rescan, which a browsed file doesn't
//!   have.
//!
//! Where Favourite and Pin are missing, Y and X take the place's Copy and Paste, and Start takes its
//! Select. The details draw these rows in order, and the pad's buttons act on them. Holding A on the
//! selected entry takes the pad to the rows themselves (see [`Shelf::in_actions`]).

use crate::out::Out;
use crate::shelf::Shelf;
use crate::shelf::files::FileAction;
use crate::{EntryAct, Pointer, ShelfId};

/// A row of the details' actions.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    Open,
    Continue,
    Enqueue,
    Favourite,
    Pin,
    File(FileAction),
}

/// Room for every action once: the five of an entry and the fourteen file actions.
const ROOM: usize = 5 + 14;

/// The details' actions, in order, with at most one of each. It doesn't allocate, because shelf draws
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
    pub fn id(self) -> ShelfId {
        match self {
            Action::Open => ShelfId::Launch,
            Action::Continue => ShelfId::Continue,
            Action::Enqueue => ShelfId::Enqueue,
            Action::Favourite => ShelfId::Favourite,
            Action::Pin => ShelfId::Pin,
            Action::File(a) => ShelfId::File(a),
        }
    }
}

impl Shelf {
    /// The details' actions, in order (see the module's doc).
    pub fn actions(&self) -> Actions {
        let mut actions = Actions::default();
        let Some((game, selected)) = self.file_target() else {
            return actions;
        };
        if selected {
            actions.push(Action::Open);
        }
        let next = self.selected_index().filter(|_| selected && game.is_group()).and_then(|g| self.library().to_continue(g));
        if next.is_some() {
            actions.push(Action::Continue);
        }
        if selected && game.runner.as_ref().is_some_and(|r| r.enqueue.is_some()) {
            actions.push(Action::Enqueue);
        }
        if selected && game.browsed.is_none() {
            actions.push(Action::Favourite);
            actions.push(Action::Pin);
        }
        for &a in self.file_actions().as_slice() {
            actions.push(a);
        }
        actions
    }

    /// Whether the pad is on the details' actions, which holding A on the selected entry does. This is a
    /// mode of its own: the face buttons' shortcuts are off, the focus stays on the actions, A confirms
    /// and B goes back.
    pub fn in_actions(&self, pointer: &Pointer) -> bool {
        pointer.show_focus() && self.actions().as_slice().iter().any(|a| pointer.focus() == Some(&crate::Id::Shelf(a.id())))
    }

    /// The action a pad button stands for: Start for Continue (or else Select), Y for Favourite (or else
    /// Copy), and X for Pin (or else Paste). There is none while the pad is on the actions themselves (see
    /// [`Shelf::button_of`]).
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
            Action::Open => self.activate(pointer, out),
            Action::Continue => self.continue_selected(out),
            Action::Enqueue => self.ask(EntryAct::Enqueue, out),
            Action::Favourite => self.toggle_favourite(out),
            Action::Pin => self.ask(EntryAct::TogglePin, out),
            Action::File(a) => self.on_file(a, out),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testkit::*;
    use crate::{Effect, Event, Id, ShelfEvent};

    #[test]
    fn an_entrys_actions_come_from_its_runner_content_and_identity() {
        let mut s = state();
        assert_eq!(s.shelf().actions().as_slice(), [Action::Open, Action::Favourite, Action::Pin]);
        assert_eq!(s.shelf().shortcut(Shortcut::Y, s.pointer()), Some(Action::Favourite));
        assert_eq!(s.shelf().shortcut(Shortcut::Start, s.pointer()), None, "a game: nothing to continue");
        tap(&mut s, Id::Shelf(ShelfId::Collection(2)));
        assert_eq!(s.shelf().actions().as_slice(), [Action::Open, Action::Continue, Action::Favourite, Action::Pin], "a series: Continue");
        let (_, fx) = s.apply(ShelfEvent::Continue.into());
        assert!(fx.iter().any(|e| matches!(e, Effect::Launch { .. })), "{fx:?}");
        s.apply(Event::tick(1));
    }

    #[test]
    fn a_held_on_the_selected_entry_takes_the_pad_to_its_actions() {
        use crate::PointerEvent;
        let mut s = state();
        let g = Id::Shelf(ShelfId::Entry(s.shelf().selected().cloned().unwrap_or_else(|| "?".into())));
        // By touch, with no focus shown, nothing happens.
        s.apply(PointerEvent::LongPress(g.clone()).into());
        assert_eq!(s.pointer().focus(), Some(&g));
        // With the pad, the focus goes to the first action.
        s.apply(PointerEvent::Focus(g.clone()).into());
        s.apply(PointerEvent::LongPress(g).into());
        assert_eq!(s.pointer().focus(), Some(&Id::Shelf(ShelfId::Launch)));
    }
}
