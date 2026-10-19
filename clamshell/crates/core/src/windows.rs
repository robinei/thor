//! The compositor's windows, as the dock and the games need them. A dock app instance is a systemd scope
//! we started, its windows on its app's workspace (see [`Instance`]): every window of its processes (a
//! second window, a program started from its terminal) belongs there, a tab beside the app's other
//! instances' windows.
//! Games are the same: each running game shelf started has a scope and a workspace of that name; an entry
//! that runs in another program's processes (Steam's games, Waydroid's apps) gets a workspace of its own
//! too, its windows found by their app id (see [`crate::running`]).

use crate::instance::Instance;

/// Shelf's window's app id (its own Wayland surface on the main screen).
pub const SHELF_APP_ID: &str = "clamshell.top";

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
    /// The launch it belongs to: its scope's, if we started it.
    pub fn instance(&self) -> Option<Instance> {
        self.unit.as_deref().and_then(Instance::from_unit)
    }

    /// What it belongs on the workspace of: its launch (one we started), shelf, or none (left where it
    /// is; Steam's games and the like are placed by the state, which knows which entry they are).
    pub fn home(&self) -> Option<Instance> {
        self.instance().or_else(|| (self.app_id == SHELF_APP_ID).then_some(Instance::Shelf))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn windows_know_their_instance_and_home() {
        let w = |unit: Option<&str>, app_id: &str| Window { unit: unit.map(Into::into), app_id: app_id.into(), ..Window::default() };
        let terminal = Instance::Run { key: "foot_1".into(), secs: 1234, n: 7 };
        let foot = w(Some("clamshell-run-foot_1-1234-7.scope"), "foot");
        assert_eq!(foot.instance(), Some(terminal.clone()));
        assert_eq!(foot.home(), Some(terminal.clone()));
        // Firefox started from that terminal: the terminal's.
        assert_eq!(w(Some("clamshell-run-foot_1-1234-7.scope"), "firefox").home(), Some(terminal));
        assert_eq!(w(Some("app-steam.scope"), "steam_app_620").home(), None, "Steam's: placed by the program");
        assert_eq!(w(Some("app-steam.scope"), "steam").home(), None, "not ours: left alone");
        assert_eq!(w(Some("clamshell.service"), SHELF_APP_ID).home(), Some(Instance::Shelf));
    }
}
