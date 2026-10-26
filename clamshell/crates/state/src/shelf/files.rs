//! Shelf's file actions on browsed entries (see [`crate::Library::set_listing`]). The open folder is
//! listed and watched while it is open. The selected entry can be renamed, copied or cut and then pasted
//! into the open folder, or moved to the Trash; in the Trash it can be restored or deleted. A new folder
//! can be made, and a removable filesystem ejected.
//!
//! What is done on disk goes out as [`FileOp`]s, and the listings come back as they change. Deleting and
//! emptying the Trash ask first: the action has to be pressed again.

use std::collections::VecDeque;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use clamshell_library::BrowseKind;

use crate::bottom::editor::{Editor, Purpose};
use crate::bottom::notice::{NoticeKind, NoticeSlot};
use crate::effect::Effect;
use crate::out::Out;
use crate::shelf::Shelf;
use crate::shelf::actions::{Action, Actions};
use crate::{Entry, EntryAct, Event, Pointer, Running};

/// What can be done to a browsed entry, or in the open folder.
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
    /// Adds the selected entry to, or removes it from, the selection that the file actions act on while
    /// it isn't empty.
    Select,
    /// Pastes into the open folder.
    Paste,
    NewFolder,
    /// Shows or hides hidden files (`.name`).
    ShowHidden,
    /// Extracts an archive's entries into a folder beside it, whose name is typed.
    Extract,
    /// Opens an archive, such as a ROM's zip, with its runner instead of browsing it.
    OpenIn,
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
            FileAction::Select => "Select",
            FileAction::Paste => "Paste",
            FileAction::NewFolder => "New folder",
            FileAction::ShowHidden => "Hidden files",
            FileAction::Extract => "Extract",
            FileAction::OpenIn => "Open in",
        }
    }

    /// Whether it can't be undone, so it asks first by having to be pressed again.
    pub fn confirms(self) -> bool {
        matches!(self, FileAction::Delete | FileAction::EmptyTrash)
    }
}

/// What is done on disk, by the files thread.
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
    /// Copies, or moves if `cut`, into `into`. A name already there gets " (2)"; nothing is replaced.
    Paste {
        path: PathBuf,
        into: PathBuf,
        cut: bool,
    },
    Trash(PathBuf),
    /// Moves an entry from the Trash's `files` back to where it was.
    Restore(PathBuf),
    Delete(PathBuf),
    /// Deletes everything in the Trash, given its `files` dir.
    EmptyTrash(PathBuf),
    /// Unmounts a removable filesystem's device.
    Eject(String),
    /// Extracts archive `archive`'s entries into a new folder `name` beside it, adding " (2)" if the name
    /// is taken.
    Extract {
        archive: PathBuf,
        name: String,
    },
}

/// What the browse thread is told: either the folder to list and watch, with none to stop, or the files
/// in use. The files in use are the open folder's, those copied, and those that running launches opened.
/// They keep the archives they are in mounted.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BrowseCommand {
    Open(Option<Browse>),
    InUse(Vec<PathBuf>),
}

/// A folder to list and watch. `folder` is its entry's id; the ids of the entries in it are that id, a
/// `/` and their names. `dir` is its directory, which for an archive is the archive itself, mounted to be
/// listed. `hidden` says whether hidden files (`.name`) are listed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Browse {
    pub folder: Arc<str>,
    pub dir: PathBuf,
    pub hidden: bool,
}

/// What the files thread says.
#[derive(Debug, Clone, PartialEq)]
pub enum FilesEvent {
    /// Folder `folder`'s current entries, in order (see [`crate::Library::set_listing`]).
    Listed { folder: Arc<str>, entries: Vec<Entry> },
    /// A paste under way: what it is doing, such as `Copying Film.mkv`, and how far through its bytes it
    /// is.
    Busy { what: String, fraction: f32 },
    /// An operation ended: what was done, or what went wrong. Each operation sent ends in one, in the
    /// order they were sent.
    Done(Result<String, String>),
    /// Something went wrong outside an operation, such as an archive that couldn't be mounted to be
    /// listed.
    Failed(String),
    /// A removable filesystem was mounted or unmounted.
    Mounts,
}

/// The file actions' state.
#[derive(Debug, Clone, Default)]
pub struct Files {
    /// The entries copied or cut.
    clip: Option<Clip>,
    /// The entries selected in the open folder.
    selected: Vec<Arc<str>>,
    /// An action waiting to be confirmed by being pressed again.
    armed: Option<Armed>,
    /// The progress of the operation under way, while any runs.
    busy: Option<Busy>,
    /// The operations sent and not yet done, oldest first: for a cut entry's paste, the entry's file,
    /// which leaves the clip once it is moved.
    running: VecDeque<Option<PathBuf>>,
    /// Whether hidden files show.
    hidden: bool,
    /// The folder the selection is in.
    folder: Option<Arc<str>>,
}

