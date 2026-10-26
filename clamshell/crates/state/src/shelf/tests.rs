use crate::bottom::keyboard::Key;
use crate::testkit::*;
use crate::*;

#[test]
fn the_list_scrolls_to_keep_the_selection_on_screen() {
    let mut s = state();
    s.applied(ShelfEvent::Rows(3).into());
    s.applied(ShelfEvent::ListStep(4).into());
    assert_eq!(s.shelf().cursor(), Cursor { index: 4, top: 2.0 });
    assert_eq!(s.shelf().selected().map(|g| &**g), Some("dark-souls"));
    assert_eq!(s.pointer().focus(), Some(&entry("dark-souls")));
    s.applied(ShelfEvent::ListStep(-100).into());
    assert_eq!(s.shelf().cursor(), Cursor { index: 0, top: 0.0 });
    s.applied(ShelfEvent::ListStep(100).into());
    assert_eq!(s.shelf().cursor().index, 6);
}

#[test]
fn a_drag_scrolls_without_moving_the_selection_and_the_pad_brings_it_back() {
    let mut s = state();
    s.applied(ShelfEvent::Rows(3).into());
    s.applied(ListEvent::Scroll(ListId::Shelf, 2.5).into());
    assert_eq!(s.shelf().cursor(), Cursor { index: 0, top: 2.5 });
    s.applied(ListEvent::Scroll(ListId::Shelf, 100.0).into());
    assert_eq!(s.shelf().cursor().top, 4.0, "the last game on the last row");
    let can_scroll = |rows| s.lists().scroll(ListId::Shelf).can_scroll(rows, s.lists().len(ListId::Shelf));
    assert!(!can_scroll(1.0) && can_scroll(-1.0));
    s.applied(ShelfEvent::ListStep(1).into());
    assert_eq!(s.shelf().cursor(), Cursor { index: 1, top: 1.0 }, "the selection on screen again");
}

#[test]
fn collections_keep_their_own_place() {
    let mut s = state();
    s.applied(ShelfEvent::ListStep(2).into());
    s.applied(ShelfEvent::CollectionStep(1).into());
    assert_eq!(s.shelf().current(), 1);
    assert_eq!(s.shelf().cursor().index, 0);
    s.applied(ShelfEvent::CollectionStep(4).into());
    assert_eq!(s.shelf().current(), 0, "wraps around (past TV, Apps and Tools)");
    assert_eq!(s.shelf().cursor().index, 2);
}

#[test]
fn a_favourite_shows_in_its_collection_and_is_saved() {
    let mut s = state();
    s.applied(ShelfEvent::ListStep(2).into());
    let name = s.shelf().collection().map(|c| c.name.clone());
    let fx = s.applied(ShelfEvent::ToggleFavourite.into());
    assert_eq!(fx, vec![Effect::Save(Save::Favourites(vec!["mk-ds".into()]))]);
    assert!(s.shelf().library().collections.iter().any(|c| c.name == "Favourites" && c.entries == vec![2]));
    assert_eq!(s.shelf().collection().map(|c| c.name.clone()), name, "the collection stays");
    assert_eq!(s.shelf().selected().map(|g| &**g), Some("mk-ds"), "so does the selection");
}

#[test]
fn a_favourite_removed_moves_the_focus_with_the_selection() {
    let mut s = state();
    // Two favourites, and the pad's focus on the first in Favourites.
    for step in [1, 1] {
        s.applied(ShelfEvent::ListStep(step).into());
        s.applied(ShelfEvent::ToggleFavourite.into());
    }
    let favourites = s.shelf().library().collections.iter().position(|c| c.name == "Favourites").unwrap();
    s.applied(ShelfEvent::CollectionStep(favourites as i32 - s.shelf().current() as i32).into());
    s.applied(ShelfEvent::ListStep(-1).into());
    let first = s.shelf().selected().cloned().unwrap();
    assert_eq!(s.pointer().focus(), Some(&Id::Shelf(ShelfId::Entry(first.clone()))));
    // Out of the favourites, it leaves the list, and the focus goes with the selection to the other.
    s.applied(ShelfEvent::ToggleFavourite.into());
    let other = s.shelf().selected().cloned().unwrap();
    assert_ne!(other, first);
    assert_eq!(s.pointer().focus(), Some(&Id::Shelf(ShelfId::Entry(other))));
}

