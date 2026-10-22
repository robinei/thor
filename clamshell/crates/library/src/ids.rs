//! Entry ids: a source's prefix, then what the source knows the entry by.

/// A desktop file's app or game: `desktop:<the file's name without .desktop>`.
pub const DESKTOP: &str = "desktop:";
/// A Steam game: `steam:<app id>`.
pub const STEAM: &str = "steam:";
/// A browsed file or folder: `browse:<place>/<path>`.
pub const BROWSE: &str = "browse:";
/// A runner's level of settings: `runner:<runner id>` (see [`crate::runner_key`]).
pub const RUNNER: &str = "runner:";
/// A program found by its tray icon that no desktop file names: `tray:<its name>`.
pub const TRAY: &str = "tray:";
/// A program found by its window that no desktop file names: `window:<its app id>`.
pub const WINDOW: &str = "window:";

/// The desktop file an entry id names, if it is a desktop file's.
pub fn desktop_file(id: &str) -> Option<&str> {
    id.strip_prefix(DESKTOP)
}
