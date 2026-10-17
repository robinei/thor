//! Shelf's navigation: the collections, each with its own cursor over its games, kept in range and on
//! screen; and changes to the library (plays, favourites) that keep the collection and selection. The
//! methods say whether the selection changed; the state saves the place and moves the focus around them.

use std::sync::Arc;

use crate::scroll::{Rows, Scroll};
use crate::{Collection, Game, Library};

/// A collection's place in its list: the selected game's position, and the rows scrolled past (whole
/// rows at rest; fractions while a finger drags the list).
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct Cursor {
    pub index: usize,
    pub top: f32,
}

/// A collection's place as kept: the selected game's position and the list's scroll.
#[derive(Debug, Clone, Default)]
struct Place {
    index: usize,
    scroll: Scroll,
}

#[derive(Debug, Default)]
pub struct Shelf {
    library: Library,
    /// The current collection, and each collection's cursor.
    current: usize,
    cursors: Vec<Place>,
    /// List rows on screen (reported by the UI).
    rows: usize,
    /// Id of the selected game (shelf's list on top, its settings below): the current collection's
    /// cursor's.
    selected: Option<Arc<str>>,
}

impl Shelf {
    pub(crate) fn new(library: Library) -> Shelf {
        let cursors = vec![Place::default(); library.collections.len()];
        let mut shelf = Shelf { library, cursors, rows: 1, ..Shelf::default() };
        shelf.sync();
        shelf
    }

    pub fn library(&self) -> &Library {
        &self.library
    }

    /// The current collection's position.
    pub fn current(&self) -> usize {
        self.current
    }

    pub fn rows(&self) -> usize {
        self.rows
    }

    pub fn selected(&self) -> Option<&Arc<str>> {
        self.selected.as_ref()
    }

    pub fn collection(&self) -> Option<&Collection> {
        self.library.collections.get(self.current)
    }

    pub fn cursor(&self) -> Cursor {
        self.cursors.get(self.current).map_or_else(Cursor::default, |p| Cursor { index: p.index, top: p.scroll.offset() })
    }

    /// The current collection's scroll.
    pub(crate) fn scroll(&self) -> &Scroll {
        self.cursors.get(self.current).map_or(&crate::scroll::EMPTY, |p| &p.scroll)
    }

    pub(crate) fn scroll_mut(&mut self) -> Option<&mut Scroll> {
        self.cursors.get_mut(self.current).map(|p| &mut p.scroll)
    }

    /// A game by its id.
    pub fn game(&self, id: &str) -> Option<&Game> {
        self.library.games.iter().find(|g| &*g.id == id)
    }

    /// The game at `position` in the current collection.
    pub fn game_at(&self, position: usize) -> Option<&Game> {
        let index = *self.collection()?.games.get(position)?;
        self.library.games.get(index)
    }

    pub fn selected_game(&self) -> Option<&Game> {
        self.game_at(self.cursor().index)
    }

    /// Where `game` is in the current collection.
    fn position(&self, game: &str) -> Option<usize> {
        self.collection()?.games.iter().position(|&i| self.library.games.get(i).is_some_and(|g| &*g.id == game))
    }

    /// Keeps the cursor in range and on screen, and `selected` in step with it. Whether the selection
    /// changed.
    pub(crate) fn sync(&mut self) -> bool {
        let before = self.selected.clone();
        let len = self.collection().map_or(0, |c| c.games.len());
        let rows = self.rows.max(1) as f32;
        if let Some(c) = self.cursors.get_mut(self.current) {
            c.index = c.index.min(len.saturating_sub(1));
            c.scroll.set_visible(rows, len);
            c.scroll.show(c.index, len);
        }
        self.selected = self.selected_game().map(|g| g.id.clone());
        self.selected != before
    }

    /// Moves the cursor by `step` games (clamped at the ends). Whether the selection changed.
    pub(crate) fn step(&mut self, step: i32) -> bool {
        let len = self.collection().map_or(0, |c| c.games.len());
        if let Some(c) = self.cursors.get_mut(self.current) {
            let target = c.index as i64 + i64::from(step);
            c.index = target.clamp(0, len.saturating_sub(1) as i64) as usize;
        }
        self.sync()
    }

