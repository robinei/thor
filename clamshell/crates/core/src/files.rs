//! Shelf's file actions, on browsed entries (see [`crate::Library::set_listing`]): the folder open is
//! listed (and watched) while it is open; the selected entry renamed, copied or cut (then pasted into the
//! folder open), moved to the Trash; in the Trash restored or deleted; a new folder made; a removable
//! filesystem ejected. What is done on disk goes out as [`FileOp`]s; the listings come back as they
//! change. Deleting and emptying the Trash ask first: the action pressed again.

use std::path::PathBuf;
use std::sync::Arc;

use clamshell_library::BrowseKind;

use crate::editor::{Editor, Purpose};
use crate::effect::Effect;
use crate::hardware::NoticeKind;
use crate::{Game, State, Tab};

/// What can be done to a browsed entry, or in the folder open.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum FileAction {
    Rename,
    Copy,
    Cut,
    Trash,
    Restore,
    Delete,
    EmptyTrash,
    Eject,
    /// Into the folder open.
    Paste,
    NewFolder,
}

impl FileAction {
    pub fn label(self) -> &'static str {
        match self {
            FileAction::Rename => "Rename",
            FileAction::Copy => "Copy",
            FileAction::Cut => "Cut",
            FileAction::Trash => "Move to Trash",
            FileAction::Restore => "Restore",
            FileAction::Delete => "Delete",
            FileAction::EmptyTrash => "Empty Trash",
            FileAction::Eject => "Eject",
            FileAction::Paste => "Paste",
            FileAction::NewFolder => "New folder",
        }
    }

    /// It can't be undone: asks first (pressed again).
    pub fn confirms(self) -> bool {
        matches!(self, FileAction::Delete | FileAction::EmptyTrash)
    }
}

/// What is done on disk (by the files thread).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FileOp {
    Rename {
        path: PathBuf,
        name: String,
    },
    NewFolder {
        dir: PathBuf,
        name: String,
    },
    /// Copied (or moved, `cut`) into `into`; a name there already gets " (2)" (never replaced).
    Paste {
        path: PathBuf,
        into: PathBuf,
        cut: bool,
    },
    Trash(PathBuf),
    /// From the Trash's `files` back where it was.
    Restore(PathBuf),
    Delete(PathBuf),
    /// Everything in the Trash (its `files` dir) deleted.
    EmptyTrash(PathBuf),
    /// A removable filesystem's device unmounted.
    Eject(String),
}

/// A folder to list and watch: its entry's id (the entries' ids are it, `/`, their names) and its dir.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Browse {
    pub folder: Arc<str>,
    pub dir: PathBuf,
}

/// What the files thread says.
#[derive(Debug, Clone, PartialEq)]
pub enum FilesEvent {
    /// Folder `folder`'s entries now, in order (see [`crate::Library::set_listing`]).
    Listed { folder: Arc<str>, entries: Vec<Game> },
    /// A paste under way: what (`Copying Film.mkv`), how far (of its bytes).
    Busy { what: String, fraction: f32 },
    /// An operation ended: what was done, or what went wrong.
    Done(Result<String, String>),
    /// A removable filesystem was mounted or unmounted.
    Mounts,
}

/// The file actions' state: the entry copied or cut, the action waiting to be confirmed (on the entry),
/// the paste under way, the folder listed and watched.
#[derive(Debug, Clone, Default)]
pub struct Files {
    pub clip: Option<Clip>,
    pub armed: Option<(FileAction, Arc<str>)>,
    pub busy: Option<(String, f32)>,
    pub(crate) browsing: Option<Arc<str>>,
}

/// An entry copied (or cut), to paste.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Clip {
    pub path: PathBuf,
    pub name: String,
    pub cut: bool,
}

/// What a file's name is edited for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FileName {
    Rename(PathBuf),
    NewFolder(PathBuf),
}

impl State {
    pub fn files(&self) -> &Files {
        &self.files
    }

    /// What the details show the actions of: the selected entry, or with none (an empty folder) the
    /// folder open; and whether it is the selected one.
    pub fn file_target(&self) -> Option<(&Game, bool)> {
        match self.shelf.selected_game() {
            Some(g) => Some((g, true)),
            None => self.shelf.group().map(|g| (g, false)),
        }
    }

    /// The folder open, if it is browsed and things may be put in it (not the Trash).
    fn open_dir(&self) -> Option<PathBuf> {
        let g = self.shelf.group()?;
        let kind = &g.browsed.as_ref()?.kind;
        (!matches!(kind, BrowseKind::Trash | BrowseKind::Trashed)).then(|| g.browsed_dir().map(PathBuf::from)).flatten()
    }

