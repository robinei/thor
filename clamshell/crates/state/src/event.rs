//! What comes in. Everything that changes the state is an [`Event`]. Events come from the UI (input and
//! layout) or from a system thread (system readings, the compositor's windows, systemd's units, games
//! starting and ending), and are grouped by subject.

use crate::bottom::touch::Pointing;
use std::sync::Arc;

use crate::DeviceKey;
use crate::id::Id;
use crate::idle::IdleEvent;
use crate::kit::list::ItemKey;
use crate::levels::slider::Control;
use crate::levels::{Audio, Level};
use crate::nav::lists::ListEvent;
use crate::nav::pad::PadInput;
use crate::notifications::NotificationEvent;
use crate::radios::bluetooth::Bluetooth;
use crate::radios::wifi::Wifi;
use crate::session::launch_unit::LaunchUnit;
use crate::session::windows::WindowEvent;
use crate::status::Status;
use crate::status::battery::Battery;
use crate::status::metrics::Metrics;

#[derive(Debug, Clone, PartialEq)]
pub enum Event {
    /// The clocks, sent once a second by a system thread. The wall clock, in epoch seconds, is for dating
    /// things. The monotonic clock (clamshell-system's `clock::mono_secs`) is for timing durations, so
    /// that a jump in the wall clock, by hand or by NTP, doesn't count as time passing.
    Tick {
        unix_secs: u64,
        mono_secs: u64,
    },
    /// Something a system thread read from the system.
    Reading(Reading),
    /// A notification sent or closed by a program, or tapped (see [`crate::notifications`]).
    Notification(crate::notifications::NotificationEvent),
    /// Touch, the mouse or the pad on a target.
    Pointer(PointerEvent),
    /// A right click on the second screen. Pastes the clipboard's text into the open field, like Ctrl+V.
    Paste,
    /// A finger or the mouse came down on the second screen (see [`crate::Bottom::dimmed`]).
    BottomTouched,
    /// A finger sliding on volume or brightness in the bar.
    Slide(Slide),
    /// A level row set by a finger or stepped by the pad.
    Level(LevelEvent),
    /// A scrolled list moved or was laid out.
    List(ListEvent),
    /// The pad in shelf, and shelf's layout.
    Shelf(ShelfEvent),
    /// From the launcher thread: a game started by shelf couldn't start, or a video ended.
    Launch(LaunchEvent),
    /// A window's lifecycle or an activation request. Placement is decided by the session.
    Window(WindowEvent),
    /// Changes from one dispatch of the compositor's X11 window manager (a window's facts and its title),
    /// applied as one event.
    WindowBatch(Vec<WindowEvent>),
    /// Our running units, after any change. These are dock app instances and games started by shelf.
    Units(Vec<LaunchUnit>),
    /// The keys of our slices found frozen at start, sent after the first [`Event::Units`]. They are
    /// treated as suspended (see [`crate::session::background`]).
    Frozen(Vec<std::sync::Arc<str>>),
    /// Programs' tray icons and their menus, sent by the tray thread after any change.
    Tray(Vec<crate::session::tray::TrayItem>),
    /// What the metadata thread fetched about an entry, such as its cover or a screenshot.
    Meta {
        id: Arc<str>,
        meta: crate::Meta,
    },
    /// The library was read again, by the Tools tab's rescan.
    Library(Box<crate::Library>),
    /// From the files thread: a browsed folder listed, file operations under way or done, or removable
    /// storage added or removed.
    Files(crate::shelf::files::FilesEvent),
    /// The metadata thread started fetching, or is done. While it fetches, what shelf shows is fetched
    /// first.
    Fetching(bool),
    /// The UI is up. The state sends what it needs at start: the tab's watch, the screens' brightness,
    /// and where the pad goes.
    Start,
    /// The pad's home button.
    Home,
    /// Whether the controller's Home button is held, for the window-switch arrows.
    HomeHeld(bool),
    /// The controller or desktop Home key was released without a button chord.
    HomeTap,
    /// Controller input returns keyboard focus to the content selected on top.
    PadActive,
    /// The View button, away from shelf. Switches the pad between the second screen and whatever else
    /// wants it, such as the pad mouse or typing.
    PadSwitch,
    /// The pad while it drives ClamShell. The UI handles these and the state ignores them.
    Pad(PadInput),
    /// An input pressed or moved on the pad while it listens, on the keymap edit screen.
    Listened(crate::keymap::Input),
    /// A text field of the app in front wants typing (true) or no longer does. Comes from the Wayland
    /// side.
    TextInput(bool),
    /// Touchpad contacts on the second screen, or none (see [`crate::bottom::touch`]).
    Pointing(Pointing),
    /// The system woke from sleep.
    Resumed,
    /// What the input sources report about input, for idle.
    Idle(IdleEvent),
    /// A hardware key, such as the device's buttons or the pad's Home with RB, or the same key sent by
    /// `clamshell ctl`.
    Device(DeviceKey),
    /// The requested screenshot was taken, with its file, or failed, with the reason.
    Screenshot(Result<String, String>),
    /// Dump recent in-process compositor window inputs for diagnosis and replay.
    DumpWindowEvents(std::path::PathBuf),
    /// Calibrates the gyro. A notice says to hold the device still until it ends.
    CalibrateGyro,
    /// A requested gyro calibration ended. True if it worked, meaning the device was held still enough.
    GyroCalibrated(bool),
    /// Progress or the end of a stick calibration (see [`crate::setup`]).
    Sticks(crate::setup::SticksEvent),
    /// The top screen's refresh rate calibration ended, with its output or the reason it failed.
    RefreshCalibrated(Result<String, String>),
    /// The running refresh rate calibration asks for the main screen at this rate, in Hz (`clamshell ctl
    /// calibration rate HZ`).
    CalibrationRate(u16),
    /// A system thread was started again after a panic, having lost what it held (see
    /// [`crate::apply::restart`]).
    Restarted(Thread),
    /// A change requested of the account helper is done, with what it did, or failed, with the reason. A
    /// new account reading follows (see [`crate::setup::account`]).
    AccountChanged(Result<String, String>),
    /// Opens a tab on the second screen, as tapping its icon does.
    ChooseTab(crate::Tab),
    /// What the keymaps' screens ask of the keyboard (see [`crate::keymap::screens`]).
    Keymap(KeymapEvent),
    /// What a kind's picker asks of the settings (see [`crate::profiles::picker`]).
    Profile(crate::profiles::picker::Kind, crate::profiles::picker::ProfileEvent),
    /// Something done to an entry, by its id.
    Entry(Arc<str>, EntryAct),
    /// Adds what shelf's details show, or the entries picked in its open folder, to what their runner
    /// plays (see [`crate::session::enqueue`]). It is about shelf's selection, so it names no entry.
    Enqueue,
    /// Opens `editor` on the keyboard tab, as for a file's name. When the editor closes, the screen goes
    /// back to the tab the keyboard came up from. While another editor is open there, it is refused with
    /// a notice.
    EditOnKeyboard(crate::Editor),
    Quit,
}

