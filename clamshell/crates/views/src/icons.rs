//! Which icon stands for one of the state's things: a Wi-Fi signal, a setting, a Bluetooth device, and a
//! Library tab's or a browsed file's icon by its name.

use clamshell_state::radios::bluetooth::Kind;
use clamshell_state::radios::wifi::signal_bars;
use clamshell_state::settings::Setting;

use clamshell_ui::gfx::icons::Icon;

/// The icon for a Wi-Fi signal's quality (0-100), or `None` when not connected.
pub(super) fn wifi(quality: Option<u8>) -> Icon {
    Icon::Wifi { bars: quality.map(signal_bars) }
}

/// A settings tile's icon.
pub(super) fn setting(s: Setting) -> Icon {
    match s {
        Setting::Power => Icon::Bolt,
        Setting::FpsCap => Icon::Gauge,
        Setting::Refresh => Icon::Refresh,
        Setting::Hud => Icon::Hud,
        Setting::Resolution => Icon::Upscale,
        Setting::Scaling => Icon::Scaling,
        Setting::Size => Icon::Size,
        Setting::AntiAliasing => Icon::AntiAlias,
        Setting::Sharpen => Icon::Sharpen,
        Setting::Curve | Setting::HdrPeak => Icon::Brightness,
        Setting::Lights => Icon::Lights,
        Setting::BottomScreen => Icon::ScreenOff,
        Setting::BigCores => Icon::Cores,
        Setting::Fan => Icon::Fan,
        Setting::PadMouse => Icon::Mouse,
        Setting::Keymap
        | Setting::Keymap2
        | Setting::Keymap3
        | Setting::Keymap4
        | Setting::Keymap5
        | Setting::Keymap6
        | Setting::Keymap7
        | Setting::Keymap8 => Icon::Buttons,
        Setting::Rumble => Icon::Rumble,
        Setting::CpuMax => Icon::Cpu,
        Setting::GpuMax => Icon::Gpu,
        Setting::ChargeLimit => Icon::Battery { tenths: 8, charging: true },
        Setting::ScreenOff => Icon::ScreenOff,
        Setting::Sleep => Icon::Moon,
        Setting::Background => Icon::Windows,
        Setting::X86 => Icon::Emulation,
        Setting::VulkanDriver => Icon::Gpu,
    }
}

/// A Bluetooth device's icon, by its kind.
pub(super) fn device(kind: Kind) -> Icon {
    match kind {
        Kind::Audio => Icon::Headphones,
        Kind::Gamepad => Icon::Gamepad,
        Kind::Keyboard => Icon::Keyboard,
        Kind::Mouse => Icon::Mouse,
        Kind::Phone => Icon::Phone,
        Kind::Computer | Kind::Other => Icon::Bluetooth { connected: false },
    }
}

/// The Library tab's icon, by the name library.json gives it: `gamepad`, `apps`, `film`, `tv`, `music`,
/// `star`, `clock`, `tools`, `folder`, `archive`, `file`, `trash` or `card`. Any other name has none.
pub(super) fn named(name: &str) -> Option<Icon> {
    Some(match name {
        "gamepad" => Icon::Gamepad,
        "apps" => Icon::Apps,
        "film" => Icon::Film,
        "tv" => Icon::Tv,
        "music" => Icon::Music,
        "star" => Icon::Star,
        "clock" => Icon::Clock,
        "tools" => Icon::Wrench,
        "folder" => Icon::Folder,
        "archive" => Icon::Archive,
        "file" => Icon::File,
        "trash" => Icon::Trash,
        "card" => Icon::Card,
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_signal_picks_its_bars() {
        assert_eq!(wifi(Some(80)), Icon::Wifi { bars: Some(3) });
        assert_eq!(wifi(None), Icon::Wifi { bars: None });
    }

    /// Every icon a place may be given in the Library's edit mode is drawn.
    #[test]
    fn every_icon_a_place_may_have_is_named() {
        for name in clamshell_state::library_view::edit::ICONS {
            assert!(named(name).is_some(), "{name}");
        }
    }
}
