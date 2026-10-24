//! Facts hardware cannot announce. Identification is pure; callers read the device tree and DMI.
pub mod charge;
pub mod colour;
pub mod fan;
pub mod gyro;
pub mod input;
use colour::PanelColour;

/// A screen's role has a stable connector name on a known device.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Screen {
    pub connector: &'static str,
    pub rotation: Rotation,
    /// Role hint only: backlights themselves are discovered at runtime.
    pub backlight: Option<&'static str>,
    pub colour: PanelColour,
}
/// Clockwise orientation of the physical panel.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Rotation {
    Normal,
    Clockwise90,
}
impl Rotation {
    pub const fn degrees(self) -> u16 {
        match self {
            Self::Normal => 0,
            Self::Clockwise90 => 90,
        }
    }
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ScreenRole {
    Top,
    Bottom,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TouchSelector {
    Named(&'static str),
    All,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Touchscreen {
    pub selector: TouchSelector,
    pub screen: ScreenRole,
}
const THOR_TOUCH: &[Touchscreen] = &[
    Touchscreen { selector: TouchSelector::Named("0:0:top_touchscreen"), screen: ScreenRole::Top },
    Touchscreen { selector: TouchSelector::Named("0:0:bottom_touchscreen"), screen: ScreenRole::Bottom },
];
const VM_TOUCH: &[Touchscreen] = &[Touchscreen { selector: TouchSelector::All, screen: ScreenRole::Bottom }];
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RefreshRate {
    pub hz: u16,
    pub attribute: &'static str,
    pub low: u16,
    pub high: u16,
    pub tolerance_millihz: u16,
    pub big_error_millihz: u16,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RefreshCalibration {
    pub program: &'static str,
    pub step: u16,
    pub rates: &'static [RefreshRate],
}
const THOR_REFRESH: RefreshCalibration = RefreshCalibration {
    program: "thor-panel-calibration",
    step: 4,
    rates: &[
        RefreshRate { hz: 60, attribute: "frame_len_60", low: 0x770, high: 0x838, tolerance_millihz: 50, big_error_millihz: 100 },
        RefreshRate { hz: 120, attribute: "frame_len_120", low: 0x10, high: 0x80, tolerance_millihz: 200, big_error_millihz: 400 },
    ],
};
/// The two-screen machine's fixed facts, identified once and passed to their readers.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Profile {
    pub name: &'static str,
    pub top: Screen,
    pub bottom: Screen,
    pub touchscreens: &'static [Touchscreen],
    /// Fallback pixels per millimetre, in tenths, only when the output reports no physical dimensions.
    pub bottom_density_tenths: Option<u16>,
    pub refresh_calibration: Option<RefreshCalibration>,
    pub controls: input::Controls,
    pub sensor_hub: Option<gyro::SensorHub>,
    pub fan: Option<fan::Fan>,
    pub charge: charge::Charge,
}
impl Default for Profile {
    fn default() -> Self {
        Self::generic()
    }
}
impl Profile {
    pub const fn thor() -> Self {
        Self {
            name: "thor",
            top: Screen { connector: "DSI-2", rotation: Rotation::Clockwise90, backlight: Some("ae96000.dsi.0"), colour: PanelColour::icna3520() },
            bottom: Screen {
                connector: "DSI-1",
                rotation: Rotation::Clockwise90,
                backlight: Some("ae94000.dsi.0"),
                colour: PanelColour { link_bits_per_channel: Some(8), ..PanelColour::srgb() },
            },
            touchscreens: THOR_TOUCH,
            bottom_density_tenths: Some(165),
            refresh_calibration: Some(THOR_REFRESH),
            controls: input::Controls::thor(),
            sensor_hub: Some(gyro::SensorHub::thor()),
            fan: Some(fan::Fan::thor()),
            charge: charge::Charge::thor(),
        }
    }
    pub const fn vm() -> Self {
        Self {
            name: "vm",
            top: Screen { connector: "Virtual-1", rotation: Rotation::Normal, backlight: None, colour: PanelColour::srgb() },
            bottom: Screen { connector: "Virtual-2", rotation: Rotation::Normal, backlight: None, colour: PanelColour::srgb() },
            touchscreens: VM_TOUCH,
            bottom_density_tenths: None,
            refresh_calibration: None,
            controls: input::Controls::thor(),
            sensor_hub: None,
            fan: None,
            charge: charge::Charge::standard(),
        }
    }
    /// Unknown physical connectors remain unmatched; desktop views still draw as ordinary windows.
    pub const fn generic() -> Self {
        Self {
            name: "generic",
            top: Screen { connector: "ClamShell-Top", rotation: Rotation::Normal, backlight: None, colour: PanelColour::srgb() },
            bottom: Screen { connector: "ClamShell-Bottom", rotation: Rotation::Normal, backlight: None, colour: PanelColour::srgb() },
            touchscreens: &[],
            bottom_density_tenths: None,
            refresh_calibration: None,
            controls: input::Controls::generic(),
            sensor_hub: None,
            fan: None,
            charge: charge::Charge::standard(),
        }
    }
    pub fn bottom_pixels_per_mm_hint(&self) -> Option<f32> {
        self.bottom_density_tenths.map(|density| density as f32 / 10.0)
    }
    pub const fn screen(&self, role: ScreenRole) -> Screen {
        match role {
            ScreenRole::Top => self.top,
            ScreenRole::Bottom => self.bottom,
        }
    }
    /// Maps identification to facts in one place. A forced unknown name is an error, not a guess.
    pub fn identify<'a>(forced: Option<&'a str>, compatible: &[u8], dmi_vendor: &str, dmi_product: &str) -> Result<Self, &'a str> {
        let detected = if compatible.split(|byte| *byte == 0).any(|part| part == b"ayn,thor") {
            "thor"
        } else if dmi_vendor.trim() == "QEMU" || dmi_product.trim() == "QEMU Virtual Machine" {
            "vm"
        } else {
            "generic"
        };
        match forced.unwrap_or(detected) {
            "thor" => Ok(Self::thor()),
            "vm" => Ok(Self::vm()),
            "generic" => Ok(Self::generic()),
            unknown => Err(unknown),
        }
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn identity_uses_exact_compatible_entries_dmi_and_an_explicit_override() {
        assert_eq!(Profile::identify(None, b"ayn,thor\0qcom,sm8550\0", "", ""), Ok(Profile::thor()));
        assert_eq!(Profile::identify(None, b"other,board\0ayn,thor\0", "", ""), Ok(Profile::thor()));
        assert_eq!(Profile::identify(None, b"ayn,thor-other\0", "", ""), Ok(Profile::generic()));
        assert_eq!(Profile::identify(None, b"", "QEMU\n", "Standard PC (Q35 + ICH9, 2009)"), Ok(Profile::vm()));
        assert_eq!(Profile::identify(None, b"", "Microsoft Corporation", "Virtual Machine"), Ok(Profile::generic()));
        assert_eq!(Profile::identify(Some("vm"), b"ayn,thor\0", "", ""), Ok(Profile::vm()));
        assert_eq!(Profile::identify(Some("generic"), b"ayn,thor\0", "", ""), Ok(Profile::generic()));
        assert_eq!(Profile::identify(Some("unknown"), b"ayn,thor\0", "", ""), Err("unknown"));
    }
    #[test]
    fn only_a_known_physical_panel_supplies_a_density_hint() {
        assert_eq!(Profile::thor().bottom_pixels_per_mm_hint(), Some(16.5));
        assert_eq!(Profile::vm().bottom_pixels_per_mm_hint(), None);
        assert_eq!(Profile::generic().bottom_pixels_per_mm_hint(), None);
    }
    #[test]
    fn profiles_always_name_distinct_top_and_bottom_screens() {
        for profile in [Profile::thor(), Profile::vm(), Profile::generic()] {
            assert_ne!(profile.top.connector, profile.bottom.connector);
            assert!(!profile.top.connector.is_empty());
            assert!(!profile.bottom.connector.is_empty());
        }
    }
}
