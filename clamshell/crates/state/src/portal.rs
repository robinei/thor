//! Apps' file choosers: ClamShell serves xdg-desktop-portal's file chooser (clamshell-system's
//! `session::portal`), and each request is a pick in the Library (see [`crate::library_view::pick`]): a
//! file or folder to open, or where to save, in the open folder under a name or over a file. It starts in
//! the folder the app suggests, if the Files tab has it.
//!
//! A request is its program's, as a dialog is: its pick shows while that program is in front, the
//! Library over it, and the program comes back once it is answered. Asked from a program behind, it waits
//! until that program is in front. Leaving the pick for something else, by Home or by showing another
//! program, sets it aside until the program is in front again; only Cancel and B answer it with nothing.
//! The program is found by the asking process: the one whose window's process it is, or started. A
//! request from no program's process is the one in front's.

use std::path::PathBuf;

use crate::library_view::pick::Takes;
use crate::session::windows::Group;

/// From the portal thread.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PortalEvent {
    /// An app asks for a file or folder to open, or where to save.
    Asked(FileRequest),
    /// The request was given up: the app cancelled it or went away. It isn't answered.
    Closed(u32),
}

/// An app's file chooser request.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileRequest {
    /// The portal thread's number for it, by which it is answered.
    pub id: u32,
    /// What the app says it is for; empty if it says nothing.
    pub title: String,
    pub asks: Asks,
    /// The names a file may have (`*.png`), from the app's filter; none: any.
    pub patterns: Vec<String>,
    /// The asking process, then its parent, and so on; empty if it isn't known.
    pub asker: Vec<u32>,
    /// The folder the app suggests starting in.
    pub folder: Option<PathBuf>,
}

/// What a request asks for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Asks {
    /// A file to open, or a folder.
    Open { folder: bool },
    /// Where to save a file, suggested as `name`.
    Save { name: String },
    /// A folder to save files of these names in.
    SaveFiles { names: Vec<String> },
}

impl FileRequest {
    pub(crate) fn takes(&self) -> Takes {
        match &self.asks {
            Asks::Open { folder: true } => Takes::Folder,
            Asks::Open { folder: false } => Takes::Files(self.patterns.clone()),
            Asks::Save { name } => Takes::Save { name: name.clone(), patterns: self.patterns.clone() },
            Asks::SaveFiles { names } => Takes::SaveFiles(names.clone()),
        }
    }
}

/// The requests not answered yet, and what to go back to.
#[derive(Debug, Default)]
pub(crate) struct Portal {
    /// In the order they came. One may be being picked (see [`crate::library_view::pick::PickFor::App`]).
    pub(crate) asked: Vec<Waiting>,
    /// The program whose request is being picked, shown again when it is answered.
    pub(crate) back: Option<Group>,
}

/// A request, and the program that asked: none if no program was in front and the process isn't one's.
#[derive(Debug)]
pub(crate) struct Waiting {
    pub(crate) request: FileRequest,
    pub(crate) program: Option<Group>,
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    use crate::library_view::pick::SaveHere;
    use crate::testkit::*;
    use crate::{Effect, Event, Id, LibraryViewId, WindowEvent};

    fn ask(s: &mut crate::State, id: u32, folder: bool) -> Vec<Effect> {
        let asks = Asks::Open { folder };
        s.applied(Event::Portal(PortalEvent::Asked(FileRequest { id, asks, patterns: vec!["*.txt".into()], ..request() })))
    }

    /// A request from a process that is no program's.
    fn request() -> FileRequest {
        FileRequest { id: 0, title: String::new(), asks: Asks::Open { folder: false }, patterns: Vec::new(), asker: Vec::new(), folder: None }
    }

    fn chosen(fx: &[Effect]) -> Option<(u32, Vec<std::path::PathBuf>)> {
        fx.iter().find_map(|e| if let Effect::FileChosen { request, paths } = e { Some((*request, paths.clone())) } else { None })
    }

