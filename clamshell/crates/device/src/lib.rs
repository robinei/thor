//! Facts hardware cannot announce. Identification is pure; callers read the device tree and DMI.
pub mod charge;
pub mod colour;
pub mod fan;
pub mod gyro;
pub mod input;
pub mod layout;
pub mod preview;
use colour::PanelColour;

/// A screen's role has a stable connector name on a known device.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Screen {
    pub connector: &'static str,
    pub rotation: Rotation,
    /// Role hint only: backlights themselves are discovered at runtime.
    pub backlight: Option<&'static str>,
    pub colour: PanelColour,
    /// The panel's width and height in millimetres, in its own orientation as its connector gives them,
    /// for a connector that gives none.
    pub millimetres: Option<(u16, u16)>,
}
/// Clockwise orientation of the physical panel.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Rotation {
    Normal,
    Clockwise90,
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
/// The test machine's absolute devices: a tablet on each screen (QEMU's, as the host's pointer), and its
/// touchscreen on the second.
const VM_TOUCH: &[Touchscreen] = &[
    Touchscreen { selector: TouchSelector::Named("1575:3:QEMU_Virtio_Tablet"), screen: ScreenRole::Top },
    Touchscreen { selector: TouchSelector::Named("1575:1:QEMU_QEMU_USB_Tablet"), screen: ScreenRole::Bottom },
    Touchscreen { selector: TouchSelector::All, screen: ScreenRole::Bottom },
];
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
pub struct DeviceProfile {
    pub name: &'static str,
    pub top: Screen,
    pub bottom: Screen,
    pub touchscreens: &'static [Touchscreen],
    pub refresh_calibration: Option<RefreshCalibration>,
    pub controls: input::Controls,
    pub sensor_hub: Option<gyro::SensorHub>,
    pub fan: Option<fan::Fan>,
    pub charge: charge::Charge,
    pub control_layout: layout::ControlLayout,
    pub preview: preview::Preview,
}
impl Default for DeviceProfile {
    fn default() -> Self {
        Self::generic()
    }
}
impl DeviceProfile {
    pub const fn thor() -> Self {
        Self {
            name: "thor",
            top: Screen {
                connector: "DSI-2",
                rotation: Rotation::Clockwise90,
                backlight: Some("ae96000.dsi.0"),
                colour: PanelColour::icna3520(),
                millimetres: None,
            },
            bottom: Screen {
                connector: "DSI-1",
                rotation: Rotation::Clockwise90,
                backlight: Some("ae94000.dsi.0"),
                colour: PanelColour { link_bits_per_channel: Some(8), ..PanelColour::srgb() },
                // 1080 by 1240 pixels, at 16.5 per millimetre.
                millimetres: Some((65, 75)),
            },
            touchscreens: THOR_TOUCH,
            refresh_calibration: Some(THOR_REFRESH),
            controls: input::Controls::thor(),
            sensor_hub: Some(gyro::SensorHub::thor()),
            fan: Some(fan::Fan::thor()),
            charge: charge::Charge::thor(),
            control_layout: layout::ControlLayout::thor(),
            preview: preview::Preview::reference(),
        }
    }
    pub const fn vm() -> Self {
        Self {
            name: "vm",
            top: Screen { connector: "Virtual-1", rotation: Rotation::Normal, backlight: None, colour: PanelColour::virtual_screen(), millimetres: None },
            bottom: Screen { connector: "Virtual-2", rotation: Rotation::Normal, backlight: None, colour: PanelColour::virtual_screen(), millimetres: None },
            touchscreens: VM_TOUCH,
            refresh_calibration: None,
            controls: input::Controls::thor(),
            sensor_hub: None,
            fan: None,
            charge: charge::Charge::standard(),
            control_layout: layout::ControlLayout::thor(),
            preview: preview::Preview::reference(),
        }
    }
    /// The test machine with the Thor's panels' colour (`CLAMSHELL_DEVICE=vm-hdr`), its top panel's SDR
    /// white at the reference (203 nits) so that its headroom to the peak is used: the HDR paths on the
    /// DRM backend, seen through the virtual screens.
    pub const fn vm_hdr() -> Self {
        let thor = Self::thor();
        let vm = Self::vm();
        Self {
            name: "vm-hdr",
            top: Screen { colour: colour::PanelColour { sdr_white: Some(203_000), ..thor.top.colour }, ..vm.top },
            bottom: Screen { colour: thor.bottom.colour, ..vm.bottom },
            ..vm
        }
    }
    /// Unknown physical connectors remain unmatched; desktop views still draw as ordinary windows.
    pub const fn generic() -> Self {
        Self {
            name: "generic",
            top: Screen { connector: "ClamShell-Top", rotation: Rotation::Normal, backlight: None, colour: PanelColour::srgb(), millimetres: None },
            bottom: Screen { connector: "ClamShell-Bottom", rotation: Rotation::Normal, backlight: None, colour: PanelColour::srgb(), millimetres: None },
            touchscreens: &[],
            refresh_calibration: None,
            controls: input::Controls::generic(),
            sensor_hub: None,
            fan: None,
            charge: charge::Charge::standard(),
            control_layout: layout::ControlLayout::standard(),
            preview: preview::Preview::reference(),
        }
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
            "vm-hdr" => Ok(Self::vm_hdr()),
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
        assert_eq!(DeviceProfile::identify(None, b"ayn,thor\0qcom,sm8550\0", "", ""), Ok(DeviceProfile::thor()));
        assert_eq!(DeviceProfile::identify(None, b"other,board\0ayn,thor\0", "", ""), Ok(DeviceProfile::thor()));
        assert_eq!(DeviceProfile::identify(None, b"ayn,thor-other\0", "", ""), Ok(DeviceProfile::generic()));
        assert_eq!(DeviceProfile::identify(None, b"", "QEMU\n", "Standard PC (Q35 + ICH9, 2009)"), Ok(DeviceProfile::vm()));
        assert_eq!(DeviceProfile::identify(None, b"", "Microsoft Corporation", "Virtual Machine"), Ok(DeviceProfile::generic()));
        assert_eq!(DeviceProfile::identify(Some("vm"), b"ayn,thor\0", "", ""), Ok(DeviceProfile::vm()));
        assert_eq!(DeviceProfile::identify(Some("generic"), b"ayn,thor\0", "", ""), Ok(DeviceProfile::generic()));
        assert_eq!(DeviceProfile::identify(Some("vm-hdr"), b"", "QEMU", ""), Ok(DeviceProfile::vm_hdr()));
        let hdr = DeviceProfile::vm_hdr();
        assert_eq!((hdr.top.connector, hdr.top.colour.sdr_white), ("Virtual-1", Some(203_000)));
        assert_eq!(hdr.bottom.colour, DeviceProfile::thor().bottom.colour);
        assert_eq!(DeviceProfile::identify(Some("unknown"), b"ayn,thor\0", "", ""), Err("unknown"));
    }
    #[test]
    fn profiles_always_name_distinct_top_and_bottom_screens() {
        for profile in [DeviceProfile::thor(), DeviceProfile::vm(), DeviceProfile::generic()] {
            assert_ne!(profile.top.connector, profile.bottom.connector);
            assert!(!profile.top.connector.is_empty());
            assert!(!profile.bottom.connector.is_empty());
        }
    }
}
