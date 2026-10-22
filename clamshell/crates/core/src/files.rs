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
use crate::notice::NoticeKind;
use crate::{Entry, State, Tab};

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
    /// The selected entry in or out of the selection the file actions act on (while it isn't empty).
    Select,
    /// Into the folder open.
    Paste,
    NewFolder,
    /// Hidden files (`.name`) shown, or not.
    ShowHidden,
    /// An archive's entries extracted into a folder beside it (its name typed).
    Extract,
    /// An archive opened by the runner its name matches (a ROM's zip), not browsed.
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
    /// Archive `archive`'s entries extracted into a new folder `name` beside it (" (2)" if taken).
    Extract {
        archive: PathBuf,
        name: String,
    },
}

/// What the browse thread is told: the folder to list and watch (none: no more), or the files in use
/// (the folder open's, those copied, those running launches opened), which keep the archives they are
/// in mounted.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BrowseCommand {
    Open(Option<Browse>),
    InUse(Vec<PathBuf>),
}

/// A folder to list and watch: its entry's id (the entries' ids are it, `/`, their names), its dir (an
/// archive's: the archive, mounted to be listed), and whether hidden files (`.name`) are listed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Browse {
    pub folder: Arc<str>,
    pub dir: PathBuf,
    pub hidden: bool,
}

/// What the files thread says.
#[derive(Debug, Clone, PartialEq)]
pub enum FilesEvent {
    /// Folder `folder`'s entries now, in order (see [`crate::Library::set_listing`]).
    Listed { folder: Arc<str>, entries: Vec<Entry> },
    /// A paste under way: what (`Copying Film.mkv`), how far (of its bytes).
    Busy { what: String, fraction: f32 },
    /// An operation ended: what was done, or what went wrong.
    Done(Result<String, String>),
    /// A removable filesystem was mounted or unmounted.
    Mounts,
}

/// The file actions' state: the entries copied or cut, those selected (in the folder open), the action
/// waiting to be confirmed (on the entry), the paste under way, whether hidden files show, the folder
/// listed and watched (and with them or not).
#[derive(Debug, Clone, Default)]
pub struct Files {
    pub clip: Option<Clip>,
    pub selected: Vec<Arc<str>>,
    pub armed: Option<(FileAction, Arc<str>)>,
    pub busy: Option<(String, f32)>,
    pub hidden: bool,
    pub(crate) browsing: Option<(Arc<str>, bool)>,
    /// The files in use last told (see [`BrowseCommand::InUse`]).
    pub(crate) in_use: Vec<PathBuf>,
}

/// Entries copied (or cut), to paste: their files and names.
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

/// An archive's name without its extensions (`Game.tar.gz`: `Game`): the folder it is extracted into.
pub fn extracted_name(name: &str) -> &str {
    let lower = name.to_lowercase();
    let two = [".tar.gz", ".tar.xz", ".tar.zst", ".tar.bz2", ".tar.lz4"];
    match two.iter().find(|e| lower.ends_with(*e)) {
        Some(e) => name.get(..name.len() - e.len()).unwrap_or(name),
        None => name.rsplit_once('.').map_or(name, |(stem, _)| if stem.is_empty() { name } else { stem }),
    }
}

impl State {
    pub fn files(&self) -> &Files {
        &self.files
    }

    /// What the details show the actions of: the selected entry, or with none (an empty folder) the
    /// folder open; and whether it is the selected one.
    pub fn file_target(&self) -> Option<(&Entry, bool)> {
        match self.shelf.selected_game() {
            Some(g) => Some((g, true)),
            None => self.shelf.group().map(|g| (g, false)),
        }
    }

    /// The folder open, if it is browsed and things may be put in it (not the Trash, nor one that is
    /// locked: an archive's).
    fn open_dir(&self) -> Option<PathBuf> {
        let g = self.shelf.group()?;
        let b = g.browsed.as_ref()?;
        (!matches!(b.kind, BrowseKind::Trash | BrowseKind::Trashed) && !b.locked).then(|| g.browsed_dir().map(PathBuf::from)).flatten()
    }

