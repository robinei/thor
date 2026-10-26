//! What the outside is kept up to date with. After an event and the steps that follow it,
//! [`crate::State::wanted`] says what the outside should have as things are now ([`Wanted`]), and [`Sent`]
//! compares that with what it was last sent and sends what differs, in a fixed order. An event that
//! leaves the outside in an unknown state, such as the UI starting, has some of it sent again
//! ([`Resend`]).

use std::path::{Path, PathBuf};
use std::sync::Arc;

use crate::device::idle::Stage;
use crate::effect::{Effect, Watch};
use crate::keymap::screens::Keymapping;
use crate::out::Out;
use crate::settings::{Setting, Values};
use crate::shelf::files::{Browse, BrowseCommand};
use crate::shelf::{Shelf, ShelfView};
use crate::{Levels, PadRoute, Running};

/// What the outside should have as things are now. It is built after every event, so it holds
/// references and small values only.
pub(crate) struct Wanted<'a> {
    pub(crate) shelf: &'a Shelf,
    /// What shelf shows while metadata is being fetched (see [`Shelf::fetch_first`]).
    pub(crate) fetch_view: Option<ShelfView>,
    /// The open browsed folder and its directory (see [`Shelf::browsed`]), and whether hidden files are
    /// listed.
    pub(crate) browsed: Option<(&'a Arc<str>, &'a Path)>,
    pub(crate) hidden: bool,
    /// The running entries, whose files are in use (see [`Shelf::in_use`]).
    pub(crate) running: &'a [Running],
    /// How far idle has gone, and the levels, for the screens' brightness while dimmed.
    pub(crate) stage: Stage,
    pub(crate) levels: &'a Levels,
    /// The program in force and the settings in force (see [`crate::State::in_force`]).
    pub(crate) program: Option<&'a Arc<str>>,
    pub(crate) settings: Values,
    /// The refresh rate of the last entry launched by the event being handled, if one was.
    pub(crate) launched: Option<u16>,
    /// The screen's refresh rates (see [`crate::device::display::DisplayModes::refresh_for`]).
    pub(crate) modes: crate::device::display::DisplayModes,
    /// The bottom screen is on (see [`crate::Bottom::off`]).
    pub(crate) bottom_on: bool,
    /// ClamShell's view is on the bottom screen, and that screen is on.
    pub(crate) bottom_seen: bool,
    /// What the open tab watches.
    pub(crate) tab: Option<Watch>,
    /// The Device view is open.
    pub(crate) device_open: bool,
    /// The keymap in force (see [`Keymapping::version_of`]), and the keymaps it is taken from.
    pub(crate) keymap: (u16, u64),
    pub(crate) keymapping: &'a Keymapping,
    /// Where the pad goes.
    pub(crate) route: PadRoute,
    pub(crate) pad_activity: bool,
}

/// What the outside was last sent.
#[derive(Debug)]
pub(crate) struct Sent {
    /// What shelf showed when the entries it shows were last asked for first.
    fetch_view: Option<ShelfView>,
    /// The folder listed and watched.
    browsing: Option<Browse>,
    /// A hash of the files in use (see [`BrowseCommand::InUse`]); 0 for none.
    in_use: u64,
    /// The idle stage the screens were last set for.
    stage: Stage,
    /// The program in force and its settings as they were applied last, or None to apply them again.
    applied: Option<(Option<Arc<str>>, Values)>,
    /// The screens' power as `Effect::Outputs` has it (whether they are on, and the bottom one while
    /// they are), or None to set it again.
    outputs: Option<(bool, bool)>,
    /// The main screen's refresh rate.
    refresh: u16,
    /// The UI is up (it has sent Start). Before that, no tab or account is watched.
    ui_up: bool,
    /// The bar's status isn't watched, since the screens are off (see [`Watch::Status`]).
    status_off: bool,
    /// The account is watched.
    account: bool,
    /// The open tab's watch.
    tab: Option<Watch>,
    /// The keymap in force as last sent to the pad, by its id and version, or None to send it again.
    keymap: Option<(u16, u64)>,
    /// The keymap sent, to check that the id and version stand for it.
    #[cfg(debug_assertions)]
    keymap_sent: Option<crate::keymap::Keymap>,
    /// Where the pad was last sent, or None to send it again.
    route: Option<PadRoute>,
    pad_activity: bool,
}

