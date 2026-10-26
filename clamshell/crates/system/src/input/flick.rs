//! A directional flick from normalised axes. It has no device or shortcut policy.

use clamshell_state::keymap::Dir;

const ACTUATE: f32 = 0.7;
const REARM: f32 = 0.3;

#[derive(Default)]
pub(crate) struct Flick {
    armed: bool,
}

impl Flick {
    /// An off-centre stick must return before its first flick counts.
    pub(crate) fn begin(&mut self, [x, y]: [f32; 2]) {
        self.armed = x.abs().max(y.abs()) < REARM;
    }

    pub(crate) fn centered([x, y]: [f32; 2]) -> bool {
        x.abs().max(y.abs()) < REARM
    }

    pub(crate) fn update(&mut self, [x, y]: [f32; 2]) -> Option<Dir> {
        let distance = x.abs().max(y.abs());
        if distance < REARM {
            self.armed = true;
        }
        if !self.armed || distance <= ACTUATE {
            return None;
        }
        self.armed = false;
        Some(if x.abs() > y.abs() {
            if x < 0.0 { Dir::Left } else { Dir::Right }
        } else if y < 0.0 {
            Dir::Up
        } else {
            Dir::Down
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn one_direction_per_flick_until_both_axes_return() {
        let mut flick = Flick::default();
        flick.begin([0.0, 0.0]);
        assert_eq!(flick.update([0.7, 0.0]), None);
        assert_eq!(flick.update([0.71, 0.8]), Some(Dir::Down));
        assert_eq!(flick.update([-1.0, 0.0]), None);
        assert_eq!(flick.update([0.3, 0.0]), None);
        assert_eq!(flick.update([-1.0, 0.0]), None);
        assert_eq!(flick.update([0.29, -0.29]), None);
        assert_eq!(flick.update([-0.9, 0.7]), Some(Dir::Left));
    }
    #[test]
    fn an_initially_deflected_stick_must_return_first() {
        let mut flick = Flick::default();
        flick.begin([0.0, -0.9]);
        assert_eq!(flick.update([0.0, -1.0]), None);
        assert_eq!(flick.update([0.0, 0.0]), None);
        assert_eq!(flick.update([0.0, -0.9]), Some(Dir::Up));
        flick.begin([0.9, 0.0]);
        assert_eq!(flick.update([1.0, 0.0]), None);
        assert_eq!(flick.update([0.0, 0.0]), None);
        assert_eq!(flick.update([0.9, 0.0]), Some(Dir::Right));
    }
}
