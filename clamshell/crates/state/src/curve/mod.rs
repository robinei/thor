//! HDR curves: SDR shown with HDR's range, for games (inverse tone mapping, as Special K does it rather
//! than Windows' Auto HDR). A curve is a profile ([`crate::profiles`]): named and shared, or a game's own,
//! chosen per game by [`crate::settings::Setting::Curve`]; 0 is Off, where SDR is shown as it is.
//!
//! The compositor applies the curve in force to the SDR surfaces of the game's windows, per pixel, on
//! luminance so that hue is kept, with no classification of what is in the picture ([`Curve::map`] is the
//! mapping its shader follows):
//!
//! 1. The black point: signal values at or below it become true black, and the rest is stretched over the
//!    range, which takes out the lifted blacks many games have.
//! 2. Shadow contrast: a power on the signal, darkening (above 1) or lifting (below 1) the shadows and
//!    midtones.
//! 3. The signal is decoded to light, with the game's white at the SDR white in nits.
//! 4. Highlights above the start (a fraction of white) expand towards the peak, along a curve that joins
//!    the line below the start smoothly.
//! 5. Saturation, around the luminance.

pub mod screens;

use crate::profiles::{Profile, Profiles};

/// The nits the compositor's linear scene has at 1.0, its reference white (BT.2408's 203).
pub const REFERENCE_NITS: f64 = 203.0;

/// What a curve holds: its parameters, in the units described on each.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Curve {
    /// The game's black floor, in tenths of a percent of the signal.
    pub black: u16,
    /// Shadow contrast, the power on the signal, in hundredths (100 changes nothing).
    pub contrast: u16,
    /// Where the game's white is shown, in nits.
    pub white: u16,
    /// Where highlights start expanding, as a percent of white (100: no expansion).
    pub start: u16,
    /// What the brightest highlights expand to, in nits (at or above white).
    pub peak: u16,
    /// Saturation, in percent (100 changes nothing).
    pub saturation: u16,
}

/// A new curve changes nothing: SDR white at the reference, no expansion.
impl Default for Curve {
    fn default() -> Curve {
        Curve { black: 0, contrast: 100, white: REFERENCE_NITS as u16, start: 100, peak: REFERENCE_NITS as u16, saturation: 100 }
    }
}

/// A curve's parameter, as its slider on the edit screen sets it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum CurveParam {
    Black,
    Contrast,
    White,
    Start,
    Peak,
    Saturation,
}

impl CurveParam {
    pub const ALL: [CurveParam; 6] = [CurveParam::Black, CurveParam::Contrast, CurveParam::White, CurveParam::Start, CurveParam::Peak, CurveParam::Saturation];

    /// The parameter's range, and the step a slider moves it by. Slider positions are the value divided by
    /// the step, which keeps them within a slider's `u8`.
    pub fn range(self) -> (u16, u16, u16) {
        match self {
            CurveParam::Black => (0, 150, 1),
            CurveParam::Contrast => (70, 150, 1),
            CurveParam::White => (100, 400, 5),
            CurveParam::Start => (30, 100, 1),
            CurveParam::Peak => (200, 1000, 10),
            CurveParam::Saturation => (50, 150, 1),
        }
    }

    pub fn get(self, c: &Curve) -> u16 {
        match self {
            CurveParam::Black => c.black,
            CurveParam::Contrast => c.contrast,
            CurveParam::White => c.white,
            CurveParam::Start => c.start,
            CurveParam::Peak => c.peak,
            CurveParam::Saturation => c.saturation,
        }
    }

    /// Sets the parameter, within its range. The peak stays at or above white, moving with it.
    pub fn set(self, c: &mut Curve, value: u16) {
        let (lo, hi, _) = self.range();
        let value = value.clamp(lo, hi);
        match self {
            CurveParam::Black => c.black = value,
            CurveParam::Contrast => c.contrast = value,
            CurveParam::White => c.white = value,
            CurveParam::Start => c.start = value,
            CurveParam::Peak => c.peak = value,
            CurveParam::Saturation => c.saturation = value,
        }
        match self {
            CurveParam::White => c.peak = c.peak.max(c.white),
            CurveParam::Peak => c.white = c.white.min(c.peak),
            _ => {}
        }
    }