    /// An app in front asking for a file: the Library shows its Files tab; the file chosen is the answer,
    /// and the app comes back in front.
    #[test]
    fn an_apps_file_is_chosen_in_the_library_and_the_app_comes_back() {
        let mut s = state();
        s.observed(vec![win(7, "app")]);
        s.applied(WindowEvent::Activate(7).into());
        ask(&mut s, 1, false);
        assert!(s.session().top_window().is_none(), "the Library in front");
        assert_eq!(s.library_view().collection().map(|c| c.name.as_str()), Some("Files"));
        assert_eq!(s.library_view().pick().map(|p| p.asks()), Some("Choose a file"));
        tap(&mut s, Id::LibraryView(LibraryViewId::Launch));
        s.library_view.select("browse:Files/Home/snes.png");
        assert!(chosen(&tap(&mut s, Id::LibraryView(LibraryViewId::Choose))).is_none(), "not a *.txt");
        s.library_view.select("browse:Files/Home/notes.txt");
        let fx = tap(&mut s, Id::LibraryView(LibraryViewId::Choose));
        assert_eq!(chosen(&fx), Some((1, vec!["/home/thor/notes.txt".into()])));
        assert_eq!(s.session().top_window().map(|w| w.id), Some(7));
        assert!(s.library_view().pick().is_none());
    }

    /// A pick of ClamShell's own left behind a window is kept, as the rest of the Library is. An app's
    /// request is picked over it; when that ends, the own pick goes on with the Files tab as it left it.
    #[test]
    fn an_own_pick_left_behind_a_window_waits_while_an_apps_goes_on() {
        use crate::library_view::pick::{PickFor, Takes};
        use crate::{LibraryMode, Top};
        let mut s = state();
        assert!(s.library_view.start_pick(Takes::Folder, PickFor::PlacePicture("p".into())));
        tap(&mut s, Id::LibraryView(LibraryViewId::Launch));
        let open = |s: &crate::State| s.library_view().group().map(|g| g.id.clone());
        let home = open(&s);
        assert!(home.is_some());
        s.observed(vec![win(7, "app")]);
        s.applied(WindowEvent::Activate(7).into());
        assert_eq!(s.top(), Top::Window { program: false });
        assert!(s.library_view().pick().is_some(), "kept behind the window");
        ask(&mut s, 1, false);
        assert_eq!(s.top(), Top::Library(LibraryMode::Pick));
        assert_eq!(s.library_view().pick().map(|p| p.asks()), Some("Choose a file"));
        tap(&mut s, Id::LibraryView(LibraryViewId::Up));
        let fx = tap(&mut s, Id::LibraryView(LibraryViewId::CancelPick));
        assert_eq!(chosen(&fx), Some((1, vec![])));
        assert_eq!(s.session().top_window().map(|w| w.id), Some(7), "the app back in front");
        assert_eq!(s.library_view().pick().map(|p| p.asks()), Some("Choose the folder's picture"), "the own pick again");
        s.applied(Event::Home);
        assert_eq!(s.top(), Top::Library(LibraryMode::Pick), "Home leaves it as it was");
        assert_eq!(open(&s), home, "the Files tab as the own pick left it");
    }

    /// A pick leaves the Files tab as it was before it.
    #[test]
    fn a_pick_leaves_the_files_tab_as_it_was() {
        let mut s = state();
        let files = s.library_view().library().collections.iter().position(|c| c.name == "Files").unwrap();
        tap(&mut s, Id::LibraryView(LibraryViewId::Collection(files as u16)));
        assert!(s.library_view().group().is_none());
        s.observed(vec![win(7, "app")]);
        s.applied(WindowEvent::Activate(7).into());
        ask(&mut s, 1, false);
        tap(&mut s, Id::LibraryView(LibraryViewId::Launch));
        assert!(s.library_view().group().is_some(), "Home opened in the pick");
        tap(&mut s, Id::LibraryView(LibraryViewId::CancelPick));
        s.applied(Event::Home);
        assert_eq!(s.library_view().collection().map(|c| c.name.as_str()), Some("Files"));
        assert!(s.library_view().group().is_none(), "as it was");
    }

    /// Requests wait their turn. Cancelled, one is answered with nothing; one the app gave up isn't
    /// answered; one left with Home is set aside, the Library staying, until its program is in front.
    #[test]
    fn requests_wait_and_are_cancelled_or_given_up() {
        let mut s = state();
        s.observed(vec![win(7, "app")]);
        s.applied(WindowEvent::Activate(7).into());
        ask(&mut s, 1, true);
        ask(&mut s, 2, true);
        ask(&mut s, 3, false);
        assert_eq!(s.library_view().pick().map(|p| p.asks()), Some("Choose a folder"));
        // The first cancelled: the second is picked at once, the Library still in front.
        let fx = tap(&mut s, Id::LibraryView(LibraryViewId::CancelPick));
        assert_eq!(chosen(&fx), Some((1, vec![])));
        assert!(s.session().top_window().is_none() && s.library_view().pick().is_some());
        // The second, given up by the app: the third.
        let fx = s.applied(Event::Portal(PortalEvent::Closed(2)));
        assert_eq!(chosen(&fx), None);
        assert_eq!(s.library_view().pick().map(|p| p.asks()), Some("Choose a file"));
        // The third, left with Home: set aside, and the Library stays; back with its program.
        let fx = s.applied(Event::Home);
        assert_eq!(chosen(&fx), None);
        assert!(s.session().top_window().is_none() && s.library_view().pick().is_none());
        s.applied(WindowEvent::Activate(7).into());
        assert_eq!(s.library_view().pick().map(|p| p.asks()), Some("Choose a file"), "its program in front");
        assert!(s.session().top_window().is_none(), "the Library over it");
    }