/// What is done to an entry.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EntryAct {
    /// Starts it, or shows it if it runs (see [`crate::State::play`]).
    Play,
    /// Shows it, as a tap on its dock tile does: its group, thawed if it was paused or suspended.
    Show,
    /// Launches it again, in another tab, if it runs and another launch of it makes sense (see
    /// [`crate::State::opens_another`]).
    OpenAnother,
    /// Freezes it until it is shown (see [`crate::session::background`]).
    Suspend,
    /// Starts it with ClamShell, or stops doing so (see [`crate::session::autostart`]).
    ToggleAutostart,
    /// Pins it to the dock, or unpins it.
    TogglePin,
    /// Quit, as from the dock.
    Quit,
    ForceKill,
}

/// What the keymaps' screens ask of the settings and the keyboard.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KeymapEvent {
    /// The capture screen's Done: the keyboard's modifiers held make the chord.
    Capture,
}

/// What a system thread read from the system.
#[derive(Debug, Clone, PartialEq)]
pub enum Reading {
    /// Battery, Wi-Fi signal, volume and brightness, when they change.
    Status(Status),
    /// The icon found for an entry that was found by its window (see [`crate::Effect::FindIcon`]).
    Icon { entry: Arc<str>, path: std::path::PathBuf },
    /// The text on the clipboard as it changes, for Ctrl+V in ClamShell's own fields. None when the
    /// clipboard holds something else or nothing.
    Clipboard(Option<Arc<str>>),
    /// CPU, GPU, memory, temperatures and power, once a second.
    Metrics(Metrics),
    /// A running game's frame rate and frame time in ms, from MangoHud, or none.
    Frames { fps: Option<f32>, frame_ms: Option<f32> },
    /// Wi-Fi as NetworkManager reports it, after any change. Sent while the Wi-Fi tab is open or a join
    /// is running.
    Wifi(Wifi),
    /// Bluetooth as BlueZ reports it, after any change.
    Bluetooth(Bluetooth),
    /// The charge history and what is using the machine. Sent every few seconds while the battery tab is
    /// open.
    Battery(Battery),
    /// The audio outputs and the apps playing, while the volume tab is open.
    Audio(Audio),
    /// The backlight names, while the brightness tab is open.
    Backlights(Vec<String>),
    /// The time zone and the ones to choose from. Sent while the time tab is open and after setting one.
    Time(crate::status::time::TimeZones),
    /// The account, while the Device view is open and after a change (see [`crate::setup::account`]).
    Account(crate::setup::account::Account),
    /// The screens as the compositor has them, at the start and when they change.
    Screens(Box<crate::presentation::Screens>),
    /// Where the pixels of the plotted window's picture are (see [`crate::State::histogram_window`]).
    Histogram(Box<crate::colour::Histogram>),
    /// The compositor told a game's process, known by its unit or Steam's app id, that its screen has this
    /// resolution, when it connected (see [`crate::State::process_game`]).
    Told { unit: Option<String>, steam_app: Option<u32>, resolution: crate::settings::Resolution },
    /// The top screen's refresh rates at its current size, and its rate at start, from the DRM host at
    /// its start; the nested host sends none.
    DisplayModes(crate::presentation::modes::DisplayModes),
    /// The sizes of the modes Xwayland offers X11 clients, read when it starts: a game switching to one
    /// shows a window of that size (see [`crate::State::window_presentation`]).
    XwaylandModes(Vec<(u32, u32)>),
}

