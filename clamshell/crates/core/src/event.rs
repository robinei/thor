//! What comes in: everything that changes the state is an [`Event`], from the UI (input, layout) or a
//! system thread (readings of the system, the compositor's windows, systemd's scopes, games starting and
//! ending), grouped by what they are about.

use crate::touch::Pointing;
use std::sync::Arc;

use crate::battery::Battery;
use crate::bluetooth::Bluetooth;
use crate::hardware::DeviceKey;
use crate::id::Id;
use crate::idle::IdleEvent;
use crate::instance::Instance;
use crate::levels::{Audio, Level};
use crate::metrics::Metrics;
use crate::pad::PadInput;
use crate::scroll::ListEvent;
use crate::slider::Control;
use crate::sys::Status;
use crate::wifi::Wifi;
use crate::windows::Window;

#[derive(Debug, Clone, PartialEq)]
pub enum Event {
    /// Wall clock, once a second (system thread).
    Tick {
        unix_secs: u64,
    },
    /// The system as a system thread read it.
    Reading(Reading),
    /// Touch, the mouse or the pad on a target.
    Pointer(PointerEvent),
    /// A right click on the second screen: the clipboard's text into the open field (as Ctrl+V).
    Paste,
    /// A finger or the mouse came down on the second screen (see [`crate::State::bottom_dimmed`]).
    BottomTouched,
    /// A finger on volume or brightness in the bar, sliding.
    Slide(Slide),
    /// A level row set by the finger, or stepped by the pad.
    Level(LevelEvent),
    /// A scrolled list moved or was laid out.
    List(ListEvent),
    /// The pad in shelf, and shelf's layout.
    Shelf(ShelfEvent),
    /// A game shelf started began, ended, or couldn't start (the launcher thread).
    Launch(LaunchEvent),
    /// The compositor's windows, after any change.
    Windows(Vec<Window>),
    /// Our running scopes (dock app instances and games shelf started), after any change.
    Units(Vec<Instance>),
    /// Our slices found frozen at start (their keys), after the first [`Event::Units`]: held as
    /// suspended (see [`crate::background`]).
    Frozen(Vec<std::sync::Arc<str>>),
    /// Programs' tray icons and their menus, after any change (the tray thread).
    Tray(Vec<crate::tray::TrayItem>),
    /// What was fetched about an entry (its cover, a screenshot, ...): the metadata thread.
    Meta {
        id: Arc<str>,
        meta: crate::Meta,
    },
    /// The library read again (the Tools tab's rescan).
    Library(Box<crate::Library>),
    /// Browsed folders listed, file operations under way or done, removable storage come or gone (the
    /// files thread).
    Files(crate::files::FilesEvent),
    /// The metadata thread started fetching (shelf's view is fetched first meanwhile), or is done.
    Fetching(bool),
    /// The UI is up: what the state in force needs at start (the tab's watch, the screens' brightness,
    /// where the pad goes).
    Start,
    /// The pad's home button.
    Home,
    /// View, away from shelf: the pad between the second screen and what else wants it (the pad mouse,
    /// typing).
    PadSwitch,
    /// The pad, while it drives ClamShell (the UI handles these; the state ignores them).
    Pad(PadInput),
    /// An input pressed or moved on the pad while it listens (the keymap's edit screen).
    Listened(crate::keymap::Input),
    /// A text field of the app in front wants typing (true), or no longer does (the Wayland side).
    TextInput(bool),
    /// The keyboard points (see [`crate::keyboard`]).
    Pointing(Pointing),
    /// The system woke from sleep.
    Resumed,
    /// What the input sources say about input (for idle).
    Idle(IdleEvent),
    /// A hardware key (the device's buttons, the pad's Home with RB), or the same asked for by
    /// `clamshell ctl`.
    Device(DeviceKey),
    /// The screenshot asked for is taken (its file), or failed (why).
    Screenshot(Result<String, String>),
    /// A calibration of the gyro asked for ended: whether it took (the device was held still enough).
    GyroCalibrated(bool),
    Quit,
}

