//! What the system threads are asked to watch: the bar's status, the account while the Device view is
//! open, and what the open tab shows while it can be seen. Each is started and stopped as that changes.

use crate::State;
use crate::bottom::Tab;
use crate::device::idle::Stage;
use crate::effect::{Effect, Watch};

/// The watches in force.
#[derive(Debug, Clone, Default)]
pub(crate) struct Watches {
    /// The UI is up (it has sent Start). Before that, no tab or account is watched.
    ui_up: bool,
    /// The open tab's watch.
    tab: Option<Watch>,
    /// The account is watched.
    account: bool,
    /// The screens are off, so the bar's status isn't watched (see [`Watch::Status`]).
    status_off: bool,
}

/// What the watches follow.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Wanted {
    /// The screens are off for idle.
    pub screens_off: bool,
    /// ClamShell's view is on the bottom screen, and that screen is on.
    pub bottom_seen: bool,
    /// What the open tab watches.
    pub tab: Option<Watch>,
    /// The Device view is open.
    pub device_open: bool,
}

impl Watches {
    pub(crate) fn ui_started(&mut self) {
        self.ui_up = true;
    }

    /// Starts and stops watches for `wanted`. The bar's status is watched while the screens are on, the
    /// account while the Device view is open and the UI is up, and the tab's watch while the tab can be
    /// seen and the UI is up.
    pub(crate) fn follow(&mut self, wanted: Wanted) -> Vec<Effect> {
        let mut effects = Vec::new();
        let screens_off = wanted.screens_off;
        if screens_off != std::mem::replace(&mut self.status_off, screens_off) {
            effects.push(Effect::Watch(Watch::Status, !screens_off));
        }
        let seen = self.ui_up && wanted.bottom_seen && !screens_off;
        let account = self.ui_up && wanted.device_open;
        if account != std::mem::replace(&mut self.account, account) {
            effects.push(Effect::Watch(Watch::Account, account));
        }
        let tab = wanted.tab.filter(|_| seen);
        if tab == self.tab {
            return effects;
        }
        let was = std::mem::replace(&mut self.tab, tab);
        effects.extend(was.map(|w| Effect::Watch(w, false)).into_iter().chain(tab.map(|w| Effect::Watch(w, true))));
        effects
    }
}

impl State {
    /// Keeps the watches up to date (see [`Watches::follow`]). The second screen watches what the open
    /// tab watches. It can be seen when ClamShell's view is on the bottom screen, that screen is on, and
    /// the screens aren't off for idle. The dock with an entry's menu open watches the battery tab's
    /// readings, to show what the entry costs.
    pub(crate) fn follow_watch(&mut self) -> Vec<Effect> {
        let tab = match self.bottom.tab() {
            Tab::Dock if self.dock.menu.is_some() => Some(Watch::Battery),
            tab => tab.watch(),
        };
        let wanted = Wanted {
            screens_off: self.idle.stage() >= Stage::Off,
            bottom_seen: !self.bottom_shows_windows() && !self.bottom_off(),
            tab,
            device_open: self.device.is_open(),
        };
        self.watches.follow(wanted)
    }
}
