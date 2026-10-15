//! Coming back to the same place after a restart: shelf's current collection and each collection's
//! selected game, and the second screen's tab, saved as text; and the screens' brightness. Both are
//! written once changes have stopped for a moment.

use crate::State;
use crate::tab::Tab;

/// Seconds after the last change before it is saved.
pub(crate) const SAVE_AFTER: u64 = 2;

/// A change waiting to be saved: since when (epoch seconds), if any.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(crate) struct Debounce {
    since: Option<u64>,
}

impl Debounce {
    /// Something changed at `now`: saved once nothing has for a moment.
    pub(crate) fn touch(&mut self, now: u64) {
        self.since = Some(now);
    }

    /// Whether it is time to save (`after` seconds since the last change); then it is no longer due.
    pub(crate) fn due(&mut self, now: u64, after: u64) -> bool {
        let due = self.since.is_some_and(|since| now >= since + after);
        if due {
            self.since = None;
        }
        due
    }
}

/// What waits to be saved.
#[derive(Debug, Clone, Copy, Default)]
pub(crate) struct Saves {
    /// Where shelf and the second screen are ([`State::resume_text`]).
    pub resume: Debounce,
    /// The screens' brightness.
    pub brightness: Debounce,
}

impl State {
    /// Where shelf and the second screen are, to come back to: shelf's current collection's name, the
    /// second screen's tab (`@mode`, the file's name for it), then each collection's name and selected
    /// game's id, tab-separated.
    pub fn resume_text(&self) -> String {
        let mut text = self.shelf.collection().map(|c| c.name.clone()).unwrap_or_default();
        text.push('\n');
        // The second screen's tab: the one chosen (not the keyboard a text field brought up).
        text.push_str("@mode\t");
        text.push_str(self.bottom.chosen_tab().name());
        text.push('\n');
        self.shelf.selections(&mut text);
        text
    }

    /// Goes back to the place `resume_text` saved (collections and games that are gone are skipped).
    pub(crate) fn restore(&mut self, text: &str) {
        let mut lines = text.lines();
        let current = lines.next().unwrap_or("");
        for line in lines {
            let Some((name, id)) = line.split_once('\t') else {
                continue;
            };
            if name == "@mode" {
                // Any tab but the touchpad (it grabs the touchscreen: only on request). What a tab
                // watches starts with Event::Start.
                if let Some(tab) = Tab::from_name(id).filter(|t| *t != Tab::Touchpad) {
                    self.bottom.tab = tab;
                }
                continue;
            }
            self.shelf.restore_selection(name, id);
        }
        self.shelf.restore_current(current);
    }
}

#[cfg(test)]
mod tests {
    use crate::testkit::*;
    use crate::*;

    #[test]
    fn the_place_is_saved_after_a_pause_and_restored() {
        let mut s = state();
        s.apply(Event::Tick { unix_secs: 100 });
        s.apply(ShelfEvent::ListStep(3).into());
        s.apply(ShelfEvent::CollectionStep(1).into());
        assert!(s.apply(Event::Tick { unix_secs: 101 }).1.is_empty(), "not while moving");
        let (_, fx) = s.apply(Event::Tick { unix_secs: 103 });
        let text = match fx.first() {
            Some(Effect::Save(Save::Resume(text))) => text.clone(),
            other => format!("{other:?}"),
        };
        assert_eq!(text, "More\n@mode\tsysinfo\nGames\tff6\nMore\tff6\n");
        let mut fresh = State::new(Init { resume: Some(text), ..init() });
        assert_eq!(fresh.shelf().collection().map(|c| c.name.as_str()), Some("More"));
        fresh.apply(ShelfEvent::CollectionStep(-1).into());
        assert_eq!(fresh.shelf().selected().map(|g| &**g), Some("ff6"));
    }

    #[test]
    fn a_keyboard_a_text_field_brought_up_isnt_saved_as_the_tab() {
        let mut s = state();
        s.apply(Event::Windows(vec![foot()]));
        tap(&mut s, Id::Tab(Tab::Dock));
        s.apply(Event::TextInput(true));
        assert_eq!(s.bottom().tab(), Tab::Keyboard);
        assert!(s.resume_text().contains("@mode\tdock"));
    }

    #[test]
    fn any_tab_but_the_touchpad_comes_back_with_its_watch() {
        let mut s = state();
        tap(&mut s, Id::Tab(Tab::Wifi));
        let mut again = State::new(Init { resume: Some(s.resume_text()), ..init() });
        assert_eq!(again.bottom().tab(), Tab::Wifi);
        assert!(again.apply(Event::Start).1.contains(&Effect::Watch(Watch::Wifi, true)));
        tap(&mut s, Id::Tab(Tab::Touchpad));
        let again = State::new(Init { resume: Some(s.resume_text()), ..init() });
        assert_eq!(again.bottom().tab(), Tab::default());
    }

    #[test]
    fn choosing_the_tab_in_front_saves_nothing() {
        let mut s = state();
        s.apply(Event::Tick { unix_secs: 100 });
        tap(&mut s, Id::Tab(Tab::Sysinfo));
        assert!(s.apply(Event::Tick { unix_secs: 110 }).1.is_empty());
    }
}