    /// The runner archive `game` opens in (Open in): its library entry's (a ROM's zip a scan has), else
    /// the one its name matches.
    pub fn opened_in<'a>(&'a self, game: &'a Entry) -> Option<&'a clamshell_library::RunnerOf> {
        let entry = self.shelf.entry_of_file(&game.id).and_then(|id| self.shelf.game(&id));
        entry.and_then(|e| e.runner.as_ref()).or(game.runner.as_ref())
    }

    /// Whether entry `game` is in a folder nothing can be changed in (an archive's).
    fn in_locked(&self, game: &Entry) -> bool {
        let parent = game.parent.and_then(|p| self.shelf.library().entries.get(p));
        parent.and_then(|p| p.browsed.as_ref()).is_some_and(|b| b.locked)
    }

    /// The file actions of what the details show (see [`State::file_target`]), from its place: its own if
    /// it is the selected entry, then the folder open's (Paste with something copied, New folder).
    pub fn file_actions(&self) -> crate::actions::Actions {
        use FileAction::*;
        let mut actions = crate::actions::Actions::default();
        let Some((game, selected)) = self.file_target() else {
            return actions;
        };
        let Some(b) = game.browsed.as_ref() else {
            return actions;
        };
        let locked = self.in_locked(game);
        let own: &[FileAction] = match (&b.kind, selected) {
            (_, false) | (BrowseKind::Root, true) => &[],
            // (In an archive: only copied out.)
            (BrowseKind::Plain, true) if locked => &[Copy, Select],
            (BrowseKind::Plain, true) if b.archive => &[Copy, Cut, Extract, Rename, Trash, Select],
            (BrowseKind::Plain, true) => &[Copy, Cut, Rename, Trash, Select],
            (BrowseKind::Trashed, true) => &[Restore, Delete, Select],
            (BrowseKind::Removable(_), true) => &[Eject],
            (BrowseKind::Trash, true) => &[EmptyTrash],
        };
        // (An archive a runner opens: a ROM's zip.)
        if selected && b.archive && self.opened_in(game).is_some() {
            actions.push(crate::actions::Action::File(OpenIn));
        }
        for &a in own {
            actions.push(crate::actions::Action::File(a));
        }
        if self.open_dir().is_some() {
            if self.files.clip.is_some() {
                actions.push(crate::actions::Action::File(Paste));
            }
            actions.push(crate::actions::Action::File(NewFolder));
        }
        if self.shelf.group().is_some_and(|g| g.browsed_dir().is_some()) {
            actions.push(crate::actions::Action::File(ShowHidden));
        }
        actions
    }

    /// What a file action acts on: the entries selected while there are any, else the one the details
    /// show; their files and names.
    fn file_targets(&self) -> Vec<(PathBuf, String)> {
        let entry = |g: &Entry| Some((g.file.clone()?, g.title.clone()));
        if self.files.selected.is_empty() {
            return self.file_target().and_then(|(g, _)| entry(g)).into_iter().collect();
        }
        self.files.selected.iter().filter_map(|id| self.shelf.game(id)).filter_map(entry).collect()
    }

    /// Whether `action` waits to be confirmed (pressed once) on what the details show.
    pub fn file_armed(&self, action: FileAction) -> bool {
        self.files.armed.as_ref().is_some_and(|(a, id)| *a == action && self.file_target().is_some_and(|(g, _)| g.id == *id))
    }

