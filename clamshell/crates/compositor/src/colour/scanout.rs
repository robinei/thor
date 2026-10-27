//! A surface's colour as the final pass draws it on a screen (see `shaders::direct`), baked into a display
//! plane's colour pipeline, so that a window scanned out directly shows as it would composed. Armada's
//! kernel gives each VIG plane of the Thor's display engine a pipeline of a 1D LUT, then a 3D LUT, ahead of
//! blending (its patch 0077, the `drm_colorop` API).
//!
//! The final pass, from a surface's signal to the screen's: on an SDR surface with a curve, the black point
//! and contrast on each channel's signal; the decoding; the curve's highlights and saturation on luminance
//! (an HDR surface: its compression into the screen's peak); the matrix to the working space and from it
//! to the screen's primaries; the screen's scaling, a pixel above its peak brought down whole; the
//! encoding. The 1D LUT shapes each channel for the 3D LUT, which takes the rest, sampled at its nodes:
//! on an SDR surface it takes the black point and contrast, keeping the signal's perceptual spacing; on an
//! HDR one it decodes and encodes again with a gamma over the game's own range (to its peak), so that the
//! nodes cover the light the game shows rather than PQ's ten thousand nits. [`Transform`] is that pass on
//! the CPU, from the same uniforms (`render::curve_uniforms`).
//!
//! How close: for most colours within a perceptual difference (ΔE) of about 1, SDR or HDR (the tests).
//! More only at the gamut's edge, where the pass clips a colour (a curve's saturation pushing it out, an
//! HDR game's colours beyond the screen's) and 17 nodes along each axis round that corner off: a pure red
//! slightly less saturated, ΔE2000 near 4 at the worst. No banding: the interpolation is smooth.

use clamshell_state::colour::ColourDescription;

use super::math;

/// The plane's 1D LUT's entries, from 0 to 1 of the signal (patch 0077: IGC v6, 257 samples).
pub(crate) const LUT1D_SIZE: usize = 257;
/// The plane's 3D LUT's nodes along each axis (patch 0077: gamut v6.1, 17³).
pub(crate) const LUT3D_EDGE: usize = 17;

/// A surface's colour on a screen: the final pass's for a surface described as `surface` with the curve's
/// uniforms, on a screen described as `output`.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Transform {
    surface: ColourDescription,
    output: ColourDescription,
    /// `curve_a` and `curve_b` (see `render::curve_uniforms`).
    curve: [[f64; 4]; 2],
    into: [[f64; 3]; 3],
    out_of: [[f64; 3]; 3],
}

impl Transform {
    /// None for a description the final pass can't draw either.
    pub(crate) fn new(surface: ColourDescription, curve: [[f32; 4]; 2], output: ColourDescription) -> Option<Self> {
        use clamshell_state::colour::ColourPrimaries::Bt2020;
        let into = math::matrix(surface.primaries, Bt2020)?;
        let out_of = math::matrix(Bt2020, output.primaries)?;
        math::decode(0.5, surface)?;
        math::encode(0.5, output)?;
        Some(Self { surface, output, curve: curve.map(|v| v.map(f64::from)), into, out_of })
    }

    fn sdr(&self) -> bool {
        super::render::sdr(self.surface)
    }

    fn on(&self) -> bool {
        self.curve[1][3] > 0.5
    }

    /// An HDR surface's light, in units of its white, at the top of the 1D LUT's range: its own peak.
    fn span(&self) -> f64 {
        self.curve[0][3].max(1.0)
    }

    /// The 1D LUT's part, on one channel's signal: an SDR surface's black point and contrast, an HDR
    /// surface's light under a gamma over its range.
    pub(crate) fn shaper(&self, v: f64) -> f64 {
        if super::render::hdr(self.surface) {
            let light = math::decode(v, self.surface).unwrap_or(0.0);
            return (light / self.span()).clamp(0.0, 1.0).powf(1.0 / 2.2);
        }
        if !(self.sdr() && self.on()) {
            return v;
        }
        let [black, contrast, ..] = self.curve[0];
        ((v - black) / (1.0 - black).max(0.000001)).clamp(0.0, 1.0).powf(contrast)
    }

    /// The 3D LUT's part: the rest of the pass, from the shaped signal to the screen's.
    pub(crate) fn rest(&self, v: [f64; 3]) -> [f64; 3] {
        self.encode(self.light(v))
    }