    /// A request is its program's, found by the asking process, here one the window's process started:
    /// asked from a program behind, it waits until that program is in front, and the program comes back
    /// once it is answered. Showing another program sets it aside.
    #[test]
    fn a_request_waits_for_its_program() {
        let mut s = state();
        let with_pid = |id: i64, app: &str, pid: u32| crate::session::windows::WindowFacts { pids: vec![pid, 1], ..win(id, app) };
        s.observed(vec![with_pid(7, "editor", 70), with_pid(8, "game", 80)]);
        s.applied(WindowEvent::Activate(8).into());
        s.applied(Event::Portal(PortalEvent::Asked(FileRequest { id: 1, asker: vec![71, 70, 1], ..request() })));
        assert!(s.library_view().pick().is_none() && s.session().top_window().map(|w| w.id) == Some(8), "the game stays in front");
        s.applied(WindowEvent::Activate(7).into());
        assert!(s.library_view().pick().is_some() && s.session().top_window().is_none(), "picked over the editor");
        // Another program shown: set aside, unanswered; back with the editor.
        let fx = s.applied(WindowEvent::Activate(8).into());
        assert_eq!(chosen(&fx), None);
        assert!(s.library_view().pick().is_none() && s.session().top_window().map(|w| w.id) == Some(8));
        s.applied(WindowEvent::Activate(7).into());
        let fx = tap(&mut s, Id::LibraryView(LibraryViewId::CancelPick));
        assert_eq!(chosen(&fx), Some((1, vec![])));
        assert_eq!(s.session().top_window().map(|w| w.id), Some(7), "the editor back");
    }

    /// A pick starts in the folder the app suggests: under the Files tab's folder that has it, each folder
    /// on the way opened as its parent is listed. One not there is logged, the pick left where it got to.
    #[test]
    fn a_pick_starts_in_the_folder_asked_for() {
        let mut s = state();
        let folder = |path: &str| Some(std::path::PathBuf::from(path));
        s.applied(Event::Portal(PortalEvent::Asked(FileRequest { id: 1, folder: folder("/home/thor/Pictures/Saves"), ..request() })));
        assert_eq!(s.library_view().group().map(|g| g.title.as_str()), Some("Pictures"), "Home listed, Pictures not yet");
        let saves = clamshell_library::Entry {
            file: Some("/home/thor/Pictures/Saves".into()),
            browsed: Some(clamshell_library::Browsed { folder: true, ..clamshell_library::Browsed::default() }),
            ..clamshell_library::Entry::new("browse:Files/Home/Pictures/Saves", "Saves")
        };
        let listed = crate::library_view::files::FilesEvent::Listed { folder: "browse:Files/Home/Pictures".into(), entries: vec![saves] };
        s.applied(Event::Files(listed));
        assert_eq!(s.library_view().group().map(|g| g.title.as_str()), Some("Saves"));
        tap(&mut s, Id::LibraryView(LibraryViewId::CancelPick));
        s.applied(Event::Portal(PortalEvent::Asked(FileRequest { id: 2, folder: folder("/home/thor/Missing"), ..request() })));
        assert_eq!(s.library_view().group().map(|g| g.title.as_str()), Some("Home"), "as far as it goes");
        assert!(s.take_log().any(|(_, line)| line.contains("no folder \"Missing\" in Home")));
        tap(&mut s, Id::LibraryView(LibraryViewId::CancelPick));
        s.applied(Event::Portal(PortalEvent::Asked(FileRequest { id: 3, folder: folder("/nowhere"), ..request() })));
        assert!(s.take_log().any(|(_, line)| line.contains("in none of the Files tab's folders")));
    }

    /// An app asks to save, starting in Home.
    fn save(s: &mut crate::State, id: u32, asks: Asks) {
        let folder = Some("/home/thor".into());
        s.applied(Event::Portal(PortalEvent::Asked(FileRequest { id, asks, folder, patterns: vec!["*.txt".into()], ..request() })));
    }

