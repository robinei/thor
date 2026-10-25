//! Presentation policy is shared by client types. Geometry and buffer transforms belong to the host.

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, serde::Serialize, serde::Deserialize)]
pub struct WindowLimits {
    pub min: (u32, u32),
    pub max: (u32, u32),
}
impl WindowLimits {
    pub fn fixed_size(self) -> Option<(u32, u32)> {
        (self.min.0 > 0 && self.min.1 > 0 && self.min == self.max).then_some(self.min)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, serde::Serialize, serde::Deserialize)]
pub enum WindowRole {
    #[default]
    Normal,
    Dialog,
    Splash,
    Utility,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum WindowLayout {
    Fullscreen,
    Centred,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum WindowScale {
    Fit,
    Native,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct WindowPresentation {
    pub layout: WindowLayout,
    pub scale: WindowScale,
}

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
    pub fn declared_peak(self) -> Option<f64> {
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

/// The screens as the compositor has them (see [`crate::Reading::Screens`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Screens {
    /// Each screen's colour as clients are told it (the top's, the second's): what a curve is shown
    /// within.
    pub colour: [ColourDescription; 2],
    /// The top screen's size in pixels, which a game's resolution is a fraction of.
    pub top_size: (u32, u32),
}

/// What a curve's plot shows (see [`crate::State::curve_plot`]), in units of the reference white.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct CurvePlot {
    /// For HDR content, its signal and peak; None for SDR.
    pub hdr: Option<HdrPlot>,
    /// What the screen shows above the reference white.
    pub headroom: f64,
    /// The screen's black.
    pub black: f64,
}

/// How a window's picture is shown: its clarity settings (see [`crate::State::window_clarity`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Clarity {
    pub resolution: crate::settings::Resolution,
    pub scaling: crate::settings::Scaling,
    pub anti_aliasing: crate::settings::AntiAliasing,
    /// The sharpening's strength in percent, 0 for none.
    pub sharpen: u16,
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
    /// Whether it is scRGB (linear light) rather than PQ.
    pub scrgb: bool,
}

impl super::windows::Window {
    /// How the window is shown: dialogs, popups and windows of a fixed size centred at their own size, other
    /// windows filling the screen, a game's scaled to it. `screen` is the sizes its screen may tell it it
    /// has (the screen's own, and the resolution its game is set to): a window fixed at one of them fills
    /// the screen too, as a game Wine shows as a window of the screen's mode.
    pub fn presentation(&self, game: bool, screen: &[(u32, u32)]) -> WindowPresentation {
        let fixed = self.limits.fixed_size().is_some_and(|size| !screen.contains(&size));
        let centred = self.parent.is_some() || self.role != WindowRole::Normal || fixed;
        WindowPresentation {
            layout: if centred { WindowLayout::Centred } else { WindowLayout::Fullscreen },
            scale: if game && !centred { WindowScale::Fit } else { WindowScale::Native },
        }
    }
}

impl crate::State {
    pub fn window_presentation(&self, id: i64) -> Option<WindowPresentation> {
        let window = self.session().window(id)?;
        let game = self.session().running_of(window).and_then(|r| self.session().entry(self.shelf().library(), &r.id)).is_some_and(crate::Entry::is_game);
        // What the top screen may tell its windows (see the compositor's top mode); the second's isn't known
        // here, and games are shown on the top one.
        let screen: Vec<(u32, u32)> =
            self.top_size().filter(|_| !window.down).map(|top| vec![top, self.window_clarity(id).resolution.size_on(top)]).unwrap_or_default();
        Some(window.presentation(game, &screen))
    }

    /// The HDR curve for window `id`'s SDR content: the curve chosen for the program running it, in its
    /// settings in force (see [`crate::settings::Setting::Curve`]). None for Off, for a curve that is gone,
    /// and for a window no launch runs.
    pub fn window_curve(&self, id: i64) -> Option<crate::curve::Curve> {
        let window = self.session().window(id)?;
        let running = self.session().running_of(window)?;
        let chosen = self.settings_of(&running.id).get(crate::settings::Setting::Curve);
        self.curving().in_force(chosen).copied()
    }

    /// The HUD for window `id`: the one in the settings in force for the program running it. Off for a
    /// window no launch runs.
    pub fn window_hud(&self, id: i64) -> crate::settings::Hud {
        let Some(running) = self.session().window(id).and_then(|window| self.session().running_of(window)) else {
            return crate::settings::Hud::Off;
        };
        self.settings_of(&running.id).choice()
    }