    /// The surface's half of the pass, from the shaped signal to light in the working space.
    fn light(&self, v: [f64; 3]) -> [f64; 3] {
        let [[_, _, start, peak], [white, saturation, headroom, _]] = self.curve;
        let decoded = if super::render::hdr(self.surface) {
            v.map(|c| c.max(0.0).powf(2.2) * self.span())
        } else {
            v.map(|c| math::decode(c, self.surface).unwrap_or(0.0))
        };
        if self.sdr() {
            let light = if self.on() {
                let white = white.min(headroom);
                let room = (headroom / white).max(1.0);
                let peak = peak.min(room).max(1.0);
                let y = 0.2126 * decoded[0] + 0.7152 * decoded[1] + 0.0722 * decoded[2];
                let t = (y - start) / (1.0 - start).max(0.000001);
                let e = if y > start && start < 1.0 { start + (1.0 - start) * t + (peak - 1.0) * t * t } else { y };
                let c = if y > 0.0 { decoded.map(|c| c * e / y) } else { [0.0; 3] };
                c.map(|c| (e + (c - e) * saturation).max(0.0) * white)
            } else {
                decoded
            };
            math::apply(self.into, light)
        } else if super::render::hdr(self.surface) {
            let v = math::apply(self.into, decoded);
            let on = self.on();
            let white = if on { white.min(headroom) } else { headroom.min(1.0) };
            let [[black, contrast, ..], _] = self.curve;
            let v = if on {
                v.map(|c| {
                    if c >= 1.0 {
                        return c;
                    }
                    let g = c.clamp(0.0, 1.0).powf(1.0 / 2.2);
                    ((g - black) / (1.0 - black).max(0.000001)).clamp(0.0, 1.0).powf(contrast).powf(2.2)
                })
            } else {
                v.map(|c| c.max(0.0))
            };
            let y = 0.2627 * v[0] + 0.6780 * v[1] + 0.0593 * v[2];
            let room = headroom / white;
            let k = (if on { start } else { 1.0 }).min(0.75 * room);
            let e = if peak > room && y > k {
                let x = y - k;
                (k + x / (1.0 + x * (1.0 / (room - k) - 1.0 / (peak - k)))).min(room)
            } else {
                y
            };
            let c = if y > 0.0 { v.map(|c| c * e / y) } else { [0.0; 3] };
            c.map(|c| (e + (c - e) * if on { saturation } else { 1.0 }).max(0.0) * white)
        } else {
            math::apply(self.into, decoded)
        }
    }

    /// The screen's half of the pass, from the working space's light to its signal.
    fn encode(&self, light: [f64; 3]) -> [f64; 3] {
        let v = math::apply(self.out_of, light);
        let pq = self.output.transfer == clamshell_state::colour::TransferFunction::Pq;
        // `math::encode` scales by the screen's luminances as the pass's `scaling` does; a pixel above the
        // screen's peak is brought down whole, keeping its hue.
        let v = if pq {
            v
        } else {
            let scaling = f64::from(self.output.luminances().max) / f64::from(self.output.luminances().reference);
            let m = v.iter().fold(0.0_f64, |m, c| m.max(*c)) / scaling;
            if m > 1.0 { v.map(|c| c / m) } else { v }
        };
        v.map(|c| math::encode(c, self.output).unwrap_or(0.0))
    }

    /// The whole pass, from the surface's signal to the screen's.
    pub(crate) fn apply(&self, v: [f64; 3]) -> [f64; 3] {
        self.rest(v.map(|c| self.shaper(c)))
    }

    /// The 1D LUT of `size` entries ([`LUT1D_SIZE`] on the Thor), from 0 to 1 of the signal, each entry
    /// red, green, blue (the same shaper on each).
    pub(crate) fn lut1d(&self, size: usize) -> Vec<[f64; 3]> {
        let size = size.max(2);
        (0..size).map(|i| [self.shaper(i as f64 / (size - 1) as f64); 3]).collect()
    }

    /// The 3D LUT of `edge` nodes along each axis ([`LUT3D_EDGE`] on the Thor), in the display engine's
    /// order, red fastest: node `r + edge (g + edge b)` (as the patch streams them, the order its DSPP
    /// gamut LUT documents, and the upstream colorop's).
    pub(crate) fn lut3d(&self, edge: usize) -> Vec<[f64; 3]> {
        let edge = edge.max(2);
        let at = |i: usize| i as f64 / (edge - 1) as f64;
        let mut lut = Vec::with_capacity(edge.pow(3));
        for b in 0..edge {
            for g in 0..edge {
                for r in 0..edge {
                    lut.push(self.rest([at(r), at(g), at(b)]).map(|c| c.clamp(0.0, 1.0)));
                }
            }
        }
        lut
    }
}

/// `value`, 0 to 1, as a `drm_color_lut32` component: the full range of a u32, of which the display engine
/// takes the top bits.
pub(crate) fn lut32(value: f64) -> u32 {
    (value.clamp(0.0, 1.0) * f64::from(u32::MAX)).round() as u32
}

/// LUT entries as a `drm_color_lut32` blob's bytes: red, green, blue and a reserved word, each a u32.
pub(crate) fn blob(lut: &[[f64; 3]]) -> Vec<u8> {
    lut.iter().flat_map(|[r, g, b]| [lut32(*r), lut32(*g), lut32(*b), 0]).flat_map(u32::to_ne_bytes).collect()
}

#[cfg(test)]
mod tests;