/// A press on a target from touch, the mouse or the pad's A, and pad focus changes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PointerEvent {
    /// A finger, button or the pad's A went down on or up from a target. The target shows as pressed in
    /// between. `Down` carries the key of the list item the target showed when it was drawn, if the UI
    /// has it; with `None` the item is the one at the target's position now (see [`crate::nav::items`]).
    Down(Id, Option<ItemKey>),
    Up(Id),
    /// Tap, click, or the pad's A on a focused item.
    Activate(Id),
    /// A press held longer than a tap. It is handled like `Activate`, as a long press. On a keyboard
    /// modifier it does nothing, since holding one is handled by `Down` and `Up`.
    LongPress(Id),
    /// The pad moved focus, and the focus is shown. The UI finds the target, since only it knows the
    /// layout.
    Focus(Id),
    /// Touch or mouse input began. The focus ring is hidden until the pad moves focus again.
    HideFocus,
}

/// A finger sliding on a bar control. `x` is in the second screen's layout units, and the control is
/// `width` wide.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Slide {
    /// The finger came down, or was held long enough. The slider comes up under it.
    Start {
        control: Control,
        x: f32,
        width: f32,
    },
    To {
        x: f32,
    },
    /// The sliding finger lifted.
    End,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LevelEvent {
    /// A level row set by a finger.
    Set(Level, u8),
    /// A level row stepped by the pad (-1 or 1).
    Step(Level, i16),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ShelfEvent {
    /// The pad moved through the list by this many games. A page is the number of rows on screen.
    ListStep(i32),
    /// The pad switched collections by this many.
    CollectionStep(i32),
    /// How many list rows shelf shows, from the layout, so the selection stays on screen.
    Rows(usize),
    /// The pad's Y (F on the desktop) adds the selected entry to the favourites or removes it.
    ToggleFavourite,
    /// Clears what was typed to find an entry. B does this while there is some.
    ClearFilter,
    /// The pad's X (P on the desktop) pins the selected entry to the dock or unpins it.
    TogglePin,
    /// The pad's Start (C on the desktop) plays the selected folder's next video, such as a series'
    /// next episode.
    Continue,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LaunchEvent {
    /// The entry's launch `unit` couldn't be started: its unit never appeared. Whether a launch is
    /// running, and when it ends, comes from systemd's list of our units ([`crate::Event::Units`]).
    LaunchFailed { entry: Arc<str>, unit: crate::LaunchUnit },
    /// A video played by mpv ended, with where mpv left it. None means it played to the end (see
    /// [`crate::Effect::ReadPosition`]).
    Position { entry: Arc<str>, position: Option<u32> },
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

into_event!(ListEvent => List, Reading => Reading, NotificationEvent => Notification, PointerEvent => Pointer, Slide => Slide, LevelEvent => Level, ShelfEvent => Shelf, LaunchEvent => Launch, Pointing => Pointing);

impl Event {
    /// A tick at `secs` on both clocks, for tests and demos where the clocks never jump.
    pub fn tick(secs: u64) -> Event {
        Event::Tick { unix_secs: secs, mono_secs: secs }
    }
}

/// The system's long-lived threads, by the names they run under. Each is started again after a panic
/// ([`Event::Restarted`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Thread {
    Pad,
    PadDevices,
    Tuning,
    Mixer,
    Battery,
    Metrics,
    Status,
    Wifi,
    Bluetooth,
    BluetoothConnect,
    Tray,
    Brightness,
    Sound,
    Store,
    System,
    Launcher,
    Icons,
    Files,
    Browse,
    Freezer,
    Units,
    Touchpad,
    Clock,
    InputWatch,
    Clipboard,
    Sleep,
    Notifications,
    Control,
}

impl Thread {
    pub const ALL: [Thread; 28] = [
        Thread::Pad,
        Thread::PadDevices,
        Thread::Tuning,
        Thread::Mixer,
        Thread::Battery,
        Thread::Metrics,
        Thread::Status,
        Thread::Wifi,
        Thread::Bluetooth,
        Thread::BluetoothConnect,
        Thread::Tray,
        Thread::Brightness,
        Thread::Sound,
        Thread::Store,
        Thread::System,
        Thread::Launcher,
        Thread::Icons,
        Thread::Files,
        Thread::Browse,
        Thread::Freezer,
        Thread::Units,
        Thread::Touchpad,
        Thread::Clock,
        Thread::InputWatch,
        Thread::Clipboard,
        Thread::Sleep,
        Thread::Notifications,
        Thread::Control,
    ];

    /// The name the thread runs under.
    pub const fn name(self) -> &'static str {
        match self {
            Thread::Pad => "pad",
            Thread::PadDevices => "pad devices",
            Thread::Tuning => "tuning",
            Thread::Mixer => "mixer",
            Thread::Battery => "battery",
            Thread::Metrics => "metrics",
            Thread::Status => "status",
            Thread::Wifi => "wifi",
            Thread::Bluetooth => "bluetooth",
            Thread::BluetoothConnect => "bluetooth-connect",
            Thread::Tray => "tray",
            Thread::Brightness => "brightness",
            Thread::Sound => "sound",
            Thread::Store => "store",
            Thread::System => "system",
            Thread::Launcher => "launcher",
            Thread::Icons => "icons",
            Thread::Files => "files",
            Thread::Browse => "browse",
            Thread::Freezer => "freezer",
            Thread::Units => "units",
            Thread::Touchpad => "touchpad",
            Thread::Clock => "clock",
            Thread::InputWatch => "input watch",
            Thread::Clipboard => "clipboard",
            Thread::Sleep => "sleep",
            Thread::Notifications => "notifications",
            Thread::Control => "control",
        }
    }

    /// The thread running under `name`.
    pub fn named(name: &str) -> Option<Thread> {
        Thread::ALL.into_iter().find(|t| t.name() == name)
    }
}
