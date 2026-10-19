//! What goes out: what the state asks the outside world to do ([`Effect`]), and which screens need a
//! new frame ([`Dirty`]).

use std::sync::Arc;

use clamshell_library::Launch;

use crate::bluetooth;
use crate::instance::Instance;
use crate::keyboard::Stroke;
use crate::levels::{Share, Sound};
use crate::pad::PadRoute;
use crate::settings::Values;
use crate::wifi;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Effect {
    /// Start this game (its launch command or Steam id, and its file) with these settings and extra
    /// environment, in a scope and on a workspace of its own.
    Launch {
        game: Arc<str>,
        instance: Instance,
        settings: Values,
        env: Vec<(String, String)>,
        launch: Launch,
        /// A video mpv plays: its file (its place read again when it ends).
        watch: Option<std::path::PathBuf>,
    },
    /// The settings in force changed: the running game's (`game`), or the global ones while none runs
    /// (also when a game ends). Apply them now.
    Apply {
        game: Option<Arc<str>>,
        settings: Values,
    },
    /// The keymap in force changed (the game's, or all games'; Default: id 0, no bindings): the pad
    /// remaps by it.
    Keymap(crate::keymap::Keymap),
    /// Write one of ClamShell's files.
    Save(Save),
    /// A key press for the focused app (virtual keyboard).
    Key(Stroke),
    /// Fingers pointing (one touch frame's), for the virtual touchpad.
    Touchpad(Vec<crate::touch::Contact>),
    /// Where the controller's input goes now.
    PadRoute(PadRoute),
    /// The compositor: show, close or move windows.
    Window(WindowCommand),
    /// End an entry's slice (all its launches): stopped (systemd asks its processes to end, then kills
    /// what is left after its timeout), or with `force` killed at once.
    EndRun {
        key: String,
        force: bool,
    },
    /// End a game Steam runs (in Steam's scope, not ours): every process under Steam's launcher for its
    /// app id, killed.
    EndSteam {
        appid: u32,
    },
    /// Run this command (a runner's `stop`, for an entry that runs in another program's processes).
    Stop(Vec<String>),
    /// An entry's slice (all its launches' processes) frozen, or thawed (see [`crate::background`]).
    Freeze {
        slice: String,
        frozen: bool,
    },
    /// Processes asked to end (SIGTERM), or killed: a program not started by ClamShell (found by its tray
    /// icon, or in the battery tab's list).
    EndProcesses {
        pids: Vec<u32>,
        force: bool,
    },
    /// Ask the tray thread: a program shown, or its menu's entry clicked.
    Tray(crate::tray::TrayCommand),
    /// Write these lines to the control socket of entry `key`'s launch (see [`crate::enqueue`]).
    Enqueue {
        key: String,
        lines: Vec<String>,
    },
    /// List this browsed folder now, and again whenever it changes (none: watch nothing).
    Browse(Option<crate::files::Browse>),
    /// A file operation (the files thread).
    Files(crate::files::FileOp),
    /// One of ClamShell's own tools (the Tools tab): `rescan` (the library read again), `refetch` (what
    /// metadata wasn't found tried again, then a rescan).
    Tool(String),
    /// What shelf shows (the selected entry first): fetch their metadata before the rest.
    FetchFirst(Vec<Arc<str>>),
    /// Record a play that ClamShell didn't start (a game Steam started), in the database.
    RecordPlay {
        game: Arc<str>,
        start: u64,
        duration: u64,
    },
    /// Output volume and mute, an app's, the output.
    Sound(Sound),
    /// The screens' brightness: each backlight to `level` times its share (both 0-100; a backlight
    /// without one: 100).
    Brightness {
        level: u8,
        shares: Vec<Share>,
    },
    Wifi(wifi::Command),
    Bluetooth(bluetooth::Command),
    /// Start (true) or stop reading something for a tab while it is open.
    Watch(Watch, bool),
    /// The screens on or off (idle; the UI asks the compositor).
    Screens(bool),
    /// Sleep (idle, in the device's session): suspend, then hibernate where the system can.
    Sleep,
    /// Both screens to a file; the result comes back as [`crate::Event::Screenshot`].
    Screenshot,
    /// Set the system's time zone (tzdata's name); the time zones are read again after.
    SetTimeZone(String),
    /// The system's clock set by the network (NTP), or not; read again after.
    SetNtp(bool),
    /// The system's clock moved by a step of one of its fields, as the local time has it (a month on
    /// from the 31st: the month's last day); read again after.
    StepClock(crate::time::ClockField, i8),
    /// A desktop notification (on the main screen, over a game: the notification daemon's), urgent ones
    /// staying until dismissed.
    Notify {
        summary: String,
        body: String,
        urgent: bool,
    },
    /// Calibrate the gyro from its next second (the device held still); the result comes back as
    /// [`crate::Event::GyroCalibrated`].
    CalibrateGyro,
    Quit,
}

