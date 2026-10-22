//! Which icon stands for one of the state's things: a tab, a Wi-Fi signal, a setting, a Bluetooth device.

use clamshell_core::Tab;
use clamshell_core::radios::bluetooth::Kind;
use clamshell_core::radios::wifi::signal_bars;
use clamshell_core::settings::Setting;

use crate::gfx::icons::Icon;

impl Icon {
    /// A tab's icon in the bar, as it looks without a reading.
    pub fn for_tab(tab: Tab) -> Icon {
        match tab {
            Tab::Settings => Icon::Sliders,
            Tab::Dock => Icon::Apps,
            Tab::Keyboard => Icon::Keyboard,
            Tab::Touchpad => Icon::Touchpad,
            Tab::Sysinfo => Icon::Sysinfo,
            Tab::Volume => Icon::Volume { waves: 3, muted: false },
            Tab::Brightness => Icon::Brightness,
            Tab::Wifi => Icon::Wifi { bars: Some(3) },
            Tab::Battery => Icon::Battery { tenths: 10, charging: false },
            Tab::Bluetooth => Icon::Bluetooth { connected: false },
            // (Its button is the clock.)
            Tab::Time => Icon::Gauge,
        }
    }

    /// A Wi-Fi signal's quality (0-100); `None`: not connected.
    pub fn wifi(quality: Option<u8>) -> Icon {
        Icon::Wifi { bars: quality.map(signal_bars) }
    }

    /// A settings tile's icon.
    pub fn for_setting(s: Setting) -> Icon {
        match s {
            Setting::Power => Icon::Bolt,
            Setting::FpsCap => Icon::Gauge,
            Setting::Refresh => Icon::Refresh,
            Setting::Hud => Icon::Hud,
            Setting::Upscale => Icon::Upscale,
            Setting::BottomScreen => Icon::ScreenOff,
            Setting::BigCores => Icon::Cores,
            Setting::Fan => Icon::Fan,
            Setting::PadMouse => Icon::Mouse,
            Setting::Keymap => Icon::Buttons,
            Setting::Rumble => Icon::Rumble,
            Setting::CpuMax => Icon::Cpu,
            Setting::GpuMax => Icon::Gpu,
            Setting::ChargeLimit => Icon::Battery { tenths: 8, charging: true },
            Setting::ScreenOff => Icon::ScreenOff,
            Setting::Sleep => Icon::Moon,
            Setting::Background => Icon::Windows,
        }
    }

    /// A Bluetooth device's icon, by its kind.
    pub fn for_device(kind: Kind) -> Icon {
        match kind {
            Kind::Audio => Icon::Headphones,
            Kind::Gamepad => Icon::Gamepad,
            Kind::Keyboard => Icon::Keyboard,
            Kind::Mouse => Icon::Mouse,
            Kind::Phone => Icon::Phone,
            Kind::Computer | Kind::Other => Icon::Bluetooth { connected: false },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_signal_picks_its_bars() {
        assert_eq!(Icon::wifi(Some(80)), Icon::Wifi { bars: Some(3) });
        assert_eq!(Icon::wifi(None), Icon::Wifi { bars: None });
    }
}