    /// Selects `game` in the current collection, if it is there. Whether the selection changed.
    pub(crate) fn select(&mut self, game: &str) -> bool {
        let position = self.position(game);
        if let (Some(p), Some(c)) = (position, self.cursors.get_mut(self.current)) {
            c.index = p;
        }
        self.sync()
    }

    /// Makes collection `i` the current one, if there is one there (whether there was).
    pub(crate) fn set_collection(&mut self, i: usize) -> bool {
        if i >= self.library.collections.len() {
            return false;
        }
        self.current = i;
        self.sync();
        true
    }

    /// Goes `step` collections on, wrapping (whether there are any).
    pub(crate) fn collection_step(&mut self, step: i32) -> bool {
        let n = self.library.collections.len() as i64;
        n > 0 && self.set_collection((self.current as i64 + i64::from(step)).rem_euclid(n) as usize)
    }

    /// How many rows are on screen. Whether the cursor moved (to keep the selection on screen).
    pub(crate) fn set_rows(&mut self, rows: usize) -> bool {
        if rows == self.rows {
            return false;
        }
        let before = self.cursor();
        self.rows = rows.max(1);
        self.sync();
        self.cursor() != before
    }

    /// Changes the library (favourites, plays) keeping the current collection, each one's place and the
    /// selected game, all by name, since derived collections may come or go. Whether the selection
    /// changed.
    pub(crate) fn relayout(&mut self, change: impl FnOnce(&mut Library)) -> bool {
        let names: Vec<String> = self.library.collections.iter().map(|c| c.name.clone()).collect();
        let current_name = names.get(self.current).cloned();
        let selected = self.selected.clone();
        change(&mut self.library);
        let old = std::mem::take(&mut self.cursors);
        self.cursors =
            self.library.collections.iter().map(|c| names.iter().position(|n| *n == c.name).and_then(|i| old.get(i).cloned()).unwrap_or_default()).collect();
        self.current = current_name.and_then(|n| self.library.collections.iter().position(|c| c.name == n)).unwrap_or(0);
        if let Some(id) = selected {
            let position = self.position(&id);
            if let (Some(p), Some(c)) = (position, self.cursors.get_mut(self.current)) {
                c.index = p;
            }
        }
        self.sync()
    }

    /// The selected game in or out of the favourites, if there is one: whether the selection changed.
    pub(crate) fn toggle_favourite(&mut self) -> Option<bool> {
        let id = self.selected.clone()?;
        Some(self.relayout(|lib| {
            if let Some(g) = lib.games.iter_mut().find(|g| g.id == id) {
                g.favourite = !g.favourite;
            }
            lib.rebuild_derived();
        }))
    }

    /// A play of `game` counted (it ended). Whether the selection changed.
    pub(crate) fn add_play(&mut self, game: &str, start: u64, duration: u64) -> bool {
        self.relayout(|lib| {
            if let Some(g) = lib.games.iter_mut().find(|g| &*g.id == game) {
                g.plays.add(start, duration);
            }
            lib.rebuild_derived();
        })
    }

    /// Each collection's name and its selected game's id, tab-separated, a line each.
    pub(crate) fn selections(&self, text: &mut String) {
        for (i, c) in self.library.collections.iter().enumerate() {
            let index = self.cursors.get(i).map_or(0, |p| p.index);
            if let Some(g) = c.games.get(index).and_then(|&g| self.library.games.get(g)) {
                text.push_str(&c.name);
                text.push('\t');
                text.push_str(&g.id);
                text.push('\n');
            }
        }
    }

    /// Selects `game` in the collection named `name` (either may be gone: then nothing).
    pub(crate) fn restore_selection(&mut self, name: &str, game: &str) {
        let Some(i) = self.library.collections.iter().position(|c| c.name == name) else {
            return;
        };
        let position = self.library.collections.get(i).and_then(|c| c.games.iter().position(|&g| self.library.games.get(g).is_some_and(|g| &*g.id == game)));
        if let (Some(p), Some(cursor)) = (position, self.cursors.get_mut(i)) {
            cursor.index = p;
        }
    }

