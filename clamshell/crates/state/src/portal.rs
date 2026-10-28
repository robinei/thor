//! Apps' file choosers: ClamShell serves xdg-desktop-portal's file chooser (clamshell-system's
//! `session::portal`), and each request is a pick in the Library (see [`crate::library_view::pick`]).
//! Requests wait their turn, one picked at a time, the Library in front; once one is answered, what was
//! in front before comes back.

use crate::library_view::pick::Takes;
use crate::session::windows::Group;

/// From the portal thread.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PortalEvent {
    /// An app asks for a file or folder to open.
    Asked(FileRequest),
    /// The request was given up: the app cancelled it or went away. It isn't answered.
    Closed(u32),
}

/// An app's request to open a file or folder.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileRequest {
    /// The portal thread's number for it, by which it is answered.
    pub id: u32,
    /// What the app says it is for; empty if it says nothing.
    pub title: String,
    /// A folder, or else a file.
    pub folder: bool,
    /// The names a file may have (`*.png`), from the app's filter; none: any.
    pub patterns: Vec<String>,
}

impl FileRequest {
    pub(crate) fn takes(&self) -> Takes {
        if self.folder { Takes::Folder } else { Takes::Files(self.patterns.clone()) }
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
        s.applied(Event::Portal(PortalEvent::Asked(FileRequest { id, title: String::new(), folder, patterns: vec!["*.txt".into()] })))
    }

    fn chosen(fx: &[Effect]) -> Option<(u32, Option<std::path::PathBuf>)> {
        fx.iter().find_map(|e| if let Effect::FileChosen { request, path } = e { Some((*request, path.clone())) } else { None })
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
        assert_eq!(chosen(&fx), Some((1, Some("/home/thor/notes.txt".into()))));
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
        assert_eq!(chosen(&fx), Some((1, None)));
        assert!(s.session().top_window().is_none() && s.library_view().pick().is_some());
        // The second, given up by the app: the third.
        let fx = s.applied(Event::Portal(PortalEvent::Closed(2)));
        assert_eq!(chosen(&fx), None);
        assert_eq!(s.library_view().pick().map(|p| p.asks()), Some("Choose a file"));
        // The third, left with Home: answered, and the Library stays.
        let fx = s.applied(Event::Home);
        assert_eq!(chosen(&fx), Some((3, None)));
        assert!(s.session().top_window().is_none() && s.library_view().pick().is_none());
    }

    /// Without a Files tab to pick in, a request is answered at once with nothing.
    #[test]
    fn without_a_files_tab_nothing_is_chosen() {
        let mut library = clamshell_library::demo::library();
        library.collections.retain(|c| c.name != "Files");
        let mut s = state_with(library);
        assert_eq!(chosen(&ask(&mut s, 1, false)), Some((1, None)));
        assert!(s.library_view().pick().is_none());
    }
}
