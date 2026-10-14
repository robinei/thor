//! The compositor's windows, as the launcher needs them. A launcher instance is a systemd scope we
//! started (`sysprog-app-<key>-<pid>-<n>.scope`) and has a workspace of the same name: every window of
//! its processes (a second window, a program started from its terminal) belongs there, and sway arranges
//! them within it. The game has one workspace, for windows of our game scopes and Steam's games.

/// The game's workspace.
pub const GAME_WORKSPACE: &str = "sysprog-game";

/// An open window (from sway's tree).
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Window {
    /// The compositor's id for it, to focus or close it by.
    pub id: i64,
    /// Its Wayland app id (or X11 class).
    pub app_id: String,
    pub title: String,
    pub focused: bool,
    /// The systemd unit its process runs in.
    pub unit: Option<String>,
    /// The workspace it is on.
    pub workspace: String,
}

impl Window {
    /// The launcher instance it belongs to: its scope's name without `.scope`, if we started it as an app.
    pub fn instance(&self) -> Option<&str> {
        self.unit.as_deref().and_then(|u| u.strip_suffix(".scope")).filter(|u| u.starts_with("sysprog-app-"))
    }

    /// Started as the launcher app with this key (`sysprog-app-<key>-…`; keys have no '-').
    pub fn from_app(&self, key: &str) -> bool {
        self.instance().and_then(|i| i.strip_prefix("sysprog-app-")).and_then(|rest| rest.split('-').next()) == Some(key)
    }

    /// The game's: started by our launcher, or a Steam game's (`steam_app_<id>`).
    pub fn of_game(&self) -> bool {
        self.unit.as_deref().is_some_and(|u| u.starts_with("sysprog-game-")) || self.app_id.starts_with("steam_app_")
    }

    /// The workspace it belongs on: its instance's, the game's, or none (left where it is).
    pub fn home(&self) -> Option<&str> {
        self.instance().or_else(|| self.of_game().then_some(GAME_WORKSPACE))
    }
}

/// An instance's place among its app's: the `<pid>-<n>` at the end of its name, as numbers.
pub fn order(instance: &str) -> (u64, u64) {
    let mut parts = instance.rsplit('-');
    let n = parts.next().and_then(|n| n.parse().ok()).unwrap_or(0);
    let pid = parts.next().and_then(|p| p.parse().ok()).unwrap_or(0);
    (pid, n)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn windows_know_their_instance_and_home() {
        let w = |unit: Option<&str>, app_id: &str| Window { unit: unit.map(Into::into), app_id: app_id.into(), ..Window::default() };
        let foot = w(Some("sysprog-app-terminal-1234-7.scope"), "foot");
        assert_eq!(foot.instance(), Some("sysprog-app-terminal-1234-7"));
        assert!(foot.from_app("terminal") && !foot.from_app("term"));
        assert_eq!(foot.home(), Some("sysprog-app-terminal-1234-7"));
        // Firefox started from that terminal: the terminal's.
        assert_eq!(w(Some("sysprog-app-terminal-1234-7.scope"), "firefox").home(), Some("sysprog-app-terminal-1234-7"));
        assert_eq!(w(Some("sysprog-game-99-1.scope"), "retroarch").home(), Some(GAME_WORKSPACE));
        assert_eq!(w(Some("app-steam.scope"), "steam_app_620").home(), Some(GAME_WORKSPACE));
        assert_eq!(w(Some("app-steam.scope"), "steam").home(), None, "not ours: left alone");
        assert!(order("sysprog-app-x-5-10") > order("sysprog-app-x-5-9"));
    }
}
