//! Shelf's rules on the state: its targets, the pad's steps through it, typing to find a title, the
//! favourites, and fetching metadata first for the entries it shows.

use crate::bottom::keyboard::{self, Key};
use crate::{Dirty, Effect, Id, Save, ShelfEvent, ShelfId, State};

impl State {
    /// Shelf's targets.
    pub(crate) fn on_shelf(&mut self, id: ShelfId) -> Vec<Effect> {
        match id {
            ShelfId::Entry(game) => {
                // Activating the selected entry again opens it if it is a group, or else plays it by
                // starting it or switching to it.
                if self.shelf.selected() == Some(&game) {
                    return self.activate_selected();
                }
                if self.shelf.select(&game) {
                    self.saves.resume.touch(self.clock.mono());
                }
                vec![]
            }
            ShelfId::Collection(i) => {
                if self.shelf.set_collection(usize::from(i)) {
                    self.moved_collection();
                }
                vec![]
            }
            ShelfId::Launch => self.activate_selected(),
            ShelfId::Favourite => self.toggle_favourite(),
            ShelfId::Pin => match self.shelf.selected().cloned() {
                Some(id) => self.toggle_pin(id),
                None => vec![],
            },
            ShelfId::Continue => self.continue_selected(),
            ShelfId::Enqueue => self.enqueue_selected(),
            ShelfId::File(action) => self.on_file(action),
            ShelfId::Up => {
                if self.shelf.up() {
                    self.moved_collection();
                }
                vec![]
            }
        }
    }

    /// Plays the entry that the selected group's Continue points to (see [`crate::Library::to_continue`]).
    pub(crate) fn continue_selected(&mut self) -> Vec<Effect> {
        let next = self.shelf.selected_index().and_then(|g| self.shelf.library().to_continue(g));
        match next.and_then(|i| self.shelf.library().entries.get(i)).map(|g| g.id.clone()) {
            Some(id) => self.play(id),
            None => vec![],
        }
    }

    /// Shelf's action on its selected entry: a group opens, anything else plays.
    pub(crate) fn activate_selected(&mut self) -> Vec<Effect> {
        if self.shelf.open() {
            self.moved_collection();
            return vec![];
        }
        match self.shelf.selected().cloned() {
            Some(game) => self.play(game),
            None => vec![],
        }
    }

    /// Shelf went to another collection or list. The place is saved, and a focus in shelf's list follows
    /// the selection.
    pub(crate) fn moved_collection(&mut self) {
        self.saves.resume.touch(self.clock.mono());
        if matches!(self.pointer.focus, Some(Id::Shelf(ShelfId::Entry(_) | ShelfId::Collection(_)))) {
            self.pointer.focus = self.shelf.selected().cloned().map(|g| Id::Shelf(ShelfId::Entry(g)));
        }
    }

    pub(crate) fn on_shelf_event(&mut self, event: ShelfEvent) -> (Dirty, Vec<Effect>) {
        match event {
            ShelfEvent::ListStep(step) => {
                if self.shelf.step(step) {
                    self.saves.resume.touch(self.clock.mono());
                }
                self.pointer.focus = self.shelf.selected().cloned().map(|g| Id::Shelf(ShelfId::Entry(g)));
                self.pointer.show_focus = true;
                (Dirty::BOTH, vec![])
            }
            ShelfEvent::CollectionStep(step) => {
                if self.shelf.collection_step(step) {
                    self.moved_collection();
                }
                self.pointer.show_focus = true;
                (Dirty::BOTH, vec![])
            }
            ShelfEvent::Rows(rows) => (if self.shelf.set_rows(rows) { Dirty::BOTH } else { Dirty::NONE }, vec![]),
            // The details' actions on the pad's buttons (see [`actions`]).
            ShelfEvent::Continue => (Dirty::BOTH, self.act(crate::shelf::actions::Shortcut::Start)),
            ShelfEvent::ToggleFavourite => (Dirty::BOTH, self.act(crate::shelf::actions::Shortcut::Y)),
            ShelfEvent::TogglePin => (Dirty::BOTH, self.act(crate::shelf::actions::Shortcut::X)),
            ShelfEvent::ClearFilter => {
                if self.shelf.set_filter(String::new()) {
                    self.moved_collection();
                }
                (Dirty::BOTH, vec![])
            }
        }
    }

    /// A key typed with shelf in front. A character narrows the current list to the titles starting with
    /// what is typed, Backspace removes the last character, Esc clears it all, and Enter opens or plays the
    /// selected entry.
    pub(crate) fn find(&mut self, stroke: keyboard::Stroke) -> Vec<Effect> {
        let mut filter = self.shelf.filter().to_owned();
        match stroke.key {
            Key::Char(c) if !stroke.ctrl && !stroke.alt && !stroke.logo => filter.push(if stroke.shift { keyboard::shifted(c) } else { c }),
            Key::Named(keyboard::Named::Backspace) => {
                filter.pop();
            }
            Key::Named(keyboard::Named::Esc) => filter.clear(),
            Key::Named(keyboard::Named::Enter) => return self.activate_selected(),
            _ => return vec![],
        }
        if self.shelf.set_filter(filter) {
            self.moved_collection();
        }
        vec![]
    }

    /// Puts the selected entry in or out of the favourites and saves them.
    pub(crate) fn toggle_favourite(&mut self) -> Vec<Effect> {
        let Some(moved) = self.shelf.toggle_favourite() else {
            return vec![];
        };
        if moved {
            self.saves.resume.touch(self.clock.mono());
        }
        let ids = self.shelf.library().favourite_ids().map(str::to_owned).collect();
        vec![Effect::Save(Save::Favourites(ids))]
    }

    /// While metadata is being fetched and what shelf shows has changed, asks for the shown entries'
    /// metadata first.
    pub(crate) fn follow_shown(&mut self) -> Option<Effect> {
        if !self.fetching {
            return None;
        }
        let view = self.shelf.view();
        if self.shown == Some(view) {
            return None;
        }
        self.shown = Some(view);
        Some(Effect::FetchFirst(self.shelf.shown()))
    }
}
