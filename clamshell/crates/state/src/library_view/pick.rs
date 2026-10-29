//! The Library's pick mode: a file or folder chosen for something of ClamShell's own, such as a source's
//! folder in the edit mode, or an app's file chooser. It shows the Files tab alone, with its browsed folders
//! and removable storage, and the details' one action is Choose: on a folder when a folder is wanted, on
//! a file whose name matches when a file is (others show dimmed). Choosing answers who asked; B, Home or
//! Cancel leaves as it was.
//!
//! Saving, the open folder is where: Save here saves in it, under the name typed (Save as), and Choose
//! on a file saves over it. Over a file that is there, each asks again first (Replace).

use std::path::{Path, PathBuf};

use clamshell_library::{BrowseKind, Entry, pattern};

use super::LibraryView;

/// What is being picked, and for what.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Pick {
    pub takes: Takes,
    pub purpose: PickFor,
    /// Saving over this file was asked for once; asked for again, it is chosen.
    confirm: Option<PathBuf>,
    /// The way to the folder to start in: the folder it goes on from, and the folders still to open from
    /// there, each as its parent is listed (see [`LibraryView::go_to`]).
    on_the_way: Option<(std::sync::Arc<str>, Vec<String>)>,
    /// The tab shown before, to go back to.
    back: usize,
    /// The Files tab's place before, to leave it in again.
    files_place: Vec<super::KeptLevel>,
}

/// What Save here would do (see [`LibraryView::save_here`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SaveHere<'a> {
    /// Nothing: no folder that things may be put in is open, it isn't listed yet, or a name isn't one
    /// file's (empty, with a `/`, or `.` or `..`).
    Off,
    /// Save, nothing being in the way.
    Saves,
    /// Save over `first`, and `more` others, that are or may be there: once `asked` again.
    Replaces { first: &'a str, more: usize, asked: bool },
}

/// Whether `name` names one file in a folder: not empty, no `/`, not `.` or `..`.
pub fn file_name(name: &str) -> bool {
    let mut parts = Path::new(name).components();
    matches!((parts.next(), parts.next()), (Some(std::path::Component::Normal(_)), None)) && !name.contains('/')
}

/// What a pick takes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Takes {
    Folder,
    /// A file with one of these names (`*.png`); none: any.
    Files(Vec<String>),
    /// A file that may be run (see [`clamshell_library::Browsed::executable`]).
    Program,
    /// Where to save a file named `name`, or a file matching `patterns` to save over.
    Save {
        name: String,
        patterns: Vec<String>,
    },
    /// A folder to save files of these names in.
    SaveFiles(Vec<String>),
}

/// Who asked for a pick.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PickFor {
    /// The edit mode's form, for one of its fields (see [`super::form::Field::picks`]).
    Field(super::form::Field),
    /// The edit mode, for the picture of the place with this id.
    PlacePicture(String),
    /// An app's file chooser (see [`crate::portal`]): its request, and what the app says it is for.
    App { request: u32, title: String },
}

impl Pick {
    /// Whether `entry` may be chosen.
    pub fn takes(&self, entry: &Entry) -> bool {
        let Some(browsed) = &entry.browsed else { return false };
        // No patterns: any name.
        let matches = |patterns: &[String]| patterns.is_empty() || patterns.iter().any(|p| pattern::matches_ignoring_case(p, &entry.title));
        match &self.takes {
            Takes::Folder => browsed.folder && !browsed.archive,
            Takes::Files(patterns) => !browsed.folder && matches(patterns),
            Takes::Program => !browsed.folder && browsed.executable,
            Takes::Save { patterns, .. } => !browsed.folder && matches(patterns),
            Takes::SaveFiles(_) => false,
        }
    }

    /// Whether it is a save: in the open folder, by Save here.
    pub fn saves(&self) -> bool {
        matches!(self.takes, Takes::Save { .. } | Takes::SaveFiles(_))
    }