    /// The value as a slider position.
    pub fn position(self, value: u16) -> u8 {
        let (_, _, step) = self.range();
        u8::try_from(value / step).unwrap_or(u8::MAX)
    }

    /// A slider position as the value.
    pub fn value(self, position: u8) -> u16 {
        let (_, _, step) = self.range();
        u16::from(position) * step
    }

    pub fn label(self) -> &'static str {
        match self {
            CurveParam::Black => "Black point",
            CurveParam::Contrast => "Shadow contrast",
            CurveParam::White => "SDR white",
            CurveParam::Start => "Highlights from",
            CurveParam::Peak => "Highlight peak",
            CurveParam::Saturation => "Saturation",
        }
    }
}

impl Curve {
    /// A pixel of SDR signal (`rgb`, each 0 to 1, sRGB's encoding) as linear light in the compositor's
    /// scene, where 1.0 is the reference white ([`REFERENCE_NITS`]). The colours are those of the input's
    /// primaries; the compositor converts them after.
    pub fn map(&self, rgb: [f64; 3]) -> [f64; 3] {
        let black = f64::from(self.black) / 1000.0;
        let contrast = f64::from(self.contrast) / 100.0;
        let white = f64::from(self.white);
        let start = f64::from(self.start) / 100.0;
        // The peak in units of white.
        let peak = f64::from(self.peak.max(self.white)) / white.max(1.0);
        let linear = rgb.map(|e| {
            let e = ((e - black) / (1.0 - black).max(1e-6)).clamp(0.0, 1.0).powf(contrast);
            srgb_decode(e)
        });
        let y = luminance(linear);
        let expanded = expand(y, start, peak);
        let scale = if y > 0.0 { expanded / y } else { 0.0 };
        let saturation = f64::from(self.saturation) / 100.0;
        let to_scene = white / REFERENCE_NITS;
        linear.map(|c| ((expanded + (c * scale - expanded) * saturation).max(0.0)) * to_scene)
    }
}

/// Luminance `y` (1 at white) with highlights above `start` expanded so that 1 reaches `peak`: below the
/// start it is unchanged; above it `s + (1 - s) t + (peak - 1) t²`, with `t` how far `y` is from the start
/// to white. Its slope is 1 at the start, so the join is smooth, and it rises for any peak at or above 1.
pub fn expand(y: f64, start: f64, peak: f64) -> f64 {
    if y <= start || start >= 1.0 {
        return y;
    }
    let t = (y - start) / (1.0 - start);
    start + (1.0 - start) * t + (peak - 1.0) * t * t
}

/// sRGB's decoding, signal to light, 0 to 1.
fn srgb_decode(e: f64) -> f64 {
    if e <= 0.04045 { e / 12.92 } else { ((e + 0.055) / 1.055).powf(2.4) }
}

/// BT.709's luminance of linear light.
fn luminance([r, g, b]: [f64; 3]) -> f64 {
    0.2126 * r + 0.7152 * g + 0.0722 * b
}

/// Every curve, by id (see [`Profiles`]).
pub type Curves = Profiles<Curve>;