    /// Makes the collection named `name` the current one, if there is one.
    pub(crate) fn restore_current(&mut self, name: &str) {
        if let Some(i) = self.library.collections.iter().position(|c| c.name == name) {
            self.current = i;
        }
        self.sync();
    }
}

/// The current collection's games as a list's rows (its place is kept by position, per collection).
impl Rows for Shelf {
    fn len(&self) -> usize {
        self.collection().map_or(0, |c| c.games.len())
    }
}

#[cfg(test)]
mod tests {
    use crate::testkit::*;
    use crate::*;

    #[test]
    fn the_list_scrolls_to_keep_the_selection_on_screen() {
        let mut s = state();
        s.apply(ShelfEvent::Rows(3).into());
        s.apply(ShelfEvent::ListStep(4).into());
        assert_eq!(s.shelf().cursor(), Cursor { index: 4, top: 2.0 });
        assert_eq!(s.shelf().selected().map(|g| &**g), Some("dark-souls"));
        assert_eq!(s.pointer().focus(), Some(&game("dark-souls")));
        s.apply(ShelfEvent::ListStep(-100).into());
        assert_eq!(s.shelf().cursor(), Cursor { index: 0, top: 0.0 });
        s.apply(ShelfEvent::ListStep(100).into());
        assert_eq!(s.shelf().cursor().index, 6);
    }

    #[test]
    fn a_drag_scrolls_without_moving_the_selection_and_the_pad_brings_it_back() {
        let mut s = state();
        s.apply(ShelfEvent::Rows(3).into());
        let (dirty, _) = s.apply(ListEvent::Scroll(ListId::Shelf, 2.5).into());
        assert_eq!((dirty, s.shelf().cursor()), (Dirty::BOTH, Cursor { index: 0, top: 2.5 }));
        s.apply(ListEvent::Scroll(ListId::Shelf, 100.0).into());
        assert_eq!(s.shelf().cursor().top, 4.0, "the last game on the last row");
        assert!(!s.list_can_scroll(ListId::Shelf, 1.0) && s.list_can_scroll(ListId::Shelf, -1.0));
        s.apply(ShelfEvent::ListStep(1).into());
        assert_eq!(s.shelf().cursor(), Cursor { index: 1, top: 1.0 }, "the selection on screen again");
    }

    #[test]
    fn collections_keep_their_own_place() {
        let mut s = state();
        s.apply(ShelfEvent::ListStep(2).into());
        s.apply(ShelfEvent::CollectionStep(1).into());
        assert_eq!(s.shelf().current(), 1);
        assert_eq!(s.shelf().cursor().index, 0);
        s.apply(ShelfEvent::CollectionStep(2).into());
        assert_eq!(s.shelf().current(), 0, "wraps around (past Apps)");
        assert_eq!(s.shelf().cursor().index, 2);
    }

    #[test]
    fn a_favourite_shows_in_its_collection_and_is_saved() {
        let mut s = state();
        s.apply(ShelfEvent::ListStep(2).into());
        let name = s.shelf().collection().map(|c| c.name.clone());
        let (_, fx) = s.apply(ShelfEvent::ToggleFavourite.into());
        assert_eq!(fx, vec![Effect::Save(Save::Favourites(vec!["mk-ds".into()]))]);
        assert!(s.shelf().library().collections.iter().any(|c| c.name == "Favourites" && c.games == vec![2]));
        assert_eq!(s.shelf().collection().map(|c| c.name.clone()), name, "the collection stays");
        assert_eq!(s.shelf().selected().map(|g| &**g), Some("mk-ds"), "so does the selection");
    }

    #[test]
    fn selecting_then_activating_again_launches() {
        let mut s = state();
        assert!(tap(&mut s, game("ff6")).is_empty());
        let fx = tap(&mut s, game("ff6"));
        assert!(
            fx.iter().any(|e| matches!(e, Effect::Launch { game, settings, env, launch: Launch::None, file: None, .. } if &**game == "ff6" && *settings == Values::default() && env.is_empty())),
            "{fx:?}"
        );
    }
}