    /// The file actions of what the details show (see [`State::file_target`]): its own if it is the
    /// selected entry, then the folder open's (Paste with something copied, New folder).
    pub fn file_actions(&self) -> Vec<FileAction> {
        use FileAction::*;
        let Some((game, selected)) = self.file_target() else {
            return vec![];
        };
        let Some(b) = game.browsed.as_ref() else {
            return vec![];
        };
        let mut actions = match (&b.kind, selected) {
            (_, false) => vec![],
            (BrowseKind::Plain, true) => vec![Copy, Cut, Rename, Trash],
            (BrowseKind::Trashed, true) => vec![Restore, Delete],
            (BrowseKind::Removable(_), true) => vec![Eject],
            (BrowseKind::Trash, true) => vec![EmptyTrash],
            (BrowseKind::Root, true) => vec![],
        };
        if self.open_dir().is_some() {
            if self.files.clip.is_some() {
                actions.push(Paste);
            }
            actions.push(NewFolder);
        }
        actions
    }

    /// Whether `action` waits to be confirmed (pressed once) on what the details show.
    pub fn file_armed(&self, action: FileAction) -> bool {
        self.files.armed.as_ref().is_some_and(|(a, id)| *a == action && self.file_target().is_some_and(|(g, _)| g.id == *id))
    }

    /// A file action pressed (if what the details show has it).
    pub(crate) fn on_file(&mut self, action: FileAction) -> Vec<Effect> {
        if !self.file_actions().contains(&action) {
            return vec![];
        }
        let Some((game, _)) = self.file_target() else {
            return vec![];
        };
        let (id, path, kind) = (game.id.clone(), game.file.clone(), game.browsed.as_ref().map(|b| b.kind.clone()));
        let name = game.title.clone();
        if action.confirms() && !self.file_armed(action) {
            self.files.armed = Some((action, id));
            return vec![];
        }
        self.files.armed = None;
        let op = match (action, path) {
            (FileAction::Paste, _) => {
                let (Some(clip), Some(into)) = (self.files.clip.clone(), self.open_dir()) else {
                    return vec![];
                };
                if clip.cut {
                    self.files.clip = None;
                }
                FileOp::Paste { path: clip.path, into, cut: clip.cut }
            }
            (FileAction::NewFolder, _) => {
                let Some(dir) = self.open_dir() else {
                    return vec![];
                };
                return self.name_file(FileName::NewFolder(dir), String::new());
            }
            (FileAction::Eject, _) => match kind {
                Some(BrowseKind::Removable(device)) => FileOp::Eject(device),
                _ => return vec![],
            },
            (_, None) => return vec![],
            (FileAction::Rename, Some(path)) => return self.name_file(FileName::Rename(path), name),
            (FileAction::Copy | FileAction::Cut, Some(path)) => {
                self.files.clip = Some(Clip { path, name, cut: action == FileAction::Cut });
                return vec![];
            }
            (FileAction::Trash, Some(path)) => FileOp::Trash(path),
            (FileAction::Restore, Some(path)) => FileOp::Restore(path),
            (FileAction::Delete, Some(path)) => FileOp::Delete(path),
            (FileAction::EmptyTrash, Some(path)) => FileOp::EmptyTrash(path),
        };
        vec![Effect::Files(op)]
    }

    /// The editor on the keyboard tab for a file's name (`text` to start from).
    fn name_file(&mut self, purpose: FileName, text: String) -> Vec<Effect> {
        self.bottom.open_editor(Editor { purpose: Purpose::FileName(purpose), text });
        if self.bottom.tab != Tab::Keyboard {
            self.bottom.raised_from = Some(self.bottom.tab);
        }
        self.set_tab(Tab::Keyboard)
    }

    /// A file's name typed: renamed, or the folder made; the keyboard goes back to the tab it came up
    /// from.
    pub(crate) fn file_named(&mut self, purpose: FileName, name: &str) -> Vec<Effect> {
        let name = name.trim();
        let mut effects = match self.bottom.raised_from.take() {
            Some(tab) => self.set_tab(tab),
            None => vec![],
        };
        if name.is_empty() || name.contains('/') || name == "." || name == ".." {
            return effects;
        }
        effects.push(Effect::Files(match purpose {
            FileName::Rename(path) => FileOp::Rename { path, name: name.to_owned() },
            FileName::NewFolder(dir) => FileOp::NewFolder { dir, name: name.to_owned() },
        }));
        effects
    }

