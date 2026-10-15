//! Volume and brightness in the bar: a finger held on either icon (or sliding off it sideways) brings
//! up a slider whose current value is under the finger; moving sets the level as it goes, lifting leaves
//! it there. A tap opens the icon's tab instead.
//!
//! The slider runs along the screen's width. Its length is what fits on both sides of where the finger
//! came down, given the level there (the part below the level goes left of the finger, the rest right),
//! so the value can go from 0 to 100 without the track leaving the screen (the icons sit near the middle
//! of the bar for this, not exactly in it).

use crate::{Effect, State};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Control {
    Volume,
    Brightness,
}

/// A slide in progress, in the second screen's layout units.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Slider {
    pub control: Control,
    /// The level when the finger came down, and now (0-100).
    pub start: u8,
    pub value: u8,
    /// Where the finger came down, and the track's length (the whole 0-100 range).
    pub x: f32,
    pub length: f32,
    /// The level changed during this touch (so lifting isn't a tap).
    pub moved: bool,
}

/// Where a slider starts when the level isn't known.
const UNKNOWN_START: u8 = 50;

/// Space kept free at each end of the track, of the screen's width.
const EDGE: f32 = 0.03;
/// The track's length at most and at least, of the screen's width (so a percent is about as far
/// wherever the finger came down).
const LONGEST: f32 = 0.5;
const SHORTEST: f32 = 0.1;

impl Slider {
    /// The longest track (up to half the screen) that stays on a screen `width` wide with level `start`
    /// at `x`: the part for 0 to `start` left of `x`, the rest right of it.
    pub fn length_at(x: f32, start: u8, width: f32) -> f32 {
        let below = f32::from(start.min(100)) / 100.0;
        let (left, right) = (x - width * EDGE, width * (1.0 - EDGE) - x);
        // A part of zero fits any length.
        let fits = |room: f32, part: f32| if part > 0.0 { room / part } else { f32::INFINITY };
        fits(left, below).min(fits(right, 1.0 - below)).clamp(width * SHORTEST, width * LONGEST)
    }

    /// Where the track starts: so that the starting level sits under the finger.
    pub fn track_start(&self) -> f32 {
        self.x - f32::from(self.start) / 100.0 * self.length
    }

    /// The level with the finger at `x`.
    fn value_at(&self, x: f32) -> u8 {
        let v = f32::from(self.start) + (x - self.x) / self.length.max(1.0) * 100.0;
        v.round().clamp(0.0, 100.0) as u8
    }
}

impl State {
    /// The level a control shows now: the slider's while sliding, else the system's.
    pub fn level(&self, control: Control) -> Option<u8> {
        match self.slider {
            Some(s) if s.control == control => Some(s.value),
            _ => match control {
                Control::Volume => self.status.volume,
                Control::Brightness => self.status.brightness,
            },
        }
    }

    pub(crate) fn slide_start(&mut self, control: Control, x: f32, width: f32) {
        // Without a reading (a desktop without a backlight), from the middle: the slider still shows.
        let start = self.level(control).unwrap_or(UNKNOWN_START);
        let length = Slider::length_at(x, start, width);
        self.slider = Some(Slider { control, start, value: start, x, length, moved: false });
    }

    pub(crate) fn slide_to(&mut self, x: f32) -> Vec<Effect> {
        let Some(slider) = self.slider.as_mut() else {
            return vec![];
        };
        let value = slider.value_at(x);
        if value == slider.value {
            return vec![];
        }
        slider.value = value;
        slider.moved = true;
        match slider.control {
            Control::Brightness => vec![Effect::SetBrightness(value)],
            // Turning it up or down unmutes.
            Control::Volume if self.status.muted => {
                self.status.muted = false;
                vec![Effect::SetMute(false), Effect::SetVolume(value)]
            }
            Control::Volume => vec![Effect::SetVolume(value)],
        }
    }

    /// The finger lifted: the level stays (the bar shows it until the system's next reading).
    pub(crate) fn slide_end(&mut self) {
        let Some(slider) = self.slider.take() else {
            return;
        };
        match slider.control {
            Control::Volume => self.status.volume = Some(slider.value),
            Control::Brightness => self.status.brightness = Some(slider.value),
        }
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    use crate::tests::state;
    use crate::{Event, Id, Status};

    fn with_levels() -> State {
        let mut s = state();
        s.apply(Event::Status(Status { volume: Some(40), brightness: Some(80), muted: true, ..Status::default() }));
        s
    }

    #[test]
    fn the_level_starts_under_the_finger_and_follows_it() {
        let mut s = with_levels();
        // A finger at 300 on a 620-wide screen, at 80 %: 281.4 fits left of it (for 80 % of the track),
        // more right of it; the track is the most allowed, half the screen.
        s.apply(Event::SlideStart { control: Control::Brightness, x: 300.0, width: 620.0 });
        let slider = s.slider.unwrap();
        assert!((slider.length - 310.0).abs() < 0.01);
        assert!((slider.track_start() + 0.8 * slider.length - 300.0).abs() < 0.01, "80% under the finger");
        assert!(slider.track_start() >= 0.0 && slider.track_start() + slider.length <= 620.0, "on screen");
        assert_eq!(s.apply(Event::SlideTo { x: 300.0 + 0.1 * slider.length }).1, vec![Effect::SetBrightness(90)]);
        assert_eq!(s.apply(Event::SlideTo { x: 2000.0 }).1, vec![Effect::SetBrightness(100)], "clamped");
        assert_eq!(s.apply(Event::SlideTo { x: 2100.0 }).1, vec![], "no change, nothing sent");
        s.apply(Event::Activate(Id::Control(Control::Brightness)));
        s.apply(Event::Up(Id::Control(Control::Brightness)));
        assert_eq!((s.slider, s.status.brightness), (None, Some(100)));
    }

    #[test]
    fn sliding_the_volume_unmutes_and_a_tap_opens_its_tab() {
        let mut s = with_levels();
        s.apply(Event::SlideStart { control: Control::Volume, x: 280.0, width: 620.0 });
        assert_eq!(s.apply(Event::SlideTo { x: 250.0 }).1, vec![Effect::SetMute(false), Effect::SetVolume(30)]);
        s.apply(Event::Up(Id::Control(Control::Volume)));
        assert_eq!(s.status.volume, Some(30));
        s.apply(Event::Activate(Id::Control(Control::Brightness)));
        assert_eq!(s.mode, crate::Mode::Brightness);
    }

    #[test]
    fn the_track_fits_wherever_the_finger_comes_down() {
        let width = 620.0;
        for x in [250.0, 284.0, 310.0, 335.0, 370.0] {
            for start in [0, 1, 30, 50, 80, 99, 100] {
                let length = Slider::length_at(x, start, width);
                let slider = Slider { control: Control::Volume, start, value: start, x, length, moved: false };
                let (from, to) = (slider.track_start(), slider.track_start() + length);
                assert!(from >= width * EDGE - 0.01 && to <= width * (1.0 - EDGE) + 0.01, "x {x}, level {start}: {from}..{to}");
                assert!(length >= width * 0.3, "x {x}, level {start}: {length} is short");
            }
        }
        assert_eq!(Slider::length_at(310.0, 50, width), width * LONGEST, "the middle: at most half the screen");
    }

    #[test]
    fn without_a_level_the_slider_starts_in_the_middle() {
        let mut s = state();
        s.apply(Event::SlideStart { control: Control::Brightness, x: 300.0, width: 620.0 });
        assert_eq!(s.slider.map(|s| s.start), Some(50));
    }
}
