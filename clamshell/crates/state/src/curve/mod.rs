//! Tone curves: a game's SDR or HDR mapped to the screen's SDR or HDR. SDR on an HDR screen is spread
//! into its headroom (inverse tone mapping, as Special K does it rather than Windows' Auto HDR), HDR is
//! brought down to the screen's peak, and SDR on SDR is shaped. A curve is a profile
//! ([`crate::profiles`]): named and shared, or a game's own, chosen per game by
//! [`crate::settings::Setting::Curve`]; 0 is Default colours, as the game sends them.
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
//! 4. Highlights above the start (a fraction of white) expand into the screen's room above white, as far
//!    as the curve says (a share of that room, so a curve means the same on any screen), along a curve
//!    that joins the line below the start smoothly.
//! 5. Saturation, around the luminance.
//!
//! On a screen, the curve is limited by what the screen can show ([`Curve::map_on`]): SDR white goes no
//! higher than its peak, and highlights expand no further. On an SDR screen there is no room, which leaves
//! the black point, contrast and saturation. An HDR game's light goes through the same curve ([`map_hdr`]): the black point,
//! contrast, white and saturation mean the same, and its own highlights are compressed into the screen's
//! peak from the knee where SDR's would start expanding. Without a curve an HDR game is only compressed
//! into the screen.

pub mod plot;
pub mod screens;

use crate::profiles::{Profile, Profiles};

/// The nits the compositor's linear scene has at 1.0, its reference white (BT.2408's 203).
pub const REFERENCE_NITS: f64 = 203.0;

/// The screen [`Curve::map`] shows a curve on: a 1000-nit one.
pub(crate) const REFERENCE_SCREEN_NITS: f64 = 1000.0;

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
    /// How far the brightest highlights expand into the screen's room above white, in percent: 0 not at
    /// all, 100 to the screen's peak.
    pub highlights: u16,
    /// Saturation, in percent (100 changes nothing).
    pub saturation: u16,
}

/// A new curve changes nothing: SDR white at the reference, no expansion.
impl Default for Curve {
    fn default() -> Curve {
        Curve { black: 0, contrast: 100, white: REFERENCE_NITS as u16, start: 100, highlights: 0, saturation: 100 }
    }
}

/// A curve's parameter, as its slider on the edit screen sets it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum CurveParam {
    Black,
    Contrast,
    White,
    Start,
    Highlights,
    Saturation,
}

impl CurveParam {
    pub const ALL: [CurveParam; 6] =
        [CurveParam::Black, CurveParam::Contrast, CurveParam::White, CurveParam::Start, CurveParam::Highlights, CurveParam::Saturation];

    /// The parameter's range, and the step a slider moves it by. Slider positions are the value divided by
    /// the step, which keeps them within a slider's `u8`.
    pub fn range(self) -> (u16, u16, u16) {
        match self {
            CurveParam::Black => (0, 150, 1),
            CurveParam::Contrast => (70, 150, 1),
            CurveParam::White => (100, 400, 5),
            CurveParam::Start => (30, 100, 1),
            CurveParam::Highlights => (0, 100, 5),
            CurveParam::Saturation => (50, 150, 1),
        }
    }

    pub fn get(self, c: &Curve) -> u16 {
        match self {
            CurveParam::Black => c.black,
            CurveParam::Contrast => c.contrast,
            CurveParam::White => c.white,
            CurveParam::Start => c.start,
            CurveParam::Highlights => c.highlights,
            CurveParam::Saturation => c.saturation,
        }
    }

    /// Sets the parameter, within its range.
    pub fn set(self, c: &mut Curve, value: u16) {
        let (lo, hi, _) = self.range();
        let value = value.clamp(lo, hi);
        match self {
            CurveParam::Black => c.black = value,
            CurveParam::Contrast => c.contrast = value,
            CurveParam::White => c.white = value,
            CurveParam::Start => c.start = value,
            CurveParam::Highlights => c.highlights = value,
            CurveParam::Saturation => c.saturation = value,
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
            CurveParam::Highlights => "Highlights to",
            CurveParam::Saturation => "Saturation",
        }
    }
}

