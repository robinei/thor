//! The Library's pick mode: a file or folder chosen for something of ClamShell's own, such as a source's
//! folder in the edit mode. It shows the Files tab alone, with its browsed folders and removable storage,
//! and the details' one action is Choose: on a folder when a folder is wanted, on a file whose name
//! matches when a file is (others show dimmed). Choosing answers who asked; B, Home or Cancel leaves as it
//! was.

use clamshell_library::{BrowseKind, Entry, pattern};

use super::LibraryView;

/// What is being picked, and for what.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Pick {
    /// A folder, or else a file.
    pub folder: bool,
    /// The names a file may have (`*.png`); none: any.
    pub patterns: Vec<String>,
    pub purpose: PickFor,
    /// The tab shown before, to go back to.
    back: usize,
}

/// Who asked for a pick.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PickFor {
    /// The edit mode's form, for a source's folder.
    SourceDir,
    /// The edit mode, for the picture of the place with this id.
    PlacePicture(String),
}

impl Pick {
    /// Whether `entry` may be chosen.
    pub fn takes(&self, entry: &Entry) -> bool {
        let Some(browsed) = &entry.browsed else { return false };
        match self.folder {
            true => browsed.folder && !browsed.archive,
            false => {
                !browsed.folder && (self.patterns.is_empty() || self.patterns.iter().any(|p| pattern::matches(&p.to_lowercase(), &entry.title.to_lowercase())))
            }
        }
    }

    /// What is asked for, as the Library says it.
    pub fn asks(&self) -> &'static str {
        match &self.purpose {
            PickFor::SourceDir => "Choose the folder to scan",
            PickFor::PlacePicture(_) => "Choose the folder's picture",
        }
    }
}

impl LibraryView {
    /// The pick going on, if one is.
    pub fn pick(&self) -> Option<&Pick> {
        self.pick.as_ref()
    }

    /// The tab of the browsed folders: the first with one of them.
    pub(super) fn files_tab(&self) -> Option<usize> {
        let library = &self.library;
        let root = |i: &usize| {
            library
                .entries
                .get(*i)
                .and_then(|g| g.browsed.as_ref())
                .is_some_and(|b| matches!(b.kind, BrowseKind::Root | BrowseKind::Removable(_) | BrowseKind::Trash))
        };
        let in_folder =
            |c: &clamshell_library::Collection| c.entries.iter().any(|i| root(i) || library.entries.get(*i).is_some_and(|g| g.items.iter().any(root)));
        library.collections.iter().position(in_folder)
    }

    /// Starts picking a folder (`folder`) or a file matching `patterns`, for `purpose`, in the Files tab.
    /// Without one, nothing is picked.
    pub(crate) fn start_pick(&mut self, folder: bool, patterns: Vec<String>, purpose: PickFor) {
        let Some(files) = self.files_tab() else { return };
        self.pick = Some(Pick { folder, patterns, purpose, back: self.current });
        self.set_collection(files);
    }

    /// Leaves the pick, back on the tab shown before it.
    pub(crate) fn cancel_pick(&mut self) {
        if let Some(pick) = self.pick.take() {
            self.set_collection(pick.back);
        }
    }

