//! An entry: a game, an app, a film, a file, or a group of them (a series' seasons and episodes, a folder),
//! with how it launches, what was fetched about it, a video's playback, and its key and colour.

use std::path::PathBuf;
use std::sync::Arc;

use crate::{Plays, RunnerOf, hash, sort};

/// How an entry starts.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum Launch {
    /// Nothing to start (a group: it opens in shelf).
    #[default]
    None,
    /// Through Steam.
    Steam { appid: u32 },
    /// A program with its arguments (filled in already), in `workdir` (else the home dir).
    Program { argv: Vec<String>, workdir: Option<PathBuf> },
    /// One of ClamShell's own tools (the Tools tab).
    Tool(Tool),
}

/// ClamShell's own tools.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tool {
    /// The library read again (library.json, its folders, Steam, the desktop files), then the missing
    /// covers and details fetched.
    Rescan,
    /// The entries whose covers and details weren't found tried again, then a rescan.
    Refetch,
    /// The Device view, ClamShell's own on the main screen (calibrating the controller and the screen,
    /// the password, remote login).
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
    /// Stable id: `<source id>:<path in the source's dir>`, `steam:<appid>` or `desktop:<file id>`. Focus,
    /// settings and play time are keyed by it. Shared, so the UI can put it in a frame's targets without
    /// allocating.
    pub id: Arc<str>,
    /// The id made fit for unit and workspace names (see [`key_of`]): what its launches are named by.
    pub key: String,
    pub title: String,
    /// What it sorts by (see [`sort`]).
    pub sort_key: Vec<(u8, char)>,
    /// Stand-in for box art while there is none or it is loading: 0xRRGGBB, from the id.
    pub color: u32,
    pub launch: Launch,
    /// Its file or folder in its content dir (none for Steam's games and desktop files).
    pub file: Option<PathBuf>,
    pub box_front: Option<PathBuf>,
    /// An icon (an app's; a folder's small picture, its row's), drawn where there is no box art (a
    /// folder's row: before it).
    pub icon: Option<PathBuf>,
    /// One of the UI's own icons by name (`folder`, `file`, `gamepad`), drawn where there is no picture
    /// (a browsed entry's; a folder's from library.json's places).
    pub glyph: Option<Arc<str>>,
    /// What was fetched about it (see [`crate::Meta`]).
    pub details: Details,
    /// A video mpv plays: where it was left and how long it is.
    pub playback: Playback,
    /// An app, not a game or a film: Recent leaves it out.
    pub app: bool,
    /// Another launch of it while it runs makes sense (the dock menu's Open another): an app from a
    /// desktop file, which opens a window of its own each time, unless its file says it has one main
    /// window (`SingleMainWindow`). Not a game, nor a file opened.
    pub another: bool,
    /// A group's entries (a series' seasons, a season's episodes), as indices into [`crate::Library::entries`], in
    /// order; empty for anything else.
    pub items: Vec<usize>,
    /// The group it is in, if any.
    pub parent: Option<usize>,
    /// Play history and favourite (as kept: clamshell-system's `store`).
    pub plays: Plays,
    pub favourite: bool,
    /// A file or folder of a browsed dir (see [`Browsed`]); none for the rest.
    pub browsed: Option<Browsed>,
    /// A folder's playlist (an `.m3u` in it): what enqueueing the folder enqueues, not its entries.
    pub playlist: Option<PathBuf>,
    /// The runner that opens it (library.json's or ClamShell's): a scanned entry's (and a folder of them),
    /// a browsed file's, Desktop for a desktop file's app or game, Steam for Steam's games. None for
    /// ClamShell's own tools.
    pub runner: Option<RunnerOf>,
}

/// What was fetched about an entry (see [`crate::Meta`]): a screenshot (a film's backdrop), what it is about, the
/// year it came out, who made it.
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
    /// `found` over these: each detail it has.
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

/// A video's playback: where mpv was when last quit (seconds; none: not started, or watched to the end),
/// when it kept that place (seconds since the epoch: it was watched then, maybe outside ClamShell), and
/// how long the video is (seconds, once read).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Playback {
    pub position: Option<u32>,
    pub at: u64,
    pub duration: Option<u32>,
}

/// A file or folder of a browsed dir (library.json's `browse`, removable storage): not scanned, but listed
/// when its folder is opened (its entries' ids its own, `/`, their names); it has the file actions instead
/// of a game's (no favourite, no pin, not in Recent).
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Browsed {
    /// A folder (it opens), not a file.
    pub folder: bool,
    /// A folder whose entries are in (its `items`), from a listing.
    pub listed: bool,
    /// An archive (a zip, 7z, rar, tar), browsed as a folder: its entries are those of its mount, read
    /// only.
    pub archive: bool,
    /// A folder nothing in it can be changed in (not writable: an archive's, the system's).
    pub locked: bool,
    pub kind: BrowseKind,
    /// A file's size (bytes), and when it was last changed (seconds since the epoch; 0: not known).
    pub size: Option<u64>,
    pub modified: u64,
}

/// What a browsed entry is besides a file or folder.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum BrowseKind {
    #[default]
    Plain,
    /// A dir library.json's `browse` names.
    Root,
    /// A removable filesystem's top (its device, to eject).
    Removable(String),
    /// The Trash (its `files` dir).
    Trash,
    /// At the top of the Trash: it can be restored.
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

    /// Whether it is a group (it opens rather than starts): one with entries, or a browsed folder.
    pub fn is_group(&self) -> bool {
        !self.items.is_empty() || self.browsed.as_ref().is_some_and(|b| b.folder)
    }

    /// A browsed folder: its dir, listed when it is opened.
    pub fn browsed_dir(&self) -> Option<&std::path::Path> {
        self.file.as_deref().filter(|_| self.browsed.as_ref().is_some_and(|b| b.folder))
    }

    /// Its file, if mpv plays it (its launch command's program is mpv): where mpv keeps its place.
    pub fn mpv_file(&self) -> Option<&std::path::Path> {
        let Launch::Program { argv, .. } = &self.launch else {
            return None;
        };
        let program = argv.first().map(|p| p.rsplit('/').next().unwrap_or(p));
        self.file.as_deref().filter(|_| program == Some("mpv"))
    }

    /// When it was last played or watched (seconds since the epoch): through ClamShell, or where mpv kept
    /// its place.
    pub fn last_watched(&self) -> u64 {
        self.plays.last.max(self.playback.at)
    }

    /// Watched: played and left at the end (mpv keeps no place), or quit near it (past [`DONE`]).
    pub fn watched(&self) -> bool {
        match (self.playback.position, self.playback.duration) {
            (None, _) => self.plays.count > 0,
            (Some(at), Some(length)) => length > 0 && f64::from(at) >= f64::from(length) * DONE,
            (Some(_), None) => false,
        }
    }
}

/// How far into a video counts as watched (the credits left).
pub const DONE: f64 = 0.9;

/// An entry's key, from its id: what its launches' units and its workspace are named by. Letters, digits
/// and `_` only (no `-`, which separates the parts of those names): the id's last part (its file's name
/// without the extension, or after `:`), lowercase, at most 16 characters, then `_` and 6 hex digits of a
/// hash of the whole id, so ids alike stay apart.
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
