//! What can be done to what shelf's details show (see docs/runners.md), each from one source: opening
//! from its runner (A) and Enqueue (a runner that takes more while it plays), Continue from its content (a
//! folder of videos: Start), the file actions from its
//! place (a browsed dir: see [`crate::files`]), Favourite (Y) and Pin (X) from an identity that outlives a
//! rescan (not a browsed file's). Where the identity's are missing, Y and X take the place's Copy and
//! Paste, Start its Select. The details draw these rows in order; the pad's buttons act on them.

use crate::files::FileAction;
use crate::{Effect, ShelfId, State};

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

/// The details' actions, in order: at most one of each, without allocating (shelf draws them every
/// frame).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Actions {
    items: [Action; 14],
    len: usize,
}

impl Default for Actions {
    fn default() -> Actions {
        Actions { items: [Action::Open; 14], len: 0 }
    }
}

impl Actions {
    pub fn push(&mut self, action: Action) {
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

impl State {
    /// The details' actions, in order (see the module's doc).
    pub fn actions(&self) -> Actions {
        let mut actions = Actions::default();
        let Some((game, selected)) = self.file_target() else {
            return actions;
        };
        if selected {
            actions.push(Action::Open);
        }
        let next = self.shelf.selected_index().filter(|_| selected && game.is_group()).and_then(|g| self.shelf.library().to_continue(g));
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

    /// The action a pad button stands for: Start Continue, Y Favourite (else Copy), X Pin (else Paste).
    pub fn shortcut(&self, key: Shortcut) -> Option<Action> {
        let wanted: &[Action] = match key {
            Shortcut::Y => &[Action::Favourite, Action::File(FileAction::Copy)],
            Shortcut::X => &[Action::Pin, Action::File(FileAction::Paste)],
            Shortcut::Start => &[Action::Continue, Action::File(FileAction::Select)],
        };
        let actions = self.actions();
        wanted.iter().copied().find(|a| actions.contains(a))
    }

    /// A pad button's action done, if the details have it.
    pub(crate) fn act(&mut self, key: Shortcut) -> Vec<Effect> {
        match self.shortcut(key) {
            Some(action) => self.on_action(action),
            None => vec![],
        }
    }

    fn on_action(&mut self, action: Action) -> Vec<Effect> {
        match action {
            Action::Open => self.activate_selected(),
            Action::Continue => self.continue_selected(),
            Action::Enqueue => self.enqueue_selected(),
            Action::Favourite => self.toggle_favourite(),
            Action::Pin => match self.shelf.selected().cloned() {
                Some(id) => self.toggle_pin(id),
                None => vec![],
            },
            Action::File(a) => self.on_file(a),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testkit::*;
    use crate::{Event, Id, ShelfEvent};

    #[test]
    fn an_entrys_actions_come_from_its_runner_content_and_identity() {
        let mut s = state();
        assert_eq!(s.actions().as_slice(), [Action::Open, Action::Favourite, Action::Pin]);
        assert_eq!(s.shortcut(Shortcut::Y), Some(Action::Favourite));
        assert_eq!(s.shortcut(Shortcut::Start), None, "a game: nothing to continue");
        tap(&mut s, Id::Shelf(ShelfId::Collection(2)));
        assert_eq!(s.actions().as_slice(), [Action::Open, Action::Continue, Action::Favourite, Action::Pin], "a series: Continue");
        let (_, fx) = s.apply(ShelfEvent::Continue.into());
        assert!(fx.iter().any(|e| matches!(e, Effect::Launch { .. })), "{fx:?}");
        s.apply(Event::Tick { unix_secs: 1 });
    }

    #[test]
    fn a_held_on_the_selected_entry_takes_the_pad_to_its_actions() {
        use crate::PointerEvent;
        let mut s = state();
        let g = Id::Shelf(ShelfId::Game(s.shelf().selected().cloned().unwrap_or_else(|| "?".into())));
        // By touch (no focus shown): nothing.
        s.apply(PointerEvent::LongPress(g.clone()).into());
        assert_eq!(s.pointer().focus(), Some(&g));
        // With the pad: its first action.
        s.apply(PointerEvent::Focus(g.clone()).into());
        s.apply(PointerEvent::LongPress(g).into());
        assert_eq!(s.pointer().focus(), Some(&Id::Shelf(ShelfId::Launch)));
    }
}
