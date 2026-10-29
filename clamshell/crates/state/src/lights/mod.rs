//! The lights under the sticks. A set of lights is a profile ([`crate::profiles`]): named and shared, or a
//! game's own, chosen per game, runner or for all by [`crate::settings::Setting::Lights`]; 0 is Off, all
//! lights off. It holds a colour per stick (hue, saturation, brightness), whether the two are edited as
//! one (linked), and whether the lights show the battery's charge while charging, from red (empty) to
//! green (full), instead of the colours.
//!
//! The lights in force are the settings in force's, or while a set is on its edit screen, that set's, so
//! that the sticks show each change as a slider moves (see [`crate::State::lights_shown`]). Sync sends
//! them as [`Lit`]s when they change ([`crate::Effect::Lights`]); the system crate writes them to the
//! LEDs the device profile names.

pub mod screens;

use crate::profiles::{Profile, Profiles};

/// A stick, left or right.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Stick {
    Left,
    Right,
}

impl Stick {
    pub const BOTH: [Stick; 2] = [Stick::Left, Stick::Right];

    pub fn label(self) -> &'static str {
        match self {
            Stick::Left => "left",
            Stick::Right => "right",
        }
    }
}

/// A stick's colour.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Colour {
    /// Hue, in degrees (0-359): 0 red, 120 green, 240 blue.
    pub hue: u16,
    /// Saturation, in percent: 0 white.
    pub saturation: u8,
    /// Brightness, in percent: 0 off.
    pub brightness: u8,
}

impl Colour {
    /// Off: no light.
    pub const OFF: Colour = Colour { hue: 0, saturation: 100, brightness: 0 };

    /// The colour at full brightness, 0-255 a channel.
    pub fn rgb(self) -> [u8; 3] {
        let s = f32::from(self.saturation.min(100)) / 100.0;
        let h = f32::from(self.hue % 360) / 60.0;
        let x = s * (1.0 - ((h % 2.0) - 1.0).abs());
        let (r, g, b) = match h as u8 {
            0 => (s, x, 0.0),
            1 => (x, s, 0.0),
            2 => (0.0, s, x),
            3 => (0.0, x, s),
            4 => (x, 0.0, s),
            _ => (s, 0.0, x),
        };
        // Value 1: the channels are lifted by what saturation leaves (1 - s).
        [r, g, b].map(|c| ((c + 1.0 - s) * 255.0).round().clamp(0.0, 255.0) as u8)
    }

    /// How it is written to a light.
    pub fn lit(self) -> Lit {
        if self.brightness == 0 { Lit::OFF } else { Lit { rgb: self.rgb(), brightness: self.brightness.min(100) } }
    }
}

/// What a stick's light shows: its colour at full brightness (0-255 a channel) and its brightness in
/// percent. Off is all zeros, so that two ways of being off compare equal.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Lit {
    pub rgb: [u8; 3],
    pub brightness: u8,
}

impl Lit {
    pub const OFF: Lit = Lit { rgb: [0; 3], brightness: 0 };
}

/// A charge level (0-100) as a colour of `brightness`: red empty, through yellow, to green full.
pub fn charge_colour(level: u8, brightness: u8) -> Colour {
    Colour { hue: u16::from(level.min(100)) * 120 / 100, saturation: 100, brightness }
}

/// What a set of lights holds.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Lights {
    /// The sliders set both sticks.
    pub linked: bool,
    pub left: Colour,
    pub right: Colour,
    /// While charging, the lights show the charge instead of their colours.
    pub battery: bool,
}

/// A new set, and Off: both lights off, linked.
impl Default for Lights {
    fn default() -> Lights {
        Lights { linked: true, left: Colour::OFF, right: Colour::OFF, battery: false }
    }
}

impl Lights {
    /// Both sticks `colour`, linked.
    pub fn both(colour: Colour) -> Lights {
        Lights { left: colour, right: colour, ..Lights::default() }
    }

    pub fn colour(&self, stick: Stick) -> Colour {
        match stick {
            Stick::Left => self.left,
            Stick::Right => self.right,
        }
    }

    pub fn colour_mut(&mut self, stick: Stick) -> &mut Colour {
        match stick {
            Stick::Left => &mut self.left,
            Stick::Right => &mut self.right,
        }
    }

    /// What the lights show, left then right: the colours, or with `charging` at a level (0-100) and
    /// [`Lights::battery`] on, the charge's colour at each stick's brightness.
    pub fn shown(&self, charging: Option<u8>) -> [Lit; 2] {
        Stick::BOTH.map(|stick| {
            let colour = self.colour(stick);
            match charging.filter(|_| self.battery) {
                Some(level) => charge_colour(level, colour.brightness).lit(),
                None => colour.lit(),
            }
        })
    }
}

/// A colour's part, as its slider on the edit screen sets it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum LightParam {
    Hue,
    Saturation,
    Brightness,
}