    /// Chooses what is selected, if the pick takes it, and answers who asked.
    pub(crate) fn choose(&mut self, out: &mut crate::out::Out) {
        let Some((entry, _)) = self.file_target() else { return };
        let Some(pick) = self.pick.as_ref().filter(|p| p.takes(entry)) else { return };
        let Some((purpose, path)) = entry.file.clone().map(|f| (pick.purpose.clone(), f)) else { return };
        self.cancel_pick();
        let read = self.library.config.as_ref();
        let path = path.to_string_lossy();
        if let Some(edit) = &mut self.edit {
            match purpose {
                PickFor::SourceDir => edit.field_typed(super::form::Field::Dir, &path),
                PickFor::PlacePicture(id) => edit.set_picture(&id, &path, read, out),
            }
        }
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use std::sync::Arc;

    use clamshell_library::config::{Config, Place};
    use clamshell_library::{Browsed, Collection, Library};

    use crate::library_view::actions::Action;
    use crate::library_view::edit::EditId;
    use crate::library_view::form::Field;
    use crate::testkit::*;
    use crate::{Id, LibraryViewId};

    /// A Games tab, and a Files tab with Home; library.json with a place to put sources in.
    fn library() -> Library {
        let home = clamshell_library::Entry {
            file: Some("/h".into()),
            browsed: Some(Browsed { folder: true, kind: clamshell_library::BrowseKind::Root, ..Browsed::default() }),
            ..clamshell_library::Entry::new("browse:Files/Home", "Home")
        };
        let config = Config { places: vec![Place { id: "games".into(), name: "Games".into(), ..Place::default() }], ..Config::default() };
        Library {
            collections: vec![Collection::new("Games", vec![0]), Collection::new("Files", vec![1])],
            entries: vec![clamshell_library::Entry::new("zelda", "Zelda"), home],
            config: Some(Arc::new(config)),
            ..Library::default()
        }
    }

    /// A source's folder is chosen in the Files tab: a folder takes Choose, which fills the form's field
    /// and goes back to the tab shown before.
    #[test]
    fn a_sources_folder_is_chosen_in_the_files_tab() {
        let mut s = state_with(library());
        s.library_view.start_edit();
        let edit = |s: &mut crate::State, id: EditId| tap(s, Id::LibraryView(LibraryViewId::Edit(id)));
        edit(&mut s, EditId::Sources);
        edit(&mut s, EditId::Row(0));
        edit(&mut s, EditId::PickDir);
        assert_eq!(s.library_view().collection().map(|c| c.name.as_str()), Some("Files"));
        assert!(!s.library_view.set_collection(0), "the Files tab stays");
        assert_eq!(s.library_view().actions().as_slice(), [Action::Choose]);
        tap(&mut s, Id::LibraryView(LibraryViewId::Choose));
        assert!(s.library_view().pick().is_none());
        assert_eq!(s.library_view().collection().map(|c| c.name.as_str()), Some("Games"), "back where it was");
        let form = s.library_view().edit().and_then(|e| e.form()).unwrap();
        assert_eq!(form.item.value(&Config::default(), Field::Dir), "/h");
        assert_eq!(s.pointer().focus(), Some(&EditId::PickDir.into()));
    }

    /// A folder's picture is an image file chosen in the Files tab; a file that isn't an image isn't
    /// taken. Its icon is one of the UI's, chosen in the details.
    #[test]
    fn a_folders_picture_and_icon_are_chosen() {
        let mut library = library();
        let file = |name: &str| clamshell_library::Entry {
            file: Some(format!("/h/{name}").into()),
            browsed: Some(Browsed::default()),
            ..clamshell_library::Entry::new(&format!("browse:Files/Home/{name}"), name)
        };
        if let Some(config) = library.config.as_mut() {
            Arc::make_mut(config).places.push(Place { id: "snes".into(), name: "SNES".into(), within: Some("games".into()), ..Place::default() });
        }
        library.entries.extend([file("notes.txt"), file("snes.png")]);
        if let Some(home) = library.entries.get_mut(1) {
            home.items = vec![2, 3];
            home.browsed.as_mut().unwrap().listed = true;
        }
        let mut s = state_with(library);
        s.library_view.start_edit();
        let edit = |s: &mut crate::State, id: EditId| tap(s, Id::LibraryView(LibraryViewId::Edit(id)));
        edit(&mut s, EditId::Picture);
        assert!(s.library_view().pick().is_some(), "SNES, a folder, selected");
        tap(&mut s, Id::LibraryView(LibraryViewId::Launch));
        let notes = s.library_view().library().entries.iter().position(|g| g.title == "notes.txt").unwrap();
        let png = s.library_view().library().entries.iter().position(|g| g.title == "snes.png").unwrap();
        let pick = s.library_view().pick().unwrap().clone();
        assert!(!pick.takes(s.library_view().library().entries.get(notes).unwrap()) && pick.takes(s.library_view().library().entries.get(png).unwrap()));
        s.library_view.select("browse:Files/Home/snes.png");
        let fx = tap(&mut s, Id::LibraryView(LibraryViewId::Choose));
        let written = fx.iter().find_map(|e| if let crate::Effect::WriteLibrary(c) = e { Some(c.clone()) } else { None }).unwrap();
        assert_eq!(written.places.iter().find(|p| p.id == "snes").and_then(|p| p.picture.as_deref()), Some("/h/snes.png"));
        edit(&mut s, EditId::Icon);
        assert!(s.library_view().edit().unwrap().choosing_icon());
        let fx = edit(&mut s, EditId::IconChoice(1));
        let written = fx.iter().find_map(|e| if let crate::Effect::WriteLibrary(c) = e { Some(c.clone()) } else { None }).unwrap();
        assert_eq!(written.places.iter().find(|p| p.id == "snes").and_then(|p| p.icon.as_deref()), Some("gamepad"));
    }
}