    /// A file name's editing cancelled: back to the tab the keyboard came up from.
    pub(crate) fn file_name_cancelled(&mut self) -> Vec<Effect> {
        match self.bottom.raised_from.take() {
            Some(tab) => self.set_tab(tab),
            None => vec![],
        }
    }

    pub(crate) fn on_files(&mut self, event: FilesEvent) -> Vec<Effect> {
        match event {
            FilesEvent::Listed { folder, entries } => {
                if self.shelf.listed(folder, entries) {
                    self.moved_collection();
                }
                vec![]
            }
            FilesEvent::Busy { what, fraction } => {
                self.files.busy = Some((what, fraction));
                vec![]
            }
            FilesEvent::Done(result) => {
                self.files.busy = None;
                self.notify(NoticeKind::Files(result));
                vec![]
            }
            // (Removable storage's groups, and sources on it, come and go with the library read again.)
            FilesEvent::Mounts => vec![Effect::Tool("rescan".into())],
        }
    }

    /// After every event: the browsed folder open listed and watched when it became the open one (none:
    /// the watch dropped); an armed action dropped once the details show something else.
    pub(crate) fn follow_browse(&mut self) -> Option<Effect> {
        if let Some((_, id)) = &self.files.armed
            && self.file_target().is_none_or(|(g, _)| g.id != *id)
        {
            self.files.armed = None;
        }
        let open = self.shelf.group().and_then(|g| Some(Browse { folder: g.id.clone(), dir: g.browsed_dir()?.to_path_buf() }));
        let id = open.as_ref().map(|b| b.folder.clone());
        if self.files.browsing == id {
            return None;
        }
        self.files.browsing = id;
        Some(Effect::Browse(open))
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::indexing_slicing)]
mod tests {
    use super::*;
    use crate::testkit::{state_with, tap};
    use crate::{Browsed, Collection, Event, Id, Library, ShelfEvent, ShelfId};

    fn entry(id: &str, folder: bool, kind: BrowseKind) -> Game {
        let name = id.rsplit('/').next().unwrap();
        let path = format!("/{}", id.strip_prefix("browse:").unwrap());
        Game { file: Some(path.into()), browsed: Some(Browsed { folder, kind, ..Browsed::default() }), ..Game::new(id, name) }
    }

    /// A Files tab: Home and the Trash.
    fn files_state() -> State {
        let mut trash = entry("browse:Trash", true, BrowseKind::Trash);
        trash.file = Some("/t/files".into());
        let games = vec![entry("browse:Home", true, BrowseKind::Root), trash];
        state_with(Library { collections: vec![Collection::new("Files", vec![0, 1])], games, ..Library::default() })
    }

    fn listed(s: &mut State, folder: &str, entries: Vec<Game>) {
        s.apply(Event::Files(FilesEvent::Listed { folder: folder.into(), entries }));
    }

    #[test]
    fn the_folder_open_is_listed_and_watched_while_open() {
        let mut s = files_state();
        let (_, fx) = s.apply(Event::Pointer(crate::PointerEvent::Activate(Id::Shelf(ShelfId::Launch))));
        assert_eq!(fx, vec![Effect::Browse(Some(Browse { folder: "browse:Home".into(), dir: "/Home".into() }))]);
        assert!(s.shelf().list().is_empty(), "nothing in it until listed");
        listed(&mut s, "browse:Home", vec![entry("browse:Home/Films", true, BrowseKind::Plain), entry("browse:Home/a.txt", false, BrowseKind::Plain)]);
        assert_eq!(s.shelf().list().len(), 2);
        assert_eq!(s.shelf().selected().map(|g| &**g), Some("browse:Home/Films"));
        // Into Films (listed and watched instead), back out (Home again).
        let fx = tap(&mut s, Id::Shelf(ShelfId::Launch));
        assert_eq!(fx, vec![Effect::Browse(Some(Browse { folder: "browse:Home/Films".into(), dir: "/Home/Films".into() }))]);
        let fx = tap(&mut s, Id::Shelf(ShelfId::Up));
        assert!(matches!(fx.as_slice(), [Effect::Browse(Some(b))] if &*b.folder == "browse:Home"));
        assert_eq!(s.shelf().selected().map(|g| &**g), Some("browse:Home/Films"), "where it was");
        let fx = tap(&mut s, Id::Shelf(ShelfId::Up));
        assert_eq!(fx, vec![Effect::Browse(None)], "out of the folders: no watch");
    }

