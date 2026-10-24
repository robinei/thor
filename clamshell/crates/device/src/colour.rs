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
        }
    }
    /// ICNA3520 starting facts from gamescope's Thor profile (Armada PR #261), not new measurements.
    pub const fn icna3520() -> Self {
        Self {
            primaries: Primaries { red: Xy { x: 680000, y: 320000 }, green: Xy { x: 265000, y: 690000 }, blue: Xy { x: 150000, y: 60000 }, white: D65 },
            transfer: Transfer::Gamma22,
            luminance: Some(Luminance { peak: 650000, frame_average: 650000, black: 2 }),
            link_bits_per_channel: Some(8),
            hdr_capable: true,
        }
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn panel_facts_preserve_the_documented_headroom_without_claiming_a_ten_bit_link() {
        let top = crate::Profile::thor().top.colour;
        assert_eq!(top, PanelColour::icna3520());
        assert_eq!(top.primaries.red, Xy { x: 680000, y: 320000 });
        assert_eq!(top.primaries.green, Xy { x: 265000, y: 690000 });
        assert_eq!(top.primaries.white, D65);
        assert_eq!(top.transfer, Transfer::Gamma22);
        assert_eq!(top.luminance, Some(Luminance { peak: 650000, frame_average: 650000, black: 2 }));
        assert_eq!(top.link_bits_per_channel, Some(8));
        assert!(top.hdr_capable);
        let bottom = crate::Profile::thor().bottom.colour;
        assert_eq!(bottom.link_bits_per_channel, Some(8));
        assert!(!bottom.hdr_capable);
        assert_eq!(bottom.primaries, PanelColour::srgb().primaries);
        for panel in [crate::Profile::vm().top, crate::Profile::generic().bottom] {
            assert_eq!(panel.colour, PanelColour::srgb());
            assert!(!panel.colour.hdr_capable);
            assert_eq!(panel.colour.luminance, None);
            assert_eq!(panel.colour.link_bits_per_channel, None);
        }
    }
}