    /// Saving: Save here saves in the open folder under the name typed; over a file that is there, or one
    /// chosen, it asks again first. Nowhere is open at the top of Files.
    #[test]
    fn a_file_is_saved_in_the_open_folder_or_over_one() {
        let mut s = state();
        let here = Id::LibraryView(LibraryViewId::SaveHere);
        let asks = Asks::Save { name: "notes.txt".into() };
        s.applied(Event::Portal(PortalEvent::Asked(FileRequest { id: 1, asks, patterns: vec!["*.txt".into()], ..request() })));
        assert!(s.library_view().save_here() == SaveHere::Off && chosen(&tap(&mut s, here.clone())).is_none(), "no folder open");
        tap(&mut s, Id::LibraryView(LibraryViewId::Launch));
        assert_eq!(s.library_view().save_here(), SaveHere::Replaces { first: "notes.txt", more: 0, asked: false });
        assert!(chosen(&tap(&mut s, here.clone())).is_none(), "asks again: notes.txt is there");
        assert_eq!(s.library_view().save_here(), SaveHere::Replaces { first: "notes.txt", more: 0, asked: true });
        // Another name: saved at once.
        tap(&mut s, Id::LibraryView(LibraryViewId::SaveAs));
        assert_eq!(s.bottom().editor().map(|e| e.text.as_str()), Some("notes.txt"), "the name on the keyboard");
        s.library_view.save_named(" new.txt ");
        assert_eq!(chosen(&tap(&mut s, here.clone())), Some((1, vec!["/home/thor/new.txt".into()])));
        // Over a file chosen, once asked again; one the filter doesn't take isn't.
        save(&mut s, 2, Asks::Save { name: "x.txt".into() });
        assert_eq!(s.library_view().group().map(|g| g.title.as_str()), Some("Home"));
        let choose = Id::LibraryView(LibraryViewId::Choose);
        s.library_view.select("browse:Files/Home/snes.png");
        assert!(chosen(&tap(&mut s, choose.clone())).is_none() && chosen(&tap(&mut s, choose.clone())).is_none(), "not a *.txt");
        s.library_view.select("browse:Files/Home/notes.txt");
        assert!(chosen(&tap(&mut s, choose.clone())).is_none(), "asks again");
        assert_eq!(chosen(&tap(&mut s, choose)), Some((2, vec!["/home/thor/notes.txt".into()])));
        // Several files: each in the folder.
        save(&mut s, 3, Asks::SaveFiles { names: vec!["a.txt".into(), "b.txt".into()] });
        assert_eq!(chosen(&tap(&mut s, here.clone())), Some((3, vec!["/home/thor/a.txt".into(), "/home/thor/b.txt".into()])));
        // Several, one of them there: Replace names that one.
        save(&mut s, 4, Asks::SaveFiles { names: vec!["a.txt".into(), "notes.txt".into()] });
        assert_eq!(s.library_view().save_here(), SaveHere::Replaces { first: "notes.txt", more: 0, asked: false });
        tap(&mut s, Id::LibraryView(LibraryViewId::CancelPick));
        // A hidden name may be there while hidden files aren't listed: asked again first.
        save(&mut s, 5, Asks::Save { name: ".bashrc".into() });
        assert_eq!(s.library_view().save_here(), SaveHere::Replaces { first: ".bashrc", more: 0, asked: false });
        // A name that isn't one file's: nothing, until another is typed.
        for name in ["../x", "/etc/x", "a/b", "..", ""] {
            s.library_view.save_named(name);
            assert_eq!(s.library_view().save_here(), SaveHere::Off, "{name:?}");
            assert!(chosen(&tap(&mut s, here.clone())).is_none());
        }
        s.library_view.save_named("fine.txt");
        assert_eq!(s.library_view().save_here(), SaveHere::Saves);
    }

    /// Until the open folder is listed, what is in it isn't known: Save here waits for its listing.
    #[test]
    fn a_save_waits_for_the_folder_to_be_listed() {
        let mut s = state();
        let folder = Some(std::path::PathBuf::from("/home/thor/Pictures"));
        s.applied(Event::Portal(PortalEvent::Asked(FileRequest { id: 1, asks: Asks::Save { name: "a.png".into() }, folder, ..request() })));
        assert_eq!(s.library_view().group().map(|g| g.title.as_str()), Some("Pictures"));
        assert_eq!(s.library_view().save_here(), SaveHere::Off, "not listed");
        let listed = crate::library_view::files::FilesEvent::Listed { folder: "browse:Files/Home/Pictures".into(), entries: vec![] };
        s.applied(Event::Files(listed));
        assert_eq!(s.library_view().save_here(), SaveHere::Saves);
    }

