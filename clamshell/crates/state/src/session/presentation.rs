//! Presentation policy is shared by client types. Geometry and buffer transforms belong to the host.

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct WindowLimits {
    pub min: (u32, u32),
    pub max: (u32, u32),
}
impl WindowLimits {
    pub fn fixed_size(self) -> Option<(u32, u32)> {
        (self.min.0 > 0 && self.min.1 > 0 && self.min == self.max).then_some(self.min)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum WindowRole {
    #[default]
    Normal,
    Dialog,
    Splash,
    Utility,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WindowLayout {
    Fullscreen,
    Centered,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WindowScale {
    Fit,
    Native,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WindowPresentation {
    pub layout: WindowLayout,
    pub scale: WindowScale,
}

/// Client colour metadata. None on a window means unspecified, not an assumption about buffer depth.
/// Rendering and scanout will consume this description when colour management is implemented.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ColourDescription {
    pub primaries: ColourPrimaries,
    pub transfer: TransferFunction,
    /// Declared minimum and maximum luminance, in millinits.
    pub luminance: Option<(u32, u32)>,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ColourPrimaries {
    Srgb,
    DisplayP3,
    Bt2020,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TransferFunction {
    Srgb,
    Linear,
    Pq,
    Hlg,
}

impl super::windows::Window {
    pub fn presentation(&self, game: bool) -> WindowPresentation {
        let centered = self.parent.is_some() || self.role != WindowRole::Normal || self.limits.fixed_size().is_some();
        WindowPresentation {
            layout: if centered { WindowLayout::Centered } else { WindowLayout::Fullscreen },
            scale: if game && !centered { WindowScale::Fit } else { WindowScale::Native },
        }
    }
}

impl crate::State {
    pub fn window_presentation(&self, id: i64) -> Option<WindowPresentation> {
        let window = self.session().window(id)?;
        let game = self.session().running_of(window).and_then(|r| self.session().entry(self.shelf().library(), &r.id)).is_some_and(crate::Entry::is_game);
        Some(window.presentation(game))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::session::windows::Window;
    #[test]
    fn dialogs_and_fixed_windows_are_centered_and_games_fit_ignored_sizes() {
        let mut window = Window::default();
        assert_eq!(window.presentation(false), WindowPresentation { layout: WindowLayout::Fullscreen, scale: WindowScale::Native });
        assert_eq!(window.presentation(true), WindowPresentation { layout: WindowLayout::Fullscreen, scale: WindowScale::Fit });
        window.parent = Some(7);
        assert_eq!(window.presentation(true), WindowPresentation { layout: WindowLayout::Centered, scale: WindowScale::Native });
        window.parent = None;
        window.limits = WindowLimits { min: (640, 480), max: (640, 480) };
        assert_eq!(window.limits.fixed_size(), Some((640, 480)));
        assert_eq!(window.presentation(true).layout, WindowLayout::Centered);
        window.limits.max = (0, 0);
        assert_eq!(window.limits.fixed_size(), None);
        assert_eq!(window.presentation(true).layout, WindowLayout::Fullscreen);
        for role in [WindowRole::Dialog, WindowRole::Splash, WindowRole::Utility] {
            window.role = role;
            assert_eq!(window.presentation(false).layout, WindowLayout::Centered);
        }
    }
}
