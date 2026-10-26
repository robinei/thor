//! An entry is a game, an app, a film, a file, or a group of them, such as a series' seasons and episodes
//! or a folder. It holds how the entry launches, what was fetched about it, a video's playback position,
//! and its key and colour.

use std::path::PathBuf;
use std::sync::Arc;

use crate::{Plays, RunnerOf, hash, sort};

/// How an entry starts.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum Launch {
    /// Nothing to start. This is a group, which opens in shelf.
    #[default]
    None,
    /// Through Steam.
    Steam { appid: u32 },
    /// A program with its arguments, already filled in. It runs in `workdir`, or in the home dir if that
    /// is none.
    Program { argv: Vec<String>, workdir: Option<PathBuf> },
    /// One of ClamShell's own tools (the Tools tab).
    Tool(Tool),
}

/// ClamShell's own tools.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tool {
    /// Reads the library again (library.json, its folders, Steam, the desktop files), then fetches the
    /// missing covers and details.
    Rescan,
    /// Tries again for the entries whose covers and details weren't found, then rescans.
    Refetch,
    /// The Device view on the main screen, for calibrating the controller and the screen, the password
    /// and remote login.
    Device,
}

impl Launch {
    /// A program with these arguments, in the home dir.
    pub fn program(argv: Vec<String>) -> Launch {
        Launch::Program { argv, workdir: None }
    }
}

/// One entry as the rest of the program sees it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Entry {
    /// A stable id: `<source id>:<path in the source's dir>`, `steam:<appid>` or `desktop:<file id>`.
    /// Focus, settings and play time are keyed by it. It is shared so the UI can put it in a frame's
    /// targets without allocating.
    pub id: Arc<str>,
    /// The id made fit for unit and slice names (see [`key_of`]). Its launches are named by it.
    pub key: String,
    pub title: String,
    /// What it sorts by (see [`sort`]).
    pub sort_key: Vec<(u8, char)>,
    /// A colour shown instead of box art while there is none or it is loading. It is 0xRRGGBB, derived
    /// from the id.
    pub color: u32,
    pub launch: Launch,
    /// The entry's file or folder in its content dir. Steam games and desktop files have none.
    pub file: Option<PathBuf>,
    pub box_front: Option<PathBuf>,
    /// An icon, such as an app's icon or a folder's small picture. It is drawn where there is no box art;
    /// in a folder's row it is drawn before the name.
    pub icon: Option<PathBuf>,
    /// One of the UI's own icons by name (`folder`, `file`, `gamepad`), drawn where there is no picture.
    /// Browsed entries and folders from library.json's places use it.
    pub glyph: Option<Arc<str>>,
    /// What was fetched about it (see [`crate::Meta`]).
    pub details: Details,
    /// For a video mpv plays: where it was left and how long it is.
    pub playback: Playback,
    /// An app rather than a game or a film. Recent leaves it out.
    pub app: bool,
    /// Whether launching it again while it runs makes sense, for the dock menu's Open another. This is
    /// true for an app from a desktop file, which opens a new window each time, unless its file says it
    /// has one main window (`SingleMainWindow`). It is false for games and opened files.
    pub another: bool,
    /// A group's entries in order, such as a series' seasons or a season's episodes, as indices into
    /// [`crate::Library::entries`]. Empty for anything else.
    pub items: Vec<usize>,
    /// The group it is in, if any.
    pub parent: Option<usize>,
    /// Play history and favourite, as kept by clamshell-system's `store`.
    pub plays: Plays,
    pub favourite: bool,
    /// Set for a file or folder in a browsed dir (see [`Browsed`]).
    pub browsed: Option<Browsed>,
    /// A folder's playlist, an `.m3u` in it. Enqueueing the folder enqueues this instead of its entries.
    pub playlist: Option<PathBuf>,
    /// The runner that opens it, from library.json or ClamShell's own. Scanned entries, folders of them
    /// and browsed files have their runner; apps and games from desktop files have Desktop, and Steam
    /// games have Steam. ClamShell's own tools have none.
    pub runner: Option<RunnerOf>,
}

/// What was fetched about an entry (see [`crate::Meta`]): a screenshot or a film's backdrop, a summary,
/// the year it came out and who made it.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct Details {
    #[cfg_attr(feature = "serde", serde(default, skip_serializing_if = "Option::is_none"))]
    pub shot: Option<PathBuf>,
    #[cfg_attr(feature = "serde", serde(default, skip_serializing_if = "String::is_empty"))]
    pub summary: String,
    #[cfg_attr(feature = "serde", serde(default, skip_serializing_if = "Option::is_none"))]
    pub year: Option<u16>,
    #[cfg_attr(feature = "serde", serde(default, skip_serializing_if = "String::is_empty"))]
    pub developer: String,
}

impl Details {
    /// Overwrites each detail that `found` has.
    pub fn merge(&mut self, found: &Details) {
        if found.shot.is_some() {
            self.shot.clone_from(&found.shot);
        }
        if !found.summary.is_empty() {
            self.summary.clone_from(&found.summary);
        }
        if found.year.is_some() {
            self.year = found.year;
        }
        if !found.developer.is_empty() {
            self.developer.clone_from(&found.developer);
        }
    }
}

/// A video's playback state.
///
/// `position` is where mpv was when it last quit, in seconds; it is none if the video wasn't started or
/// was watched to the end. `at` is when mpv saved that position, in seconds since the epoch; that is
/// when it was watched, possibly outside ClamShell. `duration` is the video's length in seconds, once
/// read.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Playback {
    pub position: Option<u32>,
    pub at: u64,
    pub duration: Option<u32>,
}

