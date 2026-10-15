//! Where the controller's events go: the route the state chose ([`PadRoute`]) and the mapping that
//! carries it out. Pure: controller events in, events for the virtual devices and the state out, so
//! switching routes is tested here (whatever a mapping holds down is let go when the pad leaves it).

use clamshell_core::hardware::DeviceKey;
use std::time::Instant;

use clamshell_core::{Event, PadInput, PadRoute};
use evdev::{AbsoluteAxisCode, KeyCode};

use super::mouse::PadMouse;
use super::navigator::Navigator;
use super::typing::Typing;
use crate::input::{Press, Range, Raw};

/// The virtual mouse's keys: the pad mouse's buttons and the keys the pad types with. What the device
/// is built with, and what leaving the pad mouse or typing lets go of.
pub const MOUSE_DEVICE_KEYS: [KeyCode; 9] = [
    KeyCode::BTN_LEFT,
    KeyCode::BTN_RIGHT,
    KeyCode::BTN_MIDDLE,
    KeyCode::KEY_UP,
    KeyCode::KEY_DOWN,
    KeyCode::KEY_LEFT,
    KeyCode::KEY_RIGHT,
    KeyCode::KEY_ENTER,
    KeyCode::KEY_BACKSPACE,
];

/// Every key of the virtual mouse up (the kernel passes on only the ones that were down).
pub fn release_mouse_keys(out: &mut Out) {
    out.mouse.extend(MOUSE_DEVICE_KEYS.iter().map(|&k| Raw::key(k, Press::Up)));
    out.mouse.push(Raw::REPORT);
}

/// What the pad's events become.
#[derive(Debug, Default, PartialEq)]
pub struct Out {
    /// For the game's virtual pad (shaped on the way, see [`super::Shaping`]).
    pub pad: Vec<Raw>,
    /// For the virtual mouse (pointer, buttons, wheel, and the keys the pad types).
    pub mouse: Vec<Raw>,
    /// For the state.
    pub events: Vec<Event>,
}

/// A way of turning the pad's events into something else, while its route is in force.
pub trait Mapping {
    /// One controller event at `now`.
    fn event(&mut self, raw: Raw, now: Instant, out: &mut Out);

    /// What time brings (pointer motion, a held direction's repeats); when to be ticked next, if it
    /// needs to be.
    fn tick(&mut self, now: Instant, out: &mut Out) -> Option<Instant>;

    /// The pad leaves this mapping: lets go of whatever it holds down, and starts over.
    fn release(&mut self, out: &mut Out);
}

/// The sticks' and the right trigger's ranges.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Ranges {
    pub stick: Range,
    pub trigger: Range,
}

/// The controller's state as last seen: to blank the virtual pad while the pad is away from the game,
/// and to restore it when it comes back.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct PadSnapshot {
    keys: Vec<KeyCode>,
    axes: Vec<Axis>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Axis {
    pub code: AbsoluteAxisCode,
    pub value: i32,
    /// Its value at rest: the middle of a stick (0), the bottom of a trigger.
    pub rest: i32,
}

impl PadSnapshot {
    /// The axes as they are when the controller is taken over; keys are assumed up.
    pub fn new(axes: Vec<Axis>) -> PadSnapshot {
        PadSnapshot { keys: Vec::new(), axes }
    }

    pub fn update(&mut self, raw: Raw) {
        if let Some((key, press)) = raw.as_key() {
            self.keys.retain(|&k| k != key);
            if press.held() {
                self.keys.push(key);
            }
        } else if let Some((code, value)) = raw.as_abs()
            && let Some(axis) = self.axes.iter_mut().find(|a| a.code == code)
        {
            axis.value = value;
        }
    }

    /// Events that make a pad look like this state (`real`) or like an untouched one.
    pub fn events(&self, real: bool, out: &mut Vec<Raw>) {
        let press = if real { Press::Down } else { Press::Up };
        out.extend(self.keys.iter().map(|&k| Raw::key(k, press)));
        out.extend(self.axes.iter().map(|a| Raw::abs(a.code, if real { a.value } else { a.rest })));
        out.push(Raw::REPORT);
    }
}

/// Away from the game, the virtual pad shows a neutral controller.
fn away(route: PadRoute) -> bool {
    route != PadRoute::Game
}

/// Routes the pad's events. The home button is always ClamShell's: on its own it is [`Event::Home`]
/// (on release, so it can start a combination), with RB a screenshot. With the pad mouse or typing, View
/// is ClamShell's too (to move the pad to the second screen).
pub struct PadRouter {
    route: PadRoute,
    /// Home is held; whether a combination used it (then letting go isn't Home).
    home: Option<bool>,
    /// RB went into a combination: its release stays ClamShell's too.
    rb_taken: bool,
    mouse: PadMouse,
    navigator: Navigator,
    typing: Typing,
    snapshot: PadSnapshot,
}