/// What a tab watches while it is open.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Watch {
    /// The outputs and the apps playing.
    Audio,
    /// The backlights.
    Backlights,
    /// The networks in range (scanned while watched).
    Wifi,
    /// The devices (looked for while watched).
    Bluetooth,
    /// The charge history and the spenders.
    Battery,
    /// The time zone and the ones to choose from.
    Time,
}

/// What ClamShell keeps (in its database): each saved whole.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Save {
    /// The entries' settings and environment.
    Settings(crate::settings::Settings),
    Keymaps(crate::keymap::Keymaps),
    /// The favourite entries' ids.
    Favourites(Vec<String>),
    /// Where shelf and the second screen are, to start there next time.
    Resume(crate::Resume),
    /// The screens' brightness, to apply at the next start.
    Brightness(crate::levels::Brightness),
    /// Which programs' windows go to the bottom screen.
    WindowRules(crate::bottom_screen::WindowRules),
    /// The dock's pinned entries' ids, in order.
    Dock(Vec<std::sync::Arc<str>>),
    /// The entries started with ClamShell (see [`crate::autostart`]).
    Autostart(Vec<std::sync::Arc<str>>),
    /// Clocks on a 12-hour dial, or 24-hour.
    TwelveHour(bool),
}

/// What the compositor is asked to do.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WindowCommand {
    /// Show this workspace: an entry's, or (Home going back) whatever was in front.
    Focus(String),
    /// Ask these windows to close (as their own close buttons would).
    Close(Vec<i64>),
    /// Move these windows to this workspace (an entry's known by app id, from where they landed).
    Move { windows: Vec<i64>, to: String },
    /// Bring shelf's window to the front.
    ShowShelf,
    /// A window to the bottom screen (marked there, so it stays), shown there; the top screen keeps the
    /// keyboard.
    Down(i64),
    /// A window moved down back to the top screen: to its workspace (or the one in front).
    Up { window: i64, to: Option<Instance> },
    /// The bottom screen shows this window moved there; the top screen keeps the keyboard.
    ShowDown(i64),
    /// The bottom screen's output on or off.
    BottomPower(bool),
    /// The windows that go to the bottom screen as they open (all of them: the rules in force).
    DownRules(Vec<crate::bottom_screen::DownRule>),
    /// The app ids whose windows land on the landing workspace as they open (all of them), to be moved
    /// to their entry's (see [`crate::running::app_id_of`]).
    LandRules(Vec<std::sync::Arc<str>>),
}

/// Which screens need a new frame after an event.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Dirty {
    pub top: bool,
    pub bottom: bool,
}

impl Dirty {
    pub const BOTH: Dirty = Dirty { top: true, bottom: true };
    pub const BOTTOM: Dirty = Dirty { top: false, bottom: true };
    pub const NONE: Dirty = Dirty { top: false, bottom: false };

    /// Either's screens.
    pub fn or(self, other: Dirty) -> Dirty {
        Dirty { top: self.top || other.top, bottom: self.bottom || other.bottom }
    }
}