/// A file or folder in a browsed dir, from library.json's `browse` or removable storage. It isn't
/// scanned but listed when its folder is opened; its entries' ids are the folder's id, `/` and their
/// names. It has file actions instead of a game's: no favourite, no pin, and it isn't in Recent.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Browsed {
    /// A folder, which opens, rather than a file.
    pub folder: bool,
    /// A folder whose entries have been listed into its `items`.
    pub listed: bool,
    /// An archive (zip, 7z, rar, tar) browsed as a folder. Its entries are those of its read-only mount.
    pub archive: bool,
    /// A folder whose contents can't be changed because it isn't writable, such as an archive's or a
    /// system folder.
    pub locked: bool,
    pub kind: BrowseKind,
    /// A file's size in bytes, and when it was last changed in seconds since the epoch (0 if not known).
    pub size: Option<u64>,
    pub modified: u64,
}

/// What a browsed entry is besides a file or folder.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum BrowseKind {
    #[default]
    Plain,
    /// A dir named by library.json's `browse`.
    Root,
    /// The top of a removable filesystem, with its device for ejecting.
    Removable(String),
    /// The Trash, meaning its `files` dir.
    Trash,
    /// An item at the top of the Trash, which can be restored.
    Trashed,
}

impl Entry {
    pub fn new(id: &str, title: &str) -> Self {
        Entry {
            id: id.into(),
            key: key_of(id),
            title: title.into(),
            sort_key: sort::key(title),
            color: color_of(id),
            launch: Launch::None,
            file: None,
            box_front: None,
            icon: None,
            glyph: None,
            playlist: None,
            details: Details::default(),
            playback: Playback::default(),
            app: false,
            another: false,
            items: Vec::new(),
            parent: None,
            plays: Plays::default(),
            favourite: false,
            browsed: None,
            runner: None,
        }
    }

    /// Whether it is a group, which opens rather than starts: one with entries, or a browsed folder.
    pub fn is_group(&self) -> bool {
        !self.items.is_empty() || self.browsed.as_ref().is_some_and(|b| b.folder)
    }

    /// Whether it is a game, rather than an app, a game launcher, a film, a file or a group. Games are
    /// paused when hidden by default, as consoles do.
    pub fn is_game(&self) -> bool {
        !self.app && self.browsed.is_none() && self.mpv_file().is_none() && !self.is_group()
    }

    /// For a browsed folder, its dir, which is listed when the folder is opened.
    pub fn browsed_dir(&self) -> Option<&std::path::Path> {
        self.file.as_deref().filter(|_| self.browsed.as_ref().is_some_and(|b| b.folder))
    }

    /// The entry's file, if its launch command runs mpv. mpv keeps its place by this file.
    pub fn mpv_file(&self) -> Option<&std::path::Path> {
        let Launch::Program { argv, .. } = &self.launch else {
            return None;
        };
        let program = argv.first().map(|p| p.rsplit('/').next().unwrap_or(p));
        self.file.as_deref().filter(|_| program == Some("mpv"))
    }

    /// When it was last played or watched, in seconds since the epoch. This is the later of the last play
    /// through ClamShell and the time mpv saved its place.
    pub fn last_watched(&self) -> u64 {
        self.plays.last.max(self.playback.at)
    }

    /// Whether it was watched: played to the end, so mpv keeps no place, or quit near the end (past
    /// [`DONE`]).
    pub fn watched(&self) -> bool {
        match (self.playback.position, self.playback.duration) {
            (None, _) => self.plays.count > 0,
            (Some(at), Some(length)) => length > 0 && f64::from(at) >= f64::from(length) * DONE,
            (Some(_), None) => false,
        }
    }
}

/// How far into a video counts as watched, leaving room for the credits.
pub const DONE: f64 = 0.9;

/// An entry's key, made from its id. Its launches' units and slice are named by it.
///
/// It has only letters, digits and `_`, because `-` separates the parts of those names. It is the id's
/// last part (the file name without its extension, or what follows `:`), lowercased and cut to 16
/// characters, then `_` and 6 hex digits of a hash of the whole id so that similar ids stay distinct.
pub fn key_of(id: &str) -> String {
    let last = id.rsplit(['/', ':']).next().unwrap_or(id);
    let stem = last.rsplit_once('.').map_or(last, |(stem, _)| stem);
    let mut slug: String = stem.chars().map(|c| if c.is_ascii_alphanumeric() { c.to_ascii_lowercase() } else { '_' }).take(16).collect();
    let trimmed = slug.trim_matches('_').len();
    if trimmed == 0 {
        slug = "entry".into();
    }
    let h = hash::fnv1a32(id.as_bytes());
    format!("{}_{:06x}", slug.trim_matches('_'), h & 0x00ff_ffff)
}

/// A muted colour from a hash of `id`, stable across runs.
pub(crate) fn color_of(id: &str) -> u32 {
    let h = hash::fnv1a32(id.as_bytes());
    let channel = |shift: u32| 0x40 + ((h >> shift) & 0x7f);
    (channel(16) << 16) | (channel(8) << 8) | channel(0)
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::indexing_slicing)]
mod tests {
    use super::*;

    #[test]
    fn colours_are_stable_and_muted() {
        assert_eq!(color_of("steam:1"), color_of("steam:1"));
        let c = color_of("anything");
        for shift in [0, 8, 16] {
            assert!((c >> shift) & 0xff >= 0x40);
        }
    }
}