impl PadRouter {
    /// A controller just taken over (its ranges, its axes now), the pad going to `route`: away from the
    /// game, the virtual pad is blanked.
    pub fn new(ranges: Ranges, snapshot: PadSnapshot, route: PadRoute, out: &mut Out) -> PadRouter {
        if away(route) {
            snapshot.events(false, &mut out.pad);
        }
        PadRouter { route, home: None, rb_taken: false, mouse: PadMouse::new(ranges), navigator: Navigator::new(ranges.stick), typing: Typing, snapshot }
    }

    fn mapping(&mut self, route: PadRoute) -> Option<&mut dyn Mapping> {
        match route {
            PadRoute::Game => None,
            PadRoute::Ui => Some(&mut self.navigator),
            PadRoute::Keys => Some(&mut self.typing),
            PadRoute::Mouse => Some(&mut self.mouse),
        }
    }

    /// The pad goes to `to`: the mapping it leaves lets go of what it holds, and the virtual pad is
    /// blanked when leaving the game (or shows the controller again when coming back).
    pub fn set_route(&mut self, to: PadRoute, out: &mut Out) {
        let from = self.route;
        if from == to {
            return;
        }
        if let Some(mapping) = self.mapping(from) {
            mapping.release(out);
        }
        if away(from) != away(to) {
            self.snapshot.events(!away(to), &mut out.pad);
        }
        self.route = to;
        log::info!("pad: {to:?}");
    }

    /// The virtual pad was made again: it shows the controller as it is (in the game) or at rest.
    pub fn show(&self, out: &mut Out) {
        self.snapshot.events(!away(self.route), &mut out.pad);
    }

    /// One controller event at `now`.
    pub fn event(&mut self, raw: Raw, now: Instant, out: &mut Out) {
        let key = raw.as_key();
        match key {
            Some((KeyCode::BTN_MODE, Press::Down)) => {
                self.home = Some(false);
                return;
            }
            Some((KeyCode::BTN_MODE, Press::Up)) => {
                if self.home.take() == Some(false) {
                    out.events.push(Event::Home);
                }
                return;
            }
            Some((KeyCode::BTN_MODE, Press::Repeat)) => return,
            // Home with RB: a screenshot (the game never sees this RB).
            Some((KeyCode::BTN_TR, Press::Down)) if self.home.is_some() => {
                self.home = Some(true);
                self.rb_taken = true;
                out.events.push(Event::Device(DeviceKey::Screenshot));
                return;
            }
            Some((KeyCode::BTN_TR, press)) if self.rb_taken => {
                if press == Press::Up {
                    self.rb_taken = false;
                }
                return;
            }
            _ => {}
        }
        self.snapshot.update(raw);
        if let Some((KeyCode::BTN_SELECT, press)) = key
            && matches!(self.route, PadRoute::Mouse | PadRoute::Keys)
        {
            if press == Press::Down {
                out.events.push(Event::Pad(PadInput::Switch));
            }
            return;
        }
        match self.mapping(self.route) {
            Some(mapping) => mapping.event(raw, now, out),
            None => out.pad.push(raw),
        }
    }

