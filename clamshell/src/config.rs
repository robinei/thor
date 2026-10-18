//! Everything ClamShell reads from its environment, read once at start and passed to what needs it.

use std::path::PathBuf;

use clamshell_core::Tab;
use clamshell_system::input::device::DeviceSpec;
use clamshell_system::paths::Base;

pub struct Env {
    /// CLAMSHELL_BOTTOM_OUTPUT: the second screen's output, which gets the bottom view as a layer-shell
    /// surface (default DSI-1, the Thor's).
    pub bottom_output: String,
    /// CLAMSHELL_BOTTOM_PANEL=1: on a desktop, the bottom view as a panel in a corner instead of a
    /// window, so tapping its keyboard leaves the keyboard focus on the app being typed into.
    pub bottom_panel: bool,
    /// CLAMSHELL_GAMEPAD: the controller, its /dev/input path or part of its name (default: the first
    /// gamepad).
    pub gamepad: DeviceSpec,
    /// CLAMSHELL_STEAM=off: without Steam's games (tests).
    pub steam: bool,
    /// CLAMSHELL_START_MODE: start in this tab (touchpad, keyboard, dock, ...; development). What it
    /// watches starts with the others' at the start.
    pub start_tab: Option<Tab>,
    /// CLAMSHELL_PAD_MOUSE=1: start with the pad mouse on (where the pad goes is sent at the start).
    pub pad_mouse: bool,
    /// CLAMSHELL_CONTROL: the control pipe, where `clamshell ctl` and tests send commands (see
    /// [`crate::control`]); default $XDG_RUNTIME_DIR/clamshell-control.
    pub control: Option<PathBuf>,
    /// CLAMSHELL_SESSION=device: ClamShell is the device's own session. It may put the device to sleep
    /// when idle, and it takes the hardware keys (volume, brightness). A desktop run leaves it unset.
    pub device_session: bool,
    /// CLAMSHELL_BACKLIGHTS: another folder laid out as /sys/class/backlight (tests, desktops without
    /// one).
    pub backlights: PathBuf,
    /// CLAMSHELL_SNAPSHOT_LIBRARY=real: snapshots of the real library instead of the demo games.
    pub snapshot_real_library: bool,
    /// CLAMSHELL_SNAPSHOT_NOW set: snapshots at the time they are taken, instead of a fixed time (with
    /// which they compare byte for byte across runs).
    pub snapshot_now: bool,
    /// SWAYSOCK: sway's socket, under sway.
    pub sway: Option<PathBuf>,
    /// HOME and the XDG base directories, where ClamShell's files go (see [`clamshell_system::paths`]).
    pub base: Base,
}

impl Env {
    pub fn from_env() -> Env {
        let var = |name: &str| std::env::var(name).ok();
        let flag = |name: &str, on: &str| var(name).as_deref() == Some(on);
        let path = |name: &str| std::env::var_os(name).map(PathBuf::from);
        Env {
            bottom_output: var("CLAMSHELL_BOTTOM_OUTPUT").unwrap_or_else(|| "DSI-1".into()),
            bottom_panel: flag("CLAMSHELL_BOTTOM_PANEL", "1"),
            gamepad: DeviceSpec::parse(var("CLAMSHELL_GAMEPAD").as_deref()),
            steam: !flag("CLAMSHELL_STEAM", "off"),
            start_tab: var("CLAMSHELL_START_MODE").and_then(|t| Tab::from_name(&t)),
            pad_mouse: flag("CLAMSHELL_PAD_MOUSE", "1"),
            control: path("CLAMSHELL_CONTROL"),
            device_session: flag("CLAMSHELL_SESSION", "device"),
            backlights: path("CLAMSHELL_BACKLIGHTS").unwrap_or_else(|| PathBuf::from("/sys/class/backlight")),
            snapshot_real_library: flag("CLAMSHELL_SNAPSHOT_LIBRARY", "real"),
            snapshot_now: std::env::var_os("CLAMSHELL_SNAPSHOT_NOW").is_some(),
            sway: path("SWAYSOCK"),
            base: Base::from_env(),
        }
    }
}
