//! Shelf's rules: its targets, the pad's steps through it, typing to find a title, the favourites, and
//! fetching metadata first for the entries it shows.

use crate::bottom::keyboard::{self, Key};
use crate::out::Out;
use crate::shelf::actions::Shortcut;
use crate::shelf::{Shelf, ShelfView};
use crate::{Effect, EntryAct, Event, Id, Pointer, Save, ShelfEvent, ShelfId};

impl Shelf {
    /// A tap, or a long press if `long`, on one of shelf's targets.
    pub(crate) fn on_target(&mut self, id: ShelfId, long: bool, pointer: &mut Pointer, out: &mut Out) {
        match id {
            // A hold on the selected entry with the pad, while its focus is showing, moves the pad to the
            // entry's actions and focuses the first. Up and down go through them, A runs one, and B goes
            // back to the list.
            ShelfId::Entry(game) if long && pointer.show_focus() && self.selected() == Some(&game) => {
                if let Some(first) = self.actions().as_slice().first() {
                    pointer.focus_on(Id::Shelf(first.id()));
                }
            }
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
            ShelfId::Enqueue => self.ask_enqueue(out),
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

    /// Asks for Enqueue on what the details show, if an entry is selected (see [`crate::session::enqueue`]).
    pub(crate) fn ask_enqueue(&self, out: &mut Out) {
        if self.selected().is_some() {
            out.events.push(Event::Enqueue);
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
            }
            ShelfEvent::CollectionStep(step) => {
                if self.collection_step(step) {
                    self.moved(pointer);
                }
                pointer.show();
            }
            ShelfEvent::Rows(rows) => {
                self.set_rows(rows);
            }
            // The details' actions on the pad's buttons (see [`crate::shelf::actions`]).
            ShelfEvent::Continue => {
                self.act(Shortcut::Start, pointer, out);
            }
            ShelfEvent::ToggleFavourite => {
                self.act(Shortcut::Y, pointer, out);
            }
            ShelfEvent::TogglePin => {
                self.act(Shortcut::X, pointer, out);
            }
            ShelfEvent::ClearFilter => {
                if self.set_filter(String::new()) {
                    self.moved(pointer);
                }
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

    /// Whether metadata is being fetched (see [`Shelf::fetch_first`]).
    pub(crate) fn set_fetching(&mut self, on: bool) {
        self.fetching = on;
    }

    /// While metadata is being fetched, what the list shows, whose entries' metadata is asked for first
    /// whenever it changes (see [`crate::sync::Sent::fetch_first`]).
    pub(crate) fn fetch_first(&self) -> Option<ShelfView> {
        self.fetching.then(|| self.view())
    }
}
