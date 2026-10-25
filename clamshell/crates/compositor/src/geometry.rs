//! One transform for drawing and hit testing. Black outside a window is not an input target.

use clamshell_state::session::{
    presentation::{WindowLayout, WindowPresentation, WindowScale},
    windows::Window,
};
use smithay::utils::{Logical, Point, Rectangle, Size};

#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct Geometry {
    pub viewport: Rectangle<i32, Logical>,
    pub native: Rectangle<i32, Logical>,
    pub origin: Point<f64, Logical>,
    pub scale: f64,
}

impl Geometry {
    pub fn new(viewport: Rectangle<i32, Logical>, native: Rectangle<i32, Logical>, policy: WindowPresentation) -> Option<Self> {
        if native.size.w <= 0 || native.size.h <= 0 {
            return None;
        }
        let fit = (viewport.size.w as f64 / native.size.w as f64).min(viewport.size.h as f64 / native.size.h as f64);
        let scale = if policy.scale == WindowScale::Fit { fit } else { fit.min(1.0) };
        let x = viewport.loc.x as f64 + (viewport.size.w as f64 - native.size.w as f64 * scale) / 2.0 - native.loc.x as f64 * scale;
        let y = viewport.loc.y as f64 + (viewport.size.h as f64 - native.size.h as f64 * scale) / 2.0 - native.loc.y as f64 * scale;
        Some(Self { viewport, native, origin: (x.round(), y.round()).into(), scale })
    }

    pub fn surface_coords(&self, position: Point<f64, Logical>) -> Point<f64, Logical> {
        ((position.x - self.origin.x) / self.scale, (position.y - self.origin.y) / self.scale).into()
    }
}

/// The size a window is configured to in `area`: a fixed size as it is, a fullscreen window `fill` (the
/// area, or the resolution its game is set to, which [`Geometry`] scales up to the area), and any other its
/// own size within the area.
pub(crate) fn configure_size(
    window: &Window,
    policy: WindowPresentation,
    area: Size<i32, Logical>,
    fill: Size<i32, Logical>,
    current: Size<i32, Logical>,
) -> Option<Size<i32, Logical>> {
    if let Some((w, h)) = window.limits.fixed_size() {
        return Some((i32::try_from(w).unwrap_or(i32::MAX), i32::try_from(h).unwrap_or(i32::MAX)).into());
    }
    if policy.layout == WindowLayout::Fullscreen {
        return Some(fill);
    }
    (current.w > 0 && current.h > 0).then(|| (current.w.min(area.w), current.h.min(area.h)).into())
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    use clamshell_state::session::presentation::WindowLimits;
    #[test]
    fn fitting_preserves_aspect_and_uses_the_same_inverse_for_input() {
        let viewport = Rectangle::from_size((960, 540).into());
        let native = Rectangle::new((10, 20).into(), (640, 480).into());
        let geometry = Geometry::new(viewport, native, Window::default().presentation(true)).unwrap();
        assert_eq!(geometry.scale, 1.125);
        let screen = geometry.origin + Point::from((330.0 * geometry.scale, 260.0 * geometry.scale));
        assert_eq!(geometry.surface_coords(screen), (330.0, 260.0).into());
        assert!(!geometry.native.to_f64().contains(geometry.surface_coords((0.0, 270.0).into())));
    }
    #[test]
    fn native_size_is_sharp_and_oversized_fixed_windows_scale_down() {
        let policy = Window { parent: Some(1), ..Window::default() }.presentation(false);
        let geometry = Geometry::new(Rectangle::from_size((960, 540).into()), Rectangle::from_size((400, 300).into()), policy).unwrap();
        assert_eq!(geometry.scale, 1.0);
        assert_eq!(geometry.origin, (280.0, 120.0).into());
        let small = Geometry::new(Rectangle::new((960, 40).into(), (620, 500).into()), Rectangle::from_size((1240, 1000).into()), policy).unwrap();
        assert_eq!(small.scale, 0.5);
        assert_eq!(small.surface_coords((1270.0, 290.0).into()), (620.0, 500.0).into());
        let fixed = Window { limits: WindowLimits { min: (1240, 1000), max: (1240, 1000) }, ..Window::default() };
        assert_eq!(configure_size(&fixed, fixed.presentation(false), (620, 500).into(), (620, 500).into(), (0, 0).into()), Some((1240, 1000).into()));
    }

    #[test]
    fn a_fullscreen_game_at_a_lower_resolution_is_configured_to_it_and_scaled_up() {
        let game = Window::default();
        let policy = game.presentation(true);
        let size = configure_size(&game, policy, (1920, 1080).into(), (1280, 720).into(), (0, 0).into()).unwrap();
        assert_eq!(size, (1280, 720).into());
        let geometry = Geometry::new(Rectangle::from_size((1920, 1080).into()), Rectangle::from_size(size), policy).unwrap();
        assert_eq!((geometry.scale, geometry.origin), (1.5, (0.0, 0.0).into()), "filling the screen");
        // A dialog keeps its own size.
        let dialog = Window { parent: Some(1), ..Window::default() };
        assert_eq!(configure_size(&dialog, dialog.presentation(true), (1920, 1080).into(), (1280, 720).into(), (400, 300).into()), Some((400, 300).into()));
    }
}
