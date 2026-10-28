//! Apps' file choosers: ClamShell serves xdg-desktop-portal's file chooser (clamshell-system's
//! `session::portal`), and each request is a pick in the Library (see [`crate::library_view::pick`]): a
//! file or folder to open, or where to save, in the open folder under a name or over a file.
//! Requests wait their turn, one picked at a time, the Library in front; once one is answered, what was
//! in front before comes back.

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

/// The requests waiting and what to go back to.
#[derive(Debug, Default)]
pub(crate) struct Portal {
    /// In the order they came; the first is being picked, if a pick for an app goes on.
    pub(crate) asked: Vec<FileRequest>,
    /// The group in front when the pick began, shown again when it is answered.
    pub(crate) back: Option<Group>,
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    use crate::testkit::*;
    use crate::{Effect, Event, Id, LibraryViewId, WindowEvent};

    fn ask(s: &mut crate::State, id: u32, folder: bool) -> Vec<Effect> {
        let asks = Asks::Open { folder };
        s.applied(Event::Portal(PortalEvent::Asked(FileRequest { id, title: String::new(), asks, patterns: vec!["*.txt".into()] })))
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

    /// Requests wait their turn. Cancelled, one is answered with nothing; Home answers so too but stays on
    /// the Library; one the app gave up isn't answered.
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
        // The third, left with Home: answered, and the Library stays.
        let fx = s.applied(Event::Home);
        assert_eq!(chosen(&fx), Some((3, vec![])));
        assert!(s.session().top_window().is_none() && s.library_view().pick().is_none());
    }

    fn save(s: &mut crate::State, id: u32, asks: Asks) {
        s.applied(Event::Portal(PortalEvent::Asked(FileRequest { id, title: String::new(), asks, patterns: vec!["*.txt".into()] })));
    }

    /// Saving: Save here saves in the open folder under the name typed; over a file that is there, or one
    /// chosen, it asks again first. Nowhere is open at the top of Files.
    #[test]
    fn a_file_is_saved_in_the_open_folder_or_over_one() {
        let mut s = state();
        let here = Id::LibraryView(LibraryViewId::SaveHere);
        save(&mut s, 1, Asks::Save { name: "notes.txt".into() });
        assert!(s.library_view().save_here().is_none() && chosen(&tap(&mut s, here.clone())).is_none(), "no folder open");
        tap(&mut s, Id::LibraryView(LibraryViewId::Launch));
        assert_eq!(s.library_view().save_here(), Some((vec!["/home/thor/notes.txt".into()], true)));
        assert!(chosen(&tap(&mut s, here.clone())).is_none(), "asks again: notes.txt is there");
        assert!(s.library_view().pick().unwrap().confirming(std::path::Path::new("/home/thor/notes.txt")));
        // Another name: saved at once.
        tap(&mut s, Id::LibraryView(LibraryViewId::SaveAs));
        assert_eq!(s.bottom().editor().map(|e| e.text.as_str()), Some("notes.txt"), "the name on the keyboard");
        s.library_view.save_named(" new.txt ");
        assert_eq!(chosen(&tap(&mut s, here.clone())), Some((1, vec!["/home/thor/new.txt".into()])));
        // Over a file chosen, once asked again; one the filter doesn't take isn't.
        save(&mut s, 2, Asks::Save { name: "x.txt".into() });
        assert_eq!(s.library_view().group().map(|g| g.title.as_str()), Some("Home"), "the Files tab where it was left");
        let choose = Id::LibraryView(LibraryViewId::Choose);
        s.library_view.select("browse:Files/Home/snes.png");
        assert!(chosen(&tap(&mut s, choose.clone())).is_none() && chosen(&tap(&mut s, choose.clone())).is_none(), "not a *.txt");
        s.library_view.select("browse:Files/Home/notes.txt");
        assert!(chosen(&tap(&mut s, choose.clone())).is_none(), "asks again");
        assert_eq!(chosen(&tap(&mut s, choose)), Some((2, vec!["/home/thor/notes.txt".into()])));
        // Several files: each in the folder.
        save(&mut s, 3, Asks::SaveFiles { names: vec!["a.txt".into(), "b.txt".into()] });
        assert_eq!(chosen(&tap(&mut s, here)), Some((3, vec!["/home/thor/a.txt".into(), "/home/thor/b.txt".into()])));
    }

    /// Without a Files tab to pick in, a request is answered at once with nothing, which is logged.
    #[test]
    fn without_a_files_tab_nothing_is_chosen() {
        let mut library = clamshell_library::demo::library();
        library.collections.retain(|c| c.name != "Files");
        let mut s = state_with(library);
        assert_eq!(chosen(&ask(&mut s, 1, false)), Some((1, vec![])));
        assert!(s.library_view().pick().is_none());
        assert!(s.take_log().any(|line| line.contains("no Files tab")), "logged");
        assert_eq!(s.take_log().count(), 0, "taken");
    }
}