#[test]
fn a_group_that_leaves_its_collection_is_closed_there() {
    let mut s = state();
    let go = |s: &mut State, name: &str| {
        let c = s.shelf().library().collections.iter().position(|c| c.name == name).unwrap() as u16;
        tap(s, Id::Shelf(ShelfId::Collection(c)));
    };
    // Metroid and Cosmos are favourites, and Cosmos is open in Favourites.
    s.applied(ShelfEvent::ListStep(1).into());
    s.applied(ShelfEvent::ToggleFavourite.into());
    go(&mut s, "TV");
    s.applied(ShelfEvent::ToggleFavourite.into());
    go(&mut s, "Favourites");
    tap(&mut s, entry("tv:Cosmos"));
    tap(&mut s, entry("tv:Cosmos"));
    assert_eq!(s.shelf().group().map(|g| &*g.id), Some("tv:Cosmos"));
    // Cosmos is no longer a favourite: Favourites shows its own list again.
    go(&mut s, "TV");
    s.applied(ShelfEvent::ToggleFavourite.into());
    go(&mut s, "Favourites");
    assert_eq!(s.shelf().group(), None);
    assert_eq!(s.shelf().selected().map(|g| &**g), Some("metroid-zm"));
}

#[test]
fn the_resume_record_keeps_a_place_past_its_shrunk_list() {
    let mut s = state();
    let go = |s: &mut State, name: &str| {
        let c = s.shelf().library().collections.iter().position(|c| c.name == name).unwrap() as u16;
        tap(s, Id::Shelf(ShelfId::Collection(c)));
    };
    // Two favourites, the second selected in Favourites.
    for _ in 0..2 {
        s.applied(ShelfEvent::ListStep(1).into());
        s.applied(ShelfEvent::ToggleFavourite.into());
    }
    go(&mut s, "Favourites");
    s.applied(ShelfEvent::ListStep(1).into());
    assert_eq!(s.shelf().selected().map(|g| &**g), Some("metroid-zm"));
    // From Games, the first is no longer a favourite: Favourites' place is past its list.
    go(&mut s, "Games");
    assert_eq!(s.shelf().selected().map(|g| &**g), Some("mk-ds"));
    s.applied(ShelfEvent::ToggleFavourite.into());
    assert!(s.resume().places.contains(&("Favourites".to_owned(), vec!["metroid-zm".to_owned()])), "{:?}", s.resume().places);
}

/// The last favourite removed while Favourites is shown: the collection goes, shelf shows the first
/// collection, and the selection and the focus stay on the entry.
#[test]
fn the_favourites_tab_goes_with_its_last_entry() {
    let mut s = state();
    s.applied(ShelfEvent::ListStep(2).into());
    s.applied(ShelfEvent::ToggleFavourite.into());
    let favourites = s.shelf().library().collections.iter().position(|c| c.name == "Favourites").unwrap();
    tap(&mut s, Id::Shelf(ShelfId::Collection(favourites as u16)));
    assert_eq!(s.shelf().selected().map(|g| &**g), Some("mk-ds"));
    s.applied(ShelfEvent::ToggleFavourite.into());
    assert!(!s.shelf().library().collections.iter().any(|c| c.name == "Favourites"));
    assert_eq!((s.shelf().current(), s.shelf().collection().map(|c| c.name.as_str())), (0, Some("Games")));
    assert_eq!(s.shelf().selected().map(|g| &**g), Some("mk-ds"));
    assert_eq!(s.pointer().focus(), Some(&entry("mk-ds")));
}

