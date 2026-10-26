//! Where a window is shown and how: its viewport on its screen, its origin and scale by its presentation
//! (native, fitted, integer, stretched; for a game, its picture brought to the screen), and the size it is
//! configured to. Drawing and input use the same [`Geometry`], so a touch lands where the picture is drawn,
//! and the black around a window is no window's.

use clamshell_state::session::{
    presentation::{WindowLayout, WindowPresentation, WindowScale},
    windows::WindowFacts,
};
use smithay::utils::{Logical, Point, Rectangle, Scale, Size};

#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct Geometry {
    pub viewport: Rectangle<i32, Logical>,
    pub native: Rectangle<i32, Logical>,
    pub origin: Point<f64, Logical>,
    /// The same on both axes, but for a picture whose shape isn't its window's (see [`Geometry::new`]).
    pub scale: Scale<f64>,
}

/// How a game's picture is shown (see [`Geometry::new`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(crate) struct Picture {
    /// The size of its buffer, the part shown (see `Surface::picture`).
    pub size: Option<Size<i32, Logical>>,
    /// Where it is shown in its window, in the window's surface's coordinates: all of it, or for Wine's at a
    /// mode of another shape, part, between Wine's own bars.
    pub place: Option<Rectangle<i32, Logical>>,
}

impl Geometry {
    /// The window of `native` geometry in `viewport`, as `policy` says. For a window scaled to the screen,
    /// it is its `picture`'s place that is brought to it (the window around it, Wine's bars, follows): a
    /// buffer whose shape is more than 2 % off its place's (a mode a viewport stretches over the screen) is
    /// fitted in its own shape, between bars; at Integer the picture is shown at the largest whole multiple
    /// of its buffer that fits (fitted as usual if even once doesn't); at Stretch it fills the viewport.
    pub fn new(viewport: Rectangle<i32, Logical>, native: Rectangle<i32, Logical>, policy: WindowPresentation, picture: Picture) -> Option<Self> {
        if native.size.w <= 0 || native.size.h <= 0 {
            return None;
        }
        // Only a window scaled to the screen: a video player's viewport may stretch on purpose (anamorphic
        // video).
        let scaled = policy.layout == WindowLayout::Fullscreen && policy.scale != WindowScale::Native;
        let buffer = picture.size.filter(|b| b.w > 0 && b.h > 0 && scaled);
        let frame = picture.place.filter(|p| p.size.w > 0 && p.size.h > 0 && buffer.is_some()).unwrap_or(native);
        let ratio = |s: Size<i32, Logical>| s.w as f64 / s.h as f64;
        let shape = buffer.filter(|b| (ratio(*b) / ratio(frame.size) - 1.0).abs() > 0.02).unwrap_or(frame.size);
        let whole = buffer.filter(|_| policy.scale == WindowScale::Integer).and_then(|b| {
            let times = (viewport.size.w / b.w).min(viewport.size.h / b.h);
            (times >= 1).then(|| Size::<i32, Logical>::from((b.w * times, b.h * times)))
        });
        let shown = match whole {
            Some(s) => (s.w as f64, s.h as f64),
            None if policy.scale == WindowScale::Stretch => (viewport.size.w as f64, viewport.size.h as f64),
            None => {
                let fit = (viewport.size.w as f64 / shape.w as f64).min(viewport.size.h as f64 / shape.h as f64);
                let fit = if policy.scale == WindowScale::Native { fit.min(1.0) } else { fit };
                (shape.w as f64 * fit, shape.h as f64 * fit)
            }
        };
        let scale = Scale::from((shown.0 / frame.size.w as f64, shown.1 / frame.size.h as f64));
        // The picture's corner on a whole pixel, so that a picture shown at a whole multiple keeps its pixels'
        // edges: the window's too, and the picture's offset in it rounded as it is drawn.
        let x = viewport.loc.x as f64 + ((viewport.size.w as f64 - shown.0) / 2.0).floor() - (frame.loc.x as f64 * scale.x).round();
        let y = viewport.loc.y as f64 + ((viewport.size.h as f64 - shown.1) / 2.0).floor() - (frame.loc.y as f64 * scale.y).round();
        Some(Self { viewport, native, origin: (x, y).into(), scale })
    }

    pub fn surface_coords(&self, position: Point<f64, Logical>) -> Point<f64, Logical> {
        ((position.x - self.origin.x) / self.scale.x, (position.y - self.origin.y) / self.scale.y).into()
    }

