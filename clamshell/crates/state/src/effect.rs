//! What goes out: what the state asks the outside world to do ([`Effect`]).

use std::sync::Arc;

use clamshell_library::Launch;

use crate::bottom::keyboard::Stroke;
use crate::levels::{Share, Sound};
use crate::nav::pad::PadRoute;
use crate::radios::bluetooth;
use crate::radios::wifi;
use crate::session::instance::Instance;
use crate::settings::Values;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Effect {
    /// Start this entry, by its launch command or Steam id and its file, with these settings and extra
    /// environment. It runs in its own scope and on its own workspace.
    Launch {
        entry: Arc<str>,
        instance: Instance,
        settings: Values,
        env: Vec<(String, String)>,
        launch: Launch,
    },
    /// The settings in force changed. They are the running entry's (`entry`), or the global ones while
    /// nothing runs, including when an entry ends. Apply them now.
    Apply {
        entry: Option<Arc<str>>,
        settings: Values,
    },
    /// The keymap in force changed, and the pad remaps by it. It is the game's own keymap or the one for
    /// all games. The Default keymap has id 0 and no bindings.
    Keymap(crate::keymap::Keymap),
    /// Write one of ClamShell's files.
    Save(Save),
    /// A key press for the focused app, through the virtual keyboard.
    Key(Stroke),
    /// The fingers pointing in one touch frame, for the virtual touchpad.
    Touchpad(Vec<crate::bottom::touch::Contact>),
    /// Where the controller's input goes now.
    PadRoute(PadRoute),
    /// Report the first game-pad input while the top window lacks keyboard focus.
    PadActivity(bool),
    /// Ask these windows to close, as their own close buttons would.
    Close(Vec<i64>),
    /// End an entry's slice, which holds all its launches. Normally it is stopped: systemd asks its
    /// processes to end, then kills what is left after its timeout. With `force` it is killed at once.
    EndRun {
        key: String,
        force: bool,
    },
    /// Run one of a runner's commands: `stop`, for an entry that runs in another program's processes, or
    /// `pause` or `resume` (see [`crate::session::background`]).
    RunnerCommand(Vec<String>),
    /// Run a runner's `awake` command, which says whether its app on screen keeps the screen on. The
    /// answer comes back as [`crate::device::idle::IdleEvent::KeptAwake`].
    AskAwake(Vec<String>),
    /// Freeze or thaw an entry's slice, which holds all its launches' processes (see
    /// [`crate::session::background`]).
    Freeze {
        slice: String,
        frozen: bool,
    },
    /// Find an icon for `entry`, which was found by its window and has no desktop file, by looking up its
    /// app id in the icon themes (see [`crate::Reading::Icon`]).
    FindIcon {
        entry: Arc<str>,
        app_id: String,
    },
    /// Adopt a program not started by ClamShell, found by its tray icon. Its process and the ones it
    /// started are moved into `instance`'s scope, in the entry's slice, as a launch of that entry. Not
    /// used for a program run by a systemd service, which belongs to the service.
    Adopt {
        instance: Instance,
        pid: u32,
    },
    /// Ask processes to end (SIGTERM), or kill them with `force`. This is for a program not started by
    /// ClamShell: one found by its tray icon and not moved into a scope of ours, or one in the battery
    /// tab's list.
    EndProcesses {
        pids: Vec<u32>,
        force: bool,
    },
    /// Ask the tray thread to show a program or to click an entry in its menu.
    Tray(crate::session::tray::TrayCommand),
    /// Write these lines to the control socket of entry `key`'s launch (see [`crate::session::enqueue`]).
    Enqueue {
        key: String,
        lines: Vec<String>,
    },
    /// Tell the browse thread a folder to list and watch, or the files in use.
    Browse(crate::shelf::files::BrowseCommand),
    /// A file operation, for the files thread.
    Files(crate::shelf::files::FileOp),
    /// Run one of ClamShell's own tools from the Tools tab. `rescan` reads the library again. `refetch`
    /// tries again for metadata that wasn't found, then rescans.
    Tool(clamshell_library::Tool),
    /// The entries shelf shows, the selected one first. Fetch their metadata before the rest.
    FetchFirst(Vec<Arc<str>>),
    /// Read where mpv left the video `file` of `entry` when it stopped playing. The answer comes back as
    /// [`crate::LaunchEvent::Position`].
    ReadPosition {
        entry: Arc<str>,
        file: std::path::PathBuf,
    },
    /// Record a play (clamshell-system's `store`).
    RecordPlay {
        entry: Arc<str>,
        start: u64,
        duration: u64,
    },
    /// Set the output volume and mute, an app's volume and mute, or the output.
    Sound(Sound),
    /// Set the screens' brightness. Each backlight is set to `level` times its share. Both are 0-100, and
    /// a backlight without a share counts as 100.
    Brightness {
        level: u8,
        shares: Vec<Share>,
    },
    Wifi(wifi::Command),
    Bluetooth(bluetooth::Command),
    /// Start (true) or stop reading something for a tab while it is open.
    Watch(Watch, bool),
    /// The screens' power, sent when it changes and when it is to be set again (the same value twice
    /// means set it again). `top` is whether the screens are on, which idle turns off together; `bottom`
    /// is whether the second screen is on while they are (see [`crate::Bottom::off`]). The UI carries it
    /// out: under sway it turns the screens off and on for idle over Wayland, and passes on a change of
    /// the second screen's alone, which the system crate asks sway for.
    Outputs {
        top: bool,
        bottom: bool,
    },
    /// The main screen's refresh rate in Hz, where 0 means the rate it had at start. It follows the
    /// settings in force, and a launch's settings from the moment it starts, so the rate changes before
    /// the game's window shows. The UI sets it.
    Refresh(u16),
    /// Sleep, for idle in the device's session. Suspends, then hibernates where the system can.
    Sleep,
    /// Save both screens to a file. The result comes back as [`crate::Event::Screenshot`].
    Screenshot,
    /// Export the in-process compositor's bounded window-event ring.
    DumpWindowEvents(std::path::PathBuf),
    /// Ask the account helper to do something (see [`crate::device::account`]). A change is reported and
    /// the account is then read again.
    Account(crate::device::account::AccountCommand),
    /// Set the system's time zone by its tzdata name. The time zones are read again afterwards.
    SetTimeZone(String),
    /// Turn network time (NTP) on or off. The time is read again afterwards.
    SetNtp(bool),
    /// Step one field of the system's clock, in local time. A month on from the 31st lands on the last
    /// day of that month. The time is read again afterwards.
    StepClock(crate::status::time::ClockField, i8),
    /// A desktop notification, shown by the notification daemon on the main screen, over a game. Urgent
    /// ones stay until dismissed.
    Notify {
        summary: String,
        body: String,
        urgent: bool,
    },
    /// Calibrate the gyro from its next second of readings, with the device held still. The result comes
    /// back as [`crate::Event::GyroCalibrated`].
    CalibrateGyro,
    /// Calibrate the sticks, on the pad thread (`clamshell_system`'s `input::pad::calibrator`). Progress
    /// and the end come back as [`crate::Event::Sticks`].
    CalibrateSticks,
    /// Calibrate the top screen's refresh rate with thor-panel-calibration. The end comes back as
    /// [`crate::Event::RefreshCalibrated`].
    CalibrateRefresh,
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
    /// The account (see [`crate::device::account`]) and the time zone, which the Device view both shows.
    Account,
    /// The system's metrics, read more often while shown.
    Metrics,
    /// The bar's status: battery, Wi-Fi and volume. Watched while the screens are on. While they are off,
    /// only the battery is read now and then.
    Status,
}

/// What ClamShell stores (clamshell-system's `store`). Each is saved whole.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Save {
    /// The entries' settings and environment.
    Settings(crate::settings::Settings),
    Keymaps(crate::keymap::Keymaps),
    Curves(crate::curve::Curves),
    /// The favourite entries' ids.
    Favourites(Vec<String>),
    /// Where shelf and the second screen are, to start there next time.
    Resume(crate::Resume),
    /// The screens' brightness, to apply at the next start.
    Brightness(crate::levels::Brightness),
    /// Which programs' windows go to the bottom screen.
    WindowRules(crate::bottom::screen::WindowRules),
    /// The dock's pinned entries' ids, in order.
    Dock(Vec<std::sync::Arc<str>>),
    /// The entries started with ClamShell (see [`crate::session::autostart`]).
    Autostart(Vec<std::sync::Arc<str>>),
    /// Whether clocks use 12-hour format rather than 24-hour.
    TwelveHour(bool),
    /// The first start is over, because the Device view was closed with Done. It doesn't open by itself
    /// again.
    FirstStartDone,
    /// The sticks' calibrations, by controller.
    Sticks(crate::device::sticks::Calibrations),
}
