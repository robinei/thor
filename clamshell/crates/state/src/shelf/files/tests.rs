use super::*;
use crate::testkit::Applied;
use crate::testkit::{state_with, tap};
use crate::{Browsed, Collection, Event, Id, Library, ShelfEvent, ShelfId, State};

fn entry(id: &str, folder: bool, kind: BrowseKind) -> Entry {
    let name = id.rsplit('/').next().unwrap();
    let path = format!("/{}", id.strip_prefix("browse:").unwrap());
    Entry { file: Some(path.into()), browsed: Some(Browsed { folder, kind, ..Browsed::default() }), ..Entry::new(id, name) }
}

/// A Files tab: Home and the Trash.
fn files_state() -> State {
    let mut trash = entry("browse:Trash", true, BrowseKind::Trash);
    trash.file = Some("/t/files".into());
    let entries = vec![entry("browse:Home", true, BrowseKind::Root), trash];
    state_with(Library { collections: vec![Collection::new("Files", vec![0, 1])], entries, ..Library::default() })
}

/// The effects in `fx` that set the folder to list and watch.
fn browses(fx: Vec<Effect>) -> Vec<Effect> {
    fx.into_iter().filter(|e| matches!(e, Effect::Browse(BrowseCommand::Open(_)))).collect()
}

#[test]
fn an_archive_is_a_folder_its_entries_only_copied_out() {
    let mut s = files_state();
    tap(&mut s, Id::Shelf(ShelfId::Launch));
    let mut zip = entry("browse:Home/game.tar.gz", true, BrowseKind::Plain);
    if let Some(b) = zip.browsed.as_mut() {
        (b.archive, b.locked) = (true, true);
    }
    zip.runner = Some(clamshell_library::RunnerOf::new("snes", "SNES", true));
    listed(&mut s, "browse:Home", vec![zip]);
    let acts = s.shelf().file_actions();
    for a in [FileAction::OpenIn, FileAction::Extract, FileAction::Rename, FileAction::Trash] {
        assert!(acts.contains(&crate::shelf::actions::Action::File(a)), "{a:?}: {acts:?}");
    }
    // Extract starts the name to type without the extensions.
    tap(&mut s, Id::Shelf(ShelfId::File(FileAction::Extract)));
    assert_eq!(s.bottom().editor().map(|e| e.text.as_str()), Some("game"));
    let fx = tap(&mut s, Id::Editor(crate::EditorId::Save));
    assert!(fx.contains(&Effect::Files(FileOp::Extract { archive: "/Home/game.tar.gz".into(), name: "game".into() })), "{fx:?}");
    // Opened, the archive is browsed as a folder with the archive itself as the dir, and it is in use
    // while open.
    let fx = tap(&mut s, Id::Shelf(ShelfId::Launch));
    assert!(fx.contains(&Effect::Browse(BrowseCommand::InUse(vec!["/Home/game.tar.gz".into()]))), "{fx:?}");
    listed(&mut s, "browse:Home/game.tar.gz", vec![entry("browse:Home/game.tar.gz/a.sfc", false, BrowseKind::Plain)]);
    // Inside it, entries can only be copied out; nothing is pasted in or made.
    let acts = s.shelf().file_actions();
    assert!(acts.contains(&crate::shelf::actions::Action::File(FileAction::Copy)));
    for a in [FileAction::Cut, FileAction::Rename, FileAction::Trash, FileAction::NewFolder] {
        assert!(!acts.contains(&crate::shelf::actions::Action::File(a)), "{a:?}");
    }
    // Once left, it is no longer in use.
    let fx = tap(&mut s, Id::Shelf(ShelfId::Up));
    assert!(fx.contains(&Effect::Browse(BrowseCommand::InUse(vec!["/Home".into()]))), "{fx:?}");
}

#[test]
fn an_archives_folder_is_named_without_its_extensions() {
    assert_eq!(extracted_name("Game.tar.gz"), "Game");
    assert_eq!(extracted_name("Game (USA).zip"), "Game (USA)");
    assert_eq!(extracted_name("noext"), "noext");
}