impl Files {
    /// The entries copied or cut.
    pub fn clip(&self) -> Option<&Clip> {
        self.clip.as_ref()
    }

    /// The entries selected in the open folder.
    pub fn selected(&self) -> &[Arc<str>] {
        &self.selected
    }

    /// The progress of an operation under way.
    pub fn busy(&self) -> Option<&Busy> {
        self.busy.as_ref()
    }

    /// Whether hidden files show.
    pub fn hidden(&self) -> bool {
        self.hidden
    }
}

/// An action waiting to be confirmed, on an entry.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Armed {
    action: FileAction,
    entry: Arc<str>,
}

/// An operation under way: what it says, and how far it is (0-1).
#[derive(Debug, Clone, PartialEq)]
pub struct Busy {
    pub what: String,
    pub fraction: f32,
}

/// Entries copied or cut, to paste: their files and names.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Clip {
    pub entries: Vec<(PathBuf, String)>,
    pub cut: bool,
}

/// What a file's name is edited for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FileName {
    Rename(PathBuf),
    NewFolder(PathBuf),
    /// The folder an archive's entries go into, beside it.
    Extract(PathBuf),
}

/// An archive's name without its extensions, such as `Game` for `Game.tar.gz`. It names the folder the
/// archive is extracted into.
pub(crate) fn extracted_name(name: &str) -> &str {
    let lower = name.to_lowercase();
    let two = [".tar.gz", ".tar.xz", ".tar.zst", ".tar.bz2", ".tar.lz4"];
    match two.iter().find(|e| lower.ends_with(*e)) {
        Some(e) => name.get(..name.len() - e.len()).unwrap_or(name),
        None => name.rsplit_once('.').map_or(name, |(stem, _)| if stem.is_empty() { name } else { stem }),
    }
}

/// The operation for a file's name typed for `purpose`, unless the name can't be one.
pub(crate) fn named(purpose: FileName, name: &str) -> Option<FileOp> {
    let name = name.trim();
    if name.is_empty() || name.contains('/') || name == "." || name == ".." {
        return None;
    }
    Some(match purpose {
        FileName::Rename(path) => FileOp::Rename { path, name: name.to_owned() },
        FileName::NewFolder(dir) => FileOp::NewFolder { dir, name: name.to_owned() },
        FileName::Extract(archive) => FileOp::Extract { archive, name: name.to_owned() },
    })
}

impl Shelf {
    pub fn files(&self) -> &Files {
        &self.files
    }

    /// Clears the entries selected in the open folder, once something used them up.
    pub(crate) fn clear_file_selection(&mut self) {
        self.files.selected.clear();
    }

    /// What the details show the actions of, and whether it is the selected entry. It is the selected
    /// entry, or the open folder when there is none, as in an empty folder.
    pub fn file_target(&self) -> Option<(&Entry, bool)> {
        match self.selected_entry() {
            Some(g) => Some((g, true)),
            None => self.group().map(|g| (g, false)),
        }
    }

    /// The open folder, if it is browsed and things may be put in it. The Trash and locked folders, such
    /// as an archive's, are left out.
    fn open_dir(&self) -> Option<PathBuf> {
        let g = self.group()?;
        let b = g.browsed.as_ref()?;
        (!matches!(b.kind, BrowseKind::Trash | BrowseKind::Trashed) && !b.locked).then(|| g.browsed_dir().map(PathBuf::from)).flatten()
    }

    /// The runner that archive `entry` opens in, for Open in. It is the runner of its library entry, as
    /// for a ROM's zip that a scan found, or else the one its name matches.
    pub fn opened_in<'a>(&'a self, entry: &'a Entry) -> Option<&'a clamshell_library::RunnerOf> {
        let of_file = self.entry_of_file(&entry.id).and_then(|id| self.entry(&id));
        of_file.and_then(|e| e.runner.as_ref()).or(entry.runner.as_ref())
    }

    /// Whether `entry` is in a folder where nothing can be changed, such as an archive's.
    fn in_locked(&self, entry: &Entry) -> bool {
        let parent = entry.parent.and_then(|p| self.library().entries.get(p));
        parent.and_then(|p| p.browsed.as_ref()).is_some_and(|b| b.locked)
    }