/// A library read without the selected entry starts its list over from the top, and the focus goes
/// with the selection.
#[test]
fn a_library_read_without_the_selected_entry_selects_the_first() {
    let mut s = state();
    s.applied(ShelfEvent::ListStep(2).into());
    let mut library = clamshell_library::demo::library();
    library.entries.retain(|g| &*g.id != "mk-ds");
    library.collections = vec![crate::Collection::new("Games", (0..6).collect())];
    s.applied(Event::Library(Box::new(library)));
    assert_eq!(s.shelf().selected().map(|g| &**g), Some("zelda-lttp"));
    assert_eq!(s.shelf().cursor().index, 0);
    assert_eq!(s.pointer().focus(), Some(&entry("zelda-lttp")));
}

/// A play recorded adds the entry to Recent, which comes in, while shelf stays where it was: the same
/// collection and selection, and the focus on it. Shown in Recent, another entry's play reorders the
/// list under the selection, which stays on its entry.
#[test]
fn a_play_recorded_keeps_shelfs_place_and_focus() {
    let mut s = state();
    s.applied(Event::tick(1000));
    launch(&mut s, "zelda-lttp");
    s.applied(Event::tick(1100));
    let fx = end(&mut s, "zelda-lttp");
    assert!(fx.iter().any(|e| matches!(e, Effect::RecordPlay { entry, duration: 100, .. } if &**entry == "zelda-lttp")), "{fx:?}");
    let recent = s.shelf().library().collections.iter().position(|c| c.name == "Recent").unwrap();
    assert_eq!(s.shelf().collection().map(|c| c.name.as_str()), Some("Games"));
    assert_eq!(s.shelf().selected().map(|g| &**g), Some("zelda-lttp"));
    assert_eq!(s.pointer().focus(), Some(&entry("zelda-lttp")));
    s.applied(ShelfEvent::ListStep(1).into());
    launch(&mut s, "metroid-zm");
    s.applied(Event::tick(1200));
    end(&mut s, "metroid-zm");
    tap(&mut s, Id::Shelf(ShelfId::Collection(recent as u16)));
    s.applied(ShelfEvent::ListStep(1).into());
    assert_eq!(s.shelf().selected().map(|g| &**g), Some("zelda-lttp"), "metroid first, then zelda");
    // (Launched from elsewhere, since shelf shows Recent.)
    launch_now(&mut s, "ff6");
    start(&mut s, "ff6");
    s.applied(Event::tick(1300));
    end(&mut s, "ff6");
    assert_eq!(s.shelf().selected().map(|g| &**g), Some("zelda-lttp"), "now third");
    assert_eq!(s.shelf().cursor().index, 2);
    assert_eq!(s.pointer().focus(), Some(&entry("zelda-lttp")));
}

/// What the metadata thread fetched goes to its entry: the details and the cover. An id the library
/// doesn't have is ignored.
#[test]
fn fetched_metadata_goes_to_its_entry() {
    let mut s = state();
    let meta = crate::Meta {
        cover: Some("/covers/ff6.png".into()),
        details: clamshell_library::Details { developer: "Square".into(), ..Default::default() },
        duration: None,
    };
    s.applied(Event::Meta { id: "ff6".into(), meta: meta.clone() });
    let ff6 = s.shelf().entry("ff6").unwrap();
    assert_eq!((ff6.details.developer.as_str(), ff6.box_front.as_deref()), ("Square", Some(std::path::Path::new("/covers/ff6.png"))));
    let before = s.shelf().library().clone();
    s.applied(Event::Meta { id: "nothing-such".into(), meta });
    assert_eq!(s.shelf().library(), &before);
}

#[test]
fn a_library_read_keeps_what_is_typed() {
    use crate::bottom::keyboard::{Key, find};
    let mut s = state();
    tap(&mut s, Id::Tab(crate::Tab::Keyboard));
    tap(&mut s, Id::Key(find(Key::Char('m')).unwrap()));
    s.applied(ShelfEvent::ListStep(1).into());
    let (list, selected) = (s.shelf().list().to_vec(), s.shelf().selected().cloned());
    assert_eq!(selected.as_deref(), Some("mk-ds"));
    s.applied(Event::Library(Box::new(clamshell_library::demo::library())));
    assert_eq!(s.shelf().filter(), "m");
    assert_eq!(s.shelf().list(), list.as_slice());
    assert_eq!(s.shelf().selected().cloned(), selected);
}

