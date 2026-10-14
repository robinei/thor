//! The compositor's windows, as the launcher needs them: which app or game each belongs to, which is in
//! front, and the order they were last in front (for "the app's most recent window").

/// An open window (from sway's tree).
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Window {
    /// The compositor's id for it, to focus or close it by.
    pub id: i64,
    /// Its Wayland app id (or X11 class).
    pub app_id: String,
    pub title: String,
    pub focused: bool,
    /// The systemd unit its process runs in (`sysprog-game-….scope`, `sysprog-app-<key>-….scope`).
    pub unit: Option<String>,
}

impl Window {
    /// Started by us as the launcher app with this key.
    pub fn from_app(&self, key: &str) -> bool {
        // `sysprog-app-<key>-<pid>-<n>.scope`; keys have no '-'.
        self.unit.as_deref().and_then(|u| u.strip_prefix("sysprog-app-")).and_then(|rest| rest.split('-').next()) == Some(key)
    }

    /// The running game's: started by our launcher, or a Steam game's (`steam_app_<id>`).
    pub fn of_game(&self, steam_appid: Option<u32>) -> bool {
        self.unit.as_deref().is_some_and(|u| u.starts_with("sysprog-game-"))
            || steam_appid.is_some_and(|id| self.app_id.strip_prefix("steam_app_").and_then(|n| n.parse().ok()) == Some(id))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn windows_know_who_started_them() {
        let w = |unit: &str, app_id: &str| Window { unit: Some(unit.into()), app_id: app_id.into(), ..Window::default() };
        assert!(w("sysprog-app-foot-1234-7.scope", "foot").from_app("foot"));
        assert!(!w("sysprog-app-foot-1234-7.scope", "foot").from_app("foo"));
        assert!(w("sysprog-game-99-1.scope", "retroarch").of_game(None));
        assert!(w("app-steam.scope", "steam_app_620").of_game(Some(620)));
        assert!(!w("app-steam.scope", "steam").of_game(Some(620)));
    }
}