    /// The file actions of what the details show (see [`Shelf::file_target`]), given by its place. They
    /// are its own actions if it is the selected entry, then the open folder's: Paste when something is
    /// copied, New folder, and Hidden files.
    pub(crate) fn file_actions(&self) -> Actions {
        use FileAction::*;
        let mut actions = Actions::default();
        let Some((entry, selected)) = self.file_target() else {
            return actions;
        };
        let Some(b) = entry.browsed.as_ref() else {
            return actions;
        };
        let locked = self.in_locked(entry);
        let own: &[FileAction] = match (&b.kind, selected) {
            (_, false) | (BrowseKind::Root, true) => &[],
            // Inside an archive, entries can only be copied out.
            (BrowseKind::Plain, true) if locked => &[Copy, Select],
            (BrowseKind::Plain, true) if b.archive => &[Copy, Cut, Extract, Rename, Trash, Select],
            (BrowseKind::Plain, true) => &[Copy, Cut, Rename, Trash, Select],
            (BrowseKind::Trashed, true) => &[Restore, Delete, Select],
            (BrowseKind::Removable(_), true) => &[Eject],
            (BrowseKind::Trash, true) => &[EmptyTrash],
        };
        // An archive that a runner opens, such as a ROM's zip.
        if selected && b.archive && self.opened_in(entry).is_some() {
            actions.push(Action::File(OpenIn));
        }
        for &a in own {
            actions.push(Action::File(a));
        }
        if self.open_dir().is_some() {
            if self.files.clip.is_some() {
                actions.push(Action::File(Paste));
            }
            actions.push(Action::File(NewFolder));
        }
        if self.group().is_some_and(|g| g.browsed_dir().is_some()) {
            actions.push(Action::File(ShowHidden));
        }
        actions
    }

    /// Adds what a file action acts on to `targets`, as files and names: the selected entries while there
    /// are any, or else the one the details show.
    fn file_targets(&self, targets: &mut Vec<(PathBuf, String)>) {
        let entry = |g: &Entry| Some((g.file.clone()?, g.title.clone()));
        if self.files.selected.is_empty() {
            targets.extend(self.file_target().and_then(|(g, _)| entry(g)));
        } else {
            targets.extend(self.files.selected.iter().filter_map(|id| self.entry(id)).filter_map(entry));
        }
    }

    /// Whether `action` was pressed once on what the details show and waits to be confirmed.
    pub fn file_armed(&self, action: FileAction) -> bool {
        self.files.armed.as_ref().is_some_and(|a| a.action == action && self.file_target().is_some_and(|(g, _)| g.id == a.entry))
    }

    /// A file action pressed, if what the details show has it.
    pub(crate) fn on_file(&mut self, action: FileAction, out: &mut Out) {
        if !self.file_actions().contains(&Action::File(action)) {
            return;
        }
        let Some((entry, _)) = self.file_target() else {
            return;
        };
        let (id, path, kind) = (entry.id.clone(), entry.file.clone(), entry.browsed.as_ref().map(|b| b.kind.clone()));
        let name = entry.title.clone();
        if action.confirms() && !self.file_armed(action) {
            self.files.armed = Some(Armed { action, entry: id });
            return;
        }
        self.files.armed = None;
        // The selected entries, or else the entry, each get an operation of their own, and the selection
        // is used up.
        let mut targets = Vec::new();
        self.file_targets(&mut targets);
        let op = match (action, path) {
            (FileAction::Select, _) => {
                match self.files.selected.iter().position(|s| *s == id) {
                    Some(i) => {
                        self.files.selected.remove(i);
                    }
                    None => self.files.selected.push(id),
                }
                return;
            }
            (FileAction::ShowHidden, _) => {
                self.files.hidden = !self.files.hidden;
                return;
            }
            (FileAction::Paste, _) => {
                // A cut entry stays in the clip until it is moved.
                let (Some(clip), Some(into)) = (self.files.clip.clone(), self.open_dir()) else {
                    return;
                };
                for (path, _) in clip.entries {
                    self.run_file_op(FileOp::Paste { path, into: into.clone(), cut: clip.cut }, out);
                }
                return;
            }
            // With nothing to copy, the clip stays as it was.
            (FileAction::Copy | FileAction::Cut, _) if targets.is_empty() => return,
            (FileAction::Copy | FileAction::Cut, _) => {
                self.files.clip = Some(Clip { entries: targets, cut: action == FileAction::Cut });
                self.files.selected.clear();
                return;
            }
            (FileAction::Trash | FileAction::Restore | FileAction::Delete, _) => {
                self.files.selected.clear();
                let op = match action {
                    FileAction::Trash => FileOp::Trash,
                    FileAction::Restore => FileOp::Restore,
                    _ => FileOp::Delete,
                };
                for (path, _) in targets {
                    self.run_file_op(op(path), out);
                }
                return;
            }
            (FileAction::NewFolder, _) => {
                if let Some(dir) = self.open_dir() {
                    name_file(FileName::NewFolder(dir), String::new(), out);
                }
                return;
            }
            (FileAction::OpenIn, _) => return out.events.push(Event::Entry(id, EntryAct::Play)),
            (FileAction::Extract, Some(path)) => return name_file(FileName::Extract(path), extracted_name(&name).to_owned(), out),
            (FileAction::Eject, _) => match kind {
                Some(BrowseKind::Removable(device)) => FileOp::Eject(device),
                _ => return,
            },
            (_, None) => return,
            (FileAction::Rename, Some(path)) => return name_file(FileName::Rename(path), name, out),
            (FileAction::EmptyTrash, Some(path)) => FileOp::EmptyTrash(path),
        };
        self.run_file_op(op, out);
    }

