//! The Library's rules: its targets, the pad's steps through it, typing to find a title, the favourites, and
//! fetching metadata first for the entries it shows.

use crate::bottom::keyboard::{self, Key};
use crate::library_view::actions::Shortcut;
use crate::library_view::{LibraryView, ShownList};
use crate::out::Out;
use crate::{Effect, EntryAct, Event, Id, LibraryViewEvent, LibraryViewId, Pointer, Save};

impl LibraryView {
    /// A tap, or a long press if `long`, on one of the Library's targets.
    pub(crate) fn on_target(&mut self, id: LibraryViewId, long: bool, pointer: &mut Pointer, out: &mut Out) {
        match id {
            // A hold on the selected entry, by touch or with the pad, opens the entry's actions: the
            // focus ring shows on the first. Up and down go through them, A runs one, and B goes back to
            // the list.
            LibraryViewId::Entry(entry) if long && self.selected() == Some(&entry) => {
                if let Some(first) = self.actions().as_slice().first() {
                    pointer.focus_on(Id::LibraryView(first.id()));
                    pointer.show();
                }
            }
            LibraryViewId::Entry(entry) => {
                // Activating the selected entry again opens it if it is a group, or else plays it by
                // starting it or switching to it.
                if self.selected() == Some(&entry) {
                    self.activate(out);
                } else {
                    self.select(&entry);
                }
            }
            LibraryViewId::Collection(i) => {
                self.set_collection(usize::from(i));
            }
            // Picking, a file is chosen rather than opened.
            LibraryViewId::Launch if self.pick().is_some() && self.selected_entry().is_some_and(|g| !g.is_group()) => self.choose(out),
            LibraryViewId::Launch => self.activate(out),
            LibraryViewId::SaveHere => self.save(),
            LibraryViewId::SaveAs => {
                if let Some(super::pick::Takes::Save { name, .. }) = self.pick().map(|p| &p.takes) {
                    let editor = crate::bottom::editor::Editor { purpose: crate::bottom::editor::Purpose::SaveName, text: name.clone() };
                    out.events.push(crate::Event::EditOnKeyboard(editor));
                }
            }
            LibraryViewId::Choose | LibraryViewId::CancelPick => {
                if id == LibraryViewId::Choose {
                    self.choose(out);
                } else {
                    self.cancel_pick();
                }
                // Back in the edit mode's form, on what began the pick.
                if self.pick().is_none()
                    && let Some(form) = self.edit().and_then(|e| e.form())
                {
                    pointer.focus_on(crate::library_view::edit::EditId::Pick(form.field as u16).into());
                }
            }
            LibraryViewId::Favourite => self.toggle_favourite(out),
            LibraryViewId::Pin => self.ask(EntryAct::TogglePin, out),
            LibraryViewId::Continue => self.continue_selected(out),
            LibraryViewId::Enqueue => self.ask_enqueue(out),
            LibraryViewId::File(action) => self.on_file(action, out),
            LibraryViewId::Edit(id) => {
                self.edit_tap(id, out);
                // Left, or a pick begun: the focus goes to the Library's selection.
                if self.edit().is_none() || self.pick().is_some() {
                    pointer.focus_library_view(self.selected());
                }
            }
            LibraryViewId::Up => {
                self.up();
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

    /// The Library's action on its selected entry: a group opens, anything else plays.
    pub(crate) fn activate(&mut self, out: &mut Out) {
        if !self.open() {
            self.ask(EntryAct::Play, out);
        }
    }

    pub(crate) fn on_event(&mut self, event: LibraryViewEvent, pointer: &mut Pointer, out: &mut Out) {
        match event {
            LibraryViewEvent::ListStep(step) => {
                self.step(step);
                pointer.focus_library_view(self.selected());
                pointer.show();
            }
            LibraryViewEvent::CollectionStep(step) => {
                self.collection_step(step);
                pointer.show();
            }
            LibraryViewEvent::Rows(rows) => {
                self.set_rows(rows);
            }
            // The details' actions on the pad's buttons (see [`crate::library_view::actions`]).
            LibraryViewEvent::Continue => {
                self.act(Shortcut::Start, pointer, out);
            }
            LibraryViewEvent::ToggleFavourite => {
                self.act(Shortcut::Y, pointer, out);
            }
            LibraryViewEvent::TogglePin => {
                self.act(Shortcut::X, pointer, out);
            }
            LibraryViewEvent::ClearFilter => {
                self.set_filter(String::new());
            }
        }
    }

    /// A key typed with the Library in front. A character narrows the current list to the titles starting with
    /// what is typed, Backspace removes the last character, Esc clears it all, and Enter opens or plays the
    /// selected entry.
    pub(crate) fn find(&mut self, stroke: keyboard::Stroke, out: &mut Out) {
        let mut filter = self.filter().to_owned();
        match stroke.key {
            Key::Char(c) if !stroke.ctrl && !stroke.alt && !stroke.logo => filter.push(if stroke.shift { keyboard::shifted(c) } else { c }),
            Key::Named(keyboard::Named::Backspace) => {
                filter.pop();
            }
            Key::Named(keyboard::Named::Esc) => filter.clear(),
            Key::Named(keyboard::Named::Enter) => return self.activate(out),
            _ => return,
        }
        self.set_filter(filter);
    }

    /// Puts the selected entry in or out of the favourites and saves them. A focus in the list follows
    /// the selection, which moves if the entry left the list.
    pub(crate) fn toggle_favourite(&mut self, out: &mut Out) {
        if self.favourite_selected().is_none() {
            return;
        }
        let ids = self.library().favourite_ids().map(str::to_owned).collect();
        out.effects.push(Effect::Save(Save::Favourites(ids)));
    }

    /// Whether metadata is being fetched (see [`LibraryView::fetch_first`]).
    pub(crate) fn set_fetching(&mut self, on: bool) {
        self.fetching = on;
    }

    /// While metadata is being fetched, what the list shows, whose entries' metadata is asked for first
    /// whenever it changes (see [`crate::sync::Sent::fetch_first`]).
    pub(crate) fn fetch_first(&self) -> Option<ShownList> {
        self.fetching.then(|| self.view())
    }
}