impl Default for Sent {
    /// As at start: the bottom screen on and the bar's status watched. The settings in force are taken as
    /// applied by [`Sent::new`].
    fn default() -> Sent {
        Sent {
            fetch_view: None,
            browsing: None,
            in_use: 0,
            stage: Stage::Awake,
            applied: None,
            outputs: Some((true, true)),
            refresh: 0,
            ui_up: false,
            status_off: false,
            account: false,
            tab: None,
            keymap: None,
            #[cfg(debug_assertions)]
            keymap_sent: None,
            route: None,
            pad_activity: false,
        }
    }
}

/// Why the outside is to be sent something again, whatever it was last sent.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Resend {
    /// The library was read again or fetching started or stopped: shelf's entries shown are asked for
    /// first again.
    FetchFirst,
    /// The UI started: the pad is sent its route and keymap.
    Start,
    /// Back from sleep, which may have reset what the settings set, such as frequency caps and cores: they
    /// are applied again.
    Resumed,
    /// The refresh rate calibration turned the bottom screen off while it ran and then restored it: it is
    /// set to what ClamShell wants.
    RefreshCalibrated,
}

impl Sent {
    /// Forgets what was sent for `why`, so that it is sent again.
    pub(crate) fn resend(&mut self, why: Resend) {
        match why {
            Resend::Start => (self.keymap, self.route) = (None, None),
            Resend::FetchFirst => self.fetch_view = None,
            Resend::Resumed => self.applied = None,
            Resend::RefreshCalibrated => self.outputs = None,
        }
    }

    /// As at start, where the pad is taken to have what `w` says.
    pub(crate) fn new(w: &Wanted) -> Sent {
        Sent {
            applied: Some((w.program.cloned(), w.settings)),
            refresh: w.modes.refresh_for(w.settings.get(Setting::Refresh), w.settings.get(Setting::FpsCap)),
            keymap: Some(w.keymap),
            route: Some(w.route),
            #[cfg(debug_assertions)]
            keymap_sent: Some(w.keymapping.in_force(w.settings.get(Setting::Keymap))),
            ..Sent::default()
        }
    }

    /// The UI is up. The open tab's and the account's watches start from now.
    pub(crate) fn ui_started(&mut self) {
        self.ui_up = true;
    }

    /// Sends what differs from `w`, in this order. The screens go off before the device sleeps. Nothing
    /// outside depends on the settings coming before the keymap. The keymap comes before the route: when a game comes in front, the pad thread then
    /// takes the game's keymap while it doesn't have the pad yet, and on getting it presses what that
    /// keymap makes of the buttons held, rather than entering the game with the old keymap and swapping
    /// it there (see the system crate's `PadRouter::set_route` and `set_keymap`).
    pub(crate) fn sync(&mut self, w: &Wanted, out: &mut Out) {
        self.fetch_first(w, out);
        self.browse(w, out);
        self.idle(w, out);
        let applied = self.settings(w, out);
        self.refresh(w, applied, out);
        self.watches(w, out);
        self.keymap(w, out);
        self.pad_route(w, out);
    }

    /// While metadata is being fetched and what shelf shows has changed, asks for the shown entries'
    /// metadata first.
    fn fetch_first(&mut self, w: &Wanted, out: &mut Out) {
        let Some(view) = w.fetch_view else {
            return;
        };
        if self.fetch_view == Some(view) {
            return;
        }
        self.fetch_view = Some(view);
        let mut ids = Vec::new();
        w.shelf.in_view(&mut ids);
        out.effects.push(Effect::FetchFirst(ids));
    }

    /// The open browsed folder is listed and watched when it becomes the open one, and the watch is
    /// dropped when there is none. The files in use are told when they change.
    fn browse(&mut self, w: &Wanted, out: &mut Out) {
        let open = w.browsed.map(|(folder, dir)| (folder, dir, w.hidden));
        if self.browsing.as_ref().map(|b| (&b.folder, b.dir.as_path(), b.hidden)) != open {
            self.browsing = open.map(|(folder, dir, hidden)| Browse { folder: folder.clone(), dir: dir.to_path_buf(), hidden });
            out.effects.push(Effect::Browse(BrowseCommand::Open(self.browsing.clone())));
        }
        // Compared by a hash, so that nothing is collected while they stay the same.
        let in_use = w.shelf.in_use(w.running);
        let mut hasher = std::hash::DefaultHasher::new();
        let mut any = false;
        for path in in_use.clone() {
            std::hash::Hash::hash(path, &mut hasher);
            any = true;
        }
        let hash = if any { std::hash::Hasher::finish(&hasher) } else { 0 };
        if hash != self.in_use {
            self.in_use = hash;
            out.effects.push(Effect::Browse(BrowseCommand::InUse(in_use.map(PathBuf::from).collect())));
        }
    }

