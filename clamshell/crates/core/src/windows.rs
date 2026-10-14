//! The compositor's windows, as the launcher needs them. A launcher instance is a systemd scope we
//! started (`clamshell-app-<key>-<pid>-<n>.scope`) and has a workspace of the same name: every window of
//! its processes (a second window, a program started from its terminal) belongs there, and sway arranges
//! them within it. Games are the same: each running game has a scope (`clamshell-game-<secs>-<n>`) and a
//! workspace of that name; a game Steam starts gets a workspace of its own too (its windows are moved
//! there from where they land).

/// Where Steam's game windows land (a sway rule) before they are moved to their game's workspace; and
/// shelf's workspace (its window's; shown again when an app's or a game's workspace empties).
pub const GAME_WORKSPACE: &str = "clamshell-game";
pub const SHELF_WORKSPACE: &str = "clamshell-shelf";

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
        self.unit.as_deref().and_then(|u| u.strip_suffix(".scope")).filter(|u| u.starts_with("clamshell-app-"))
    }

    /// Started as the launcher app with this key (`clamshell-app-<key>-…`; keys have no '-').
    pub fn from_app(&self, key: &str) -> bool {
        self.instance().and_then(|i| i.strip_prefix("clamshell-app-")).and_then(|rest| rest.split('-').next()) == Some(key)
    }

    /// The game's: started by our launcher, or a Steam game's (`steam_app_<id>`).
    pub fn of_game(&self) -> bool {
        self.unit.as_deref().is_some_and(|u| u.starts_with("clamshell-game-")) || self.app_id.starts_with("steam_app_")
    }

    /// The game it belongs to, if we started it: its scope's name without `.scope` (its workspace's).
    pub fn game_scope(&self) -> Option<&str> {
        self.unit.as_deref().and_then(|u| u.strip_suffix(".scope")).filter(|u| u.starts_with("clamshell-game-"))
    }

    /// Steam's id of the game it is a window of (`steam_app_<id>`).
    pub fn steam_id(&self) -> Option<u32> {
        self.app_id.strip_prefix("steam_app_").and_then(|n| n.parse().ok())
    }

    /// The workspace it belongs on: its instance's, its game's (one we started), shelf's, or none (left
    /// where it is; Steam's games are placed by the program, which knows which game they are).
    pub fn home(&self) -> Option<&str> {
        self.instance()
            .or_else(|| self.game_scope())
            .or_else(|| (self.app_id == crate::apps::SHELF_APP_ID).then_some(SHELF_WORKSPACE))
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
        let foot = w(Some("clamshell-app-terminal-1234-7.scope"), "foot");
        assert_eq!(foot.instance(), Some("clamshell-app-terminal-1234-7"));
        assert!(foot.from_app("terminal") && !foot.from_app("term"));
        assert_eq!(foot.home(), Some("clamshell-app-terminal-1234-7"));
        // Firefox started from that terminal: the terminal's.
        assert_eq!(w(Some("clamshell-app-terminal-1234-7.scope"), "firefox").home(), Some("clamshell-app-terminal-1234-7"));
        assert_eq!(w(Some("clamshell-game-99-1.scope"), "retroarch").home(), Some("clamshell-game-99-1"));
        assert_eq!(w(Some("app-steam.scope"), "steam_app_620").home(), None, "Steam's: placed by the program");
        assert_eq!(w(Some("app-steam.scope"), "steam_app_620").steam_id(), Some(620));
        assert_eq!(w(Some("app-steam.scope"), "steam").home(), None, "not ours: left alone");
        assert_eq!(w(Some("clamshell.service"), crate::apps::SHELF_APP_ID).home(), Some(SHELF_WORKSPACE));
        assert!(order("clamshell-app-x-5-10") > order("clamshell-app-x-5-9"));
    }
}
