//! The program's state and its rules. There is no I/O here. Every change comes in as an [`Event`], and
//! what the outside world must do comes back as [`Effect`]s. The UI and the system threads only read the
//! state and send events, so all of this can be unit-tested.
//!
//! The state is made of areas, one per subject, and so are the modules. Each area is a type that owns its
//! fields privately, with its rules as methods and the getters the UI reads. A rule takes what it reads
//! of other areas as parameters, and an area it changes, where that area alone owns the change, as a
//! `&mut` parameter (a notice, the pointer's focus). The areas:
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
//! [`State`] holds the areas and coordinates them. Outside code reads an area through State's accessor
//! for it (`s.radios().wifi()`), and changes state only by sending an event to [`State::apply`], which
//! runs in three phases:
//!
//! 1. Handle: the event goes to the rules of its area. They push effects into one [`out::Out`] in the
//!    order things happen and mark the screens they changed. What needs several areas, such as a tab
//!    change, playing an entry or choosing a keymap, is queued as an event, which State handles after
//!    the rule returns, so its effects follow those pushed before it. Outside code may send the same
//!    events.
//! 2. Settle: the steps that bring every area up to date with what happened, in an order where each
//!    finds the ones it reads settled (see [`State::settle`]).
//! 3. Sync: what the outside should have as things are now is compared with what it was last sent, and
//!    the difference is sent ([`sync`]).
//!
//! The workspace lints rule out panics: no `unwrap`, `expect` or indexing. A lookup whose index is valid
//! by construction, such as an enum's position in an array sized for it, or an index held by the
//! library's own tree, falls back quietly. Where a bug could reach a fallback, a `debug_assert!` catches
//! it in tests.

pub mod bottom;
mod clock;
pub mod device;
mod effect;
mod event;
mod id;
pub mod keymap;
pub mod kit;
pub mod levels;
pub mod nav;
mod out;
pub mod radios;
mod resume;
pub mod session;
pub mod settings;
pub mod shelf;
mod state;
pub mod status;
mod sync;
#[cfg(test)]
#[allow(clippy::unwrap_used)]
pub(crate) mod testkit;
#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests;

use settings::Settings;

// What other crates use to talk to the state: the events and effects and what they carry, the ids, and
// the groups the getters return.
pub use clock::Clock;
pub use effect::{Dirty, Effect, Save, Watch, WindowCommand};
pub use event::{EntryAct, Event, KeymapEvent, LaunchEvent, LevelEvent, PointerEvent, Reading, ShelfEvent, Slide};
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
pub use session::running::{Front, Launching, Origin, Running};
pub use settings::panel::{Panel, Prefs, Program, Scope, TileOrigin};
pub use shelf::{Cursor, Shelf};
pub use state::State;
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