    /// The screens for the idle stage: brightness goes down while dimmed or darker and comes back when
    /// awake, the screens' power follows (see [`Sent::outputs`]), and the device sleeps.
    fn idle(&mut self, w: &Wanted, out: &mut Out) {
        let was = std::mem::replace(&mut self.stage, w.stage);
        if was != w.stage && (was >= Stage::Dimmed) != (w.stage >= Stage::Dimmed) {
            out.effects.extend(if w.stage >= Stage::Dimmed { w.levels.dimmed_effect() } else { w.levels.undimmed_effect() });
        }
        self.outputs(w, out);
        if was != w.stage && w.stage == Stage::Asleep {
            out.effects.push(Effect::Sleep);
        }
    }

    /// The screens are on while idle hasn't turned them off, and the bottom one as it is set (see
    /// [`crate::Bottom::off`]), when either changes. The UI tells a change of the bottom one alone from
    /// idle by what it was sent last.
    fn outputs(&mut self, w: &Wanted, out: &mut Out) {
        let outputs = (w.stage < Stage::Off, w.bottom_on);
        if self.outputs.replace(outputs) != Some(outputs) {
            out.effects.push(Effect::Outputs { top: outputs.0, bottom: outputs.1 });
        }
    }

    /// The settings in force are applied when the program in force changes, and when they change.
    /// Returns whether they were.
    fn settings(&mut self, w: &Wanted, out: &mut Out) -> bool {
        if self.applied.as_ref().is_some_and(|(program, settings)| program.as_ref() == w.program && *settings == w.settings) {
            return false;
        }
        self.applied = Some((w.program.cloned(), w.settings));
        out.effects.push(Effect::Apply { entry: w.program.cloned(), settings: w.settings });
        true
    }

    /// The main screen's refresh rate follows the settings of the last entry launched, from its launch,
    /// and the settings in force when they are applied, whichever came last: their Refresh, or under a
    /// frame rate cap one it divides (see [`crate::device::display::DisplayModes::refresh_for`]).
    fn refresh(&mut self, w: &Wanted, applied: bool, out: &mut Out) {
        let hz = if applied { Some(w.modes.refresh_for(w.settings.get(Setting::Refresh), w.settings.get(Setting::FpsCap))) } else { w.launched };
        if let Some(hz) = hz.filter(|&hz| hz != self.refresh) {
            self.refresh = hz;
            out.effects.push(Effect::Refresh(hz));
        }
    }

    /// The bar's status is watched while the screens are on, the account while the Device view is open
    /// and the UI is up, and the open tab's watch while the tab can be seen and the UI is up.
    fn watches(&mut self, w: &Wanted, out: &mut Out) {
        let screens_off = w.stage >= Stage::Off;
        if screens_off != std::mem::replace(&mut self.status_off, screens_off) {
            out.effects.push(Effect::Watch(Watch::Status, !screens_off));
        }
        let account = self.ui_up && w.device_open;
        if account != std::mem::replace(&mut self.account, account) {
            out.effects.push(Effect::Watch(Watch::Account, account));
        }
        let seen = self.ui_up && w.bottom_seen && !screens_off;
        let tab = w.tab.filter(|_| seen);
        if tab == self.tab {
            return;
        }
        let was = std::mem::replace(&mut self.tab, tab);
        out.effects.extend(was.map(|w| Effect::Watch(w, false)).into_iter().chain(tab.map(|w| Effect::Watch(w, true))));
    }

    /// The pad gets the keymap in force when it changed.
    fn keymap(&mut self, w: &Wanted, out: &mut Out) {
        let id = w.settings.get(Setting::Keymap);
        if self.keymap == Some(w.keymap) {
            // Compared in place, since this runs after every event.
            #[cfg(debug_assertions)]
            debug_assert!(
                self.keymap_sent.as_ref().is_some_and(|k| match w.keymapping.keymaps().get(id) {
                    Some(stored) => k == stored,
                    None => k.id == 0,
                }),
                "keymap {id} unchanged by its version"
            );
            return;
        }
        self.keymap = Some(w.keymap);
        let keymap = w.keymapping.in_force(id);
        #[cfg(debug_assertions)]
        {
            self.keymap_sent = Some(keymap.clone());
        }
        out.effects.push(Effect::Keymap(keymap));
    }

    /// The pad goes where it is wanted (see [`crate::Pad::reconcile`]).
    fn pad_route(&mut self, w: &Wanted, out: &mut Out) {
        if self.pad_activity != w.pad_activity {
            self.pad_activity = w.pad_activity;
            out.effects.push(Effect::PadActivity(w.pad_activity));
        }
        if self.route.replace(w.route) != Some(w.route) {
            out.effects.push(Effect::PadRoute(w.route));
        }
    }
}
