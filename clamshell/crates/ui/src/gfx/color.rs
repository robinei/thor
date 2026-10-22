//! Colours: 0xRRGGBB, opaque or with an alpha.

use tiny_skia::Color;

/// A colour as 0xRRGGBB, opaque.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Rgb(pub u32);

impl Rgb {
    /// At `alpha` (0-1): what is under it shows through.
    pub fn alpha(self, alpha: f32) -> Color {
        let c = self.0;
        Color::from_rgba8((c >> 16) as u8, (c >> 8) as u8, c as u8, (alpha.clamp(0.0, 1.0) * 255.0) as u8)
    }
}

impl From<Rgb> for Color {
    fn from(c: Rgb) -> Color {
        rgb(c.0)
    }
}

/// 0xRRGGBB to an opaque colour.
pub fn rgb(c: u32) -> Color {
    Color::from_rgba8((c >> 16) as u8, (c >> 8) as u8, c as u8, 255)
}
