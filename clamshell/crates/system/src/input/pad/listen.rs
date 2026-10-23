//! Listening to the pad, for a keymap's edit screen. Each input pressed or moved is sent to the state as
//! itself ([`Event::Listened`]) instead of moving the focus. That means a button as it goes down, a D-pad
//! direction, a stick's direction once pushed past half its travel, and a trigger past half. Home stays
//! with the router and leaves the screen. Back is an input like the others.

use std::time::Instant;

use clamshell_state::Event;
use clamshell_state::keymap::{Dir, Input, Side};
use evdev::AbsoluteAxisCode;

use super::remap::{HALF, button_of, stick_axes, trigger_axis};
use super::route::{Mapping, Out, Ranges};
use crate::os::evdev::{Press, Raw};

#[derive(Debug)]
pub struct Listener {
    ranges: Ranges,
    /// The directions and triggers now past half. Each counts once until let go.
    pushed: Vec<Input>,
}

impl Listener {
    pub fn new(ranges: Ranges) -> Listener {
        Listener { ranges, pushed: Vec::new() }
    }

    /// Notes whether `input` is pushed (`on`), and sends it when it starts being pushed.
    fn push(&mut self, input: Input, on: bool, out: &mut Out) {
        let was = self.pushed.contains(&input);
        if on && !was {
            self.pushed.push(input);
            out.events.push(Event::Listened(input));
        } else if !on && was {
            self.pushed.retain(|i| *i != input);
        }
    }
}

impl Mapping for Listener {
    fn event(&mut self, raw: Raw, _: Instant, out: &mut Out) {
        if let Some((code, press)) = raw.as_key() {
            if let (Some(b), Press::Down) = (button_of(code), press) {
                out.events.push(Event::Listened(Input::Button(b)));
            }
            return;
        }
        let Some((axis, v)) = raw.as_abs() else {
            return;
        };
        match axis {
            AbsoluteAxisCode::ABS_HAT0X => {
                self.push(Input::Dpad(Dir::Left), v < 0, out);
                self.push(Input::Dpad(Dir::Right), v > 0, out);
            }
            AbsoluteAxisCode::ABS_HAT0Y => {
                self.push(Input::Dpad(Dir::Up), v < 0, out);
                self.push(Input::Dpad(Dir::Down), v > 0, out);
            }
            _ => {
                for s in Side::ALL {
                    let (ax, ay) = stick_axes(s);
                    let c = self.ranges.stick.centered(v);
                    if axis == ax {
                        self.push(Input::StickDir(s, Dir::Left), c <= -HALF, out);
                        self.push(Input::StickDir(s, Dir::Right), c >= HALF, out);
                    } else if axis == ay {
                        self.push(Input::StickDir(s, Dir::Up), c <= -HALF, out);
                        self.push(Input::StickDir(s, Dir::Down), c >= HALF, out);
                    } else if axis == trigger_axis(s) {
                        self.push(Input::Trigger(s), self.ranges.trigger.unit(v) >= HALF, out);
                    }
                }
            }
        }
    }

    fn tick(&mut self, _: Instant, _: &mut Out) -> Option<Instant> {
        None
    }

    /// Nothing is held down outside ClamShell, so what was pushed is just forgotten.
    fn release(&mut self, _: &mut Out) {
        self.pushed.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::os::evdev::Range;
    use clamshell_state::keymap::Button;
    use evdev::KeyCode;

    #[test]
    fn presses_and_pushes_are_inputs() {
        let mut l = Listener::new(Ranges { stick: Range::STICK, trigger: Range { min: 0, max: 1023 } });
        let mut out = Out::default();
        let now = Instant::now();
        for raw in [
            Raw::key(KeyCode::BTN_SOUTH, Press::Down),
            Raw::key(KeyCode::BTN_SOUTH, Press::Repeat),
            Raw::key(KeyCode::BTN_SOUTH, Press::Up),
            Raw::abs(AbsoluteAxisCode::ABS_HAT0Y, 1),
            Raw::abs(AbsoluteAxisCode::ABS_RX, 10000),
            Raw::abs(AbsoluteAxisCode::ABS_RX, 20000),
            Raw::abs(AbsoluteAxisCode::ABS_RX, 25000),
            Raw::abs(AbsoluteAxisCode::ABS_RX, 0),
            Raw::abs(AbsoluteAxisCode::ABS_RX, 30000),
            Raw::abs(AbsoluteAxisCode::ABS_Z, 900),
            Raw::key(KeyCode::BTN_TRIGGER_HAPPY2, Press::Down),
        ] {
            l.event(raw, now, &mut out);
        }
        let inputs: Vec<Input> = out.events.iter().filter_map(|e| if let Event::Listened(i) = e { Some(*i) } else { None }).collect();
        let right = Input::StickDir(Side::Right, Dir::Right);
        assert_eq!(
            inputs,
            vec![Input::Button(Button::A), Input::Dpad(Dir::Down), right, right, Input::Trigger(Side::Left), Input::Button(Button::R4)],
            "a stick counts past half, once until let go"
        );
    }
}