impl Curves {
    /// The curves there are before any were saved: three to start from.
    pub fn initial() -> Curves {
        let curve = |black, contrast, white, start, peak, saturation| Curve { black, contrast, white, start, peak, saturation };
        Curves::from_list(vec![
            Profile::with(1, "True black", curve(20, 110, 203, 60, 650, 105)),
            Profile::with(2, "Punchy", curve(10, 115, 220, 50, 1000, 115)),
            Profile::with(3, "Gentle", curve(0, 100, 203, 80, 400, 100)),
        ])
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn near(a: f64, b: f64) {
        assert!((a - b).abs() < 1e-9, "{a} != {b}");
    }

    #[test]
    fn a_new_curve_changes_nothing() {
        let c = Curve::default();
        for e in [0.0, 0.04, 0.2, 0.5, 0.8, 1.0] {
            let [r, g, b] = c.map([e, e, e]);
            near(r, srgb_decode(e));
            near(g, r);
            near(b, r);
        }
        let [r, g, b] = c.map([1.0, 0.5, 0.0]);
        near(r, 1.0);
        near(g, srgb_decode(0.5));
        near(b, 0.0);
    }

    #[test]
    fn the_black_point_takes_out_lifted_blacks_and_white_stays() {
        let c = Curve { black: 50, ..Curve::default() };
        near(c.map([0.05, 0.05, 0.05])[0], 0.0);
        near(c.map([0.03, 0.03, 0.03])[0], 0.0);
        near(c.map([1.0, 1.0, 1.0])[0], 1.0);
        assert!(c.map([0.3, 0.3, 0.3])[0] < srgb_decode(0.3), "the rest is stretched down from the floor");
    }

    #[test]
    fn highlights_expand_to_the_peak_smoothly_and_midtones_stay() {
        let c = Curve { white: 203, start: 60, peak: 812, ..Curve::default() };
        // White reaches the peak: four times the reference.
        near(c.map([1.0, 1.0, 1.0])[0], 4.0);
        // Below the start nothing changes.
        let mid = srgb_decode(0.5);
        assert!(mid < 0.6);
        near(c.map([0.5, 0.5, 0.5])[0], mid);
        // The join at the start is smooth: the slope just above it is 1, as below.
        let d = 1e-6;
        let slope = (expand(0.6 + d, 0.6, 4.0) - expand(0.6, 0.6, 4.0)) / d;
        assert!((slope - 1.0).abs() < 1e-3, "{slope}");
        // It rises all the way.
        let mut last = 0.0;
        for i in 0..=100 {
            let y = expand(f64::from(i) / 100.0, 0.6, 4.0);
            assert!(y >= last);
            last = y;
        }
    }

    #[test]
    fn hue_is_kept_and_saturation_works_around_luminance() {
        let c = Curve { start: 50, peak: 600, ..Curve::default() };
        let [r, g, b] = c.map([1.0, 0.8, 0.6]);
        let [r0, g0, b0] = Curve::default().map([1.0, 0.8, 0.6]);
        near(r / g, r0 / g0);
        near(g / b, g0 / b0);
        let grey = Curve { saturation: 50, ..Curve::default() }.map([0.8, 0.8, 0.8]);
        near(grey[0], grey[1]);
        let pale = Curve { saturation: 50, ..Curve::default() }.map([1.0, 0.0, 0.0]);
        assert!(pale[1] > 0.0 && pale[0] < 1.0, "towards grey");
    }

    #[test]
    fn white_on_its_own_moves_the_picture_and_the_peak_keeps_above_it() {
        let c = Curve { white: 406, peak: 406, ..Curve::default() };
        near(c.map([1.0, 1.0, 1.0])[0], 2.0);
        let mut c = Curve::default();
        CurveParam::White.set(&mut c, 300);
        assert_eq!((c.white, c.peak), (300, 300));
        CurveParam::Peak.set(&mut c, 250);
        assert_eq!((c.white, c.peak), (250, 250), "a peak below white brings white down");
        CurveParam::Peak.set(&mut c, 5000);
        assert_eq!(c.peak, 1000, "within range");
    }

    #[test]
    fn slider_positions_fit_and_round_trip_on_the_steps() {
        for p in CurveParam::ALL {
            let (lo, hi, step) = p.range();
            assert!(hi / step <= u16::from(u8::MAX), "{p:?}");
            assert_eq!(p.value(p.position(lo)), lo);
            assert_eq!(p.value(p.position(hi)), hi);
        }
    }
}