fn listed(s: &mut State, folder: &str, entries: Vec<Entry>) {
    s.applied(Event::Files(FilesEvent::Listed { folder: folder.into(), entries }));
}

#[test]
fn the_folder_open_is_listed_and_watched_while_open() {
    let mut s = files_state();
    let fx = browses(s.applied(Event::Pointer(crate::PointerEvent::Activate(Id::Shelf(ShelfId::Launch)))));
    assert_eq!(fx, vec![Effect::Browse(BrowseCommand::Open(Some(Browse { folder: "browse:Home".into(), dir: "/Home".into(), hidden: false })))]);
    assert!(s.shelf().list().is_empty(), "nothing in it until listed");
    listed(&mut s, "browse:Home", vec![entry("browse:Home/Films", true, BrowseKind::Plain), entry("browse:Home/a.txt", false, BrowseKind::Plain)]);
    assert_eq!(s.shelf().list().len(), 2);
    assert_eq!(s.shelf().selected().map(|g| &**g), Some("browse:Home/Films"));
    // Going into Films lists and watches it instead; going back out lists Home again.
    let fx = browses(tap(&mut s, Id::Shelf(ShelfId::Launch)));
    assert_eq!(fx, vec![Effect::Browse(BrowseCommand::Open(Some(Browse { folder: "browse:Home/Films".into(), dir: "/Home/Films".into(), hidden: false })))]);
    let fx = browses(tap(&mut s, Id::Shelf(ShelfId::Up)));
    assert!(matches!(fx.as_slice(), [Effect::Browse(BrowseCommand::Open(Some(b)))] if &*b.folder == "browse:Home"));
    assert_eq!(s.shelf().selected().map(|g| &**g), Some("browse:Home/Films"), "where it was");
    let fx = browses(tap(&mut s, Id::Shelf(ShelfId::Up)));
    assert_eq!(fx, vec![Effect::Browse(BrowseCommand::Open(None))], "out of the folders: no watch");
}

#[test]
fn listings_outlive_the_library_read_again() {
    let mut s = files_state();
    tap(&mut s, Id::Shelf(ShelfId::Launch));
    listed(&mut s, "browse:Home", vec![entry("browse:Home/Films", true, BrowseKind::Plain)]);
    tap(&mut s, Id::Shelf(ShelfId::Launch));
    listed(&mut s, "browse:Home/Films", vec![entry("browse:Home/Films/x.mkv", false, BrowseKind::Plain)]);
    let library = files_state().shelf().library().clone();
    let fx = s.applied(Event::Library(Box::new(library)));
    assert_eq!(s.shelf().selected().map(|g| &**g), Some("browse:Home/Films/x.mkv"), "still in Films");
    assert!(!fx.iter().any(|e| matches!(e, Effect::Browse(BrowseCommand::Open(_)))), "the same folder: watched still");
}

#[test]
fn copy_then_paste_into_the_folder_open() {
    let mut s = files_state();
    tap(&mut s, Id::Shelf(ShelfId::Launch));
    listed(&mut s, "browse:Home", vec![entry("browse:Home/Films", true, BrowseKind::Plain), entry("browse:Home/a.txt", false, BrowseKind::Plain)]);
    s.applied(ShelfEvent::ListStep(1).into());
    let file = |a: &[FileAction]| a.iter().map(|&a| crate::shelf::actions::Action::File(a)).collect::<Vec<_>>();
    assert_eq!(
        s.shelf().file_actions().as_slice(),
        file(&[FileAction::Copy, FileAction::Cut, FileAction::Rename, FileAction::Trash, FileAction::Select, FileAction::NewFolder, FileAction::ShowHidden])
    );
    // On a browsed entry, Y is Copy and X is Paste.
    s.applied(ShelfEvent::ToggleFavourite.into());
    assert_eq!(s.shelf().files().clip().map(|c| c.entries.iter().map(|(_, n)| n.as_str()).collect::<Vec<_>>()), Some(vec!["a.txt"]));
    assert!(s.shelf().file_actions().contains(&crate::shelf::actions::Action::File(FileAction::Paste)));
    s.applied(ShelfEvent::ListStep(-1).into());
    tap(&mut s, Id::Shelf(ShelfId::Launch));
    assert_eq!(s.shelf().file_target().map(|(g, selected)| (&*g.id, selected)), Some(("browse:Home/Films", false)), "empty: the folder's actions");
    let paste = vec![Effect::Files(FileOp::Paste { path: "/Home/a.txt".into(), into: "/Home/Films".into(), cut: false })];
    assert_eq!(s.applied(ShelfEvent::TogglePin.into()), paste);
    assert_eq!(tap(&mut s, Id::Shelf(ShelfId::File(FileAction::Paste))), paste);
    assert!(s.shelf().files().clip().is_some(), "a copy pastes again");
}