    #[test]
    fn listings_outlive_the_library_read_again() {
        let mut s = files_state();
        tap(&mut s, Id::Shelf(ShelfId::Launch));
        listed(&mut s, "browse:Home", vec![entry("browse:Home/Films", true, BrowseKind::Plain)]);
        tap(&mut s, Id::Shelf(ShelfId::Launch));
        listed(&mut s, "browse:Home/Films", vec![entry("browse:Home/Films/x.mkv", false, BrowseKind::Plain)]);
        let library = files_state().shelf().library().clone();
        let (_, fx) = s.apply(Event::Library(Box::new(library)));
        assert_eq!(s.shelf().selected().map(|g| &**g), Some("browse:Home/Films/x.mkv"), "still in Films");
        assert!(!fx.iter().any(|e| matches!(e, Effect::Browse(_))), "the same folder: watched still");
    }

    #[test]
    fn copy_then_paste_into_the_folder_open() {
        let mut s = files_state();
        tap(&mut s, Id::Shelf(ShelfId::Launch));
        listed(&mut s, "browse:Home", vec![entry("browse:Home/Films", true, BrowseKind::Plain), entry("browse:Home/a.txt", false, BrowseKind::Plain)]);
        s.apply(ShelfEvent::ListStep(1).into());
        assert_eq!(s.file_actions(), vec![FileAction::Copy, FileAction::Cut, FileAction::Rename, FileAction::Trash, FileAction::NewFolder]);
        // Y on a browsed entry: Copy; X: Paste.
        s.apply(ShelfEvent::ToggleFavourite.into());
        assert_eq!(s.files().clip.as_ref().map(|c| c.name.as_str()), Some("a.txt"));
        assert!(s.file_actions().contains(&FileAction::Paste));
        s.apply(ShelfEvent::ListStep(-1).into());
        tap(&mut s, Id::Shelf(ShelfId::Launch));
        assert_eq!(s.file_target().map(|(g, selected)| (&*g.id, selected)), Some(("browse:Home/Films", false)), "empty: the folder's actions");
        let paste = vec![Effect::Files(FileOp::Paste { path: "/Home/a.txt".into(), into: "/Home/Films".into(), cut: false })];
        assert_eq!(s.apply(ShelfEvent::TogglePin.into()).1, paste);
        assert_eq!(tap(&mut s, Id::Shelf(ShelfId::File(FileAction::Paste))), paste);
        assert!(s.files().clip.is_some(), "a copy pastes again");
    }

    #[test]
    fn deleting_asks_first() {
        let mut s = files_state();
        s.apply(ShelfEvent::ListStep(1).into());
        tap(&mut s, Id::Shelf(ShelfId::Launch));
        listed(&mut s, "browse:Trash", vec![entry("browse:Trash/old", false, BrowseKind::Plain)]);
        assert_eq!(s.file_actions(), vec![FileAction::Restore, FileAction::Delete], "the Trash: nothing pasted or made in it");
        assert!(tap(&mut s, Id::Shelf(ShelfId::File(FileAction::Delete))).is_empty());
        assert!(s.file_armed(FileAction::Delete));
        let fx = tap(&mut s, Id::Shelf(ShelfId::File(FileAction::Delete)));
        assert_eq!(fx, vec![Effect::Files(FileOp::Delete("/Trash/old".into()))]);
        assert!(!s.file_armed(FileAction::Delete));
    }

    #[test]
    fn renaming_types_on_the_keyboard_and_goes_back() {
        let mut s = files_state();
        tap(&mut s, Id::Shelf(ShelfId::Launch));
        listed(&mut s, "browse:Home", vec![entry("browse:Home/a.txt", false, BrowseKind::Plain)]);
        let before = s.bottom().tab();
        tap(&mut s, Id::Shelf(ShelfId::File(FileAction::Rename)));
        assert_eq!(s.bottom().tab(), Tab::Keyboard);
        assert_eq!(s.bottom().editor().map(|e| e.text.as_str()), Some("a.txt"));
        let fx = tap(&mut s, Id::Editor(crate::EditorId::Save));
        assert_eq!(s.bottom().tab(), before);
        assert!(fx.contains(&Effect::Files(FileOp::Rename { path: "/Home/a.txt".into(), name: "a.txt".into() })), "{fx:?}");
    }
}
