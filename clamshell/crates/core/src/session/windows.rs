//! The compositor's windows, as the dock and the running entries need them. Each launch of a library entry
//! is a systemd scope we started. Every window of its processes, such as a second window or a program
//! started from its terminal, goes on the entry's workspace as a tab (see [`Instance`]). An entry that
//! runs in another program's processes, such as a Steam game or a Waydroid app, gets a workspace of its own
//! too, and its windows are found by their app id (see [`crate::session::running`]).

use crate::session::instance::Instance;

/// The app id of shelf's window, its own Wayland surface on the main screen.
pub const SHELF_APP_ID: &str = "clamshell.top";

/// An open window, from sway's tree.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Window {
    /// The compositor's id for it, to focus or close it by.
    pub id: i64,
    /// Its Wayland app id, or its X11 class.
    pub app_id: String,
    pub title: String,
    pub focused: bool,
    /// The systemd unit its process runs in.
    pub unit: Option<String>,
    /// Its process, then that process's parent, and so on, leaving out pid 1. A program found by its tray
    /// icon's process has the windows of that process and of the processes it started.
    pub pids: Vec<u32>,
    /// The workspace it is on.
    pub workspace: String,
    /// Whether we moved it to the bottom screen, which sway's mark on it records. It is left there.
    pub down: bool,
    /// Whether it is in view: the tab its workspace shows, on a workspace that is shown.
    pub visible: bool,
}

impl Window {
    /// The launch it belongs to: its scope's, if we started it.
    pub fn instance(&self) -> Option<Instance> {
        self.unit.as_deref().and_then(Instance::from_unit)
    }

    /// Whose workspace it belongs on: its launch's, if we started it, or shelf's. Otherwise none, and it is
    /// left where it is; Steam's games and the like are placed by the state, which knows their entries.
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
        // Firefox started from that terminal belongs to the terminal.
        assert_eq!(w(Some("clamshell-run-foot_1-1234-7.scope"), "firefox").home(), Some(terminal));
        assert_eq!(w(Some("app-steam.scope"), "steam_app_620").home(), None, "Steam's: placed by the program");
        assert_eq!(w(Some("app-steam.scope"), "steam").home(), None, "not ours: left alone");
        assert_eq!(w(Some("clamshell.service"), SHELF_APP_ID).home(), Some(Instance::Shelf));
    }
}