    /// A file action pressed (if what the details show has it).
    pub(crate) fn on_file(&mut self, action: FileAction) -> Vec<Effect> {
        if !self.file_actions().contains(&crate::actions::Action::File(action)) {
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
        // (Those selected, or the entry: each its own operation; the selection used up.)
        let targets = self.file_targets();
        let each = |op: fn(PathBuf) -> FileOp| targets.iter().map(|(path, _)| Effect::Files(op(path.clone()))).collect::<Vec<_>>();
        let op = match (action, path) {
            (FileAction::Select, _) => {
                match self.files.selected.iter().position(|s| *s == id) {
                    Some(i) => {
                        self.files.selected.remove(i);
                    }
                    None => self.files.selected.push(id),
                }
                return vec![];
            }
            (FileAction::ShowHidden, _) => {
                self.files.hidden = !self.files.hidden;
                return vec![];
            }
            (FileAction::Paste, _) => {
                let (Some(clip), Some(into)) = (self.files.clip.clone(), self.open_dir()) else {
                    return vec![];
                };
                if clip.cut {
                    self.files.clip = None;
                }
                return clip.entries.into_iter().map(|(path, _)| Effect::Files(FileOp::Paste { path, into: into.clone(), cut: clip.cut })).collect();
            }
            (FileAction::Copy | FileAction::Cut, _) => {
                self.files.clip = Some(Clip { entries: targets, cut: action == FileAction::Cut });
                self.files.selected.clear();
                return vec![];
            }
            (FileAction::Trash | FileAction::Restore | FileAction::Delete, _) => {
                self.files.selected.clear();
                return each(match action {
                    FileAction::Trash => FileOp::Trash,
                    FileAction::Restore => FileOp::Restore,
                    _ => FileOp::Delete,
                });
            }
            (FileAction::NewFolder, _) => {
                let Some(dir) = self.open_dir() else {
                    return vec![];
                };
                return self.name_file(FileName::NewFolder(dir), String::new());
            }
            (FileAction::OpenIn, _) => return self.play(id),
            (FileAction::Extract, Some(path)) => {
                let name = extracted_name(&name).to_owned();
                return self.name_file(FileName::Extract(path), name);
            }
            (FileAction::Eject, _) => match kind {
                Some(BrowseKind::Removable(device)) => FileOp::Eject(device),
                _ => return vec![],
            },
            (_, None) => return vec![],
            (FileAction::Rename, Some(path)) => return self.name_file(FileName::Rename(path), name),
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
            FileName::Extract(archive) => FileOp::Extract { archive, name: name.to_owned() },
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
            FilesEvent::Mounts => vec![Effect::Tool(clamshell_library::Tool::Rescan)],
        }
    }

    /// After every event: the browsed folder open listed and watched when it became the open one (none:
    /// the watch dropped); the files in use told when they change; an armed action dropped once the
    /// details show something else.
    pub(crate) fn follow_browse(&mut self) -> Vec<Effect> {
        let mut effects: Vec<Effect> = self.follow_open_folder().into_iter().collect();
        // (The folder open's, what is copied (to be pasted), and what running launches opened: the
        // archives they are in stay mounted.)
        let open = self.shelf.group().and_then(|g| g.browsed_dir()).map(PathBuf::from);
        let copied = self.files.clip.iter().flat_map(|c| c.entries.iter().map(|(p, _)| p.clone()));
        let running = self.session.running.iter().filter_map(|r| r.entry.as_ref()?.file.clone());
        let in_use: Vec<PathBuf> = open.into_iter().chain(copied).chain(running).collect();
        if in_use != self.files.in_use {
            self.files.in_use = in_use.clone();
            effects.push(Effect::Browse(BrowseCommand::InUse(in_use)));
        }
        effects
    }

    fn follow_open_folder(&mut self) -> Option<Effect> {
        if let Some((_, id)) = &self.files.armed
            && self.file_target().is_none_or(|(g, _)| g.id != *id)
        {
            self.files.armed = None;
        }
        let hidden = self.files.hidden;
        let open = self.shelf.group().and_then(|g| Some(Browse { folder: g.id.clone(), dir: g.browsed_dir()?.to_path_buf(), hidden }));
        let now = open.as_ref().map(|b| (b.folder.clone(), hidden));
        if self.files.browsing == now {
            return None;
        }
        // (Another folder: the selection was the last one's.)
        if self.files.browsing.as_ref().map(|(f, _)| f) != now.as_ref().map(|(f, _)| f) {
            self.files.selected.clear();
        }
        self.files.browsing = now;
        Some(Effect::Browse(BrowseCommand::Open(open)))
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::indexing_slicing)]
mod tests {
    use super::*;
    use crate::testkit::{state_with, tap};
    use crate::{Browsed, Collection, Event, Id, Library, ShelfEvent, ShelfId};

