//! The colour model: how a window's content or a screen is described (in the units of Wayland's colour
//! management), and the histogram the compositor measures of a window's picture.

/// Client colour metadata. None on a window means unspecified, not an assumption about buffer depth.
/// Primary and target volumes retain the protocol's units so reported precision is not discarded.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct ColourDescription {
    pub primaries: ColourPrimaries,
    pub transfer: TransferFunction,
    pub luminance: Option<ColourLuminance>,
    pub target: Option<ColourTarget>,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum ColourPrimaries {
    Srgb,
    DisplayP3,
    Bt2020,
    /// CIE xy coordinates in millionths: red, green, blue and white.
    Custom([(i32, i32); 4]),
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum TransferFunction {
    Srgb,
    Linear,
    Pq,
    Hlg,
    /// Power-law exponent multiplied by 10000.
    Gamma(u32),
}

/// Wayland colour-management luminance units: minimum in 1/10000 nit, others in nits.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct ColourLuminance {
    pub min: u32,
    pub max: u32,
    pub reference: u32,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, serde::Serialize, serde::Deserialize)]
pub struct ColourTarget {
    pub primaries: Option<ColourPrimaries>,
    pub luminance: Option<(u32, u32)>,
    pub max_cll: Option<u32>,
    pub max_fall: Option<u32>,
}
impl Default for ColourDescription {
    fn default() -> Self {
        Self { primaries: ColourPrimaries::Srgb, transfer: TransferFunction::Srgb, luminance: None, target: None }
    }
}
impl ColourDescription {
    /// Whether it describes HDR content: PQ, or linear light (scRGB's).
    pub fn hdr(self) -> bool {
        matches!(self.transfer, TransferFunction::Pq | TransferFunction::Linear)
    }

    /// The brightest it declares, over its reference white: its MaxCLL, else its mastering display's
    /// peak, else its nominal maximum (10000 nits for linear light, which has none).
    pub fn peak(self) -> f64 {
        let luminance = self.luminances();
        let nominal = if self.transfer == TransferFunction::Linear { 10000 } else { luminance.max };
        self.declared_peak().unwrap_or(f64::from(nominal) / f64::from(luminance.reference.max(1)))
    }

    /// The brightest it declares itself, over its reference white: its MaxCLL, else its mastering
    /// display's peak; none if it gives neither.
    pub(crate) fn declared_peak(self) -> Option<f64> {
        let target = self.target.unwrap_or_default();
        let peak = target.max_cll.or(target.luminance.map(|(_, maximum)| maximum))?;
        Some(f64::from(peak) / f64::from(self.luminances().reference.max(1)))
    }

    /// For a screen's description: what it shows above its reference white, at least 1.
    pub fn headroom(self) -> f64 {
        self.peak().max(1.0)
    }

    pub fn luminances(self) -> ColourLuminance {
        self.luminance.unwrap_or(match self.transfer {
            TransferFunction::Pq => ColourLuminance { min: 50, max: 10000, reference: 203 },
            TransferFunction::Linear => ColourLuminance { min: 0, max: 80, reference: 80 },
            _ => ColourLuminance { min: 2000, max: 80, reference: 80 },
        })
    }
    /// Matching volumes are eligible for a colour-preserving scanout path; buffer constraints still apply.
    pub fn matches_output(self, output: Self) -> bool {
        self.primaries.coordinates() == output.primaries.coordinates() && self.transfer == output.transfer && self.luminances() == output.luminances()
    }
}

impl ColourPrimaries {
    pub fn coordinates(self) -> [(i32, i32); 4] {
        let (red, green, blue) = match self {
            Self::Srgb => ((640000, 330000), (300000, 600000), (150000, 60000)),
            Self::DisplayP3 => ((680000, 320000), (265000, 690000), (150000, 60000)),
            Self::Bt2020 => ((708000, 292000), (170000, 797000), (131000, 46000)),
            Self::Custom(value) => return value,
        };
        [red, green, blue, (312700, 329000)]
    }
}
impl From<clamshell_device::colour::PanelColour> for ColourDescription {
    fn from(panel: clamshell_device::colour::PanelColour) -> Self {
        let xy = |value: clamshell_device::colour::Xy| (value.x as i32, value.y as i32);
        let primaries = ColourPrimaries::Custom([xy(panel.primaries.red), xy(panel.primaries.green), xy(panel.primaries.blue), xy(panel.primaries.white)]);
        // SDR white at the panel's, or at its peak without headroom.
        let luminance = panel.luminance.map(|value| ColourLuminance {
            min: value.black.saturating_mul(10),
            max: value.peak / 1000,
            reference: panel.sdr_white.unwrap_or(value.peak).min(value.peak) / 1000,
        });
        let target = panel.luminance.map(|value| ColourTarget {
            primaries: Some(primaries),
            luminance: Some((value.black.saturating_mul(10), value.peak / 1000)),
            max_cll: Some(value.peak / 1000),
            max_fall: Some(value.frame_average / 1000),
        });
        Self { primaries, transfer: TransferFunction::Gamma(22000), luminance, target }
    }
}

/// How many bins a [`Histogram`] has.
pub const HISTOGRAM_BINS: usize = 64;

/// Where the pixels of a window's picture are, as the compositor measures them while its curve's edit
/// screen is open (see [`crate::State::histogram_window`]): how many pixels' luminance falls in each of
/// [`HISTOGRAM_BINS`] equal steps of the content's signal, SDR's (sRGB's encoding) or for HDR PQ's, over
/// its whole range with the content's white at the reference white.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Histogram {
    pub window: i64,
    pub hdr: bool,
    pub bins: [u16; HISTOGRAM_BINS],
}
