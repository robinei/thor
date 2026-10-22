//! Volume and brightness in the bar. A finger held on either icon, or sliding off it sideways, brings up
//! a slider with the current value under the finger. Moving sets the level as it goes, and lifting
//! leaves it there. A tap opens the icon's tab instead. The same slider sets a keymap binding's
//! parameters from the edit screen's panel ([`Control::Param`], see [`crate::keymap::screens`]).
//!
//! The slider runs along the screen's width. Its length is what fits on both sides of where the finger
//! came down, given the level there: the part below the level goes left of the finger and the rest goes
//! right. This lets the value go from 0 to 100 without the track leaving the screen. For the same reason
//! the icons sit near the middle of the bar, but not exactly in it.

use crate::levels::{Level, Levels};
use crate::out::Out;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Control {
    Volume,
    Brightness,
    /// A parameter of the input selected on the keymap edit screen, in the layer shown.
    Param(crate::keymap::Param),
}

impl Control {
    /// The level it sets, if it sets one.
    pub fn level(self) -> Option<Level> {
        match self {
            Control::Volume => Some(Level::Volume),
            Control::Brightness => Some(Level::Brightness),
            Control::Param(_) => None,
        }
    }
}

/// A slide in progress, in the second screen's layout units.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Slider {
    pub control: Control,
    /// The value when the finger came down, and the value now, in `lo..=hi`. That is 0-100 for a level,
    /// or a parameter's range.
    pub start: u8,
    pub value: u8,
    pub lo: u8,
    pub hi: u8,
    /// Where the finger came down, and the length of the track, which covers the whole range.
    pub x: f32,
    pub length: f32,
    /// The level changed during this touch, so lifting isn't a tap.
    pub moved: bool,
}

/// A finger held this long on volume or brightness brings up the slider. A shorter touch is a tap, which
/// opens the tab. Moving sideways [`SLIDE_MOVE`] layout units brings the slider up at once. This applies
/// to touch and the mouse.
pub const SLIDE_AFTER: std::time::Duration = std::time::Duration::from_millis(200);
pub const SLIDE_MOVE: f32 = 8.0;

/// Where a slider starts when the level isn't known.
const UNKNOWN_START: u8 = 50;

/// Space kept free at each end of the track, as a fraction of the screen's width.
const EDGE: f32 = 0.03;
/// The longest and shortest track, as a fraction of the screen's width. This keeps a percent about the
/// same distance wherever the finger came down.
const LONGEST: f32 = 0.5;
const SHORTEST: f32 = 0.1;

impl Slider {
    /// A slider for `control` at value `start` (0-100), with the finger at `x` on a screen `width` wide.
    pub fn new(control: Control, start: u8, x: f32, width: f32) -> Slider {
        Slider::ranged(control, start, 0, 100, x, width)
    }

    /// A slider over `lo..=hi`, at `start`.
    pub fn ranged(control: Control, start: u8, lo: u8, hi: u8, x: f32, width: f32) -> Slider {
        let start = start.clamp(lo, hi.max(lo));
        let mut s = Slider { control, start, value: start, lo, hi: hi.max(lo), x, length: 0.0, moved: false };
        s.length = Slider::length_at(x, s.fraction(start), width);
        s
    }

    /// Where a value is along the range, 0 to 1.
    pub fn fraction(&self, v: u8) -> f32 {
        let span = f32::from(self.hi.saturating_sub(self.lo)).max(1.0);
        (f32::from(v.clamp(self.lo, self.hi)) - f32::from(self.lo)) / span
    }

    /// The value now, as a percent of the range. This is where the track's thumb is.
    pub fn percent(&self) -> u8 {
        (self.fraction(self.value) * 100.0).round() as u8
    }

    /// The longest track, up to half the screen, that stays on a screen `width` wide when the fraction
    /// `below` (0 to 1) of it is left of `x` and the rest is right of it.
    pub fn length_at(x: f32, below: f32, width: f32) -> f32 {
        let below = below.clamp(0.0, 1.0);
        let (left, right) = (x - width * EDGE, width * (1.0 - EDGE) - x);
        // A part of zero fits any length.
        let fits = |room: f32, part: f32| if part > 0.0 { room / part } else { f32::INFINITY };
        fits(left, below).min(fits(right, 1.0 - below)).clamp(width * SHORTEST, width * LONGEST)
    }

    /// Where the track starts, so that the starting level sits under the finger.
    pub fn track_start(&self) -> f32 {
        self.x - self.fraction(self.start) * self.length
    }

    /// The value with the finger at `x`.
    pub(crate) fn value_at(&self, x: f32) -> u8 {
        let span = f32::from(self.hi - self.lo);
        let v = f32::from(self.start) + (x - self.x) / self.length.max(1.0) * span;
        v.round().clamp(f32::from(self.lo), f32::from(self.hi)) as u8
    }
}

impl Levels {
    pub(crate) fn slide_start(&mut self, control: Control, x: f32, width: f32) {
        // Without a reading, as on a desktop without a backlight, start from the middle so the slider
        // still shows.
        let start = self.control(control).unwrap_or(UNKNOWN_START);
        self.slider = Some(Slider::new(control, start, x, width));
    }

    pub(crate) fn slide_to(&mut self, x: f32, out: &mut Out) {
        let Some(slider) = self.slider.as_mut() else {
            return;
        };
        let Some(level) = slider.control.level() else {
            return;
        };
        let value = slider.value_at(x);
        if value == slider.value {
            return;
        }
        slider.value = value;
        slider.moved = true;
        // The same level as the tab's row. Turning the volume up or down unmutes.
        self.set(level, value, out);
    }