    fn entry(id: &str, folder: bool, kind: BrowseKind) -> Entry {
        let name = id.rsplit('/').next().unwrap();
        let path = format!("/{}", id.strip_prefix("browse:").unwrap());
        Entry { file: Some(path.into()), browsed: Some(Browsed { folder, kind, ..Browsed::default() }), ..Entry::new(id, name) }
    }

    /// A Files tab: Home and the Trash.
    fn files_state() -> State {
        let mut trash = entry("browse:Trash", true, BrowseKind::Trash);
        trash.file = Some("/t/files".into());
        let games = vec![entry("browse:Home", true, BrowseKind::Root), trash];
        state_with(Library { collections: vec![Collection::new("Files", vec![0, 1])], entries: games, ..Library::default() })
    }

    /// The folder listed and watched, of `fx`.
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
        let acts = s.file_actions();
        for a in [FileAction::OpenIn, FileAction::Extract, FileAction::Rename, FileAction::Trash] {
            assert!(acts.contains(&crate::actions::Action::File(a)), "{a:?}: {acts:?}");
        }
        // Extract: its name typed, without the extensions.
        tap(&mut s, Id::Shelf(ShelfId::File(FileAction::Extract)));
        assert_eq!(s.bottom().editor().map(|e| e.text.as_str()), Some("game"));
        let fx = tap(&mut s, Id::Editor(crate::EditorId::Save));
        assert!(fx.contains(&Effect::Files(FileOp::Extract { archive: "/Home/game.tar.gz".into(), name: "game".into() })), "{fx:?}");
        // Opened: browsed as a folder (the archive itself the dir), and in use while open.
        let fx = tap(&mut s, Id::Shelf(ShelfId::Launch));
        assert!(fx.contains(&Effect::Browse(BrowseCommand::InUse(vec!["/Home/game.tar.gz".into()]))), "{fx:?}");
        listed(&mut s, "browse:Home/game.tar.gz", vec![entry("browse:Home/game.tar.gz/a.sfc", false, BrowseKind::Plain)]);
        // Inside: only copied out; nothing pasted in or made.
        let acts = s.file_actions();
        assert!(acts.contains(&crate::actions::Action::File(FileAction::Copy)));
        for a in [FileAction::Cut, FileAction::Rename, FileAction::Trash, FileAction::NewFolder] {
            assert!(!acts.contains(&crate::actions::Action::File(a)), "{a:?}");
        }
        // Left: no longer in use.
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
        s.apply(Event::Files(FilesEvent::Listed { folder: folder.into(), entries }));
    }

    #[test]
    fn the_folder_open_is_listed_and_watched_while_open() {
        let mut s = files_state();
        let fx = browses(s.apply(Event::Pointer(crate::PointerEvent::Activate(Id::Shelf(ShelfId::Launch)))).1);
        assert_eq!(fx, vec![Effect::Browse(BrowseCommand::Open(Some(Browse { folder: "browse:Home".into(), dir: "/Home".into(), hidden: false })))]);
        assert!(s.shelf().list().is_empty(), "nothing in it until listed");
        listed(&mut s, "browse:Home", vec![entry("browse:Home/Films", true, BrowseKind::Plain), entry("browse:Home/a.txt", false, BrowseKind::Plain)]);
        assert_eq!(s.shelf().list().len(), 2);
        assert_eq!(s.shelf().selected().map(|g| &**g), Some("browse:Home/Films"));
        // Into Films (listed and watched instead), back out (Home again).
        let fx = browses(tap(&mut s, Id::Shelf(ShelfId::Launch)));
        assert_eq!(
            fx,
            vec![Effect::Browse(BrowseCommand::Open(Some(Browse { folder: "browse:Home/Films".into(), dir: "/Home/Films".into(), hidden: false })))]
        );
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
        let (_, fx) = s.apply(Event::Library(Box::new(library)));
        assert_eq!(s.shelf().selected().map(|g| &**g), Some("browse:Home/Films/x.mkv"), "still in Films");
        assert!(!fx.iter().any(|e| matches!(e, Effect::Browse(BrowseCommand::Open(_)))), "the same folder: watched still");
    }