#[test]
fn typing_with_shelf_in_front_finds_entries_by_their_titles_start() {
    use crate::bottom::keyboard::find;
    let mut s = state();
    tap(&mut s, Id::Tab(crate::Tab::Keyboard));
    let key = |s: &mut State, k| find(k).map(|pos| tap(s, Id::Key(pos))).unwrap_or_default();
    let titles = |s: &State| s.shelf().list().iter().filter_map(|&i| s.shelf().library().entries.get(i)).map(|g| g.title.clone()).collect::<Vec<_>>();
    assert!(key(&mut s, Key::Char('m')).is_empty(), "not sent to an app: shelf is in front");
    assert_eq!(titles(&s), ["Metroid: Zero Mission", "Mario Kart DS"]);
    key(&mut s, Key::Char('a'));
    assert_eq!(titles(&s), ["Mario Kart DS"], "any case: Ma");
    assert_eq!(s.shelf().selected().map(|g| &**g), Some("mk-ds"));
    // Going into a folder and back out, each list keeps what was typed in it.
    key(&mut s, Key::Named(crate::bottom::keyboard::Named::Backspace));
    assert_eq!(titles(&s).len(), 2);
    s.applied(ShelfEvent::ClearFilter.into());
    assert_eq!(s.shelf().filter(), "");
    assert_eq!(titles(&s).len(), 7);
    tap(&mut s, Id::Shelf(ShelfId::Collection(2)));
    key(&mut s, Key::Char('c'));
    assert_eq!(titles(&s), ["Cosmos"]);
    let fx = key(&mut s, Key::Named(crate::bottom::keyboard::Named::Enter));
    assert!(fx.is_empty() && s.shelf().group().is_some(), "Enter: opened");
    assert_eq!(s.shelf().filter(), "", "the folder's own list");
    tap(&mut s, Id::Shelf(ShelfId::Up));
    assert_eq!(s.shelf().filter(), "c", "the series' list as it was left");
}

#[test]
fn a_group_opens_and_back_goes_out_to_it() {
    let mut s = state();
    tap(&mut s, Id::Shelf(ShelfId::Collection(2)));
    assert_eq!(tap(&mut s, entry("tv:Cosmos")), vec![], "selected already: opened, nothing launched");
    assert_eq!(s.shelf().group().map(|g| g.title.as_str()), Some("Cosmos"));
    let titles = |s: &State| s.shelf().list().iter().map(|&i| s.shelf().library().entries[i].title.clone()).collect::<Vec<_>>();
    assert_eq!(titles(&s), vec!["Season 1", "Season 2"]);
    s.applied(ShelfEvent::ListStep(1).into());
    tap(&mut s, Id::Shelf(ShelfId::Launch));
    assert_eq!(titles(&s), vec!["1. Standing Up in the Milky Way"]);
    let fx = tap(&mut s, Id::Shelf(ShelfId::Launch));
    assert!(fx.iter().any(|e| matches!(e, Effect::Launch { entry, .. } if entry.ends_with("Milky Way.mkv"))), "an episode plays: {fx:?}");
    tap(&mut s, Id::Shelf(ShelfId::Up));
    assert_eq!(s.shelf().selected().map(|g| &**g), Some("tv:Cosmos/Season 2"), "back where it was");
    tap(&mut s, Id::Shelf(ShelfId::Up));
    assert_eq!(s.shelf().group(), None);
    assert_eq!(s.shelf().selected().map(|g| &**g), Some("tv:Cosmos"));
    assert!(!tap(&mut s, Id::Shelf(ShelfId::Up)).iter().any(|_| true) && s.shelf().group().is_none(), "nothing to go out of");
}

