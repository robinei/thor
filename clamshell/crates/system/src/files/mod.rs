//! Browsed dirs and file operations (see clamshell-core's `files`).
//!
//! The browse thread lists the folder shelf has open, and lists it again whenever it changes, using
//! inotify. It also notices removable filesystems being mounted and unmounted (udisks2's, under
//! `/run/media`).
//!
//! The files thread runs the operations one after another:
//! - rename and new folder;
//! - paste, as a copy or a move. A move across filesystems is a copy then a delete, with progress shown.
//! - moving to the Trash (freedesktop's: the home Trash, or a filesystem's own `.Trash-<uid>`);
//! - delete;
//! - eject, through udisks2;
//! - extract, with `bsdtar`. Its progress is read from how far it has read the archive.
//!
//! An archive is browsed as a folder through its mount (see [`crate::files::archives`]).

pub(crate) mod archives;
pub(crate) mod browse;
pub(crate) mod mounts;
pub(crate) mod ops;
mod trash;