    #[test]
    fn copy_then_paste_into_the_folder_open() {
        let mut s = files_state();
        tap(&mut s, Id::Shelf(ShelfId::Launch));
        listed(&mut s, "browse:Home", vec![entry("browse:Home/Films", true, BrowseKind::Plain), entry("browse:Home/a.txt", false, BrowseKind::Plain)]);
        s.apply(ShelfEvent::ListStep(1).into());
        let file = |a: &[FileAction]| a.iter().map(|&a| crate::actions::Action::File(a)).collect::<Vec<_>>();
        assert_eq!(
            s.file_actions().as_slice(),
            file(&[
                FileAction::Copy,
                FileAction::Cut,
                FileAction::Rename,
                FileAction::Trash,
                FileAction::Select,
                FileAction::NewFolder,
                FileAction::ShowHidden
            ])
        );
        // Y on a browsed entry: Copy; X: Paste.
        s.apply(ShelfEvent::ToggleFavourite.into());
        assert_eq!(s.files().clip.as_ref().map(|c| c.entries.iter().map(|(_, n)| n.as_str()).collect::<Vec<_>>()), Some(vec!["a.txt"]));
        assert!(s.file_actions().contains(&crate::actions::Action::File(FileAction::Paste)));
        s.apply(ShelfEvent::ListStep(-1).into());
        tap(&mut s, Id::Shelf(ShelfId::Launch));
        assert_eq!(s.file_target().map(|(g, selected)| (&*g.id, selected)), Some(("browse:Home/Films", false)), "empty: the folder's actions");
        let paste = vec![Effect::Files(FileOp::Paste { path: "/Home/a.txt".into(), into: "/Home/Films".into(), cut: false })];
        assert_eq!(s.apply(ShelfEvent::TogglePin.into()).1, paste);
        assert_eq!(tap(&mut s, Id::Shelf(ShelfId::File(FileAction::Paste))), paste);
        assert!(s.files().clip.is_some(), "a copy pastes again");
    }

    #[test]
    fn selected_entries_are_what_the_actions_act_on() {
        let mut s = files_state();
        tap(&mut s, Id::Shelf(ShelfId::Launch));
        let three = ["a", "b", "c"].map(|n| entry(&format!("browse:Home/{n}"), false, BrowseKind::Plain)).to_vec();
        listed(&mut s, "browse:Home", three);
        // Start on a and c: selected; Move to Trash: both, the selection used up.
        s.apply(ShelfEvent::Continue.into());
        s.apply(ShelfEvent::ListStep(2).into());
        s.apply(ShelfEvent::Continue.into());
        assert_eq!(s.files().selected.len(), 2);
        let fx = tap(&mut s, Id::Shelf(ShelfId::File(FileAction::Trash)));
        assert_eq!(fx, vec![Effect::Files(FileOp::Trash("/Home/a".into())), Effect::Files(FileOp::Trash("/Home/c".into()))]);
        assert!(s.files().selected.is_empty());
        // Hidden files: the folder listed again with them.
        let fx = tap(&mut s, Id::Shelf(ShelfId::File(FileAction::ShowHidden)));
        assert!(matches!(fx.as_slice(), [Effect::Browse(BrowseCommand::Open(Some(Browse { hidden: true, .. })))]), "{fx:?}");
    }

    #[test]
    fn deleting_asks_first() {
        let mut s = files_state();
        s.apply(ShelfEvent::ListStep(1).into());
        tap(&mut s, Id::Shelf(ShelfId::Launch));
        listed(&mut s, "browse:Trash", vec![entry("browse:Trash/old", false, BrowseKind::Plain)]);
        let file = |a: &[FileAction]| a.iter().map(|&a| crate::actions::Action::File(a)).collect::<Vec<_>>();
        assert_eq!(
            s.file_actions().as_slice(),
            file(&[FileAction::Restore, FileAction::Delete, FileAction::Select, FileAction::ShowHidden]),
            "the Trash: nothing pasted or made in it"
        );
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
