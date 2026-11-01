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
    /// $XDG_RUNTIME_DIR/clamshell/sockets; without a runtime dir there are none, rather than a shared
    /// folder anyone could make first.
    pub(crate) sockets: Option<PathBuf>,
    /// $XDG_RUNTIME_DIR, where systemd's user manager has its socket.
    pub runtime: Option<PathBuf>,
    /// ClamShell's own folder there, $XDG_RUNTIME_DIR/clamshell: the session's lock, the clipboard kept,
    /// archives mounted, the launches' sockets, the Android filter's settings. Everything that must agree
    /// on the folder takes it from here.
    pub own_runtime: Option<PathBuf>,
    /// HOME: a launch's working directory when it names none, mpv's `~`.
    pub(crate) home: Option<PathBuf>,
    /// $XDG_DATA_HOME, else ~/.local/share: the home Trash is there.
    pub(crate) data_home: Option<PathBuf>,
    /// The XDG data dirs, the user's (`data_home`) first: desktop files and icon themes.
    pub(crate) data_dirs: Vec<PathBuf>,
    /// The control pipe `clamshell ctl` writes to ($XDG_RUNTIME_DIR/clamshell-control).
    pub control: Option<PathBuf>,
    /// Where screenshots go (~/Pictures/Screenshots).
    pub(crate) screenshots: Option<PathBuf>,
    /// Where the x86 emulation presets are copied, ~/.cache/clamshell/fex (see [`crate::launch::fex`]).
    pub fex: Option<PathBuf>,
    /// The grabbed controllers' ids as `SDL_GAMECONTROLLER_IGNORE_DEVICES` takes them,
    /// $XDG_RUNTIME_DIR/clamshell/grabbed, for Steam's launch hook (see [`crate::input::pad::Taken`]).
    pub grabbed: Option<PathBuf>,
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
    /// XDG_DATA_DIRS, the system's data dirs, `:`-separated.
    data_dirs: Option<String>,
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
            data_dirs: std::env::var("XDG_DATA_DIRS").ok(),
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
        let cache = base.xdg(base.cache_home.as_ref(), ".cache");
        let art = cache.as_ref().map(|c| c.join("clamshell/art"));
        let state_home = base.xdg(base.state_home.as_ref(), ".local/state");
        let watch_later = crate::launch::mpv::watch_later_dirs(config.as_deref(), state_home.as_deref(), base.home.as_deref());
        let own_runtime = base.runtime.as_ref().map(|r| r.join("clamshell"));
        let data_dirs = crate::os::xdg::data_dirs(data.as_deref(), base.data_dirs.as_deref());
        Paths {
            sockets: own_runtime.as_ref().map(|r| r.join("sockets")),
            grabbed: own_runtime.as_ref().map(|r| r.join("grabbed")),
            own_runtime,
            home: base.home.clone(),
            data_home: data,
            data_dirs,
            runtime: base.runtime.clone(),
            control: base.runtime.as_ref().map(|r| r.join("clamshell-control")),
            screenshots: base.home.as_ref().map(|h| h.join("Pictures/Screenshots")),
            library: config.map(|c| c.join("clamshell/library.json")),
            state_dir,
            art,
            fex: cache.map(|c| c.join("clamshell/fex")),
            watch_later,
        }
    }

    /// The control socket for launches of entry `key`. See [`clamshell_state::session::enqueue`].
    pub(crate) fn socket(&self, key: &str) -> Option<PathBuf> {
        Some(self.sockets.as_ref()?.join(clamshell_state::session::launch_unit::LaunchUnit::socket_of(key)))
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
        assert_eq!(paths.fex.as_deref(), Some(Path::new("/h/.cache/clamshell/fex")));
        assert_eq!(paths.grabbed.as_deref(), Some(Path::new("/run/u/clamshell/grabbed")));
        assert_eq!(Paths::new(&Base::default()).state_dir, None, "no HOME, no XDG dirs");
    }

    /// The session's lock, the launches' sockets, the grabbed controllers and the Android filter's
    /// settings agree on ClamShell's runtime folder by taking it from here; without a runtime dir there is
    /// none, and no shared folder in /tmp instead. The data dirs are worked out here once too.
    #[test]
    fn the_runtime_folder_and_the_data_dirs_are_worked_out_once() {
        let base = Base { home: Some("/h".into()), runtime: Some("/run/u".into()), ..Base::default() };
        let paths = Paths::new(&base);
        assert_eq!(paths.own_runtime.as_deref(), Some(Path::new("/run/u/clamshell")));
        assert_eq!(paths.sockets.as_deref(), Some(Path::new("/run/u/clamshell/sockets")));
        assert_eq!(paths.socket("k").map(|s| s.starts_with("/run/u/clamshell/sockets")), Some(true));
        assert_eq!(paths.control.as_deref(), Some(Path::new("/run/u/clamshell-control")));
        let without = Paths::new(&Base { home: Some("/h".into()), ..Base::default() });
        assert_eq!((without.own_runtime, without.sockets, without.grabbed, without.control), (None, None, None, None));
        let dirs = |base: &Base| Paths::new(base).data_dirs;
        assert_eq!(dirs(&base), ["/h/.local/share", "/usr/local/share", "/usr/share"].map(PathBuf::from));
        let set = Base { data_home: Some("/d".into()), data_dirs: Some("/a::/b".into()), ..base };
        assert_eq!(dirs(&set), ["/d", "/a", "/b"].map(PathBuf::from));
    }
}
