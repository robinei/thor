//! Everything ClamShell reads from its environment, read once at start and passed to what needs it.

use std::path::PathBuf;

use clamshell_state::{Profile, Tab};
use clamshell_system::os::evdev::device::DeviceSpec;
use clamshell_system::paths::Base;

pub struct Env {
    /// Fixed hardware facts, selected once from the kernel identity or CLAMSHELL_DEVICE.
    pub profile: Profile,
    /// CLAMSHELL_BOTTOM_OUTPUT: the second screen's output, which gets the bottom view as a layer-shell
    /// surface (default: the selected profile's bottom connector).
    pub bottom_output: String,
    /// CLAMSHELL_BOTTOM_PANEL=1: on a desktop, the bottom view as a panel in a corner instead of a
    /// window, so tapping its keyboard leaves the keyboard focus on the app being typed into.
    pub bottom_panel: bool,
    /// CLAMSHELL_GAMEPAD: the controller, as its /dev/input path or part of its name. The default is the
    /// first gamepad.
    pub gamepad: DeviceSpec,
    /// CLAMSHELL_STEAM=off: leave out Steam's games, for tests.
    pub steam: bool,
    /// CLAMSHELL_START_TAB: start in this tab (touchpad, keyboard, dock, ...), for development. Whatever
    /// the tab watches is started at startup along with the rest.
    pub start_tab: Option<Tab>,
    /// CLAMSHELL_PAD_MOUSE=1: start with the pad mouse on. Where the pad's input goes is sent at startup.
    pub pad_mouse: bool,
    /// CLAMSHELL_CONTROL: the control pipe, where `clamshell ctl` and tests send commands (see
    /// [`crate::control`]). The default is $XDG_RUNTIME_DIR/clamshell-control.
    pub control: Option<PathBuf>,
    /// CLAMSHELL_SESSION=device: ClamShell is the device's own session. It may put the device to sleep
    /// when idle, and it takes the hardware keys (volume, brightness). A desktop run leaves it unset.
    pub device_session: bool,
    /// CLAMSHELL_BACKLIGHTS: a folder laid out like /sys/class/backlight to use instead, for tests and
    /// for desktops without a backlight.
    pub backlights: PathBuf,
    /// SWAYSOCK: sway's socket, under sway.
    pub sway: Option<PathBuf>,
    /// HOME and the XDG base directories, where ClamShell's files go (see [`clamshell_system::paths`]).
    pub base: Base,
}

impl Env {
    pub fn from_env() -> Result<Env, String> {
        let var = |name: &str| std::env::var(name).ok();
        let flag = |name: &str, on: &str| var(name).as_deref() == Some(on);
        let path = |name: &str| std::env::var_os(name).map(PathBuf::from);
        let profile = clamshell_system::profile::detect(std::path::Path::new("/"), var("CLAMSHELL_DEVICE").as_deref())?;
        Ok(Env {
            profile,
            bottom_output: var("CLAMSHELL_BOTTOM_OUTPUT").unwrap_or_else(|| profile.bottom.connector.into()),
            bottom_panel: flag("CLAMSHELL_BOTTOM_PANEL", "1"),
            gamepad: DeviceSpec::parse(var("CLAMSHELL_GAMEPAD").as_deref()),
            steam: !flag("CLAMSHELL_STEAM", "off"),
            start_tab: var("CLAMSHELL_START_TAB").and_then(|t| Tab::from_name(&t)),
            pad_mouse: flag("CLAMSHELL_PAD_MOUSE", "1"),
            control: path("CLAMSHELL_CONTROL"),
            device_session: flag("CLAMSHELL_SESSION", "device"),
            backlights: path("CLAMSHELL_BACKLIGHTS").unwrap_or_else(|| PathBuf::from("/sys/class/backlight")),
            sway: path("SWAYSOCK"),
            base: Base::from_env(),
        })
    }
}