    /// Whether saving over `path` was asked for once, and is asked again (Replace).
    pub fn confirming(&self, path: &Path) -> bool {
        self.confirm.as_deref() == Some(path)
    }

    /// What is asked for, as the Library says it.
    pub fn asks(&self) -> &str {
        match &self.purpose {
            PickFor::Field(f) => f.asks(),
            PickFor::PlacePicture(_) => "Choose the folder's picture",
            PickFor::App { title, .. } if !title.is_empty() => title,
            PickFor::App { .. } => match &self.takes {
                Takes::Folder => "Choose a folder",
                Takes::Save { .. } => "Save",
                Takes::SaveFiles(_) => "Save the files",
                _ => "Choose a file",
            },
        }
    }
}

impl LibraryView {
    /// The pick going on, if one is.
    pub fn pick(&self) -> Option<&Pick> {
        self.pick.as_ref()
    }

    /// Whether there is a Files tab to pick in.
    pub fn can_pick(&self) -> bool {
        self.files_tab().is_some()
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

    /// Starts picking what `takes` says, for `purpose`, in the Files tab, and whether it did: without a
    /// Files tab, nothing is picked.
    pub(crate) fn start_pick(&mut self, takes: Takes, purpose: PickFor) -> bool {
        let Some(files) = self.files_tab() else { return false };
        // A pick of ClamShell's own waits while an app's goes on (see `crate::portal`).
        if matches!(purpose, PickFor::App { .. }) && self.pick.as_ref().is_some_and(|p| !matches!(p.purpose, PickFor::App { .. })) {
            self.own_aside = self.pick.take();
        }
        let files_place = self.kept_place(files);
        self.pick = Some(Pick { takes, purpose, confirm: None, on_the_way: None, back: self.current, files_place });
        self.set_collection(files);
        true
    }

    /// Opens `folder` in the pick, under the Files tab's folder that has it: the folders on the way are
    /// opened as each one's parent is listed ([`LibraryView::follow_way`]). An error says why it can't be.
    pub(crate) fn go_to(&mut self, folder: &Path) -> Result<(), String> {
        let library = &self.library;
        let files = self.files_tab().and_then(|c| library.collections.get(c)).ok_or("no Files tab")?;
        // The deepest of the tab's folders that has it: at its top, or in one of its groups, as removable
        // storage is.
        let tops = files.entries.iter().filter_map(|&i| library.entries.get(i));
        let (root, rest) = tops
            .clone()
            .chain(tops.flat_map(|g| g.items.iter().filter_map(|&i| library.entries.get(i))))
            .filter(|g| g.browsed.is_some())
            .filter_map(|g| Some((g, folder.strip_prefix(g.browsed_dir()?).ok()?)))
            .max_by_key(|(_, rest)| std::cmp::Reverse(rest.components().count()))
            .ok_or("in none of the Files tab's folders")?;
        let (root, mut rest): (std::sync::Arc<str>, Vec<String>) =
            (root.id.clone(), rest.components().map(|c| c.as_os_str().to_string_lossy().into_owned()).collect());
        rest.reverse();
        if !self.reveal(&root) {
            return Err("its Files folder isn't shown".into());
        }
        if let Some(pick) = &mut self.pick {
            pick.on_the_way = Some((root, rest));
        }
        self.follow_way()
    }

    /// Opens the next folders on the way to the one to start in, as far as they are listed. An error says
    /// where the way ends, a folder listed without the next.
    /// The way goes on only from the folder it reached: one opened by hand since ends it.
    pub(crate) fn follow_way(&mut self) -> Result<(), String> {
        while let Some((from, next)) = self.pick.as_ref().and_then(|p| p.on_the_way.as_ref()).map(|(from, rest)| (from.clone(), rest.last().cloned())) {
            let open = self.group().filter(|g| g.id == from);
            let (Some(open), Some(next)) = (open, next) else {
                // Arrived, or left for another folder.
                if let Some(pick) = &mut self.pick {
                    pick.on_the_way = None;
                }
                break;
            };
            let listed = open.browsed.as_ref().is_some_and(|b| b.listed);
            let found = open.items.iter().filter_map(|&i| self.library.entries.get(i)).find(|g| g.title == next && g.is_group()).map(|g| g.id.clone());
            match found {
                Some(id) => {
                    self.select(&id);
                    self.open();
                    if let Some((from, rest)) = self.pick.as_mut().and_then(|p| p.on_the_way.as_mut()) {
                        rest.pop();
                        *from = id;
                    }
                }
                None if listed => {
                    let missing = format!("no folder {next:?} in {}", open.title);
                    if let Some(pick) = &mut self.pick {
                        pick.on_the_way = None;
                    }
                    return Err(missing);
                }
                // Not listed yet: on when it is.
                None => break,
            }
        }
        self.sync();
        Ok(())
    }

    /// Leaves the pick, back on the tab shown before it. An app's is answered: nothing chosen.
    pub(crate) fn cancel_pick(&mut self) {
        if let Some(PickFor::App { request, .. }) = self.end_pick() {
            self.answered = Some((request, Vec::new()));
        }
    }

    /// The names a save is of: the one typed, or each of those asked for; none if the pick isn't a save.
    fn save_names(&self) -> &[String] {
        match self.pick.as_ref().map(|p| &p.takes) {
            Some(Takes::Save { name, .. }) => std::slice::from_ref(name),
            Some(Takes::SaveFiles(names)) => names,
            _ => &[],
        }
    }

    /// What Save here would do, from what is known: the open folder, whether it is listed, whether hidden
    /// files are, and the names (see [`SaveHere`]).
    pub fn save_here(&self) -> SaveHere<'_> {
        let names = self.save_names();
        let folder = self.group().filter(|g| g.browsed.as_ref().is_some_and(|b| b.listed));
        if names.is_empty() || !names.iter().all(|n| file_name(n)) || self.open_dir_path().is_none() {
            return SaveHere::Off;
        }
        // Until it is listed, what is in it isn't known.
        let Some(folder) = folder else { return SaveHere::Off };
        let listed = |n: &str| folder.items.iter().filter_map(|&i| self.library.entries.get(i)).any(|e| e.title == n);
        // A hidden file isn't listed while hidden files don't show: it may be there.
        let hidden_unknown = !self.files.hidden();
        let mut there = names.iter().filter(|n| listed(n) || (hidden_unknown && n.starts_with('.')));
        let Some(first) = there.next() else { return SaveHere::Saves };
        // (Compared by its folder and name, not a path made for it: this is asked every frame.)
        let asked =
            self.pick.as_ref().and_then(|p| p.confirm.as_deref()).is_some_and(|c| c.parent() == self.open_dir_path() && c.file_name() == Some(first.as_ref()));
        SaveHere::Replaces { first, more: there.count(), asked }
    }

