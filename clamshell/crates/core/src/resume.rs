//! Coming back to the same place after a restart: the current tab, each tab's selected game, and the
//! second screen's mode, saved as text.

use crate::{Id, Mode, State};

/// Seconds after the last move before the place is saved.
pub(crate) const RESUME_AFTER: u64 = 2;

impl State {
    /// Where shelf is, to come back to: the current tab's name, then each tab's name and selected
    /// game's id, tab-separated.
    pub fn resume_text(&self) -> String {
        let mut text = self.collection().map(|c| c.name.clone()).unwrap_or_default();
        text.push('\n');
        // The second screen's mode.
        text.push_str("@mode\t");
        text.push_str(self.mode.name());
        text.push('\n');
        for (i, c) in self.library.collections.iter().enumerate() {
            let cursor = self.cursors.get(i).copied().unwrap_or_default();
            if let Some(g) = c.games.get(cursor.index).and_then(|&g| self.library.games.get(g)) {
                text.push_str(&c.name);
                text.push('\t');
                text.push_str(&g.id);
                text.push('\n');
            }
        }
        text
    }

    /// Goes back to the place `resume_text` saved (tabs and games that are gone are skipped).
    pub fn restore(&mut self, text: &str) {
        let mut lines = text.lines();
        let current = lines.next().unwrap_or("");
        for line in lines {
            let Some((name, id)) = line.split_once('\t') else {
                continue;
            };
            if name == "@mode" {
                // Not into touchpad (grabs the touchscreen; only on request) or Wi-Fi (watches only
                // when opened).
                if let Some(mode) = Mode::from_name(id).filter(|m| !matches!(m, Mode::Touchpad | Mode::Wifi)) {
                    self.mode = mode;
                    self.bar_mode = mode;
                }
                continue;
            }
            let Some(tab) = self.library.collections.iter().position(|c| c.name == name) else {
                continue;
            };
            let position = self.library.collections.get(tab).and_then(|c| {
                c.games.iter().position(|&g| self.library.games.get(g).is_some_and(|g| &*g.id == id))
            });
            if let (Some(p), Some(cursor)) = (position, self.cursors.get_mut(tab)) {
                cursor.index = p;
            }
        }
        if let Some(tab) = self.library.collections.iter().position(|c| c.name == current) {
            self.tab = tab;
        }
        self.sync();
        self.focus = self.selected.clone().map(Id::Game);
        self.resume_dirty = None;
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use crate::tests::state;
    use crate::*;

    #[test]
    fn the_place_is_saved_after_a_pause_and_restored() {
        let mut s = state();
        s.apply(Event::Tick { unix_secs: 100 });
        s.apply(Event::ListStep(3));
        s.apply(Event::TabStep(1));
        assert!(s.apply(Event::Tick { unix_secs: 101 }).1.is_empty(), "not while moving");
        let (_, fx) = s.apply(Event::Tick { unix_secs: 103 });
        let text = match fx.first() {
            Some(Effect::SaveResume(text)) => text.clone(),
            other => format!("{other:?}"),
        };
        assert_eq!(text, "More\n@mode\tsysinfo\nGames\tff6\nMore\tff6\n");
        let mut fresh = state();
        fresh.restore(&text);
        assert_eq!(fresh.collection().map(|c| c.name.as_str()), Some("More"));
        fresh.apply(Event::TabStep(-1));
        assert_eq!(fresh.selected.as_deref(), Some("ff6"));
    }
}
