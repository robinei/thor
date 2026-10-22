//! The program's state and its rules. No I/O: every change comes in as an [`Event`], and what the outside
//! world must do comes back as [`Effect`]s. The UI and the system threads only read the state and send
//! events, so all of this is unit-testable.
//!
//! The state is grouped by what it is about: shelf ([`Shelf`]), the second screen ([`Bottom`]), what runs
//! and is in front ([`Session`]), the dock ([`Dock`]), volume and brightness ([`Levels`]), the radios
//! ([`Radios`]), the controller ([`Pad`]), focus and presses ([`Pointer`]), and the system's readings
//! ([`Sys`]). The rules are a module per feature, each an `impl State` block, since most features read
//! more than one group. A few pure models live here too because the system threads share them with the
//! state: the fan's mode ([`settings::fan`]) and the sticks' calibration ([`crate::device::sticks`]).
//!
//! Panics are ruled out by the workspace's lints (no `unwrap`, `expect` or indexing). A lookup whose
//! index is valid by construction (an enum's place in an array sized for it, an index the library's own
//! tree holds) falls back quietly; where a bug could reach a fallback, a `debug_assert!` says so in tests.

mod apply;
pub mod bottom;
pub mod device;
mod effect;
mod event;
mod id;
pub mod keymap;
pub mod kit;
pub mod levels;
pub mod nav;
pub mod radios;
mod resume;
pub mod session;
pub mod settings;
pub mod shelf;
pub mod status;
#[cfg(test)]
#[allow(clippy::unwrap_used)]
pub(crate) mod testkit;
#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests;

use crate::bottom::keyboard::Keyboard;
pub use resume::Resume;
use resume::Saves;
use settings::Settings;

pub use crate::bottom::editor::Editor;
pub use crate::bottom::{Bottom, Tab};
pub use crate::kit::list::Scroll;
pub use crate::nav::lists::{ListEvent, ListId, Lists, RowPart};
pub use crate::nav::pad::{Claimant, Pad, PadInput, PadMouse, PadRoute};
pub use crate::nav::pointer::Pointer;
pub use crate::session::dock::{Dock, DockItem};
pub use crate::session::instance::Instance;
pub use crate::session::program::Program;
pub use crate::session::running::{Front, Launching, Origin, Running};
pub use crate::settings::env::{count_env, parse_env};
pub use crate::settings::panel::{Scope, TileOrigin};
pub use crate::status::time::{ClockField, TimePage, TimeRow, TimeZones};
pub use crate::status::{Status, Sys};
pub use effect::{Dirty, Effect, Save, Watch, WindowCommand};
pub use event::{Event, LaunchEvent, LevelEvent, PointerEvent, Reading, ShelfEvent, Slide};
pub use id::{AudioId, BatteryId, BtId, DeviceId, DockId, EditorId, Hold, Id, KeymapId, SettingsId, ShelfId, Target, TimeId, WifiId};
pub use levels::{Brightness, Level, Levels, Sound};
pub use radios::{RadioStatus, Radios};
pub use session::Session;
pub use shelf::{Cursor, Shelf};

pub use clamshell_library::{BrowseKind, Browsed, Collection, Entry, Launch, Library, Meta, RunnerOf, runner_key};

/// What the state starts from: what was read from disk and the environment.
#[derive(Debug, Default)]
pub struct Init {
    pub library: Library,
    /// Every game's settings and environment.
    pub settings: Settings,
    /// The keymaps.
    pub keymaps: keymap::Keymaps,
    /// The screens' brightness as it was left.
    pub brightness: Brightness,
    /// The dock's pinned entries, in order.
    pub dock: Dock,
    /// The entries started with ClamShell (see [`crate::session::autostart`]).
    pub autostart: Vec<std::sync::Arc<str>>,
    /// Where shelf and the second screen were.
    pub resume: Option<Resume>,
    /// The tab to start in instead (a development option).
    pub start_tab: Option<Tab>,
    /// Start with the pad mouse on.
    pub pad_mouse: bool,
    /// ClamShell is the device's own session: it may put the device to sleep when idle.
    pub device_session: bool,
    /// Which programs' windows go to the bottom screen.
    pub window_rules: crate::bottom::screen::WindowRules,
    /// Clocks on a 12-hour dial (else 24-hour).
    pub twelve_hour: bool,
    /// ClamShell's first start (the Device view not closed with Done yet): the Device view opens (see
    /// [`device`]).
    pub first_start: bool,
    /// The sticks' calibrations, by controller (see [`crate::device::sticks`]).
    pub sticks: crate::device::sticks::Calibrations,
}

