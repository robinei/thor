//! The compositor's windows, as the dock and the games need them. A dock app instance is a systemd scope
//! we started, its windows on its app's workspace (see [`Instance`]): every window of its processes (a
//! second window, a program started from its terminal) belongs there, a tab beside the app's other
//! instances' windows.
//! Games are the same: each running game shelf started has a scope and a workspace of that name; a game
//! Steam starts gets a workspace of its own too (its windows are moved there from where they land).

use crate::instance::{self, Instance, STEAM_APP_PREFIX};

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
    /// Moved to the bottom screen by us (sway's mark on it): left there.
    pub down: bool,
    /// In view: the tab its workspace shows, on a workspace shown.
    pub visible: bool,
}

impl Window {
    /// The dock app instance or game it belongs to: its scope's, if we started it.
    pub fn instance(&self) -> Option<Instance> {
        self.unit.as_deref().and_then(Instance::from_unit)
    }

    /// The game's: started by our launcher, or a Steam game's (`steam_app_<id>`).
    pub fn of_game(&self) -> bool {
        self.unit.as_deref().is_some_and(Instance::is_game_unit) || self.app_id.starts_with(STEAM_APP_PREFIX)
    }

    /// Steam's id of the game it is a window of (`steam_app_<id>`).
    pub fn steam_id(&self) -> Option<u32> {
        instance::steam_app_id(&self.app_id)
    }

    /// The workspace it belongs on: its instance's or game's (one we started), shelf's, or none (left
    /// where it is; Steam's games are placed by the program, which knows which game they are).
    pub fn home(&self) -> Option<Instance> {
        self.instance().or_else(|| (self.app_id == crate::apps::SHELF_APP_ID).then_some(Instance::Shelf))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn windows_know_their_instance_and_home() {
        let w = |unit: Option<&str>, app_id: &str| Window { unit: unit.map(Into::into), app_id: app_id.into(), ..Window::default() };
        let terminal = Instance::App { key: "terminal".into(), secs: 1234, n: 7 };
        let foot = w(Some("clamshell-app-terminal-1234-7.scope"), "foot");
        assert_eq!(foot.instance(), Some(terminal.clone()));
        assert_eq!(foot.home(), Some(terminal.clone()));
        // Firefox started from that terminal: the terminal's.
        assert_eq!(w(Some("clamshell-app-terminal-1234-7.scope"), "firefox").home(), Some(terminal));
        assert_eq!(w(Some("clamshell-game-99-1.scope"), "retroarch").home(), Some(Instance::Game { secs: 99, n: 1 }));
        assert!(w(Some("clamshell-game-99-1.scope"), "retroarch").of_game());
        assert_eq!(w(Some("app-steam.scope"), "steam_app_620").home(), None, "Steam's: placed by the program");
        assert_eq!(w(Some("app-steam.scope"), "steam_app_620").steam_id(), Some(620));
        assert_eq!(w(Some("app-steam.scope"), "steam").home(), None, "not ours: left alone");
        assert_eq!(w(Some("clamshell.service"), crate::apps::SHELF_APP_ID).home(), Some(Instance::Shelf));
    }
}
