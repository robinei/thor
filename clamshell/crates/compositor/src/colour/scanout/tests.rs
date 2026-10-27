#![allow(clippy::unwrap_used, clippy::indexing_slicing)]
use super::*;
use clamshell_state::colour::{ColourLuminance, ColourPrimaries, TransferFunction};
use clamshell_state::curve::{Curve, REFERENCE_NITS};

/// A curve that does something: a raised black point, more contrast, highlights from 70 % of white
/// expanded half way into the screen's room, more saturation.
fn curve() -> Curve {
    Curve { black: 20, contrast: 115, white: 203, start: 70, highlights: 50, saturation: 120 }
}

fn uniforms(curve: Option<&Curve>, headroom: f32, hdr_peak: Option<f32>) -> [[f32; 4]; 2] {
    super::super::render::curve_uniforms(curve, headroom, hdr_peak)
}

/// An sRGB screen at `nits` peak over the reference white.
fn screen(nits: u32) -> ColourDescription {
    ColourDescription { luminance: Some(ColourLuminance { min: 0, max: nits, reference: REFERENCE_NITS as u32 }), ..ColourDescription::default() }
}

/// What the display engine shows for `v` through the two LUTs: the 1D LUT's 12 bits interpolated linearly,
/// then the 3D LUT's 10-bit nodes tetrahedrally (as patch 0077 loads them).
fn hardware(lut1d: &[[f64; 3]], lut3d: &[[f64; 3]], v: [f64; 3]) -> [f64; 3] {
    let bits = |x: f64, n: u32| (lut32(x) >> (32 - n)) as f64 / f64::from((1u32 << n) - 1);
    let shaped: Vec<f64> = (0..3)
        .map(|c| {
            let at = v[c].clamp(0.0, 1.0) * (LUT1D_SIZE - 1) as f64;
            let i = (at.floor() as usize).min(LUT1D_SIZE - 2);
            let f = at - i as f64;
            bits(lut1d[i][c], 12) * (1.0 - f) + bits(lut1d[i + 1][c], 12) * f
        })
        .collect();
    let n = LUT3D_EDGE - 1;
    let at: Vec<f64> = shaped.iter().map(|s| s.clamp(0.0, 1.0) * n as f64).collect();
    let base: Vec<usize> = at.iter().map(|a| (a.floor() as usize).min(n - 1)).collect();
    let f: Vec<f64> = at.iter().zip(&base).map(|(a, b)| a - *b as f64).collect();
    let node = |dr: usize, dg: usize, db: usize| {
        let entry = lut3d[(base[0] + dr) + LUT3D_EDGE * ((base[1] + dg) + LUT3D_EDGE * (base[2] + db))];
        entry.map(|x| bits(x, 10))
    };
    // The tetrahedron of the cube the point is in, by the order of its fractions.
    let (fr, fg, fb) = (f[0], f[1], f[2]);
    let steps: [((usize, usize, usize), f64); 3] = if fr >= fg && fg >= fb {
        [((1, 0, 0), fr), ((1, 1, 0), fg), ((1, 1, 1), fb)]
    } else if fr >= fb && fb >= fg {
        [((1, 0, 0), fr), ((1, 0, 1), fb), ((1, 1, 1), fg)]
    } else if fb >= fr && fr >= fg {
        [((0, 0, 1), fb), ((1, 0, 1), fr), ((1, 1, 1), fg)]
    } else if fg >= fr && fr >= fb {
        [((0, 1, 0), fg), ((1, 1, 0), fr), ((1, 1, 1), fb)]
    } else if fg >= fb && fb >= fr {
        [((0, 1, 0), fg), ((0, 1, 1), fb), ((1, 1, 1), fr)]
    } else {
        [((0, 0, 1), fb), ((0, 1, 1), fg), ((1, 1, 1), fr)]
    };
    let mut out = node(0, 0, 0);
    let mut previous = node(0, 0, 0);
    for ((dr, dg, db), weight) in steps {
        let next = node(dr, dg, db);
        for c in 0..3 {
            out[c] += (next[c] - previous[c]) * weight;
        }
        previous = next;
    }
    out
}

/// CIELAB of a screen's signal, its white at L* 100 at the reference white, for ΔE.
fn lab(screen: ColourDescription, signal: [f64; 3]) -> [f64; 3] {
    let light = signal.map(|c| math::decode(c, screen).unwrap());
    let srgb = math::apply(math::matrix(screen.primaries, ColourPrimaries::Srgb).unwrap(), light);
    let xyz = math::apply([[0.4124, 0.3576, 0.1805], [0.2126, 0.7152, 0.0722], [0.0193, 0.1192, 0.9505]], srgb);
    let f = |t: f64| if t > 216.0 / 24389.0 { t.cbrt() } else { (24389.0 / 27.0 * t + 16.0) / 116.0 };
    let [x, y, z] = [xyz[0] / 0.9505, xyz[1], xyz[2] / 1.089];
    [116.0 * f(y) - 16.0, 500.0 * (f(x) - f(y)), 200.0 * (f(y) - f(z))]
}

