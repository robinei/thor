//! Where ClamShell's files are: worked out once at start from the XDG base directories and HOME, then
//! passed to whatever reads or writes one.

use std::path::PathBuf;

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Paths {
    /// The library's content dirs (~/.config/clamshell/library.json, see [`crate::library::content`]).
    pub library: Option<PathBuf>,
    /// What ClamShell keeps (see [`crate::store`]): ~/.local/share/clamshell.
    pub state_dir: Option<PathBuf>,
    /// Fetched covers and screenshots (see [`crate::library::meta`]): ~/.cache/clamshell/art.
    pub art: Option<PathBuf>,
    /// Where mpv keeps the places of videos quit before their end (see [`crate::launch::mpv`]).
    pub watch_later: Vec<PathBuf>,
    /// Launches handed to sway's exec ($XDG_RUNTIME_DIR/clamshell/jobs, see [`crate::launch::job`]).
    pub jobs: PathBuf,
    /// MangoHud's config files and frame logs, in RAM ($XDG_RUNTIME_DIR/clamshell/mangohud).
    pub mangohud: PathBuf,
    /// Launches' control sockets, a command's `{socket}` ($XDG_RUNTIME_DIR/clamshell/sockets).
    pub sockets: PathBuf,
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
    pub cache_home: Option<PathBuf>,
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
            cache_home: var("XDG_CACHE_HOME"),
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
        let state_dir = data.map(|d| d.join("clamshell"));
        let art = base.xdg(base.cache_home.as_ref(), ".cache").map(|c| c.join("clamshell/art"));
        let watch_later = crate::launch::mpv::watch_later_dirs(config.as_deref(), base.xdg(base.state_home.as_ref(), ".local/state").as_deref());
        Paths {
            jobs: base.runtime.clone().unwrap_or_else(std::env::temp_dir).join("clamshell/jobs"),
            mangohud: base.runtime.clone().unwrap_or_else(|| PathBuf::from("/tmp")).join("clamshell/mangohud"),
            sockets: base.runtime.clone().unwrap_or_else(std::env::temp_dir).join("clamshell/sockets"),
            runtime: base.runtime.clone(),
            control: base.runtime.as_ref().map(|r| r.join("clamshell-control")),
            screenshots: base.home.as_ref().map(|h| h.join("Pictures/Screenshots")),
            library: config.map(|c| c.join("clamshell/library.json")),
            state_dir,
            art,
            watch_later,
        }
    }

    /// The control socket of entry `key`'s launches (see [`clamshell_core::session::enqueue`]).
    pub fn socket(&self, key: &str) -> PathBuf {
        self.sockets.join(clamshell_core::session::instance::Instance::socket_of(key))
    }

    /// ClamShell's database (see [`crate::store`]).
    /// Whether there is a systemd user session to put apps and games in scopes of.
    pub fn has_user_systemd(&self) -> bool {
        self.runtime.as_deref().is_some_and(|d| d.join("systemd/private").exists())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    #[test]
    fn xdg_dirs_or_their_defaults_under_home() {
        let base = Base { home: Some("/h".into()), runtime: Some("/run/u".into()), ..Base::default() };
        let paths = Paths::new(&base);
        assert_eq!(paths.library.as_deref(), Some(Path::new("/h/.config/clamshell/library.json")));
        assert_eq!(paths.state_dir.as_deref(), Some(Path::new("/h/.local/share/clamshell")));
        assert_eq!((paths.jobs.as_path(), paths.mangohud.as_path()), (Path::new("/run/u/clamshell/jobs"), Path::new("/run/u/clamshell/mangohud")));
        assert_eq!(paths.art.as_deref(), Some(Path::new("/h/.cache/clamshell/art")));
        assert_eq!(Paths::new(&Base::default()).state_dir, None, "no HOME, no XDG dirs");
    }
}