    /// Sends `op` to the files thread, and counts it as running until it is done.
    pub(crate) fn run_file_op(&mut self, op: FileOp, out: &mut Out) {
        let cut = match &op {
            FileOp::Paste { path, cut: true, .. } => Some(path.clone()),
            _ => None,
        };
        self.files.running.push_back(cut);
        out.effects.push(Effect::Files(op));
    }

    /// What the files thread said. A listing drops the selected entries that are gone. A notice says how
    /// an operation ended.
    /// The files thread was started again after a panic: the operation it was carrying out, the oldest
    /// sent, ends as failed, since it may be half done. Those after it wait in its channel still.
    pub(crate) fn files_interrupted(&mut self, pointer: &mut Pointer, notice: &mut NoticeSlot, now: u64, out: &mut Out) {
        if !self.files.running.is_empty() {
            let failed = FilesEvent::Done(Err("Stopped partway: the files thread failed".into()));
            self.on_files(failed, pointer, notice, now, out);
        }
    }

    pub(crate) fn on_files(&mut self, event: FilesEvent, pointer: &mut Pointer, notice: &mut NoticeSlot, now: u64, out: &mut Out) {
        match event {
            FilesEvent::Listed { folder, entries } => {
                if self.listed(folder, entries) {
                    self.moved(pointer);
                }
                let library = &self.library;
                self.files.selected.retain(|id| library.entry(id).is_some());
            }
            FilesEvent::Busy { what, fraction } => self.files.busy = Some(Busy { what, fraction }),
            FilesEvent::Done(result) => {
                // The oldest operation ended. A cut entry that was moved leaves the clip.
                if let (Some(Some(moved)), Ok(_)) = (self.files.running.pop_front(), &result)
                    && let Some(clip) = self.files.clip.as_mut().filter(|c| c.cut)
                {
                    clip.entries.retain(|(path, _)| *path != moved);
                    if clip.entries.is_empty() {
                        self.files.clip = None;
                    }
                }
                if self.files.running.is_empty() {
                    self.files.busy = None;
                }
                notice.show(NoticeKind::Files(result), now);
            }
            FilesEvent::Failed(e) => notice.show(NoticeKind::Files(Err(e)), now),
            // Removable storage's groups, and sources on it, come and go when the library is read again.
            FilesEvent::Mounts => out.effects.push(Effect::Tool(clamshell_library::Tool::Rescan)),
        }
    }

    /// Runs after every event. An armed action is dropped once the details show something else, and the
    /// selection is dropped when another folder opens, since it belonged to the last one.
    pub(crate) fn follow_folder(&mut self) {
        if let Some(armed) = &self.files.armed
            && self.file_target().is_none_or(|(g, _)| g.id != armed.entry)
        {
            self.files.armed = None;
        }
        let open = self.browsed().map(|(folder, _)| folder);
        if self.files.folder.as_ref() != open {
            self.files.folder = open.cloned();
            self.files.selected.clear();
        }
    }

    /// The open browsed folder and its directory, which is listed and watched while it is open (see
    /// [`crate::sync::Sent::browse`]).
    pub(crate) fn browsed(&self) -> Option<(&Arc<str>, &Path)> {
        self.group().and_then(|g| Some((&g.id, g.browsed_dir()?)))
    }

    /// The files in use: the open folder's, what is copied to be pasted, and what `running` launches
    /// opened. The archives they are in stay mounted (see [`crate::sync::Sent::browse`]).
    pub(crate) fn in_use<'a>(&'a self, running: &'a [Running]) -> impl Iterator<Item = &'a Path> + Clone {
        let open = self.browsed().map(|(_, dir)| dir);
        let copied = self.files.clip.iter().flat_map(|c| c.entries.iter().map(|(p, _)| p.as_path()));
        let running = running.iter().filter_map(|r| r.entry.as_ref()?.file.as_deref());
        open.into_iter().chain(copied).chain(running)
    }
}

/// Asks for the editor on the keyboard tab for a file's name, starting from `text`.
fn name_file(purpose: FileName, text: String, out: &mut Out) {
    out.events.push(Event::EditOnKeyboard(Editor { purpose: Purpose::FileName(purpose), text }));
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::indexing_slicing)]
mod tests;
