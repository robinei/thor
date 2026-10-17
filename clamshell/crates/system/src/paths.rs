//! Where ClamShell's files are: worked out once at start from the XDG base directories and HOME, then
//! passed to whatever reads or writes one.

use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Paths {
    /// The library's content dirs (~/.config/clamshell/library.json, see
    /// [`crate::library::content`]).
    pub library: Option<PathBuf>,
    /// Play time (stats.db), favourites (favorites.txt), the entries' settings (game-settings.txt), the
    /// keymaps (keymaps.txt), the dock and the window rules: ~/.local/share/clamshell.
    pub state_dir: Option<PathBuf>,
    /// Where shelf and the second screen were (~/.local/state/clamshell/last).
    pub resume: Option<PathBuf>,
    /// The screens' brightness as it was left (the level and each screen's share), next to `resume`.
    pub brightness: Option<PathBuf>,
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
    pub state_home: Option<PathBuf>,
    pub runtime: Option<PathBuf>,
}

impl Base {
    /// HOME and the XDG base directories from the environment.
    pub fn from_env() -> Base {
        let var = |name: &str| std::env::var_os(name).map(PathBuf::from);
        Base {
            home: var("HOME"),
            config_home: var("XDG_CONFIG_HOME"),
            data_home: var("XDG_DATA_HOME"),
            state_home: var("XDG_STATE_HOME"),
            runtime: var("XDG_RUNTIME_DIR"),
        }
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
        let state = base.xdg(base.state_home.as_ref(), ".local/state");
        let state_dir = data.map(|d| d.join("clamshell"));
        let resume = state.map(|d| d.join("clamshell/last"));
        Paths {
            brightness: resume.as_ref().and_then(|f| f.parent()).map(|d| d.join("brightness")),
            resume,
            jobs: base.runtime.clone().unwrap_or_else(std::env::temp_dir).join("clamshell/jobs"),
            mangohud: base.runtime.clone().unwrap_or_else(|| PathBuf::from("/tmp")).join("clamshell/mangohud"),
            runtime: base.runtime.clone(),
            control: base.runtime.as_ref().map(|r| r.join("clamshell-control")),
            screenshots: base.home.as_ref().map(|h| h.join("Pictures/Screenshots")),
            library: config.map(|c| c.join("clamshell/library.json")),
            state_dir,
        }
    }

    pub fn stats_db(&self) -> Option<PathBuf> {
        self.state_dir.as_ref().map(|d| d.join("stats.db"))
    }

    pub fn game_settings(&self) -> Option<PathBuf> {
        self.state_dir.as_ref().map(|d| d.join("game-settings.txt"))
    }

    /// The keymaps (keymaps.txt), next to the games' settings.
    pub fn keymaps(&self) -> Option<PathBuf> {
        self.state_dir.as_ref().map(|d| d.join("keymaps.txt"))
    }

    /// Which programs' windows go to the bottom screen.
    pub fn window_rules(&self) -> Option<PathBuf> {
        self.state_dir.as_ref().map(|d| d.join("window-rules.txt"))
    }

    /// The dock's pinned entries, in order.
    pub fn dock(&self) -> Option<PathBuf> {
        self.state_dir.as_ref().map(|d| d.join("dock.txt"))
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
        let base = Base { home: Some("/h".into()), state_home: Some("/s".into()), runtime: Some("/run/u".into()), ..Base::default() };
        let paths = Paths::new(&base);
        assert_eq!(paths.library.as_deref(), Some(Path::new("/h/.config/clamshell/library.json")));
        assert_eq!(paths.state_dir.as_deref(), Some(Path::new("/h/.local/share/clamshell")));
        assert_eq!(paths.resume.as_deref(), Some(Path::new("/s/clamshell/last")));
        assert_eq!(paths.brightness.as_deref(), Some(Path::new("/s/clamshell/brightness")));
        assert_eq!((paths.jobs.as_path(), paths.mangohud.as_path()), (Path::new("/run/u/clamshell/jobs"), Path::new("/run/u/clamshell/mangohud")));
        assert_eq!(paths.stats_db().as_deref(), Some(Path::new("/h/.local/share/clamshell/stats.db")));
        assert_eq!(Paths::new(&Base::default()).state_dir, None, "no HOME, no XDG dirs");
    }
}
