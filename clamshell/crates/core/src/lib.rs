//! The program's state and its rules. There is no I/O here. Every change comes in as an [`Event`], and
//! what the outside world must do comes back as [`Effect`]s. The UI and the system threads only read the
//! state and send events, so all of this can be unit-tested.
//!
//! The state is grouped by subject, and so are the modules. Each area holds its model and its rules. The
//! rules are `impl State` blocks, since most of them read more than one group.
//!
//!
//! - [`shelf`]: the library view on the main screen ([`Shelf`]), with its actions and files;
//! - [`bottom`]: the second screen's tabs ([`Bottom`]), the editor, the on-screen keyboard, and the
//!   screen's own rules;
//! - [`session`]: what is running and in front ([`Session`]), covering launches, windows, programs
//!   found, the tray, the dock and Home;
//! - [`levels`]: volume and brightness, and the slider that sets them;
//! - [`radios`]: Wi-Fi and Bluetooth;
//! - [`settings`]: the settings table and panel, the environment variables, and the fan mode;
//! - [`device`]: the Device view, the account, stick calibration, the device buttons, and idle;
//! - [`status`]: the system readings ([`Sys`]), covering status, metrics, the battery and the time;
//! - [`keymap`]: keymaps and their screens;
//! - [`nav`]: focus and presses ([`Pointer`]), the lists, and the pad's routing ([`Pad`]);
//! - [`kit`]: generic parts the areas build on, which know nothing about them.
//!
//! `apply.rs` passes each event to its area, and then runs everything that follows from it, in order.
//!
//! The workspace lints rule out panics: no `unwrap`, `expect` or indexing. A lookup whose index is valid
//! by construction, such as an enum's position in an array sized for it, or an index held by the
//! library's own tree, falls back quietly. Where a bug could reach a fallback, a `debug_assert!` catches
//! it in tests.

mod apply;
pub mod bottom;
mod clock;
pub mod device;
mod effect;
mod event;
mod id;
pub mod keymap;
pub mod kit;
pub mod levels;
mod mutation;
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
mod watch;

use bottom::keyboard::Keyboard;
use resume::Saves;
use settings::Settings;

// What other crates use to talk to the state: the events and effects and what they carry, the ids, and
// the groups the getters return.
pub use clock::Clock;
pub use effect::{Dirty, Effect, Save, Watch, WindowCommand};
pub use event::{Event, LaunchEvent, LevelEvent, PointerEvent, Reading, ShelfEvent, Slide};
pub use id::{AudioId, BatteryId, BtId, DeviceId, DockId, EditorId, Hold, Id, KeymapId, SettingsId, ShelfId, Target, TimeId, WifiId};
pub use resume::Resume;

pub use bottom::editor::Editor;
pub use bottom::{Bottom, Tab};
pub use kit::list::Scroll;
pub use levels::slider::Control;
pub use levels::{Brightness, Level, Levels, Sound};
pub use nav::lists::{ListEvent, ListId, Lists, RowPart};
pub use nav::pad::{Claimant, Pad, PadInput, PadMouse, PadRoute};
pub use nav::pointer::Pointer;
pub use radios::{RadioStatus, Radios};
pub use session::Session;
pub use session::dock::{Dock, DockItem};
pub use session::instance::Instance;
pub use session::program::Program;
pub use session::running::{Front, Launching, Origin, Running};
pub use settings::panel::{Scope, TileOrigin};
pub use shelf::{Cursor, Shelf};
pub use status::time::{ClockField, TimePage, TimeRow, TimeZones};
pub use status::{Status, Sys};

pub use clamshell_library::{BrowseKind, Browsed, Collection, Entry, Launch, Library, Meta, RunnerOf, runner_key};

/// What the state starts from, as read from disk and the environment.
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
    /// ClamShell is the device's own session, so it may put the device to sleep when idle.
    pub device_session: bool,
    /// Which programs' windows go to the bottom screen.
    pub window_rules: crate::bottom::screen::WindowRules,
    /// Show clocks in 12-hour format rather than 24-hour.
    pub twelve_hour: bool,
    /// This is ClamShell's first start, meaning the Device view hasn't been closed with Done yet. The
    /// Device view opens (see [`device`]).
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
    watches: watch::Watches,
    /// The Device view on the main screen (see [`device`]).
    device: device::Device,
    /// Programs paused or ended while hidden (see [`crate::session::background`]).
    background: crate::session::background::Background,
    /// The entries started with ClamShell, and whether they have been started yet (see
    /// [`crate::session::autostart`]).
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
    /// The keymap screens shown over the settings tab (see [`keymap::screens`]).
    keymap_screens: crate::keymap::screens::Screens,
    /// The keymap in force as last sent to the pad.
    keymap_sent: Option<keymap::Keymap>,
    lists: Lists,
    clock: Clock,
    saves: Saves,
    idle: crate::device::idle::Idle,
    /// ClamShell is the device's own session (see [`Init::device_session`]).
    device_session: bool,
    /// Which programs' windows go to the bottom screen.
    window_rules: crate::bottom::screen::WindowRules,
    /// Whether metadata is being fetched, and what shelf showed when it was last asked to fetch it first
    /// (see [`State::follow_shown`]).
    fetching: bool,
    shown: Option<shelf::ShelfView>,
    /// Pins of entries that no longer exist were dropped at start. The dock is saved once the UI is up.
    pins_dropped: bool,
    /// The clipboard and confirmation for file actions, and the folder being browsed (see
    /// [`crate::shelf::files`]).
    files: crate::shelf::files::Files,
    /// The text on the system clipboard, for Ctrl+V in the editor. Files copied in the Files tab use their
    /// own clipboard, above.
    clipboard: Option<std::sync::Arc<str>>,
}

impl State {
    pub fn new(init: Init) -> State {
        let mut s = State {
            shelf: Shelf::new(init.library),
            settings: init.settings,
            keymaps: init.keymaps,
            window_rules: init.window_rules,
            sys: Sys::new(init.twelve_hour),
            levels: Levels::new(init.brightness),
            dock: init.dock,
            autostart: init.autostart,
            device_session: init.device_session,
            device: device::Device::new(init.sticks, init.first_start),
            ..State::default()
        };
        if let Some(resume) = &init.resume {
            s.restore(resume);
        }
        // Saved once the UI is up.
        s.pins_dropped = s.drop_gone_pins();
        s.follow_pausable_runners();
        s.pointer.focus = s.shelf.selected().cloned().map(|g| Id::Shelf(ShelfId::Entry(g)));
        if init.first_start {
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

    pub fn clock(&self) -> &Clock {
        &self.clock
    }

    /// Whether the dock menu offers Open another for `id`. It does when `id` is running and another launch
    /// of it makes sense (see [`crate::Entry::another`]).
    pub fn opens_another(&self, id: &str) -> bool {
        // Not for an entry handed over to another program, such as Android's: launching it again just
        // shows the one already running.
        self.session.running(id).is_some() && self.entry(id).is_some_and(|g| g.another && !g.runner.as_ref().is_some_and(|r| r.handover))
    }

    /// A library entry by its id. An entry that is still running after leaving the library, because its
    /// file was deleted, is returned as it was.
    pub fn entry(&self, id: &str) -> Option<&Entry> {
        self.shelf.entry(id).or_else(|| self.session.running(id).and_then(|r| r.entry.as_deref()))
    }

    /// Whether the pad, while on ClamShell's screens, may go to shelf's screen or only to the second
    /// screen. It may go to shelf's when shelf is in front and nothing else has the pad.
    pub fn pad_on_top(&self) -> bool {
        self.pad.on_top()
    }
}