    /// The finger lifted. The level stays where it was set, and readings count again.
    pub(crate) fn slide_end(&mut self) {
        if self.slider.take().is_some() {
            self.release();
        }
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    use crate::testkit::*;
    use crate::*;

    fn with_levels() -> State {
        let mut s = state();
        read(&mut s, Reading::Status(Status { volume: Some(40), brightness: Some(80), muted: true, ..Status::default() }));
        s
    }

    fn slide(s: &mut State, slide: Slide) -> Vec<Effect> {
        s.applied(slide.into()).1
    }

    #[test]
    fn a_slide_goes_on_while_another_finger_opens_a_tab() {
        let mut s = with_levels();
        s.applied(Slide::Start { control: Control::Brightness, x: 300.0, width: 620.0 }.into());
        s.applied(PointerEvent::Activate(Id::Tab(crate::Tab::Wifi)).into());
        let fx = s.applied(Slide::To { x: 200.0 }.into()).1;
        assert!(matches!(fx.as_slice(), [Effect::Brightness { .. }]), "still sliding: {fx:?}");
    }

    #[test]
    fn the_level_starts_under_the_finger_and_follows_it() {
        let mut s = with_levels();
        // A finger at 300 on a 620-wide screen, at 80 %. 281.4 fits left of it for 80 % of the track, and
        // more fits right of it, so the track is the longest allowed, half the screen.
        slide(&mut s, Slide::Start { control: Control::Brightness, x: 300.0, width: 620.0 });
        let slider = *s.levels().slider().unwrap();
        assert!((slider.length - 310.0).abs() < 0.01);
        assert!((slider.track_start() + 0.8 * slider.length - 300.0).abs() < 0.01, "80% under the finger");
        assert!(slider.track_start() >= 0.0 && slider.track_start() + slider.length <= 620.0, "on screen");
        assert_eq!(slide(&mut s, Slide::To { x: 300.0 + 0.1 * slider.length }), vec![Effect::Brightness { level: 90, shares: vec![] }]);
        assert_eq!(slide(&mut s, Slide::To { x: 2000.0 }), vec![Effect::Brightness { level: 100, shares: vec![] }], "clamped");
        assert_eq!(slide(&mut s, Slide::To { x: 2100.0 }), vec![], "no change, nothing sent");
        slide(&mut s, Slide::End);
        assert_eq!((s.levels().slider(), s.levels().control(Control::Brightness)), (None, Some(100)));
    }

    #[test]
    fn sliding_the_volume_unmutes_and_a_tap_opens_its_tab() {
        let mut s = with_levels();
        slide(&mut s, Slide::Start { control: Control::Volume, x: 280.0, width: 620.0 });
        assert_eq!(slide(&mut s, Slide::To { x: 250.0 }), vec![Effect::Sound(Sound::Mute(false)), Effect::Sound(Sound::Volume(30))]);
        slide(&mut s, Slide::End);
        assert_eq!(s.levels().volume(), Some(30));
        tap(&mut s, Id::Control(Control::Brightness));
        assert_eq!(s.bottom().tab(), Tab::Brightness);
    }

    #[test]
    fn holding_the_icon_without_moving_makes_up_no_level() {
        let mut s = state();
        slide(&mut s, Slide::Start { control: Control::Volume, x: 300.0, width: 620.0 });
        slide(&mut s, Slide::End);
        assert_eq!(s.levels().volume(), None);
    }

    #[test]
    fn a_range_other_than_0_to_100_spans_the_whole_track() {
        // Turbo: Off (0) to 30 a second, from Off under the finger.
        let s = Slider::ranged(Control::Volume, 0, 0, 30, 100.0, 620.0);
        let start = s.track_start();
        assert_eq!(start, 100.0, "Off under the finger, the track to its right");
        assert_eq!((s.value_at(start + s.length), s.value_at(start + s.length / 2.0), s.value_at(start - 50.0)), (30, 15, 0));
        assert_eq!(Slider { value: 15, ..s }.percent(), 50, "the thumb halfway");
        // A range that starts above 0, such as a hold's time, 20 to 100.
        let s = Slider::ranged(Control::Volume, 40, 20, 100, 300.0, 620.0);
        assert_eq!((s.value_at(s.track_start()), s.value_at(s.track_start() + s.length)), (20, 100));
    }

    #[test]
    fn the_track_fits_wherever_the_finger_comes_down() {
        let width = 620.0;
        for x in [250.0, 284.0, 310.0, 335.0, 370.0] {
            for start in [0, 1, 30, 50, 80, 99, 100] {
                let length = Slider::length_at(x, f32::from(start) / 100.0, width);
                let slider = Slider { control: Control::Volume, start, value: start, lo: 0, hi: 100, x, length, moved: false };
                let (from, to) = (slider.track_start(), slider.track_start() + length);
                assert!(from >= width * EDGE - 0.01 && to <= width * (1.0 - EDGE) + 0.01, "x {x}, level {start}: {from}..{to}");
                assert!(length >= width * 0.3, "x {x}, level {start}: {length} is short");
            }
        }
        assert_eq!(Slider::length_at(310.0, 0.5, width), width * LONGEST, "the middle: at most half the screen");
    }

    #[test]
    fn without_a_level_the_slider_starts_in_the_middle() {
        let mut s = state();
        slide(&mut s, Slide::Start { control: Control::Brightness, x: 300.0, width: 620.0 });
        assert_eq!(s.levels().slider().map(|s| s.start), Some(50));
    }
}
