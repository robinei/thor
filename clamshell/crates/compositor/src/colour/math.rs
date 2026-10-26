//! Colour maths on the CPU: the matrix from one set of primaries to another (through CIE XYZ, for primaries
//! of the same white point, as nothing adapts between whites), and each transfer function's decoding to
//! linear light and encoding from it. The shaders are built with these, and a description they can't
//! handle is refused before a shader is made for it.

use clamshell_state::colour::{ColourDescription, ColourPrimaries, TransferFunction};

type Matrix = [[f64; 3]; 3];

pub(crate) fn coordinates(primaries: ColourPrimaries) -> [(i32, i32); 4] {
    primaries.coordinates()
}
fn dot([a, b, c]: [f64; 3], [x, y, z]: [f64; 3]) -> f64 {
    a * x + b * y + c * z
}
pub(crate) fn apply([a, b, c]: Matrix, value: [f64; 3]) -> [f64; 3] {
    [dot(a, value), dot(b, value), dot(c, value)]
}
fn inverse([[a, b, c], [d, e, f], [g, h, i]]: Matrix) -> Option<Matrix> {
    let det = a * (e * i - f * h) - b * (d * i - f * g) + c * (d * h - e * g);
    if !det.is_finite() || det.abs() < 1e-12 {
        return None;
    }
    Some([
        [(e * i - f * h) / det, (c * h - b * i) / det, (b * f - c * e) / det],
        [(f * g - d * i) / det, (a * i - c * g) / det, (c * d - a * f) / det],
        [(d * h - e * g) / det, (b * g - a * h) / det, (a * e - b * d) / det],
    ])
}
fn xyz((x, y): (i32, i32)) -> Option<[f64; 3]> {
    let (x, y) = (f64::from(x) / 1e6, f64::from(y) / 1e6);
    if y <= 0.0 || !(-2.0..=2.0).contains(&x) || !(-2.0..=2.0).contains(&y) {
        return None;
    }
    Some([x / y, 1.0, (1.0 - x - y) / y])
}
fn to_xyz(primaries: ColourPrimaries) -> Option<Matrix> {
    let [r, g, b, w] = coordinates(primaries);
    let ([rx, ry, rz], [gx, gy, gz], [bx, by, bz]) = (xyz(r)?, xyz(g)?, xyz(b)?);
    let matrix = [[rx, gx, bx], [ry, gy, by], [rz, gz, bz]];
    let [r, g, b] = apply(inverse(matrix)?, xyz(w)?);
    if [r, g, b].iter().any(|v| !v.is_finite() || *v <= 0.0) {
        return None;
    }
    Some([[rx * r, gx * g, bx * b], [ry * r, gy * g, by * b], [rz * r, gz * g, bz * b]])
}
/// Both spaces currently use their declared white directly. Non-D65 white requires adaptation.
pub(crate) fn matrix(from: ColourPrimaries, to: ColourPrimaries) -> Option<Matrix> {
    let [_, _, _, from_white] = coordinates(from);
    let [_, _, _, to_white] = coordinates(to);
    if from_white != to_white {
        return None;
    }
    let inv = inverse(to_xyz(to)?)?;
    let [[a, b, c], [d, e, f], [g, h, i]] = to_xyz(from)?;
    let [x, y, z] = [apply(inv, [a, d, g]), apply(inv, [b, e, h]), apply(inv, [c, f, i])];
    let ([a, d, g], [b, e, h], [c, f, i]) = (x, y, z);
    Some([[a, b, c], [d, e, f], [g, h, i]])
}
/// SMPTE ST 2084, with absolute luminance in nits (ITU-R BT.2100).
fn pq_decode(value: f64) -> f64 {
    let p = value.max(0.0).powf(32.0 / 2523.0);
    ((p - 3424.0 / 4096.0).max(0.0) / (2413.0 / 128.0 - 2392.0 / 128.0 * p).max(1e-12)).powf(16384.0 / 2610.0) * 10000.0
}
fn pq_encode(nits: f64) -> f64 {
    let p = (nits.max(0.0) / 10000.0).powf(2610.0 / 16384.0);
    ((3424.0 / 4096.0 + 2413.0 / 128.0 * p) / (1.0 + 2392.0 / 128.0 * p)).powf(2523.0 / 32.0)
}
pub(crate) fn decode(value: f64, description: ColourDescription) -> Option<f64> {
    let reference = f64::from(description.luminances().reference);
    if reference <= 0.0 {
        return None;
    }
    let scale = f64::from(description.luminances().max) / reference;
    Some(match description.transfer {
        TransferFunction::Srgb => {
            if value <= 0.04045 {
                value / 12.92 * scale
            } else {
                ((value + 0.055) / 1.055).powf(2.4) * scale
            }
        }
        TransferFunction::Gamma(exponent) if (10000..=100000).contains(&exponent) => value.max(0.0).powf(f64::from(exponent) / 10000.0) * scale,
        TransferFunction::Linear => value * scale,
        TransferFunction::Pq => pq_decode(value) / reference,
        _ => return None,
    })
}
pub(crate) fn encode(linear: f64, description: ColourDescription) -> Option<f64> {
    let linear = if description.transfer == TransferFunction::Pq {
        linear
    } else {
        linear * f64::from(description.luminances().reference) / f64::from(description.luminances().max)
    };
    Some(match description.transfer {
        TransferFunction::Srgb => {
            if linear <= 0.0031308 {
                linear * 12.92
            } else {
                1.055 * linear.powf(1.0 / 2.4) - 0.055
            }
        }
        TransferFunction::Gamma(exponent) if (10000..=100000).contains(&exponent) => linear.max(0.0).powf(10000.0 / f64::from(exponent)),
        TransferFunction::Linear => linear,
        TransferFunction::Pq => pq_encode(linear * f64::from(description.luminances().reference)),
        _ => return None,
    })
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    fn near(a: f64, b: f64) {
        assert!((a - b).abs() < 1e-5, "{a} != {b}");
    }
    #[test]
    fn pq_absolute_vectors_and_reference_white_are_preserved() {
        let pq = ColourDescription { primaries: ColourPrimaries::Bt2020, transfer: TransferFunction::Pq, ..Default::default() };
        near(pq_decode(0.5080784215), 100.0);
        near(pq_decode(0.7518270962), 1000.0);
        near(pq_decode(1.0), 10000.0);
        near(decode(pq_encode(203.0), pq).unwrap(), 1.0);
        near(encode(1.0, pq).unwrap(), pq_encode(203.0));
        near(decode(1.0, ColourDescription::default()).unwrap(), 1.0);
    }
    #[test]
    fn sdr_roundtrips_and_primary_conversions_preserve_white() {
        for transfer in [TransferFunction::Srgb, TransferFunction::Gamma(22000), TransferFunction::Linear] {
            let description = ColourDescription { transfer, ..Default::default() };
            for value in [0.0, 0.01, 0.18, 0.5, 1.0] {
                near(encode(decode(value, description).unwrap(), description).unwrap(), value);
            }
        }
        let forward = matrix(ColourPrimaries::Srgb, ColourPrimaries::Bt2020).unwrap();
        let reverse = matrix(ColourPrimaries::Bt2020, ColourPrimaries::Srgb).unwrap();
        for value in [[1.0, 1.0, 1.0], [0.2, 0.4, 0.8], [1.0, 0.0, 0.0]] {
            let [r, g, b] = apply(reverse, apply(forward, value));
            let [x, y, z] = value;
            near(r, x);
            near(g, y);
            near(b, z);
        }
        for value in apply(forward, [1.0, 1.0, 1.0]) {
            near(value, 1.0);
        }
        assert!(matrix(ColourPrimaries::Custom([(0, 0); 4]), ColourPrimaries::Srgb).is_none());
    }
    #[test]
    fn matching_output_checks_transfer_and_primary_luminance_but_not_mastering_metadata() {
        let description = ColourDescription::default();
        assert!(description.matches_output(description));
        assert!(!description.matches_output(ColourDescription { transfer: TransferFunction::Gamma(22000), ..description }));
        assert!(!description.matches_output(ColourDescription { primaries: ColourPrimaries::DisplayP3, ..description }));
    }
}