#[test]
fn selected_entries_are_what_the_actions_act_on() {
    let mut s = files_state();
    tap(&mut s, Id::Shelf(ShelfId::Launch));
    let three = ["a", "b", "c"].map(|n| entry(&format!("browse:Home/{n}"), false, BrowseKind::Plain)).to_vec();
    listed(&mut s, "browse:Home", three);
    // Start on a and on c selects them. Move to Trash then acts on both and uses up the selection.
    s.applied(ShelfEvent::Continue.into());
    s.applied(ShelfEvent::ListStep(2).into());
    s.applied(ShelfEvent::Continue.into());
    assert_eq!(s.shelf().files().selected().len(), 2);
    let fx = tap(&mut s, Id::Shelf(ShelfId::File(FileAction::Trash)));
    assert_eq!(fx, vec![Effect::Files(FileOp::Trash("/Home/a".into())), Effect::Files(FileOp::Trash("/Home/c".into()))]);
    assert!(s.shelf().files().selected().is_empty());
    // Hidden files lists the folder again with them.
    let fx = tap(&mut s, Id::Shelf(ShelfId::File(FileAction::ShowHidden)));
    assert!(matches!(fx.as_slice(), [Effect::Browse(BrowseCommand::Open(Some(Browse { hidden: true, .. })))]), "{fx:?}");
}

#[test]
fn a_listing_drops_the_selected_entries_that_are_gone() {
    let mut s = files_state();
    tap(&mut s, Id::Shelf(ShelfId::Launch));
    let ab = ["a", "b"].map(|n| entry(&format!("browse:Home/{n}"), false, BrowseKind::Plain)).to_vec();
    listed(&mut s, "browse:Home", ab.clone());
    s.applied(ShelfEvent::Continue.into());
    assert_eq!(s.shelf().files().selected().len(), 1);
    // a is deleted elsewhere: the selection is empty, and Copy copies the entry the details show.
    listed(&mut s, "browse:Home", ab[1..].to_vec());
    assert!(s.shelf().files().selected().is_empty());
    s.applied(ShelfEvent::ToggleFavourite.into());
    assert_eq!(s.shelf().files().clip().map(|c| c.entries.iter().map(|(_, n)| n.as_str()).collect::<Vec<_>>()), Some(vec!["b"]));
}

#[test]
fn a_cut_entry_stays_in_the_clip_until_it_is_moved() {
    let mut s = files_state();
    tap(&mut s, Id::Shelf(ShelfId::Launch));
    listed(&mut s, "browse:Home", vec![entry("browse:Home/Films", true, BrowseKind::Plain), entry("browse:Home/a.txt", false, BrowseKind::Plain)]);
    s.applied(ShelfEvent::ListStep(1).into());
    tap(&mut s, Id::Shelf(ShelfId::File(FileAction::Cut)));
    s.applied(ShelfEvent::ListStep(-1).into());
    tap(&mut s, Id::Shelf(ShelfId::Launch));
    let paste = vec![Effect::Files(FileOp::Paste { path: "/Home/a.txt".into(), into: "/Home/Films".into(), cut: true })];
    assert_eq!(tap(&mut s, Id::Shelf(ShelfId::File(FileAction::Paste))), paste);
    assert!(s.shelf().files().clip().is_some(), "not moved yet");
    // The move fails: the entry is still cut, to paste again.
    s.applied(Event::Files(FilesEvent::Done(Err("no room".into()))));
    assert!(s.shelf().files().clip().is_some(), "failed: still cut");
    assert_eq!(tap(&mut s, Id::Shelf(ShelfId::File(FileAction::Paste))), paste);
    s.applied(Event::Files(FilesEvent::Done(Ok("Moved a.txt".into()))));
    assert_eq!(s.shelf().files().clip(), None, "moved");
}