    /// What a curve's plot shows for the program whose settings are edited: whether its window on screen
    /// shows HDR content and how bright it declares, and its screen's headroom. Without a window, SDR
    /// content on the top screen.
    pub fn curve_plot(&self) -> CurvePlot {
        let panel = self.panel();
        let program = panel.editing().map(|program| program.id.clone());
        let window = program.as_deref().and_then(|id| {
            let mut windows = self.session().windows_of(id);
            windows.find(|w| !w.down).or_else(|| self.session().windows_of(id).next())
        });
        let colour = window.and_then(|w| w.colour.as_deref().copied()).filter(|colour| colour.hdr());
        let screen = self.screen_colour(window.is_some_and(|w| w.down));
        let luminance = screen.luminances();
        let black = f64::from(luminance.min) / 10000.0 / f64::from(luminance.reference.max(1));
        let hdr = colour.map(|colour| HdrPlot {
            peak: colour.peak(),
            nits: colour.peak() * f64::from(colour.luminances().reference),
            reference: f64::from(colour.luminances().reference),
            declared: colour.declared_peak().is_some(),
            scrgb: colour.transfer == TransferFunction::Linear,
        });
        CurvePlot { hdr, headroom: screen.headroom(), black }
    }

    /// How window `id`'s picture is shown (`docs/picture.md`): the clarity settings in force for the program
    /// running it; the defaults (native, bilinear, nothing more) for a window no launch runs.
    pub fn window_clarity(&self, id: i64) -> Clarity {
        use crate::settings::Setting;
        let Some(running) = self.session().window(id).and_then(|window| self.session().running_of(window)) else {
            return Clarity::default();
        };
        let v = self.settings_of(&running.id);
        Clarity { resolution: v.choice(), scaling: v.choice(), anti_aliasing: v.choice(), sharpen: v.get(Setting::Sharpen) }
    }

    /// The frame rate cap for window `id`: the one in the settings in force for the program running it.
    /// None for no cap, and for a window no launch runs.
    pub fn window_fps_cap(&self, id: i64) -> Option<u16> {
        let window = self.session().window(id)?;
        let running = self.session().running_of(window)?;
        Some(self.settings_of(&running.id).get(crate::settings::Setting::FpsCap)).filter(|cap| *cap > 0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::session::windows::Window;

    #[test]
    fn a_windows_fps_cap_is_the_one_set_for_its_program() {
        use crate::testkit::*;
        use crate::{Id, SettingsId, Tab, settings::Setting};
        let mut s = state();
        launch(&mut s, "zelda-lttp");
        show_window(&mut s, "zelda-lttp", 7);
        assert_eq!(s.window_fps_cap(7), None, "no cap");
        tap(&mut s, Id::Tab(Tab::Settings));
        tap(&mut s, Id::Settings(SettingsId::Tile(Setting::FpsCap)));
        assert_eq!(s.window_fps_cap(7), Some(30));
        assert_eq!(s.window_fps_cap(99), None, "no such window");
    }
    #[test]
    fn dialogs_and_fixed_windows_are_centred_and_games_fit_ignored_sizes() {
        let mut window = Window::default();
        assert_eq!(window.presentation(false, &[]), WindowPresentation { layout: WindowLayout::Fullscreen, scale: WindowScale::Native });
        assert_eq!(window.presentation(true, &[]), WindowPresentation { layout: WindowLayout::Fullscreen, scale: WindowScale::Fit });
        window.parent = Some(7);
        assert_eq!(window.presentation(true, &[]), WindowPresentation { layout: WindowLayout::Centred, scale: WindowScale::Native });
        window.parent = None;
        window.limits = WindowLimits { min: (640, 480), max: (640, 480) };
        assert_eq!(window.limits.fixed_size(), Some((640, 480)));
        assert_eq!(window.presentation(true, &[]).layout, WindowLayout::Centred);
        window.limits.max = (0, 0);
        assert_eq!(window.limits.fixed_size(), None);
        assert_eq!(window.presentation(true, &[]).layout, WindowLayout::Fullscreen);
        for role in [WindowRole::Dialog, WindowRole::Splash, WindowRole::Utility] {
            window.role = role;
            assert_eq!(window.presentation(false, &[]).layout, WindowLayout::Centred);
        }
    }
    #[test]
    fn a_game_window_fixed_at_its_screens_mode_fills_the_screen() {
        // As Wine shows a game: a window fixed at the mode.
        let window = Window { limits: WindowLimits { min: (1200, 675), max: (1200, 675) }, ..Window::default() };
        let screen = [(1200, 675), (1000, 562)];
        assert_eq!(window.presentation(true, &screen), WindowPresentation { layout: WindowLayout::Fullscreen, scale: WindowScale::Fit });
        let lower = Window { limits: WindowLimits { min: (1000, 562), max: (1000, 562) }, ..Window::default() };
        assert_eq!(lower.presentation(true, &screen).scale, WindowScale::Fit, "at the game's resolution, scaled up");
        let small = Window { limits: WindowLimits { min: (800, 600), max: (800, 600) }, ..Window::default() };
        assert_eq!(small.presentation(true, &screen).layout, WindowLayout::Centred, "another fixed size stays centred");
    }
}
