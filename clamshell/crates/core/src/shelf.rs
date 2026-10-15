//! shelf's navigation: the collection tabs, each with its own cursor over its games, kept in range and
//! on screen; and changes to the library (plays, favourites) that keep the tab and selection.

use crate::{Collection, Cursor, Game, Id, Library, State};

impl State {
    pub fn collection(&self) -> Option<&Collection> {
        self.library.collections.get(self.current)
    }

    pub fn cursor(&self) -> Cursor {
        self.cursors.get(self.current).copied().unwrap_or_default()
    }

    /// The game at `position` in the current tab.
    pub fn game_at(&self, position: usize) -> Option<&Game> {
        let index = *self.collection()?.games.get(position)?;
        self.library.games.get(index)
    }

    pub fn selected_game(&self) -> Option<&Game> {
        self.game_at(self.cursor().index)
    }

    /// Keeps the cursor in range and on screen, and `selected` in step with it.
    pub(crate) fn sync(&mut self) {
        let before = self.selected.clone();
        let len = self.collection().map_or(0, |c| c.games.len());
        let rows = self.rows.max(1);
        if let Some(c) = self.cursors.get_mut(self.current) {
            c.index = c.index.min(len.saturating_sub(1));
            if c.index < c.top {
                c.top = c.index;
            } else if c.index >= c.top + rows {
                c.top = c.index + 1 - rows;
            }
            c.top = c.top.min(len.saturating_sub(rows));
        }
        self.selected = self.selected_game().map(|g| g.id.clone());
        if self.selected != before {
            self.resume_dirty = Some(self.unix_secs);
        }
    }

    /// Moves the cursor by `step` games (clamped at the ends), selecting and focusing that game.
    pub(crate) fn step_list(&mut self, step: i32) {
        let len = self.collection().map_or(0, |c| c.games.len());
        if let Some(c) = self.cursors.get_mut(self.current) {
            let target = c.index as i64 + i64::from(step);
            c.index = target.clamp(0, len.saturating_sub(1) as i64) as usize;
        }
        self.sync();
        self.focus = self.selected.clone().map(Id::Game);
    }

    /// Changes the library (favourites, plays) keeping the current tab, each tab's place and the
    /// selected game, all by name, since derived tabs may come or go.
    pub(crate) fn relayout(&mut self, change: impl FnOnce(&mut Library)) {
        let names: Vec<String> = self.library.collections.iter().map(|c| c.name.clone()).collect();
        let tab_name = names.get(self.current).cloned();
        let selected = self.selected.clone();
        change(&mut self.library);
        let old = std::mem::take(&mut self.cursors);
        self.cursors = self
            .library
            .collections
            .iter()
            .map(|c| names.iter().position(|n| *n == c.name).and_then(|i| old.get(i).copied()).unwrap_or_default())
            .collect();
        self.current = tab_name.and_then(|n| self.library.collections.iter().position(|c| c.name == n)).unwrap_or(0);
        if let Some(id) = selected {
            let position = self.collection().and_then(|c| {
                c.games.iter().position(|&i| self.library.games.get(i).is_some_and(|g| g.id == id))
            });
            if let (Some(p), Some(c)) = (position, self.cursors.get_mut(self.current)) {
                c.index = p;
            }
        }
        self.sync();
    }

    pub(crate) fn set_collection(&mut self, tab: usize) {
        if tab < self.library.collections.len() {
            self.current = tab;
            self.resume_dirty = Some(self.unix_secs);
            self.sync();
            if matches!(self.focus, Some(Id::Game(_)) | Some(Id::Collection(_))) {
                self.focus = self.selected.clone().map(Id::Game);
            }
        }
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use crate::tests::state;
    use crate::*;

    #[test]
    fn the_list_scrolls_to_keep_the_selection_on_screen() {
        let mut s = state();
        s.apply(Event::Rows(3));
        s.apply(Event::ListStep(4));
        assert_eq!(s.cursor(), Cursor { index: 4, top: 2 });
        assert_eq!(s.selected.as_deref(), Some("dark-souls"));
        assert_eq!(s.focus, Some(Id::Game("dark-souls".into())));
        s.apply(Event::ListStep(-100));
        assert_eq!(s.cursor(), Cursor { index: 0, top: 0 });
        s.apply(Event::ListStep(100));
        assert_eq!(s.cursor().index, 6);
    }

    #[test]
    fn tabs_keep_their_own_place() {
        let mut s = state();
        s.apply(Event::ListStep(2));
        s.apply(Event::CollectionStep(1));
        assert_eq!(s.current, 1);
        assert_eq!(s.cursor().index, 0);
        s.apply(Event::CollectionStep(1));
        assert_eq!(s.current, 0, "wraps around");
        assert_eq!(s.cursor().index, 2);
    }

    #[test]
    fn a_favourite_shows_in_its_tab_and_is_saved() {
        let mut s = state();
        s.apply(Event::ListStep(2));
        let tab_name = s.collection().map(|c| c.name.clone());
        let (_, fx) = s.apply(Event::ToggleFavourite);
        assert_eq!(fx, vec![Effect::SaveFavourites(vec!["mk-ds".into()])]);
        assert!(s.library.collections.iter().any(|c| c.name == "Favourites" && c.games == vec![2]));
        assert_eq!(s.collection().map(|c| c.name.clone()), tab_name, "the tab stays");
        assert_eq!(s.selected.as_deref(), Some("mk-ds"), "so does the selection");
    }
}