#[test]
fn busy_lasts_while_any_operation_runs() {
    let mut s = files_state();
    tap(&mut s, Id::Shelf(ShelfId::Launch));
    let two = ["a", "b"].map(|n| entry(&format!("browse:Home/{n}"), false, BrowseKind::Plain)).to_vec();
    listed(&mut s, "browse:Home", two);
    tap(&mut s, Id::Shelf(ShelfId::File(FileAction::Trash)));
    s.applied(ShelfEvent::ListStep(1).into());
    tap(&mut s, Id::Shelf(ShelfId::File(FileAction::Trash)));
    s.applied(Event::Files(FilesEvent::Busy { what: "Trashing a".into(), fraction: 0.5 }));
    s.applied(Event::Files(FilesEvent::Done(Ok("Moved a to the Trash".into()))));
    assert!(s.shelf().files().busy().is_some(), "b runs still");
    // Something that went wrong outside an operation doesn't count as one ending.
    s.applied(Event::Files(FilesEvent::Failed("can't mount".into())));
    assert!(s.shelf().files().busy().is_some());
    s.applied(Event::Files(FilesEvent::Done(Ok("Moved b to the Trash".into()))));
    assert_eq!(s.shelf().files().busy(), None);
}

#[test]
fn a_listing_that_changes_the_entries_shown_asks_for_their_metadata_first() {
    let mut s = files_state();
    s.applied(Event::Fetching(true));
    tap(&mut s, Id::Shelf(ShelfId::Launch));
    let fetch = |fx: Vec<Effect>| fx.into_iter().filter(|e| matches!(e, Effect::FetchFirst(_))).collect::<Vec<_>>();
    let fx = s.applied(Event::Files(FilesEvent::Listed { folder: "browse:Home".into(), entries: vec![entry("browse:Home/a", false, BrowseKind::Plain)] }));
    assert_eq!(fetch(fx), vec![Effect::FetchFirst(vec!["browse:Home/a".into(), "browse:Home".into()])]);
    // The same entries again: nothing new to ask for.
    let fx = s.applied(Event::Files(FilesEvent::Listed { folder: "browse:Home".into(), entries: vec![entry("browse:Home/a", false, BrowseKind::Plain)] }));
    assert_eq!(fetch(fx), vec![]);
}

#[test]
fn deleting_asks_first() {
    let mut s = files_state();
    s.applied(ShelfEvent::ListStep(1).into());
    tap(&mut s, Id::Shelf(ShelfId::Launch));
    listed(&mut s, "browse:Trash", vec![entry("browse:Trash/old", false, BrowseKind::Plain)]);
    let file = |a: &[FileAction]| a.iter().map(|&a| crate::shelf::actions::Action::File(a)).collect::<Vec<_>>();
    assert_eq!(
        s.shelf().file_actions().as_slice(),
        file(&[FileAction::Restore, FileAction::Delete, FileAction::Select, FileAction::ShowHidden]),
        "the Trash: nothing pasted or made in it"
    );
    assert!(tap(&mut s, Id::Shelf(ShelfId::File(FileAction::Delete))).is_empty());
    assert!(s.shelf().file_armed(FileAction::Delete));
    let fx = tap(&mut s, Id::Shelf(ShelfId::File(FileAction::Delete)));
    assert_eq!(fx, vec![Effect::Files(FileOp::Delete("/Trash/old".into()))]);
    assert!(!s.shelf().file_armed(FileAction::Delete));
}

