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
        file: Option<std::path::PathBuf>,
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
    /// Where the controller's input goes now.
    PadRoute(PadRoute),
    /// Start a dock app's command as a new instance (its scope and workspace).
    RunApp {
        instance: Instance,
        command: Vec<String>,
    },
    /// The compositor: show, close or move windows.
    Window(WindowCommand),
    /// Quit the game: close its windows, then end what is left of it in a few seconds: its scopes (a game
    /// shelf started), or (a game Steam started, which runs in Steam's scope) the processes under Steam's
    /// launcher for its app id (`steam`), which every process of the game descends from.
    QuitGame {
        windows: Vec<i64>,
        units: Vec<Instance>,
        steam: Option<u32>,
    },
    /// Force kill the game at once: its scopes, or the processes under Steam's launcher for it.
    KillGame {
        units: Vec<Instance>,
        steam: Option<u32>,
    },
    /// End dock app `key`'s slice (all its instances): stopped (`force`: killed at once).
    EndApp {
        key: String,
        force: bool,
    },
    /// Record a play that ClamShell didn't start (a game Steam started): in stats.db.
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
    /// Touchpad mode: the input side grabs the touchscreen.
    Touchpad,
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

/// ClamShell's own files.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Save {
    /// The games' settings (see [`crate::settings::Settings::text`]).
    Settings(String),
    /// The keymaps (see [`crate::keymap::Keymaps::text`]).
    Keymaps(String),
    /// The favourites (favorites.txt).
    Favourites(Vec<String>),
    /// Where shelf and the second screen are (see [`crate::State::resume_text`]), to start there next
    /// time.
    Resume(String),
    /// The screens' brightness (see [`crate::levels::Brightness::text`]), to apply at the next start.
    Brightness(String),
    /// The window rules (see [`crate::bottom_screen::WindowRules::text`]).
    WindowRules(String),
}

/// What the compositor is asked to do.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WindowCommand {
    /// Show this workspace: a dock app instance's, a game's, or (Home going back) whatever was in front.
    Focus(String),
    /// Ask these windows to close (as their own close buttons would): an instance's.
    Close(Vec<i64>),
    /// Move these windows to this instance's workspace (a Steam game's, from where they landed).
    Move { windows: Vec<i64>, to: Instance },
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