/// The difference between what the LUTs show and the pass, as ΔE76 over a grid of signals (`signal` of each
/// point from 0 to 1): the most, and what 99 % of the grid is within.
fn errors(transform: &Transform, screen: ColourDescription, signal: impl Fn([f64; 3]) -> [f64; 3]) -> (f64, f64) {
    let (lut1d, lut3d) = (transform.lut1d(LUT1D_SIZE), transform.lut3d(LUT3D_EDGE));
    let steps = 32;
    let mut all = Vec::new();
    for r in 0..=steps {
        for g in 0..=steps {
            for b in 0..=steps {
                let v = signal([r, g, b].map(|x| f64::from(x) / f64::from(steps)));
                let (want, got) = (lab(screen, transform.apply(v).map(|c| c.clamp(0.0, 1.0))), lab(screen, hardware(&lut1d, &lut3d, v)));
                all.push(((want[0] - got[0]).powi(2) + (want[1] - got[1]).powi(2) + (want[2] - got[2]).powi(2)).sqrt());
            }
        }
    }
    all.sort_by(f64::total_cmp);
    (all[all.len() - 1], all[all.len() * 99 / 100])
}

#[test]
fn the_pass_on_an_sdr_surface_is_the_curves() {
    // On an SDR screen, sRGB to sRGB: the curve's light, encoded, with what is above white brought down
    // whole.
    let transform = Transform::new(ColourDescription::default(), uniforms(Some(&curve()), 1.0, None), screen(REFERENCE_NITS as u32)).unwrap();
    for v in [[0.0, 0.0, 0.0], [0.1, 0.2, 0.3], [0.5, 0.5, 0.5], [0.9, 0.4, 0.1], [1.0, 1.0, 1.0]] {
        let light = curve().map_on(v, 1.0);
        let m = light.iter().fold(1.0_f64, |m, c| m.max(*c));
        let want = light.map(|c| math::encode(c / m, ColourDescription::default()).unwrap());
        let got = transform.apply(v);
        for c in 0..3 {
            assert!((want[c] - got[c]).abs() < 1e-6, "{v:?}: {want:?} != {got:?}");
        }
    }
}

#[test]
fn the_pass_on_an_hdr_surface_is_the_compression_into_the_screen() {
    let pq = ColourDescription { primaries: ColourPrimaries::Bt2020, transfer: TransferFunction::Pq, ..ColourDescription::default() };
    let headroom = 650.0 / REFERENCE_NITS as f32;
    let output = ColourDescription { primaries: ColourPrimaries::Bt2020, ..screen(650) };
    let transform = Transform::new(pq, uniforms(Some(&curve()), headroom, Some(4.0)), output).unwrap();
    // Within its peak (four times its white), as the 1D LUT takes it.
    for v in [[0.3, 0.4, 0.5], [0.6, 0.6, 0.6], [0.62, 0.5, 0.2]] {
        let light = v.map(|c| math::decode(c, pq).unwrap());
        let mapped = clamshell_state::curve::map_hdr(Some(&curve()), light, 4.0, f64::from(headroom));
        let want = transform.encode(mapped);
        let got = transform.apply(v);
        for c in 0..3 {
            assert!((want[c] - got[c]).abs() < 1e-6, "{v:?}: {want:?} != {got:?}");
        }
    }
}

/// The LUTs show the pass within a perceptual difference (ΔE76) of about 1 for most colours, more only
/// at the gamut's edge: where a colour leaves it, more saturated by a curve, the pass clips it at once,
/// while 17 nodes along each axis can only round that corner off. A shaper can't help there, as clipping
/// mixes the channels. ΔE76 overstates differences that saturated (ΔE2000 puts the worst near 4).
#[test]
fn the_luts_show_the_pass_but_at_the_gamuts_edge() {
    // A game on the Thor's top panel as gamescope's profile gives it (primaries and 650 nits): an sRGB one
    // with a curve and without, and an HDR one with colours as games have them, sRGB's in BT.2020, up to
    // its peak of four times its white.
    let panel = ColourDescription {
        primaries: ColourPrimaries::Custom([(680_000, 320_000), (265_000, 690_000), (150_000, 60_000), (312_700, 329_000)]),
        ..screen(650)
    };
    let headroom = 650.0 / REFERENCE_NITS as f32;
    let srgb = ColourDescription::default();
    let pq = ColourDescription { primaries: ColourPrimaries::Bt2020, transfer: TransferFunction::Pq, ..ColourDescription::default() };
    let to_bt2020 = math::matrix(ColourPrimaries::Srgb, ColourPrimaries::Bt2020).unwrap();
    let hdr = |v: [f64; 3]| math::apply(to_bt2020, v.map(|c| c.powf(2.2) * 4.0)).map(|c| math::encode(c, pq).unwrap());
    let sdr = |v: [f64; 3]| v;
    for (name, surface, curve, peak, signal, limits) in [
        ("SDR, no curve", srgb, None, None, &sdr as &dyn Fn([f64; 3]) -> [f64; 3], (1.5, 0.5)),
        ("SDR with a curve", srgb, Some(curve()), None, &sdr, (5.0, 2.5)),
        ("HDR, no curve", pq, None, Some(4.0), &hdr, (9.0, 3.0)),
        ("HDR with a curve", pq, Some(curve()), Some(4.0), &hdr, (21.0, 7.5)),
    ] {
        let transform = Transform::new(surface, uniforms(curve.as_ref(), headroom, peak), panel).unwrap();
        let (worst, p99) = errors(&transform, panel, signal);
        assert!(worst < limits.0 && p99 < limits.1, "{name}: ΔE at most {worst:.2}, 99 % within {p99:.2}");
    }
}

#[test]
fn a_lut_is_a_blob_of_drm_color_lut32() {
    let blob = blob(&[[0.0, 0.5, 1.0]]);
    let words: Vec<u32> = blob.chunks(4).map(|w| u32::from_ne_bytes(w.try_into().unwrap())).collect();
    assert_eq!(words, [0, 0x8000_0000, u32::MAX, 0]);
}