/// An armed Delete waits only while nothing else happens to its entry: another action disarms it, and
/// so do the details moving to another entry, also when they come back.
#[test]
fn another_action_or_the_details_changing_disarms() {
    let mut s = files_state();
    s.applied(ShelfEvent::ListStep(1).into());
    tap(&mut s, Id::Shelf(ShelfId::Launch));
    listed(&mut s, "browse:Trash", vec![entry("browse:Trash/a", false, BrowseKind::Trashed), entry("browse:Trash/b", false, BrowseKind::Trashed)]);
    let delete = Id::Shelf(ShelfId::File(FileAction::Delete));
    tap(&mut s, delete.clone());
    assert!(s.shelf().file_armed(FileAction::Delete));
    tap(&mut s, Id::Shelf(ShelfId::File(FileAction::Select)));
    assert!(!s.shelf().file_armed(FileAction::Delete), "another action");
    tap(&mut s, Id::Shelf(ShelfId::File(FileAction::Select)));
    tap(&mut s, delete.clone());
    s.applied(ShelfEvent::ListStep(1).into());
    assert!(!s.shelf().file_armed(FileAction::Delete), "another entry");
    s.applied(ShelfEvent::ListStep(-1).into());
    assert!(!s.shelf().file_armed(FileAction::Delete), "and back");
    assert_eq!(tap(&mut s, delete), vec![], "asks again");
}

/// New folder types its name on the keyboard and makes it in the folder open; Cancel, or a name that
/// can't be one, makes nothing, and the keyboard goes back.
#[test]
fn a_new_folder_is_named_on_the_keyboard() {
    let mut s = files_state();
    tap(&mut s, Id::Shelf(ShelfId::Launch));
    listed(&mut s, "browse:Home", vec![entry("browse:Home/a.txt", false, BrowseKind::Plain)]);
    let before = s.bottom().tab();
    let new_folder = Id::Shelf(ShelfId::File(FileAction::NewFolder));
    tap(&mut s, new_folder.clone());
    assert_eq!(s.bottom().editor().map(|e| e.text.as_str()), Some(""));
    s.editor_mut().unwrap().text = "Saves".into();
    let fx = tap(&mut s, Id::Editor(crate::EditorId::Save));
    assert!(fx.contains(&Effect::Files(FileOp::NewFolder { dir: "/Home".into(), name: "Saves".into() })), "{fx:?}");
    assert_eq!(s.bottom().tab(), before);
    tap(&mut s, new_folder.clone());
    s.editor_mut().unwrap().text = "x".into();
    let fx = tap(&mut s, Id::Editor(crate::EditorId::Cancel));
    assert!(!fx.iter().any(|e| matches!(e, Effect::Files(_))), "{fx:?}");
    assert_eq!(s.bottom().tab(), before, "cancelled: back");
    tap(&mut s, new_folder);
    s.editor_mut().unwrap().text = "../up".into();
    let fx = tap(&mut s, Id::Editor(crate::EditorId::Save));
    assert!(!fx.iter().any(|e| matches!(e, Effect::Files(_))), "not a name: {fx:?}");
    assert_eq!(s.bottom().tab(), before);
}

/// Open in plays an archive with its runner; Eject unmounts removable storage; a change of the mounts
/// reads the library again.
#[test]
fn open_in_eject_and_mounts() {
    let mut s = files_state();
    tap(&mut s, Id::Shelf(ShelfId::Launch));
    let mut zip = entry("browse:Home/game.zip", false, BrowseKind::Plain);
    if let Some(b) = zip.browsed.as_mut() {
        b.archive = true;
    }
    zip.runner = Some(clamshell_library::RunnerOf::new("snes", "SNES", true));
    listed(&mut s, "browse:Home", vec![zip, entry("browse:Home/card", true, BrowseKind::Removable("/dev/sdb1".into()))]);
    let fx = tap(&mut s, Id::Shelf(ShelfId::File(FileAction::OpenIn)));
    assert!(fx.iter().any(|e| matches!(e, Effect::Launch { entry, .. } if &**entry == "browse:Home/game.zip")), "{fx:?}");
    s.applied(ShelfEvent::ListStep(1).into());
    assert_eq!(tap(&mut s, Id::Shelf(ShelfId::File(FileAction::Eject))), vec![Effect::Files(FileOp::Eject("/dev/sdb1".into()))]);
    assert_eq!(s.applied(Event::Files(FilesEvent::Mounts)), vec![Effect::Tool(clamshell_library::Tool::Rescan)]);
}

