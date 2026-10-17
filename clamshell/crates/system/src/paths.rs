//! Where ClamShell's files are: worked out once at start from the XDG base directories and HOME, then
//! passed to whatever reads or writes one.

use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Paths {
    /// The library's content dirs (~/.config/clamshell/library.json, see
    /// [`crate::library::content`]).
    pub library: Option<PathBuf>,
    /// ClamShell's database (see [`crate::store`]): ~/.local/share/clamshell.
    pub state_dir: Option<PathBuf>,
    /// Launches handed to sway's exec ($XDG_RUNTIME_DIR/clamshell/jobs, see [`crate::job`]).
    pub jobs: PathBuf,
    /// MangoHud's config files and frame logs, in RAM ($XDG_RUNTIME_DIR/clamshell/mangohud).
    pub mangohud: PathBuf,
    /// $XDG_RUNTIME_DIR, where systemd's user manager has its socket.
    pub runtime: Option<PathBuf>,
    /// The control pipe `clamshell ctl` writes to ($XDG_RUNTIME_DIR/clamshell-control).
    pub control: Option<PathBuf>,
    /// Where screenshots go (~/Pictures/Screenshots).
    pub screenshots: Option<PathBuf>,
}

/// The environment the paths come from.
#[derive(Debug, Clone, Default)]
pub struct Base {
    pub home: Option<PathBuf>,
    pub config_home: Option<PathBuf>,
    pub data_home: Option<PathBuf>,
    pub runtime: Option<PathBuf>,
}

impl Base {
    /// HOME and the XDG base directories from the environment.
    pub fn from_env() -> Base {
        let var = |name: &str| std::env::var_os(name).map(PathBuf::from);
        Base { home: var("HOME"), config_home: var("XDG_CONFIG_HOME"), data_home: var("XDG_DATA_HOME"), runtime: var("XDG_RUNTIME_DIR") }
    }

    /// An XDG directory, else its default under HOME.
    fn xdg(&self, set: Option<&PathBuf>, under_home: &str) -> Option<PathBuf> {
        set.cloned().or_else(|| self.home.as_ref().map(|h| h.join(under_home)))
    }
}

impl Paths {
    /// The paths for `base`.
    pub fn new(base: &Base) -> Paths {
        let config = base.xdg(base.config_home.as_ref(), ".config");
        let data = base.xdg(base.data_home.as_ref(), ".local/share");
        let state_dir = data.map(|d| d.join("clamshell"));
        Paths {
            jobs: base.runtime.clone().unwrap_or_else(std::env::temp_dir).join("clamshell/jobs"),
            mangohud: base.runtime.clone().unwrap_or_else(|| PathBuf::from("/tmp")).join("clamshell/mangohud"),
            runtime: base.runtime.clone(),
            control: base.runtime.as_ref().map(|r| r.join("clamshell-control")),
            screenshots: base.home.as_ref().map(|h| h.join("Pictures/Screenshots")),
            library: config.map(|c| c.join("clamshell/library.json")),
            state_dir,
        }
    }

    /// ClamShell's database (see [`crate::store`]).
    pub fn db(&self) -> Option<PathBuf> {
        self.state_dir.as_ref().map(|d| d.join("clamshell.db"))
    }

    /// Whether there is a systemd user session to put apps and games in scopes of.
    pub fn has_user_systemd(&self) -> bool {
        self.runtime.as_deref().is_some_and(|d| d.join("systemd/private").exists())
    }
}

/// Writes one of ClamShell's files, its folder made if needed.
pub fn write(file: &Path, text: &str) -> std::io::Result<()> {
    if let Some(dir) = file.parent() {
        std::fs::create_dir_all(dir)?;
    }
    std::fs::write(file, text)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn xdg_dirs_or_their_defaults_under_home() {
        let base = Base { home: Some("/h".into()), runtime: Some("/run/u".into()), ..Base::default() };
        let paths = Paths::new(&base);
        assert_eq!(paths.library.as_deref(), Some(Path::new("/h/.config/clamshell/library.json")));
        assert_eq!(paths.state_dir.as_deref(), Some(Path::new("/h/.local/share/clamshell")));
        assert_eq!((paths.jobs.as_path(), paths.mangohud.as_path()), (Path::new("/run/u/clamshell/jobs"), Path::new("/run/u/clamshell/mangohud")));
        assert_eq!(paths.db().as_deref(), Some(Path::new("/h/.local/share/clamshell/clamshell.db")));
        assert_eq!(Paths::new(&Base::default()).state_dir, None, "no HOME, no XDG dirs");
    }
}