/// What a system thread read from the system.
#[derive(Debug, Clone, PartialEq)]
pub enum Reading {
    /// Battery, Wi-Fi signal, volume, brightness (when they change).
    Status(Status),
    /// The picture found for an entry found by its window (see [`crate::Effect::FindIcon`]).
    Icon { entry: Arc<str>, path: std::path::PathBuf },
    /// The text on the clipboard (as it changes; none: something else, or nothing), for ClamShell's own
    /// fields' Ctrl+V.
    Clipboard(Option<Arc<str>>),
    /// CPU, GPU, memory, temperatures, power (once a second).
    Metrics(Metrics),
    /// A running game's frame rate and frame time in ms (MangoHud), or none.
    Frames { fps: Option<f32>, frame_ms: Option<f32> },
    /// Wi-Fi as NetworkManager has it, after any change (while the Wi-Fi tab is open, or a join runs).
    Wifi(Wifi),
    /// Bluetooth as BlueZ has it, after any change.
    Bluetooth(Bluetooth),
    /// The charge history and what is using the machine (every few seconds while the battery tab is
    /// open).
    Battery(Battery),
    /// The outputs and the apps playing (while the volume tab is open).
    Audio(Audio),
    /// The backlights' names (while the brightness tab is open).
    Backlights(Vec<String>),
    /// The time zone and the ones to choose from (while the time tab is open, and after setting one).
    Time(crate::time::TimeZones),
}

/// A press on a target, from touch, the mouse or the pad's A; and the pad's focus.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PointerEvent {
    /// A finger, button or the pad's A went down on / up from a target: shown as pressed meanwhile.
    Down(Id),
    Up(Id),
    /// Tap, click, or the pad's A on a focused item.
    Activate(Id),
    /// A press held longer than a tap (locks a keyboard modifier; otherwise as `Activate`).
    LongPress(Id),
    /// The pad moved focus (the UI finds the target, since only it knows the layout). Shows the focus.
    Focus(Id),
    /// Touch or mouse input began: the focus ring is hidden until the pad moves focus again.
    HideFocus,
}

/// A finger on a bar control, sliding (`x` in the second screen's layout units, which is `width` wide).
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Slide {
    /// The finger came down (or was held long enough): the slider comes up under it.
    Start {
        control: Control,
        x: f32,
        width: f32,
    },
    To {
        x: f32,
    },
    /// The finger that slides lifted.
    End,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LevelEvent {
    /// A level row set by the finger.
    Set(Level, u8),
    /// A level row stepped by the pad (-1 or 1).
    Step(Level, i16),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ShelfEvent {
    /// The pad moved through the list: by this many games (a page is the rows on screen).
    ListStep(i32),
    /// The pad switched collections: by this many.
    CollectionStep(i32),
    /// How many list rows shelf shows (from the layout), so the selection stays on screen.
    Rows(usize),
    /// The pad's Y (desktop: F): the selected entry in or out of the favourites.
    ToggleFavourite,
    /// What is typed to find an entry by cleared (B, while there is some).
    ClearFilter,
    /// The pad's X (desktop: P): the selected entry pinned to the dock, or unpinned.
    TogglePin,
    /// The pad's Start (desktop: C): the selected folder's next video played (a series' episode).
    Continue,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LaunchEvent {
    /// The launcher started a game.
    Started(Arc<str>),
    /// The launcher is done with a launch of it: it ended (whether the entry still runs is the state's
    /// to say, by its scopes and its windows).
    Ended(Arc<str>),
    /// It couldn't be started.
    LaunchFailed(Arc<str>),
    /// A video mpv played ended: where mpv left it (none: at its end).
    Position { game: Arc<str>, position: Option<u32> },
}

macro_rules! into_event {
    ($($group:ident => $variant:ident),*) => {
        $(impl From<$group> for Event {
            fn from(e: $group) -> Event {
                Event::$variant(e)
            }
        })*
    };
}

into_event!(ListEvent => List, Reading => Reading, PointerEvent => Pointer, Slide => Slide, LevelEvent => Level, ShelfEvent => Shelf, LaunchEvent => Launch, Pointing => Pointing);