/// A listing without a folder that is open closes it, back to the folder listed, which is then the one
/// watched.
#[test]
fn a_listing_closes_the_folders_gone_from_it() {
    let mut s = files_state();
    tap(&mut s, Id::Shelf(ShelfId::Launch));
    listed(&mut s, "browse:Home", vec![entry("browse:Home/Films", true, BrowseKind::Plain), entry("browse:Home/a.txt", false, BrowseKind::Plain)]);
    tap(&mut s, Id::Shelf(ShelfId::Launch));
    assert_eq!(s.shelf().group().map(|g| &*g.id), Some("browse:Home/Films"));
    let fx = browses(
        s.applied(Event::Files(FilesEvent::Listed { folder: "browse:Home".into(), entries: vec![entry("browse:Home/a.txt", false, BrowseKind::Plain)] })),
    );
    assert_eq!(s.shelf().group().map(|g| &*g.id), Some("browse:Home"), "Films closed");
    assert!(matches!(fx.as_slice(), [Effect::Browse(BrowseCommand::Open(Some(b)))] if &*b.folder == "browse:Home"), "{fx:?}");
    assert_eq!(s.shelf().selected().map(|g| &**g), Some("browse:Home/a.txt"));
}

#[test]
fn renaming_types_on_the_keyboard_and_goes_back() {
    let mut s = files_state();
    tap(&mut s, Id::Shelf(ShelfId::Launch));
    listed(&mut s, "browse:Home", vec![entry("browse:Home/a.txt", false, BrowseKind::Plain)]);
    let before = s.bottom().tab();
    tap(&mut s, Id::Shelf(ShelfId::File(FileAction::Rename)));
    assert_eq!(s.bottom().tab(), crate::Tab::Keyboard);
    assert_eq!(s.bottom().editor().map(|e| e.text.as_str()), Some("a.txt"));
    let fx = tap(&mut s, Id::Editor(crate::EditorId::Save));
    assert_eq!(s.bottom().tab(), before);
    assert!(fx.contains(&Effect::Files(FileOp::Rename { path: "/Home/a.txt".into(), name: "a.txt".into() })), "{fx:?}");
}

#[test]
fn a_restarted_files_thread_fails_the_operation_it_had_and_browse_lists_again() {
    let mut s = files_state();
    tap(&mut s, Id::Shelf(ShelfId::Launch));
    let two = ["a", "b"].map(|n| entry(&format!("browse:Home/{n}"), false, BrowseKind::Plain)).to_vec();
    listed(&mut s, "browse:Home", two);
    tap(&mut s, Id::Shelf(ShelfId::File(FileAction::Trash)));
    s.applied(ShelfEvent::ListStep(1).into());
    tap(&mut s, Id::Shelf(ShelfId::File(FileAction::Trash)));
    s.applied(Event::Files(FilesEvent::Busy { what: "Trashing a".into(), fraction: 0.5 }));
    s.applied(Event::Restarted(crate::Thread::Files));
    assert!(matches!(crate::testkit::notice(&s), Some(NoticeKind::Files(Err(_)))), "{:?}", crate::testkit::notice(&s));
    assert!(s.shelf().files().busy().is_some(), "b waits in the thread's channel still");
    s.applied(Event::Files(FilesEvent::Done(Ok("Moved b to the Trash".into()))));
    assert_eq!(s.shelf().files().busy(), None);
    // Nothing runs: nothing more fails.
    s.applied(Event::Files(FilesEvent::Done(Ok("Moved b to the Trash".into()))));
    let fx = s.applied(Event::Restarted(crate::Thread::Files));
    assert!(matches!(crate::testkit::notice(&s), Some(NoticeKind::Files(Ok(_)))), "{fx:?}");
    let fx = s.applied(Event::Restarted(crate::Thread::Browse));
    assert!(fx.iter().any(|e| matches!(e, Effect::Browse(BrowseCommand::Open(Some(b))) if &*b.folder == "browse:Home")), "{fx:?}");
    assert!(fx.iter().any(|e| matches!(e, Effect::Browse(BrowseCommand::InUse(_)))), "{fx:?}");
}
