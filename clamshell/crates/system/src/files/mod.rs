//! Browsed dirs and file operations (see clamshell-core's `files`). The browse thread lists the folder
//! shelf has open and lists it again whenever it changes (inotify), and notices removable filesystems
//! mounted and unmounted (udisks2's, under `/run/media`). The files thread does the operations one after
//! another: rename, new folder, paste (a copy, or a move; across filesystems a copy then a delete, with
//! its progress), the Trash (freedesktop's: the home Trash, or a filesystem's own `.Trash-<uid>`), delete,
//! eject (udisks2), extract (`bsdtar`, its progress read from how far it has read the archive).
//! An archive is browsed as a folder through its mount (see [`crate::files::archives`]).

pub(crate) mod archives;
pub(crate) mod browse;
pub(crate) mod mounts;
pub(crate) mod ops;
mod trash;