    /// Ticks the mapping in force; when to tick it next, if it needs to be.
    pub fn tick(&mut self, now: Instant, out: &mut Out) -> Option<Instant> {
        self.mapping(self.route).and_then(|m| m.tick(now, out))
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    fn router(route: PadRoute) -> (PadRouter, Out) {
        let ranges = Ranges { stick: Range::STICK, trigger: Range { min: 0, max: 1023 } };
        let axes = vec![Axis { code: AbsoluteAxisCode::ABS_X, value: 1000, rest: 0 }, Axis { code: AbsoluteAxisCode::ABS_RZ, value: 0, rest: 0 }];
        let mut out = Out::default();
        let router = PadRouter::new(ranges, PadSnapshot::new(axes), route, &mut out);
        (router, out)
    }

    fn key(code: KeyCode, press: Press) -> Raw {
        Raw::key(code, press)
    }

    /// The keys down after these events, as the kernel would keep them, by code.
    fn held(events: &[Raw]) -> Vec<KeyCode> {
        let mut down = Vec::new();
        for (k, press) in events.iter().filter_map(|e| e.as_key()) {
            down.retain(|&d| d != k);
            if press.held() {
                down.push(k);
            }
        }
        down.sort_by_key(|k| k.0);
        down
    }

    #[test]
    fn leaving_the_pad_mouse_lets_go_of_its_buttons() {
        let (mut r, mut out) = router(PadRoute::Mouse);
        let now = Instant::now();
        r.event(key(KeyCode::BTN_SOUTH, Press::Down), now, &mut out);
        r.event(Raw::abs(AbsoluteAxisCode::ABS_HAT0Y, -1), now, &mut out);
        r.event(Raw::abs(AbsoluteAxisCode::ABS_RZ, 1000), now, &mut out);
        assert_eq!(held(&out.mouse), vec![KeyCode::KEY_UP, KeyCode::BTN_LEFT]);
        r.set_route(PadRoute::Ui, &mut out);
        assert_eq!(held(&out.mouse), vec![], "no stuck keys");
        // Back on the pad mouse: the trigger is still held, but counts only once pressed again.
        r.set_route(PadRoute::Mouse, &mut out);
        r.event(Raw::abs(AbsoluteAxisCode::ABS_RZ, 1000), now, &mut out);
        assert_eq!(held(&out.mouse), vec![KeyCode::BTN_LEFT]);
    }

    #[test]
    fn leaving_typing_lets_go_of_its_keys() {
        let (mut r, mut out) = router(PadRoute::Keys);
        let now = Instant::now();
        r.event(key(KeyCode::BTN_EAST, Press::Down), now, &mut out);
        r.event(key(KeyCode::BTN_EAST, Press::Repeat), now, &mut out);
        r.event(Raw::abs(AbsoluteAxisCode::ABS_HAT0X, 1), now, &mut out);
        assert_eq!(held(&out.mouse), vec![KeyCode::KEY_BACKSPACE, KeyCode::KEY_RIGHT]);
        r.set_route(PadRoute::Game, &mut out);
        assert_eq!(held(&out.mouse), vec![]);
    }

    #[test]
    fn leaving_navigation_stops_a_held_direction() {
        let (mut r, mut out) = router(PadRoute::Ui);
        let t0 = Instant::now();
        r.event(Raw::abs(AbsoluteAxisCode::ABS_HAT0Y, 1), t0, &mut out);
        assert_eq!(out.events, vec![Event::Pad(PadInput::Down)]);
        assert!(r.tick(t0, &mut out).is_some(), "repeats while held");
        r.set_route(PadRoute::Mouse, &mut out);
        r.set_route(PadRoute::Ui, &mut out);
        out.events.clear();
        assert_eq!(r.tick(t0 + std::time::Duration::from_secs(1), &mut out), None);
        assert!(out.events.is_empty(), "no repeat after coming back");
    }

    #[test]
    fn the_virtual_pad_is_blanked_away_from_the_game_and_restored_after() {
        let (mut r, mut out) = router(PadRoute::Game);
        assert!(out.pad.is_empty(), "the game sees the controller");
        let now = Instant::now();
        r.event(key(KeyCode::BTN_TL, Press::Down), now, &mut out);
        assert_eq!(out.pad, vec![key(KeyCode::BTN_TL, Press::Down)], "passed through");
        out.pad.clear();
        r.set_route(PadRoute::Mouse, &mut out);
        let axis = |v| Raw::abs(AbsoluteAxisCode::ABS_X, v);
        let rz = Raw::abs(AbsoluteAxisCode::ABS_RZ, 0);
        assert_eq!(out.pad, vec![key(KeyCode::BTN_TL, Press::Up), axis(0), rz, Raw::REPORT], "at rest");
        out.pad.clear();
        r.set_route(PadRoute::Keys, &mut out);
        assert!(out.pad.is_empty(), "still away");
        r.set_route(PadRoute::Game, &mut out);
        assert_eq!(out.pad, vec![key(KeyCode::BTN_TL, Press::Down), axis(1000), rz, Raw::REPORT], "as it is");
        // A new virtual pad (another kind) is shown the same.
        out.pad.clear();
        r.show(&mut out);
        assert_eq!(out.pad, vec![key(KeyCode::BTN_TL, Press::Down), axis(1000), rz, Raw::REPORT]);
    }

    #[test]
    fn home_with_rb_is_a_screenshot_and_the_game_sees_neither() {
        let (mut r, mut out) = router(PadRoute::Game);
        let now = Instant::now();
        r.event(key(KeyCode::BTN_MODE, Press::Down), now, &mut out);
        r.event(key(KeyCode::BTN_TR, Press::Down), now, &mut out);
        r.event(key(KeyCode::BTN_MODE, Press::Up), now, &mut out);
        r.event(key(KeyCode::BTN_TR, Press::Up), now, &mut out);
        assert_eq!(out.events, vec![Event::Device(DeviceKey::Screenshot)], "not Home as well");
        assert!(out.pad.is_empty(), "RB stays ClamShell's: {:?}", out.pad);
        // RB on its own is the game's again.
        r.event(key(KeyCode::BTN_TR, Press::Down), now, &mut out);
        assert_eq!(out.pad, vec![key(KeyCode::BTN_TR, Press::Down)]);
    }

    #[test]
    fn home_is_always_clamshells_and_view_with_the_mouse_or_typing() {
        let (mut r, mut out) = router(PadRoute::Game);
        let now = Instant::now();
        r.event(key(KeyCode::BTN_MODE, Press::Down), now, &mut out);
        assert!(out.events.is_empty(), "Home acts on release");
        r.event(key(KeyCode::BTN_MODE, Press::Up), now, &mut out);
        r.event(key(KeyCode::BTN_SELECT, Press::Down), now, &mut out);
        assert_eq!(out.events, vec![Event::Home]);
        assert_eq!(out.pad, vec![key(KeyCode::BTN_SELECT, Press::Down)], "View is the game's");
        r.set_route(PadRoute::Mouse, &mut out);
        out = Out::default();
        r.event(key(KeyCode::BTN_SELECT, Press::Down), now, &mut out);
        r.event(key(KeyCode::BTN_SELECT, Press::Up), now, &mut out);
        assert_eq!(out, Out { events: vec![Event::Pad(PadInput::Switch)], ..Out::default() });
    }
}
