//! Coming back to the same place after a restart. This covers shelf's current collection, each
//! collection's open groups and selected entry, and the second screen's tab, and also the screens'
//! brightness. Both are written once changes have stopped for a moment.

use crate::Shelf;
use crate::bottom::{Bottom, Tab};

/// Seconds after the last change before it is saved.
pub(crate) const SAVE_AFTER: u64 = 2;

/// Where shelf and the second screen are, to come back to after a restart.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Resume {
    /// Shelf's current collection, by name.
    pub collection: String,
    /// The second screen's tab, by name. This is the tab that was chosen, not the keyboard a text field
    /// brought up.
    pub tab: String,
    /// Each collection's place: its name, then the ids of its open groups and of its selected entry.
    pub places: Vec<(String, Vec<String>)>,
}

impl Resume {
    /// Where `shelf` and the second screen (`bottom`) are.
    pub(crate) fn of(shelf: &Shelf, bottom: &Bottom) -> Resume {
        Resume::at(shelf, bottom.chosen_tab())
    }

    /// Where `shelf` is, with the second screen's chosen tab `tab`.
    pub(crate) fn at(shelf: &Shelf, tab: Tab) -> Resume {
        let mut places = Vec::new();
        shelf.places(&mut places);
        Resume { collection: shelf.collection().map(|c| c.name.clone()).unwrap_or_default(), tab: tab.name().to_owned(), places }
    }

    /// Takes `shelf` and the second screen (`bottom`) back to this place. Collections and entries that are
    /// gone are skipped.
    pub(crate) fn restore(&self, shelf: &mut Shelf, bottom: &mut Bottom) {
        // Any tab but the touchpad. There every touch moves the pointer, so it opens only on request.
        // What a tab watches starts with Event::Start.
        if let Some(tab) = Tab::from_name(&self.tab).filter(|t| *t != Tab::Touchpad) {
            bottom.restore_tab(tab);
        }
        for (name, ids) in &self.places {
            let ids: Vec<&str> = ids.iter().map(String::as_str).collect();
            shelf.restore_selection(name, &ids);
        }
        shelf.restore_current(&self.collection);
    }
}

#[cfg(test)]
mod tests {
    use crate::testkit::*;
    use crate::*;

    #[test]
    fn the_place_is_saved_after_a_pause_and_restored() {
        let mut s = state();
        s.applied(Event::tick(100));
        s.applied(ShelfEvent::ListStep(3).into());
        s.applied(ShelfEvent::CollectionStep(1).into());
        assert!(s.applied(Event::tick(101)).1.is_empty(), "not while moving");
        let (_, fx) = s.applied(Event::tick(103));
        let Some(Effect::Save(Save::Resume(resume))) = fx.first() else {
            unreachable!("{fx:?}");
        };
        let place = |c: &str, id: &str| (c.to_owned(), vec![id.to_owned()]);
        assert_eq!((resume.collection.as_str(), resume.tab.as_str()), ("More", "sysinfo"));
        assert_eq!(
            resume.places,
            vec![place("Games", "ff6"), place("More", "ff6"), place("TV", "tv:Cosmos"), place("Apps", "desktop:steam"), place("Tools", "tool:device")]
        );
        let mut fresh = State::new(Init { resume: Some(resume.clone()), ..init() });
        assert_eq!(fresh.shelf().collection().map(|c| c.name.as_str()), Some("More"));
        fresh.applied(ShelfEvent::CollectionStep(-1).into());
        assert_eq!(fresh.shelf().selected().map(|g| &**g), Some("ff6"));
    }

    #[test]
    fn open_groups_are_restored() {
        let mut s = state();
        tap(&mut s, Id::Shelf(ShelfId::Collection(2)));
        tap(&mut s, Id::Shelf(ShelfId::Launch));
        tap(&mut s, Id::Shelf(ShelfId::Launch));
        s.applied(ShelfEvent::ListStep(2).into());
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
        s.applied(Event::Windows(vec![foot()]));
        tap(&mut s, Id::Tab(Tab::Dock));
        s.applied(Event::TextInput(true));
        assert_eq!(s.bottom().tab(), Tab::Keyboard);
        assert_eq!(s.resume().tab, "dock");
    }

    #[test]
    fn any_tab_but_the_touchpad_comes_back_with_its_watch() {
        let mut s = state();
        tap(&mut s, Id::Tab(Tab::Wifi));
        let mut again = State::new(Init { resume: Some(s.resume()), ..init() });
        assert_eq!(again.bottom().tab(), Tab::Wifi);
        assert!(again.applied(Event::Start).1.contains(&Effect::Watch(Watch::Wifi, true)));
        tap(&mut s, Id::Tab(Tab::Touchpad));
        let again = State::new(Init { resume: Some(s.resume()), ..init() });
        assert_eq!(again.bottom().tab(), Tab::default());
    }

    #[test]
    fn choosing_the_tab_in_front_saves_nothing() {
        let mut s = state();
        s.applied(Event::tick(100));
        tap(&mut s, Id::Tab(Tab::Sysinfo));
        assert!(s.applied(Event::tick(110)).1.is_empty());
    }
}
