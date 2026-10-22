//! Entry ids. Each is a prefix naming the source, followed by what that source knows the entry by.

/// An app or game from a desktop file: `desktop:<the file name without .desktop>`.
pub const DESKTOP: &str = "desktop:";
/// A Steam game: `steam:<app id>`.
pub const STEAM: &str = "steam:";
/// A browsed file or folder: `browse:<place>/<path>`.
pub const BROWSE: &str = "browse:";
/// The settings level of a runner: `runner:<runner id>` (see [`crate::runner_key`]).
pub const RUNNER: &str = "runner:";
/// A program found by its tray icon that no desktop file names: `tray:<its name>`.
pub const TRAY: &str = "tray:";
/// A program found by its window that no desktop file names: `window:<its app id>`.
pub const WINDOW: &str = "window:";

/// The desktop file named by an entry id, if the id is a desktop file's.
pub fn desktop_file(id: &str) -> Option<&str> {
    id.strip_prefix(DESKTOP)
}