    /// Save here: answered with the paths in the open folder; over a file that is or may be there, once
    /// asked again (see [`SaveHere::Replaces`]).
    pub(crate) fn save(&mut self) {
        let replaces = match self.save_here() {
            SaveHere::Off => return,
            SaveHere::Saves => None,
            SaveHere::Replaces { first, asked, .. } => (!asked).then(|| first.to_owned()),
        };
        let Some(dir) = self.open_dir() else { return };
        if let Some(first) = replaces {
            if let Some(pick) = &mut self.pick {
                pick.confirm = Some(dir.join(first));
            }
            return;
        }
        let paths = self.save_names().iter().map(|n| dir.join(n)).collect();
        self.answer(paths);
    }

    /// Save as: the name typed for the file saved.
    pub(crate) fn save_named(&mut self, typed: &str) {
        if let Some(Pick { takes: Takes::Save { name, .. }, confirm, .. }) = &mut self.pick {
            typed.trim().clone_into(name);
            *confirm = None;
        }
    }

    /// Ends an app's pick with `paths` as its answer.
    fn answer(&mut self, paths: Vec<PathBuf>) {
        if let Some(PickFor::App { request, .. }) = self.end_pick() {
            self.answered = Some((request, paths));
        }
    }

    /// Leaves the pick without answering, back on the tab shown before it with the Files tab where it was,
    /// and says what it was for. A pick of ClamShell's own that waited for it goes on.
    pub(crate) fn end_pick(&mut self) -> Option<PickFor> {
        let pick = self.pick.take()?;
        if let Some(files) = self.files_tab() {
            self.reset_place(files, &pick.files_place);
        }
        self.set_collection(pick.back);
        self.pick = self.own_aside.take();
        Some(pick.purpose)
    }