#[derive(Debug, Default)]
pub struct State {
    shelf: Shelf,
    bottom: Bottom,
    session: Session,
    dock: Dock,
    /// Programs' tray icons (see [`crate::session::tray`]).
    tray: Vec<crate::session::tray::TrayItem>,
    /// The UI is up (its Start), and the watch in force (see [`State::follow_watch`]).
    ui_up: bool,
    watching: Option<Watch>,
    /// The account is watched (the Device view open: see [`State::follow_watch`]).
    account_watched: bool,
    /// The Device view on the main screen (see [`device`]).
    device: device::Device,
    /// The screens are off: the bar's status isn't watched (see [`Watch::Status`]).
    status_unwatched: bool,
    /// The battery tab's row whose menu is open (see [`crate::status::battery`]).
    spender_menu: Option<crate::status::battery::Group>,
    /// Programs paused or ended while hidden (see [`crate::session::background`]).
    background: crate::session::background::Background,
    /// The entries started with ClamShell, and whether they have been (see [`crate::session::autostart`]).
    autostart: Vec<std::sync::Arc<str>>,
    autostarted: bool,
    levels: Levels,
    radios: Radios,
    pad: Pad,
    pointer: Pointer,
    sys: Sys,
    settings: Settings,
    keyboard: Keyboard,
    keymaps: keymap::Keymaps,
    /// The keymaps' screens over the settings tab (see [`keymap::screens`]).
    keymap_screens: crate::keymap::screens::Screens,
    /// The keymap in force as last sent to the pad.
    keymap_sent: Option<keymap::Keymap>,
    lists: Lists,
    unix_secs: u64,
    /// A monotonic clock, as last ticked (see [`Event::Tick`]): every duration is timed by it (idle, the
    /// bottom screen, notices, saves, plays, background grace, handoffs), so that a step of the wall clock
    /// (by hand, by NTP) is no time gone by. The wall clock dates things only.
    mono_secs: u64,
    saves: Saves,
    idle: crate::device::idle::Idle,
    /// ClamShell is the device's own session (see [`Init::device_session`]).
    device_session: bool,
    /// Which programs' windows go to the bottom screen.
    window_rules: crate::bottom::screen::WindowRules,
    /// Clocks on a 12-hour dial (see [`crate::status::time`]).
    pub(crate) twelve_hour: bool,
    /// Metadata is being fetched; what shelf showed when it was last asked for first (see
    /// [`State::follow_shown`]).
    fetching: bool,
    shown: Option<shelf::ShelfView>,
    /// Pins of entries gone were dropped at start: saved once the UI is up.
    pins_dropped: bool,
    /// ClamShell's first start (see [`Init::first_start`]).
    first_start: bool,
    /// The file actions' clipboard and confirmation, the folder browsed (see [`crate::shelf::files`]).
    files: crate::shelf::files::Files,
    /// The text on the system's clipboard, for the editor's Ctrl+V (files copied in the Files tab are its
    /// own, above).
    clipboard: Option<std::sync::Arc<str>>,
}

impl State {
    pub fn new(init: Init) -> State {
        let mut s = State {
            shelf: Shelf::new(init.library),
            settings: init.settings,
            keymaps: init.keymaps,
            window_rules: init.window_rules,
            twelve_hour: init.twelve_hour,
            levels: Levels::new(init.brightness),
            dock: init.dock,
            autostart: init.autostart,
            device_session: init.device_session,
            first_start: init.first_start,
            ..State::default()
        };
        if let Some(resume) = &init.resume {
            s.restore(resume);
        }
        // (Saved once the UI is up.)
        s.pins_dropped = s.drop_gone_pins();
        s.follow_pausable_runners();
        s.pointer.focus = s.shelf.selected().cloned().map(|g| Id::Shelf(ShelfId::Entry(g)));
        s.device.calibrations = init.sticks;
        if init.first_start {
            s.device.open = true;
            s.pointer.focus = Some(Id::Device(crate::DeviceId::Wifi));
        }
        if let Some(tab) = init.start_tab {
            s.bottom.tab = tab;
        }
        s.reconcile_pad();
        s.keymap_sent = Some(s.keymap_in_force());
        if init.pad_mouse {
            s.pad.toggle_mouse();
            s.reconcile_pad();
        }
        s
    }

    pub fn shelf(&self) -> &Shelf {
        &self.shelf
    }

    pub fn bottom(&self) -> &Bottom {
        &self.bottom
    }

    pub fn session(&self) -> &Session {
        &self.session
    }

    pub fn levels(&self) -> &Levels {
        &self.levels
    }

    pub fn radios(&self) -> &Radios {
        &self.radios
    }

    pub fn pad(&self) -> &Pad {
        &self.pad
    }

    pub fn pointer(&self) -> &Pointer {
        &self.pointer
    }

    pub fn sys(&self) -> &Sys {
        &self.sys
    }

    pub fn settings(&self) -> &Settings {
        &self.settings
    }

    pub fn keyboard(&self) -> &Keyboard {
        &self.keyboard
    }

    pub fn idle(&self) -> &crate::device::idle::Idle {
        &self.idle
    }

    /// Wall clock (epoch seconds), as last ticked.
    pub fn unix_secs(&self) -> u64 {
        self.unix_secs
    }

    /// Whether the clocks have been read (the first [`Event::Tick`] came).
    pub(crate) fn clock_read(&self) -> bool {
        self.unix_secs != 0
    }

    /// Epoch seconds at monotonic time `mono` (as the wall clock reads now: a step of it since moves it).
    pub(crate) fn unix_at(&self, mono: u64) -> u64 {
        self.unix_secs.saturating_sub(self.mono_secs.saturating_sub(mono))
    }

    /// Whether the dock menu offers Open another for `id`: it runs, and another launch of it makes sense
    /// (see [`crate::Entry::another`]).
    pub fn opens_another(&self, id: &str) -> bool {
        // (Not one handed over to another program, Android's: a launch shows the one it runs.)
        self.session.running(id).is_some() && self.entry(id).is_some_and(|g| g.another && !g.runner.as_ref().is_some_and(|r| r.handover))
    }

    /// A library entry by its id, or one that runs after leaving the library (its file deleted), as it was.
    pub fn entry(&self, id: &str) -> Option<&Entry> {
        self.shelf.entry(id).or_else(|| self.session.running(id).and_then(|r| r.entry.as_deref()))
    }

    /// Whether the pad, on ClamShell's screens, may go to shelf's (shelf is in front and nothing else
    /// has the pad), or only to the second screen.
    pub fn pad_on_top(&self) -> bool {
        self.pad.on_top()
    }
}
