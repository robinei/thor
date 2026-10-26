//! A curve's plot on its edit screen: the curve drawn over what the edited program's window shows, its
//! screen's headroom and black, and where the content's pixels are.

use crate::State;

/// What a curve's plot shows (see [`crate::State::curve_plot`]), in units of the reference white.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct CurvePlot {
    /// For HDR content, its signal and peak; None for SDR.
    pub hdr: Option<HdrPlot>,
    /// What the screen shows above the reference white.
    pub headroom: f64,
    /// The screen's black.
    pub black: f64,
    /// Where the content's pixels are, as the compositor last measured them, if it did for its window.
    pub histogram: Option<crate::colour::Histogram>,
}

/// An HDR game's content in a curve's plot.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct HdrPlot {
    /// The brightest it declares, or the nominal maximum its signal allows, over its reference white.
    pub peak: f64,
    /// The same in the game's own nits (scRGB's white is 80, which is shown at the reference white).
    pub nits: f64,
    /// Its white in its own nits.
    pub reference: f64,
    /// Whether it declares its peak (MaxCLL or its mastering display's).
    pub declared: bool,
    /// Whether its peak is the one set for it (see [`crate::settings::Setting::HdrPeak`]).
    pub set: bool,
    /// Whether it is scRGB (linear light) rather than PQ.
    pub scrgb: bool,
}

impl State {
    /// The window whose picture the curve's edit screen plots: the edited program's, on the main screen if
    /// it has one there.
    fn plot_window(&self) -> Option<&crate::session::windows::Window> {
        let program = self.panel().editing().map(|program| program.id.clone());
        let id = program.as_deref().and_then(|id| {
            let mut windows = self.session().windows_of(id);
            windows.find(|w| !self.session().below(w.id)).or_else(|| self.session().windows_of(id).next()).map(|w| w.id)
        })?;
        self.session().window(id)
    }

    /// The window whose picture the compositor measures for the plot's histogram: the plotted one, while a
    /// curve's edit screen is open.
    pub fn histogram_window(&self) -> Option<i64> {
        matches!(self.curving().screen(), Some(crate::curve::screens::CurveScreen::Edit(_))).then(|| self.plot_window().map(|w| w.id)).flatten()
    }

    /// What a curve's plot shows for the program whose settings are edited: whether its window on screen
    /// shows HDR content and how bright it declares, and its screen's headroom. Without a window, SDR
    /// content on the top screen.
    pub fn curve_plot(&self) -> CurvePlot {
        let window = self.plot_window();
        let colour = window.and_then(|w| w.colour.as_deref().copied()).filter(|colour| colour.hdr());
        let screen = self.screen_colour(window.is_some_and(|w| self.session().below(w.id)));
        let luminance = screen.luminances();
        let black = f64::from(luminance.min) / 10000.0 / f64::from(luminance.reference.max(1));
        // The content's peak: the one set for the program edited (at the level edited), else what it
        // declares or is assumed.
        let set = self.panel().tile_value(crate::settings::Setting::HdrPeak);
        let hdr = colour.map(|colour| {
            let reference = f64::from(colour.luminances().reference);
            let nits = if set > 0 { f64::from(set) } else { colour.peak() * reference };
            HdrPlot {
                peak: nits / reference.max(1.0),
                nits,
                reference,
                declared: colour.declared_peak().is_some(),
                set: set > 0,
                scrgb: colour.transfer == crate::colour::TransferFunction::Linear,
            }
        });
        let histogram = self.histogram.filter(|h| window.is_some_and(|w| w.id == h.window));
        CurvePlot { hdr, headroom: screen.headroom(), black, histogram }
    }
}