impl LightParam {
    pub const ALL: [LightParam; 3] = [LightParam::Hue, LightParam::Saturation, LightParam::Brightness];

    /// The range, and the step a slider moves it by. Slider positions are the value divided by the step,
    /// which keeps them within a slider's `u8`.
    pub fn range(self) -> (u16, u16, u16) {
        match self {
            LightParam::Hue => (0, 358, 2),
            LightParam::Saturation | LightParam::Brightness => (0, 100, 1),
        }
    }

    pub fn get(self, c: &Colour) -> u16 {
        match self {
            LightParam::Hue => c.hue,
            LightParam::Saturation => u16::from(c.saturation),
            LightParam::Brightness => u16::from(c.brightness),
        }
    }

    /// Sets the part, within its range.
    pub fn set(self, c: &mut Colour, value: u16) {
        let (lo, hi, _) = self.range();
        let value = value.clamp(lo, hi);
        let percent = u8::try_from(value).unwrap_or(100);
        match self {
            LightParam::Hue => c.hue = value,
            LightParam::Saturation => c.saturation = percent,
            LightParam::Brightness => c.brightness = percent,
        }
    }

    /// The value as a slider position.
    pub fn position(self, value: u16) -> u8 {
        let (_, _, step) = self.range();
        u8::try_from(value / step).unwrap_or(u8::MAX)
    }

    /// A slider position as the value.
    pub fn value(self, position: u8) -> u16 {
        let (_, _, step) = self.range();
        u16::from(position) * step
    }

    pub fn label(self) -> &'static str {
        match self {
            LightParam::Hue => "Hue",
            LightParam::Saturation => "Saturation",
            LightParam::Brightness => "Brightness",
        }
    }
}

/// Every set of lights, by id (see [`Profiles`]).
pub type LightSets = Profiles<Lights>;

impl LightSets {
    /// The sets there are before any were saved: two to start from.
    pub fn initial() -> LightSets {
        let colour = |hue, saturation, brightness| Colour { hue, saturation, brightness };
        LightSets::from_list(vec![Profile::with(1, "Ember", Lights::both(colour(24, 85, 40))), Profile::with(2, "Ice", Lights::both(colour(196, 60, 40)))])
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hues_go_round_from_red_and_saturation_goes_to_white() {
        let full = |hue| Colour { hue, saturation: 100, brightness: 100 }.rgb();
        assert_eq!(full(0), [255, 0, 0]);
        assert_eq!(full(60), [255, 255, 0]);
        assert_eq!(full(120), [0, 255, 0]);
        assert_eq!(full(240), [0, 0, 255]);
        assert_eq!(full(30), [255, 128, 0]);
        assert_eq!(Colour { hue: 200, saturation: 0, brightness: 50 }.rgb(), [255, 255, 255]);
        assert_eq!(Colour { hue: 0, saturation: 50, brightness: 50 }.rgb(), [255, 128, 128]);
    }

    #[test]
    fn a_light_at_no_brightness_is_off_whatever_its_colour() {
        assert_eq!(Colour { hue: 200, saturation: 30, brightness: 0 }.lit(), Lit::OFF);
        assert_eq!(Lights::default().shown(None), [Lit::OFF; 2]);
        assert_eq!(Colour { hue: 0, saturation: 100, brightness: 40 }.lit(), Lit { rgb: [255, 0, 0], brightness: 40 });
    }

    #[test]
    fn charging_shows_the_charge_from_red_to_green_at_each_sticks_brightness() {
        let lights = Lights { battery: true, left: Colour { hue: 240, saturation: 100, brightness: 30 }, ..Lights::both(Colour::OFF) };
        let [left, right] = lights.shown(Some(0));
        assert_eq!(left, Lit { rgb: [255, 0, 0], brightness: 30 }, "empty: red");
        assert_eq!(right, Lit::OFF, "a light off stays off");
        assert_eq!(lights.shown(Some(100))[0].rgb, [0, 255, 0], "full: green");
        assert_eq!(lights.shown(Some(50))[0].rgb, [255, 255, 0], "half: yellow");
        assert_eq!(lights.shown(None)[0].rgb, [0, 0, 255], "unplugged: the colour");
        let off = Lights { battery: false, ..lights };
        assert_eq!(off.shown(Some(50))[0].rgb, [0, 0, 255], "not asked for: the colour while charging");
    }

    #[test]
    fn slider_positions_fit_and_round_trip_on_the_steps() {
        for p in LightParam::ALL {
            let (lo, hi, step) = p.range();
            assert!(hi / step <= u16::from(u8::MAX), "{p:?}");
            assert_eq!(p.value(p.position(lo)), lo);
            assert_eq!(p.value(p.position(hi)), hi);
        }
        let mut c = Colour::OFF;
        LightParam::Brightness.set(&mut c, 500);
        assert_eq!(c.brightness, 100, "within range");
    }
}
