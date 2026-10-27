//! Everything ClamShell reads from its environment, read once at start and passed to what needs it.

use std::path::PathBuf;

use clamshell_state::{DeviceProfile, Tab};
use clamshell_system::os::evdev::device::DeviceSpec;
use clamshell_system::paths::Base;

pub struct Env {
    /// Fixed hardware facts, selected once from the kernel identity or CLAMSHELL_DEVICE.
    pub profile: DeviceProfile,
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
    /// CLAMSHELL_BACKLIGHTS: a folder laid out like /sys/class/backlight to use instead, for tests and
    /// for desktops without a backlight.
    pub backlights: PathBuf,
    /// CLAMSHELL_SCANOUT=1: try direct scanout on the device's screens (best effort; see the compositor's
    /// `scanout`).
    pub scanout: bool,
    /// CLAMSHELL_CHARGE_LIMIT: how the charge limit is held (threshold, firmware, current), when not as
    /// detected.
    pub charge_limit: Option<String>,
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
            gamepad: DeviceSpec::parse(var("CLAMSHELL_GAMEPAD").as_deref()),
            steam: !flag("CLAMSHELL_STEAM", "off"),
            start_tab: var("CLAMSHELL_START_TAB").and_then(|t| Tab::from_name(&t)),
            pad_mouse: flag("CLAMSHELL_PAD_MOUSE", "1"),
            control: path("CLAMSHELL_CONTROL"),
            backlights: path("CLAMSHELL_BACKLIGHTS").unwrap_or_else(|| PathBuf::from("/sys/class/backlight")),
            scanout: flag("CLAMSHELL_SCANOUT", "1"),
            charge_limit: var("CLAMSHELL_CHARGE_LIMIT"),
            base: Base::from_env(),
        })
    }
}