    /// Whether the window is shown at its own size.
    pub fn one_to_one(&self) -> bool {
        self.scale.x == 1.0 && self.scale.y == 1.0
    }
}

/// The size a window is configured to in `area`: a fixed size as it is, a fullscreen window `fill` (the
/// area, or the resolution its game is set to, which [`Geometry`] scales up to the area), and any other its
/// own size within the area.
pub(crate) fn configure_size(
    window: &WindowFacts,
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
    use clamshell_state::{session::presentation::WindowLimits, settings::Size};
    #[test]
    fn fitting_preserves_aspect_and_uses_the_same_inverse_for_input() {
        let viewport = Rectangle::from_size((960, 540).into());
        let native = Rectangle::new((10, 20).into(), (640, 480).into());
        let geometry = Geometry::new(viewport, native, WindowFacts::default().presentation(true, &[], Size::Default), Picture::default()).unwrap();
        assert_eq!(geometry.scale, Scale::from(1.125));
        let screen = geometry.origin + Point::from((330.0 * geometry.scale.x, 260.0 * geometry.scale.y));
        assert_eq!(geometry.surface_coords(screen), (330.0, 260.0).into());
        assert!(!geometry.native.to_f64().contains(geometry.surface_coords((0.0, 270.0).into())));
    }
    #[test]
    fn native_size_is_sharp_and_oversized_fixed_windows_scale_down() {
        let policy = WindowFacts { parent: Some(1), ..WindowFacts::default() }.presentation(false, &[], Size::Default);
        let geometry = Geometry::new(Rectangle::from_size((960, 540).into()), Rectangle::from_size((400, 300).into()), policy, Picture::default()).unwrap();
        assert_eq!(geometry.scale, Scale::from(1.0));
        assert_eq!(geometry.origin, (280.0, 120.0).into());
        let small =
            Geometry::new(Rectangle::new((960, 40).into(), (620, 500).into()), Rectangle::from_size((1240, 1000).into()), policy, Picture::default()).unwrap();
        assert_eq!(small.scale, Scale::from(0.5));
        assert_eq!(small.surface_coords((1270.0, 290.0).into()), (620.0, 500.0).into());
        let fixed = WindowFacts { limits: WindowLimits { min: (1240, 1000), max: (1240, 1000) }, ..WindowFacts::default() };
        assert_eq!(
            configure_size(&fixed, fixed.presentation(false, &[], Size::Default), (620, 500).into(), (620, 500).into(), (0, 0).into()),
            Some((1240, 1000).into())
        );
    }

    #[test]
    fn a_fullscreen_game_at_a_lower_resolution_is_configured_to_it_and_scaled_up() {
        let game = WindowFacts::default();
        let policy = game.presentation(true, &[], Size::Default);
        let size = configure_size(&game, policy, (1920, 1080).into(), (1280, 720).into(), (0, 0).into()).unwrap();
        assert_eq!(size, (1280, 720).into());
        let geometry = Geometry::new(Rectangle::from_size((1920, 1080).into()), Rectangle::from_size(size), policy, Picture::default()).unwrap();
        assert_eq!((geometry.scale, geometry.origin), (Scale::from(1.5), (0.0, 0.0).into()), "filling the screen");
        // A dialog keeps its own size.
        let dialog = WindowFacts { parent: Some(1), ..WindowFacts::default() };
        assert_eq!(
            configure_size(&dialog, dialog.presentation(true, &[], Size::Default), (1920, 1080).into(), (1280, 720).into(), (400, 300).into()),
            Some((400, 300).into())
        );
    }

    #[test]
    fn a_mode_stretched_over_the_screen_keeps_its_shape() {
        // Xwayland shows a 640x480 mode as a window of the screen's size; it is fitted as 4:3, centred.
        let policy = WindowFacts::default().presentation(true, &[], Size::Default);
        let screen = Rectangle::from_size((960, 540).into());
        let stretched = Picture { size: Some((640, 480).into()), place: None };
        let geometry = Geometry::new(screen, screen, policy, stretched).unwrap();
        assert_eq!(geometry.scale, Scale::from((0.75, 1.0)));
        assert_eq!(geometry.origin, (120.0, 0.0).into());
        assert_eq!(geometry.surface_coords((480.0, 270.0).into()), (480.0, 270.0).into(), "the middle is the middle");
        assert_eq!(geometry.surface_coords((120.0, 0.0).into()), (0.0, 0.0).into(), "the picture's corner");
        let dialog = WindowFacts { parent: Some(1), ..WindowFacts::default() }.presentation(true, &[], Size::Default);
        assert_eq!(Geometry::new(screen, screen, dialog, stretched).unwrap().scale, Scale::from(1.0), "only fullscreen");
        let app = WindowFacts::default().presentation(false, &[], Size::Default);
        assert_eq!(Geometry::new(screen, screen, app, stretched).unwrap().scale, Scale::from(1.0), "only a game's");
    }

    #[test]
    fn integer_scaling_shows_the_largest_whole_multiple_of_the_picture() {
        let game = WindowFacts::default().presentation(true, &[], Size::Integer);
        let thor = Rectangle::from_size((1920, 1080).into());
        let whole = |size: (i32, i32)| Picture { size: Some(size.into()), place: None };
        // A 640x480 mode stretched over the screen: twice, 1280x960, centred.
        let g = Geometry::new(thor, thor, game, whole((640, 480))).unwrap();
        assert_eq!((g.scale, g.origin), (Scale::from((1280.0 / 1920.0, 960.0 / 1080.0)), (320.0, 60.0).into()));
        // Wine's: a 600x336 picture in a 1200x675 window, on a 1200x675 screen: twice, 1200x672.
        let nested = Rectangle::from_size((1200, 675).into());
        let g = Geometry::new(nested, nested, game, whole((600, 336))).unwrap();
        assert_eq!(g.scale, Scale::from((1.0, 672.0 / 675.0)));
        assert_eq!(g.origin, (0.0, 1.0).into(), "whole pixels: the 3 rows left over, 1 above and 2 below");
        // A third of the Thor's screen: exactly three times.
        let window = Rectangle::from_size((640, 360).into());
        assert_eq!(Geometry::new(thor, window, game, whole((640, 360))).unwrap().scale, Scale::from(3.0));
        // Larger than the screen: fitted as usual.
        assert_eq!(Geometry::new(thor, thor, game, whole((2560, 1440))).unwrap().scale, Scale::from(1.0));
        assert_eq!(Geometry::new(thor, Rectangle::from_size((2560, 1440).into()), game, whole((2560, 1440))).unwrap().scale, Scale::from(0.75));
    }

    #[test]
    fn a_picture_in_part_of_its_window_is_what_is_fitted() {
        // Wine's 640x480 mode: shown at 720x540 between its own bars in a 960x540 window.
        let game = WindowFacts::default().presentation(true, &[], Size::Default);
        let screen = Rectangle::from_size((960, 540).into());
        let wine = Picture { size: Some((640, 480).into()), place: Some(Rectangle::new((120, 0).into(), (720, 540).into())) };
        let g = Geometry::new(screen, screen, game, wine).unwrap();
        assert_eq!((g.scale, g.origin), (Scale::from(1.0), (0.0, 0.0).into()), "already fitted: as it is");
        // Integer: once, 640x480, centred; the window around it follows.
        let integer = WindowFacts::default().presentation(true, &[], Size::Integer);
        let g = Geometry::new(screen, screen, integer, wine).unwrap();
        assert_eq!(g.scale, Scale::from((640.0 / 720.0, 480.0 / 540.0)));
        let corner = g.origin + Point::from(((120.0 * g.scale.x).round(), 0.0));
        assert_eq!(corner, (160.0, 30.0).into(), "the picture's corner, as drawn");
        // On a larger screen, the picture fills its height.
        let thor = Rectangle::from_size((1920, 1080).into());
        let g = Geometry::new(thor, screen, game, wine).unwrap();
        assert_eq!((g.scale, g.surface_coords((240.0, 0.0).into())), (Scale::from(2.0), (120.0, 0.0).into()));
        // Stretch: the picture fills the screen, its shape aside.
        let stretch = WindowFacts::default().presentation(true, &[], Size::Stretch);
        let g = Geometry::new(screen, screen, stretch, wine).unwrap();
        assert_eq!(g.scale, Scale::from((960.0 / 720.0, 1.0)));
        assert_eq!(g.origin + Point::from(((120.0 * g.scale.x).round(), 0.0)), (0.0, 0.0).into(), "the picture's corner, as drawn");
    }
}
