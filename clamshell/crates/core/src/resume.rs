//! Coming back to the same place after a restart: shelf's current collection and each collection's open
//! groups and selected entry, and the second screen's tab; and the screens' brightness. Both are
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
    /// Where shelf and the second screen are ([`State::resume`]).
    pub resume: Debounce,
    /// The screens' brightness.
    pub brightness: Debounce,
}

/// Where shelf and the second screen are, to come back to after a restart.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Resume {
    /// Shelf's current collection, by name.
    pub collection: String,
    /// The second screen's tab, by its name: the one chosen (not the keyboard a text field brought up).
    pub tab: String,
    /// Each collection's place: its name, then its open groups' ids and its selected entry's id.
    pub places: Vec<(String, Vec<String>)>,
}

impl State {
    /// Where shelf and the second screen are, to come back to.
    pub fn resume(&self) -> Resume {
        Resume {
            collection: self.shelf.collection().map(|c| c.name.clone()).unwrap_or_default(),
            tab: self.bottom.chosen_tab().name().to_owned(),
            places: self.shelf.places(),
        }
    }

    /// Goes back to the place `resume` saved (collections and entries that are gone are skipped).
    pub(crate) fn restore(&mut self, resume: &Resume) {
        // Any tab but the touchpad (any touch there points: only on request). What a tab watches starts
        // with Event::Start.
        if let Some(tab) = Tab::from_name(&resume.tab).filter(|t| *t != Tab::Touchpad) {
            self.bottom.tab = tab;
        }
        for (name, ids) in &resume.places {
            let ids: Vec<&str> = ids.iter().map(String::as_str).collect();
            self.shelf.restore_selection(name, &ids);
        }
        self.shelf.restore_current(&resume.collection);
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
        let Some(Effect::Save(Save::Resume(resume))) = fx.first() else {
            unreachable!("{fx:?}");
        };
        let place = |c: &str, id: &str| (c.to_owned(), vec![id.to_owned()]);
        assert_eq!((resume.collection.as_str(), resume.tab.as_str()), ("More", "sysinfo"));
        assert_eq!(resume.places, vec![place("Games", "ff6"), place("More", "ff6"), place("TV", "tv:Cosmos"), place("Apps", "desktop:steam")]);
        let mut fresh = State::new(Init { resume: Some(resume.clone()), ..init() });
        assert_eq!(fresh.shelf().collection().map(|c| c.name.as_str()), Some("More"));
        fresh.apply(ShelfEvent::CollectionStep(-1).into());
        assert_eq!(fresh.shelf().selected().map(|g| &**g), Some("ff6"));
    }

    #[test]
    fn open_groups_are_restored() {
        let mut s = state();
        tap(&mut s, Id::Shelf(ShelfId::Collection(2)));
        tap(&mut s, Id::Shelf(ShelfId::Launch));
        tap(&mut s, Id::Shelf(ShelfId::Launch));
        s.apply(ShelfEvent::ListStep(2).into());
        let resume = s.resume();
        let path = ["tv:Cosmos", "tv:Cosmos/Season 1", "tv:Cosmos/Season 1/10. The Edge of Forever.mkv"].map(str::to_owned).to_vec();
        assert!(resume.places.contains(&("TV".to_owned(), path)), "{resume:?}");
        let fresh = State::new(Init { resume: Some(resume), ..init() });
        assert_eq!(fresh.shelf().group().map(|g| g.title.as_str()), Some("Season 1"));
        assert_eq!(fresh.shelf().selected().map(|g| &**g), Some("tv:Cosmos/Season 1/10. The Edge of Forever.mkv"));
    }

    #[test]
    fn a_keyboard_a_text_field_brought_up_isnt_saved_as_the_tab() {
        let mut s = state();
        s.apply(Event::Windows(vec![foot()]));
        tap(&mut s, Id::Tab(Tab::Dock));
        s.apply(Event::TextInput(true));
        assert_eq!(s.bottom().tab(), Tab::Keyboard);
        assert_eq!(s.resume().tab, "dock");
    }

    #[test]
    fn any_tab_but_the_touchpad_comes_back_with_its_watch() {
        let mut s = state();
        tap(&mut s, Id::Tab(Tab::Wifi));
        let mut again = State::new(Init { resume: Some(s.resume()), ..init() });
        assert_eq!(again.bottom().tab(), Tab::Wifi);
        assert!(again.apply(Event::Start).1.contains(&Effect::Watch(Watch::Wifi, true)));
        tap(&mut s, Id::Tab(Tab::Touchpad));
        let again = State::new(Init { resume: Some(s.resume()), ..init() });
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
