//! Where ClamShell's files are. The paths are worked out once at start from the XDG base directories and
//! HOME, then passed to whatever reads or writes the files.

use std::path::PathBuf;

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Paths {
    /// The file listing the library's content directories: ~/.config/clamshell/library.json. See
    /// [`crate::library::content`].
    pub(crate) library: Option<PathBuf>,
    /// ClamShell's own data, ~/.local/share/clamshell. See [`crate::store`].
    pub state_dir: Option<PathBuf>,
    /// Fetched covers and screenshots, ~/.cache/clamshell/art. See [`crate::library::meta`].
    pub art: Option<PathBuf>,
    /// Where mpv keeps the position of videos quit before the end. See [`crate::launch::mpv`].
    pub(crate) watch_later: Vec<PathBuf>,
    /// The launches' control sockets, which a command gets as `{socket}`. They are in
    /// $XDG_RUNTIME_DIR/clamshell/sockets.
    pub(crate) sockets: PathBuf,
    /// $XDG_RUNTIME_DIR, where systemd's user manager has its socket.
    pub runtime: Option<PathBuf>,
    /// ClamShell's own folder there, $XDG_RUNTIME_DIR/clamshell: the clipboard kept, archives mounted,
    /// the launches' sockets.
    pub(crate) own_runtime: Option<PathBuf>,
    /// HOME: a launch's working directory when it names none, mpv's `~`.
    pub(crate) home: Option<PathBuf>,
    /// $XDG_DATA_HOME, else ~/.local/share: the home Trash is there.
    pub(crate) data_home: Option<PathBuf>,
    /// The control pipe `clamshell ctl` writes to ($XDG_RUNTIME_DIR/clamshell-control).
    pub control: Option<PathBuf>,
    /// Where screenshots go (~/Pictures/Screenshots).
    pub(crate) screenshots: Option<PathBuf>,
}

/// The environment the paths come from.
#[derive(Debug, Clone, Default)]
pub struct Base {
    pub home: Option<PathBuf>,
    config_home: Option<PathBuf>,
    data_home: Option<PathBuf>,
    cache_home: Option<PathBuf>,
    state_home: Option<PathBuf>,
    runtime: Option<PathBuf>,
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
        let state_dir = data.as_ref().map(|d| d.join("clamshell"));
        let art = base.xdg(base.cache_home.as_ref(), ".cache").map(|c| c.join("clamshell/art"));
        let state_home = base.xdg(base.state_home.as_ref(), ".local/state");
        let watch_later = crate::launch::mpv::watch_later_dirs(config.as_deref(), state_home.as_deref(), base.home.as_deref());
        let own_runtime = base.runtime.as_ref().map(|r| r.join("clamshell"));
        Paths {
            sockets: own_runtime.clone().unwrap_or_else(|| std::env::temp_dir().join("clamshell")).join("sockets"),
            own_runtime,
            home: base.home.clone(),
            data_home: data,
            runtime: base.runtime.clone(),
            control: base.runtime.as_ref().map(|r| r.join("clamshell-control")),
            screenshots: base.home.as_ref().map(|h| h.join("Pictures/Screenshots")),
            library: config.map(|c| c.join("clamshell/library.json")),
            state_dir,
            art,
            watch_later,
        }
    }

    /// The control socket for launches of entry `key`. See [`clamshell_state::session::enqueue`].
    pub(crate) fn socket(&self, key: &str) -> PathBuf {
        self.sockets.join(clamshell_state::session::launch_unit::LaunchUnit::socket_of(key))
    }

    /// Whether there is a systemd user session to run apps and games as units of.
    pub(crate) fn has_user_systemd(&self) -> bool {
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
        assert_eq!(paths.art.as_deref(), Some(Path::new("/h/.cache/clamshell/art")));
        assert_eq!(Paths::new(&Base::default()).state_dir, None, "no HOME, no XDG dirs");
    }
}
