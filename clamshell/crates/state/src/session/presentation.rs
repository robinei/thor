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

impl super::windows::Window {
    pub fn presentation(&self, game: bool) -> WindowPresentation {
        let centred = self.parent.is_some() || self.role != WindowRole::Normal || self.limits.fixed_size().is_some();
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
        Some(window.presentation(game))
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
        assert_eq!(window.presentation(false), WindowPresentation { layout: WindowLayout::Fullscreen, scale: WindowScale::Native });
        assert_eq!(window.presentation(true), WindowPresentation { layout: WindowLayout::Fullscreen, scale: WindowScale::Fit });
        window.parent = Some(7);
        assert_eq!(window.presentation(true), WindowPresentation { layout: WindowLayout::Centred, scale: WindowScale::Native });
        window.parent = None;
        window.limits = WindowLimits { min: (640, 480), max: (640, 480) };
        assert_eq!(window.limits.fixed_size(), Some((640, 480)));
        assert_eq!(window.presentation(true).layout, WindowLayout::Centred);
        window.limits.max = (0, 0);
        assert_eq!(window.limits.fixed_size(), None);
        assert_eq!(window.presentation(true).layout, WindowLayout::Fullscreen);
        for role in [WindowRole::Dialog, WindowRole::Splash, WindowRole::Utility] {
            window.role = role;
            assert_eq!(window.presentation(false).layout, WindowLayout::Centred);
        }
    }
}