impl Curve {
    /// A pixel of SDR signal (`rgb`, each 0 to 1, sRGB's encoding) as linear light in the compositor's
    /// scene, where 1.0 is the reference white ([`REFERENCE_NITS`]), on a [`REFERENCE_SCREEN_NITS`] screen.
    /// The colours are those of the input's primaries; the compositor converts them after.
    pub fn map(&self, rgb: [f64; 3]) -> [f64; 3] {
        self.map_on(rgb, REFERENCE_SCREEN_NITS / REFERENCE_NITS)
    }

    /// The brightest highlights' level in units of white, on a screen whose room above white is `room`
    /// (its peak over where white is shown): that share of the room.
    pub fn peak_in(&self, room: f64) -> f64 {
        1.0 + (room - 1.0).max(0.0) * f64::from(self.highlights.min(100)) / 100.0
    }

    /// [`Curve::map`] on a screen with `headroom`: its peak over the reference white (1 for an SDR
    /// screen). SDR white goes no higher than the peak, and highlights expand into the room above it.
    pub fn map_on(&self, rgb: [f64; 3], headroom: f64) -> [f64; 3] {
        let black = f64::from(self.black) / 1000.0;
        let contrast = f64::from(self.contrast) / 100.0;
        let white = f64::from(self.white);
        let start = f64::from(self.start) / 100.0;
        let to_scene = (white / REFERENCE_NITS).min(headroom);
        let peak = self.peak_in(headroom / to_scene);
        let linear = rgb.map(|e| {
            let e = ((e - black) / (1.0 - black).max(1e-6)).clamp(0.0, 1.0).powf(contrast);
            srgb_decode(e)
        });
        let y = luminance(linear);
        let expanded = expand(y, start, peak);
        let scale = if y > 0.0 { expanded / y } else { 0.0 };
        let saturation = f64::from(self.saturation) / 100.0;
        linear.map(|c| ((expanded + (c * scale - expanded) * saturation).max(0.0)) * to_scene)
    }
}

/// An HDR game's light (linear, 1 at its reference white, BT.2020's primaries) as the compositor shows it
/// on a screen with `headroom` (its peak over the reference white). With a curve: the black point and
/// contrast on each channel's light below white, as on SDR's signal (with gamma 2.2 standing in for it);
/// white at the curve's white, within the screen's peak; saturation around the luminance. With or
/// without one, the highlights above the knee (the curve's start, or white; at most three quarters of the
/// screen's peak) are compressed into the screen's peak when the game's own (`content_peak`, the same
/// units) is beyond it.
pub fn map_hdr(curve: Option<&Curve>, light: [f64; 3], content_peak: f64, headroom: f64) -> [f64; 3] {
    let (black, contrast, white, start, saturation) = match curve {
        Some(c) => (
            f64::from(c.black) / 1000.0,
            f64::from(c.contrast) / 100.0,
            f64::from(c.white) / REFERENCE_NITS,
            f64::from(c.start) / 100.0,
            f64::from(c.saturation) / 100.0,
        ),
        None => (0.0, 1.0, 1.0, 1.0, 1.0),
    };
    let to_scene = white.min(headroom);
    let light = light.map(|v| {
        if curve.is_none() || v > 1.0 {
            return v.max(0.0);
        }
        let e = v.max(0.0).powf(1.0 / 2.2);
        ((e - black) / (1.0 - black).max(1e-6)).clamp(0.0, 1.0).powf(contrast).powf(2.2)
    });
    let y = 0.2627 * light[0] + 0.6780 * light[1] + 0.0593 * light[2];
    let compressed = compress(y, start, content_peak, headroom / to_scene);
    let scale = if y > 0.0 { compressed / y } else { 0.0 };
    light.map(|c| ((compressed + (c * scale - compressed) * saturation).max(0.0)) * to_scene)
}

