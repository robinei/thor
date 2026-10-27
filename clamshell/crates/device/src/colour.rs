//! Panel facts describe the signal and available headroom, not a compositor's HDR policy.

/// CIE 1931 xy coordinates, in millionths.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Xy {
    pub x: u32,
    pub y: u32,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Primaries {
    pub red: Xy,
    pub green: Xy,
    pub blue: Xy,
    pub white: Xy,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Transfer {
    Gamma22,
    /// The sRGB curve, as a host shows a virtual screen.
    Srgb,
}
/// Luminance in millinits. Missing measurements remain unknown rather than invented.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Luminance {
    pub peak: u32,
    pub frame_average: u32,
    pub black: u32,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PanelColour {
    pub primaries: Primaries,
    pub transfer: Transfer,
    pub luminance: Option<Luminance>,
    pub link_bits_per_channel: Option<u8>,
    /// The compositor can tone-map highlights into panel headroom; this is not a PQ/HLG link.
    pub hdr_capable: bool,
    /// Where SDR white is shown, in millinits, the rest up to the peak being headroom for highlights.
    /// None: at the peak, no headroom. The Thor's panels have none until the panel is driven brighter for
    /// HDR content with SDR drawn dimmer to match (phase 4h).
    pub sdr_white: Option<u32>,
}
const D65: Xy = Xy { x: 312700, y: 329000 };
impl PanelColour {
    /// An unprofiled panel: sRGB primaries at gamma 2.2, with no claimed HDR headroom.
    pub const fn srgb() -> Self {
        Self {
            primaries: Primaries { red: Xy { x: 640000, y: 330000 }, green: Xy { x: 300000, y: 600000 }, blue: Xy { x: 150000, y: 60000 }, white: D65 },
            transfer: Transfer::Gamma22,
            luminance: None,
            link_bits_per_channel: None,
            hdr_capable: false,
            sdr_white: None,
        }
    }
    /// A virtual machine's screen, a window on its host, which shows it as sRGB.
    pub const fn virtual_screen() -> Self {
        Self { transfer: Transfer::Srgb, ..Self::srgb() }
    }
    /// ICNA3520 starting facts from gamescope's Thor profile (Armada PR #261), not new measurements.
    pub const fn icna3520() -> Self {
        Self {
            primaries: Primaries { red: Xy { x: 680000, y: 320000 }, green: Xy { x: 265000, y: 690000 }, blue: Xy { x: 150000, y: 60000 }, white: D65 },
            transfer: Transfer::Gamma22,
            luminance: Some(Luminance { peak: 650000, frame_average: 650000, black: 2 }),
            link_bits_per_channel: Some(8),
            hdr_capable: true,
            sdr_white: None,
        }
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn panel_facts_preserve_the_documented_headroom_without_claiming_a_ten_bit_link() {
        let top = crate::DeviceProfile::thor().top.colour;
        assert_eq!(top, PanelColour::icna3520());
        assert_eq!(top.primaries.red, Xy { x: 680000, y: 320000 });
        assert_eq!(top.primaries.green, Xy { x: 265000, y: 690000 });
        assert_eq!(top.primaries.white, D65);
        assert_eq!(top.transfer, Transfer::Gamma22);
        assert_eq!(top.luminance, Some(Luminance { peak: 650000, frame_average: 650000, black: 2 }));
        assert_eq!(top.link_bits_per_channel, Some(8));
        assert!(top.hdr_capable);
        let bottom = crate::DeviceProfile::thor().bottom.colour;
        assert_eq!(bottom.link_bits_per_channel, Some(8));
        assert!(!bottom.hdr_capable);
        assert_eq!(bottom.primaries, PanelColour::srgb().primaries);
        assert_eq!(crate::DeviceProfile::vm().top.colour, PanelColour::virtual_screen());
        for panel in [crate::DeviceProfile::vm().top, crate::DeviceProfile::generic().bottom] {
            assert_eq!(panel.colour.primaries, PanelColour::srgb().primaries);
            assert!(!panel.colour.hdr_capable);
            assert_eq!(panel.colour.luminance, None);
            assert_eq!(panel.colour.link_bits_per_channel, None);
        }
    }
}