    /// An app's pick answered since the last look: its request, and the paths chosen, none if cancelled.
    pub(crate) fn take_answered(&mut self) -> Option<(u32, Vec<PathBuf>)> {
        self.answered.take()
    }

    /// Chooses what is selected, if the pick takes it, and answers who asked.
    pub(crate) fn choose(&mut self, out: &mut crate::out::Out) {
        let Some((entry, _)) = self.file_target() else { return };
        if !self.pick.as_ref().is_some_and(|p| p.takes(entry)) {
            return;
        }
        let Some(path) = entry.file.clone() else { return };
        // Saving over a file: once asked again.
        if let Some(pick) = self.pick.as_mut().filter(|p| p.saves())
            && pick.confirm.replace(path.clone()).as_ref() != Some(&path)
        {
            return;
        }
        let Some(purpose) = self.end_pick() else { return };
        match (purpose, &mut self.edit) {
            (PickFor::App { request, .. }, _) => self.answered = Some((request, vec![path])),
            (PickFor::Field(f), Some(edit)) => edit.field_picked(f, &path.to_string_lossy()),
            (PickFor::PlacePicture(id), Some(edit)) => {
                let mut shown = super::edit::ConfigShown { read: self.library.config.as_ref(), written: &mut self.written };
                edit.set_picture(&id, &path.to_string_lossy(), &mut shown, out);
            }
            (_, None) => {}
        }
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use std::sync::Arc;

    use clamshell_library::config::{Config, Place};
    use clamshell_library::{Browsed, Collection, Library};

    use super::{Pick, PickFor, Takes};
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
        edit(&mut s, EditId::Pick(2));
        assert_eq!(s.library_view().collection().map(|c| c.name.as_str()), Some("Files"));
        assert!(!s.library_view.set_collection(0), "the Files tab stays");
        assert_eq!(s.library_view().actions().as_slice(), [Action::Choose]);
        tap(&mut s, Id::LibraryView(LibraryViewId::Choose));
        assert!(s.library_view().pick().is_none());
        assert_eq!(s.library_view().collection().map(|c| c.name.as_str()), Some("Games"), "back where it was");
        let form = s.library_view().edit().and_then(|e| e.form()).unwrap();
        assert_eq!(form.item.value(&Config::default(), Field::Dir), "/h");
        assert_eq!(s.pointer().focus(), Some(&EditId::Pick(2).into()));
    }

    /// A command's program is a file that may be run; another file isn't taken, nor a folder.
    #[test]
    fn a_program_is_a_file_that_may_be_run() {
        let pick = Pick { takes: Takes::Program, purpose: PickFor::Field(Field::Launch), confirm: None, on_the_way: None, back: 0, files_place: Vec::new() };
        let file = |executable: bool, folder: bool| clamshell_library::Entry {
            browsed: Some(Browsed { executable, folder, ..Browsed::default() }),
            ..clamshell_library::Entry::new("browse:Files/Home/x", "x")
        };
        assert!(pick.takes(&file(true, false)));
        assert!(!pick.takes(&file(false, false)) && !pick.takes(&file(false, true)));
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