#[test]
fn a_group_started_from_elsewhere_opens_in_shelf() {
    let mut s = state();
    let fx = launch_now(&mut s, "tv:Cosmos/Season 1");
    assert!(fx.is_empty());
    assert!(s.session().shelf_in_front());
    assert_eq!(s.shelf().collection().map(|c| c.name.as_str()), Some("TV"));
    assert_eq!(s.shelf().group().map(|g| &*g.id), Some("tv:Cosmos/Season 1"));
    assert_eq!(s.shelf().selected().map(|g| &**g), Some("tv:Cosmos/Season 1/1. The Shores of the Cosmic Ocean.mkv"));
    assert!(s.session().launching().is_empty(), "nothing started");
}

#[test]
fn the_details_actions_toggle_the_favourite_and_the_pin() {
    let mut s = state();
    let fx = tap(&mut s, Id::Shelf(ShelfId::Favourite));
    assert_eq!(fx, vec![Effect::Save(Save::Favourites(vec!["zelda-lttp".into()]))]);
    let fx = tap(&mut s, Id::Shelf(ShelfId::Pin));
    assert!(matches!(fx.as_slice(), [Effect::Save(Save::Dock(ids))] if ids.len() == 1), "{fx:?}");
    assert!(s.dock().is_pinned("zelda-lttp"));
}

#[test]
fn continue_plays_on_where_the_series_was_left() {
    let mut s = state();
    tap(&mut s, Id::Shelf(ShelfId::Collection(2)));
    let first = "tv:Cosmos/Season 1/1. The Shores of the Cosmic Ocean.mkv";
    let launched = |fx: &[Effect]| {
        fx.iter().find_map(|e| match e {
            Effect::Launch { entry, .. } => Some(entry.to_string()),
            _ => None,
        })
    };
    // The demo has the second episode under way. This test marks it as watched to its end instead.
    s.applied(LaunchEvent::Position { entry: "tv:Cosmos/Season 1/2. One Voice in the Cosmic Fugue.mkv".into(), position: None }.into());
    let fx = tap(&mut s, Id::Shelf(ShelfId::Continue));
    assert_eq!(launched(&fx).as_deref(), Some(first), "none watched: the first");
    crate::testkit::start(&mut s, first);
    s.applied(LaunchEvent::Position { entry: first.into(), position: Some(600) }.into());
    end(&mut s, first);
    let fx = s.applied(ShelfEvent::Continue.into());
    assert_eq!(launched(&fx).as_deref(), Some(first), "left early: it again");
    crate::testkit::start(&mut s, first);
    s.applied(LaunchEvent::Position { entry: first.into(), position: None }.into());
    end(&mut s, first);
    let fx = s.applied(ShelfEvent::Continue.into());
    assert_eq!(launched(&fx).as_deref(), Some("tv:Cosmos/Season 1/2. One Voice in the Cosmic Fugue.mkv"), "watched to its end: the next");
}

#[test]
fn a_tap_on_an_entry_not_in_the_list_does_nothing() {
    let mut s = state();
    let (focus, selected) = (s.pointer().focus().cloned(), s.shelf().selected().cloned());
    // Cosmos is in TV, not in the Games list shown, as a tap on a row that just left the list.
    assert_eq!(tap(&mut s, entry("tv:Cosmos")), vec![]);
    assert_eq!(long(&mut s, entry("tv:Cosmos")), vec![]);
    assert_eq!(s.pointer().focus().cloned(), focus, "the focus stays");
    assert_eq!(s.shelf().selected().cloned(), selected);
}

#[test]
fn selecting_then_activating_again_launches() {
    let mut s = state();
    assert!(tap(&mut s, entry("ff6")).is_empty());
    let fx = tap(&mut s, entry("ff6"));
    assert!(
        fx.iter().any(
            |e| matches!(e, Effect::Launch { entry, settings, env, launch: Launch::None, .. } if &**entry == "ff6" && *settings == Values::default() && env.is_empty())
        ),
        "{fx:?}"
    );
}
