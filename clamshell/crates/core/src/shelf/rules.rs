//! Shelf's rules: its targets, the pad's steps through it, typing to find a title, the favourites, and
//! fetching metadata first for the entries it shows.

use crate::bottom::keyboard::{self, Key};
use crate::out::Out;
use crate::shelf::Shelf;
use crate::shelf::actions::Shortcut;
use crate::{Dirty, Effect, EntryAct, Event, Pointer, Save, ShelfEvent, ShelfId};

impl Shelf {
    /// Shelf's targets.
    pub(crate) fn on_target(&mut self, id: ShelfId, pointer: &mut Pointer, out: &mut Out) {
        match id {
            ShelfId::Entry(game) => {
                // Activating the selected entry again opens it if it is a group, or else plays it by
                // starting it or switching to it.
                if self.selected() == Some(&game) {
                    self.activate(pointer, out);
                } else if self.select(&game) {
                    self.unsaved = true;
                }
            }
            ShelfId::Collection(i) => {
                if self.set_collection(usize::from(i)) {
                    self.moved(pointer);
                }
            }
            ShelfId::Launch => self.activate(pointer, out),
            ShelfId::Favourite => self.toggle_favourite(out),
            ShelfId::Pin => self.ask(EntryAct::TogglePin, out),
            ShelfId::Continue => self.continue_selected(out),
            ShelfId::Enqueue => self.ask(EntryAct::Enqueue, out),
            ShelfId::File(action) => self.on_file(action, out),
            ShelfId::Up => {
                if self.up() {
                    self.moved(pointer);
                }
            }
        }
    }

    /// Asks for `act` on the selected entry, if there is one.
    pub(crate) fn ask(&self, act: EntryAct, out: &mut Out) {
        if let Some(id) = self.selected() {
            out.events.push(Event::Entry(id.clone(), act));
        }
    }

    /// Plays the entry that the selected group's Continue points to (see [`crate::Library::to_continue`]).
    pub(crate) fn continue_selected(&self, out: &mut Out) {
        let next = self.selected_index().and_then(|g| self.library().to_continue(g));
        if let Some(g) = next.and_then(|i| self.library().entries.get(i)) {
            out.events.push(Event::Entry(g.id.clone(), EntryAct::Play));
        }
    }

    /// Shelf's action on its selected entry: a group opens, anything else plays.
    pub(crate) fn activate(&mut self, pointer: &mut Pointer, out: &mut Out) {
        if self.open() {
            self.moved(pointer);
        } else {
            self.ask(EntryAct::Play, out);
        }
    }

    pub(crate) fn on_event(&mut self, event: ShelfEvent, pointer: &mut Pointer, out: &mut Out) {
        match event {
            ShelfEvent::ListStep(step) => {
                if self.step(step) {
                    self.unsaved = true;
                }
                pointer.focus_shelf(self.selected());
                pointer.show();
                out.mark(Dirty::BOTH);
            }
            ShelfEvent::CollectionStep(step) => {
                if self.collection_step(step) {
                    self.moved(pointer);
                }
                pointer.show();
                out.mark(Dirty::BOTH);
            }
            ShelfEvent::Rows(rows) => {
                if self.set_rows(rows) {
                    out.mark(Dirty::BOTH);
                }
            }
            // The details' actions on the pad's buttons (see [`crate::shelf::actions`]).
            ShelfEvent::Continue => {
                out.mark(Dirty::BOTH);
                self.act(Shortcut::Start, pointer, out);
            }
            ShelfEvent::ToggleFavourite => {
                out.mark(Dirty::BOTH);
                self.act(Shortcut::Y, pointer, out);
            }
            ShelfEvent::TogglePin => {
                out.mark(Dirty::BOTH);
                self.act(Shortcut::X, pointer, out);
            }
            ShelfEvent::ClearFilter => {
                if self.set_filter(String::new()) {
                    self.moved(pointer);
                }
                out.mark(Dirty::BOTH);
            }
        }
    }

    /// A key typed with shelf in front. A character narrows the current list to the titles starting with
    /// what is typed, Backspace removes the last character, Esc clears it all, and Enter opens or plays the
    /// selected entry.
    pub(crate) fn find(&mut self, stroke: keyboard::Stroke, pointer: &mut Pointer, out: &mut Out) {
        let mut filter = self.filter().to_owned();
        match stroke.key {
            Key::Char(c) if !stroke.ctrl && !stroke.alt && !stroke.logo => filter.push(if stroke.shift { keyboard::shifted(c) } else { c }),
            Key::Named(keyboard::Named::Backspace) => {
                filter.pop();
            }
            Key::Named(keyboard::Named::Esc) => filter.clear(),
            Key::Named(keyboard::Named::Enter) => return self.activate(pointer, out),
            _ => return,
        }
        if self.set_filter(filter) {
            self.moved(pointer);
        }
    }

    /// Puts the selected entry in or out of the favourites and saves them.
    pub(crate) fn toggle_favourite(&mut self, out: &mut Out) {
        let Some(moved) = self.favourite_selected() else {
            return;
        };
        if moved {
            self.unsaved = true;
        }
        let ids = self.library().favourite_ids().map(str::to_owned).collect();
        out.effects.push(Effect::Save(Save::Favourites(ids)));
    }

    /// Whether metadata is being fetched (see [`Shelf::follow_shown`]).
    pub(crate) fn set_fetching(&mut self, on: bool) {
        self.fetching = on;
        self.shown = None;
    }

    /// While metadata is being fetched and what shelf shows has changed, asks for the shown entries'
    /// metadata first.
    pub(crate) fn follow_shown(&mut self, out: &mut Out) {
        if !self.fetching {
            return;
        }
        let view = self.view();
        if self.shown == Some(view) {
            return;
        }
        self.shown = Some(view);
        let mut ids = Vec::new();
        self.in_view(&mut ids);
        out.effects.push(Effect::FetchFirst(ids));
    }
}