    /// A request whose program closes its last window while it waits (to its tray, say) is picked over
    /// whatever is in front: it couldn't come in front any more.
    #[test]
    fn a_request_of_a_program_without_windows_is_picked_over_any() {
        let mut s = state();
        let with_pid = |id: i64, app: &str, pid: u32| crate::session::windows::WindowFacts { pids: vec![pid, 1], ..win(id, app) };
        s.observed(vec![with_pid(7, "editor", 70), with_pid(8, "game", 80)]);
        s.applied(WindowEvent::Activate(8).into());
        s.applied(Event::Portal(PortalEvent::Asked(FileRequest { id: 1, asker: vec![70, 1], ..request() })));
        assert!(s.library_view().pick().is_none());
        s.observed(vec![with_pid(8, "game", 80)]);
        assert!(s.library_view().pick().is_some(), "the editor has no window left");
    }

    /// On the way to the folder asked for, a folder opened by hand ends the way: its listing opens nothing
    /// more.
    #[test]
    fn the_way_to_the_folder_asked_for_ends_where_the_person_goes() {
        let mut s = state();
        s.applied(Event::Portal(PortalEvent::Asked(FileRequest { id: 1, folder: Some("/home/thor/Pictures/Saves".into()), ..request() })));
        assert_eq!(s.library_view().group().map(|g| g.title.as_str()), Some("Pictures"));
        tap(&mut s, Id::LibraryView(LibraryViewId::Up));
        s.library_view.select("browse:Files/Home/Roms");
        tap(&mut s, Id::LibraryView(LibraryViewId::Launch));
        let saves = clamshell_library::Entry {
            file: Some("/home/thor/Roms/Saves".into()),
            browsed: Some(clamshell_library::Browsed { folder: true, ..clamshell_library::Browsed::default() }),
            ..clamshell_library::Entry::new("browse:Files/Home/Roms/Saves", "Saves")
        };
        s.applied(Event::Files(crate::library_view::files::FilesEvent::Listed { folder: "browse:Files/Home/Roms".into(), entries: vec![saves] }));
        assert_eq!(s.library_view().group().map(|g| g.title.as_str()), Some("Roms"), "not Roms/Saves");
        assert_eq!(s.take_log().count(), 0, "nothing missing either");
    }

    /// A folder asked for on removable storage, which the Files tab has in a group, is found there.
    #[test]
    fn a_folder_on_removable_storage_is_gone_to() {
        let mut library = clamshell_library::demo::library();
        let sd = library.entries.len();
        library.entries.push(clamshell_library::Entry {
            file: Some("/run/media/thor/SD".into()),
            browsed: Some(clamshell_library::Browsed {
                folder: true,
                listed: true,
                kind: clamshell_library::BrowseKind::Removable("/dev/mmcblk0p1".into()),
                ..clamshell_library::Browsed::default()
            }),
            ..clamshell_library::Entry::new("browse:Files/Storage/SD", "SD")
        });
        library.entries.push(clamshell_library::Entry { items: vec![sd], ..clamshell_library::Entry::new("folder:storage", "Storage") });
        if let Some(files) = library.collections.iter_mut().find(|c| c.name == "Files") {
            files.entries.push(sd + 1);
        }
        if let Some(card) = library.entries.get_mut(sd) {
            card.parent = Some(sd + 1);
        }
        let mut s = state_with(library);
        s.applied(Event::Portal(PortalEvent::Asked(FileRequest { id: 1, folder: Some("/run/media/thor/SD".into()), ..request() })));
        assert_eq!(s.library_view().group().map(|g| g.title.as_str()), Some("SD"));
    }

    /// Without a Files tab to pick in, a request is answered at once with nothing, which is logged.
    #[test]
    fn without_a_files_tab_nothing_is_chosen() {
        let mut library = clamshell_library::demo::library();
        library.collections.retain(|c| c.name != "Files");
        let mut s = state_with(library);
        assert_eq!(chosen(&ask(&mut s, 1, false)), Some((1, vec![])));
        assert!(s.library_view().pick().is_none());
        assert!(s.take_log().any(|(_, line)| line.contains("no Files tab")), "logged");
        assert_eq!(s.take_log().count(), 0, "taken");
    }
}