/// Luminance `y` with what is above the knee compressed so that the content's `peak` reaches the screen's
/// (`target`), all in units of white: unchanged below the knee and when the content fits, else
/// `k + x / (1 + x (1/T - 1/X))`, with `x`, `X` and `T` how far `y`, the content's peak and the target are
/// above the knee. Its slope is 1 at the knee, it rises all the way, and the content's peak lands on the
/// target. The knee is at most three quarters of the target, so that something is left to compress into.
pub(crate) fn compress(y: f64, knee: f64, peak: f64, target: f64) -> f64 {
    let knee = knee.min(0.75 * target);
    if peak <= target || y <= knee {
        return y;
    }
    let (x, content, room) = (y - knee, peak - knee, target - knee);
    (knee + x / (1.0 + x * (1.0 / room - 1.0 / content))).min(target)
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
        let curve = |black, contrast, white, start, highlights, saturation| Curve { black, contrast, white, start, highlights, saturation };
        Curves::from_list(vec![
            Profile::with(1, "True black", curve(20, 110, 203, 60, 60, 105)),
            Profile::with(2, "Punchy", curve(10, 115, 220, 50, 100, 115)),
            Profile::with(3, "Gentle", curve(0, 100, 203, 80, 25, 100)),
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
    fn on_a_screen_white_and_highlights_stay_within_its_peak() {
        let c = Curve { white: 300, start: 60, highlights: 100, ..Curve::default() };
        // On an SDR screen: white at the screen's white, no expansion, the rest of the curve applies.
        let sdr = Curve { white: 203, ..c };
        near(c.map_on([1.0, 1.0, 1.0], 1.0)[0], 1.0);
        near(c.map_on([0.5, 0.5, 0.5], 1.0)[0], sdr.map_on([0.5, 0.5, 0.5], 1.0)[0]);
        // All the room: the highlights reach the screen's peak, whatever it is.
        near(c.map_on([1.0, 1.0, 1.0], 650.0 / 203.0)[0], 650.0 / 203.0);
        near(c.map_on([1.0, 1.0, 1.0], 10.0)[0], 10.0);
        // Half of it: halfway from white to the peak, on any screen.
        let half = Curve { highlights: 50, ..c };
        let white = 300.0 / 203.0;
        for headroom in [650.0 / 203.0, 10.0] {
            near(half.map_on([1.0, 1.0, 1.0], headroom)[0], white + (headroom - white) / 2.0);
        }
    }

    #[test]
    fn an_hdr_games_highlights_are_compressed_into_the_screen_from_the_knee() {
        let headroom = 650.0 / 203.0;
        // Off, a game reaching 1000 nits: its peak lands on the screen's, white and below unchanged.
        let peak = 1000.0 / 203.0;
        near(map_hdr(None, [peak; 3], peak, headroom)[0], headroom);
        near(map_hdr(None, [1.0; 3], peak, headroom)[0], 1.0);
        near(map_hdr(None, [0.2; 3], peak, headroom)[0], 0.2);
        // A game within the screen's peak is left as it is.
        near(map_hdr(None, [3.0; 3], 3.0, headroom)[0], 3.0);
        // It rises all the way, smoothly from the knee.
        let mut last = 0.0;
        for i in 0..=500 {
            let y = compress(f64::from(i) / 100.0, 1.0, peak, headroom);
            assert!(y >= last && y <= headroom);
            last = y;
        }
        let d = 1e-6;
        assert!(((compress(1.0 + d, 1.0, peak, headroom) - 1.0) / d - 1.0).abs() < 1e-3);
        // A new curve changes nothing, as for SDR.
        for v in [0.0, 0.05, 0.5, 1.0, 2.0, 4.0] {
            near(map_hdr(Some(&Curve::default()), [v; 3], peak, headroom)[0], map_hdr(None, [v; 3], peak, headroom)[0]);
        }
        // The black point and white mean what they do for SDR: the floor to black, white to the curve's.
        let c = Curve { black: 50, white: 300, ..Curve::default() };
        near(map_hdr(Some(&c), [0.001; 3], peak, headroom)[0], 0.0);
        near(map_hdr(Some(&c), [1.0; 3], peak, headroom)[0], 300.0 / 203.0);
        // On an SDR screen it all goes into white.
        assert!(map_hdr(Some(&c), [peak; 3], peak, 1.0)[0] <= 1.0);
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
        let c = Curve { white: 203, start: 60, highlights: 100, ..Curve::default() };
        // White reaches the reference screen's peak.
        near(c.map([1.0, 1.0, 1.0])[0], REFERENCE_SCREEN_NITS / REFERENCE_NITS);
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
        let c = Curve { start: 50, highlights: 50, ..Curve::default() };
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
    fn white_on_its_own_moves_the_picture_and_highlights_stay_within_range() {
        let c = Curve { white: 406, ..Curve::default() };
        near(c.map([1.0, 1.0, 1.0])[0], 2.0);
        let mut c = Curve::default();
        CurveParam::Highlights.set(&mut c, 500);
        assert_eq!(c.highlights, 100, "within range");
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
