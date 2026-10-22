//! Where the controller's events go: the route the state chose ([`PadRoute`]) and the mapping that
//! carries it out: the game's (the keymap in force, [`Remap`]), or ClamShell's (navigation, typing, the
//! pad mouse, listening). Pure: controller and gyro events in, events for the virtual devices and the
//! state out, so switching routes is tested here (whatever a mapping holds down is let go when the pad
//! leaves it).

use clamshell_core::hardware::DeviceKey;
use std::time::Instant;

use clamshell_core::{Event, PadInput, PadRoute};
use evdev::{AbsoluteAxisCode, KeyCode};

use clamshell_core::keymap::Keymap;

use super::listen::Listener;
use super::motion::{Sample, interval};
use super::mouse::PadMouse;
use super::navigator::Navigator;
use super::remap::{self, BACK_KEY, Remap, mouse_codes};
use super::typing::Typing;
use crate::input::{Press, Range, Raw};

/// The virtual mouse's keys: the pad mouse's buttons and the keys the pad types with. What the device
/// is built with, and what leaving the pad mouse or typing lets go of.
const MOUSE_DEVICE_KEYS: [KeyCode; 9] = [
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

/// Every key the virtual mouse has: its own (the pad mouse, typing), and what a keymap may press (the
/// keyboard's keys, the mouse's side buttons).
pub fn mouse_device_keys() -> Vec<KeyCode> {
    let mut keys = MOUSE_DEVICE_KEYS.to_vec();
    keys.extend(mouse_codes());
    keys.extend(crate::keyboard::all_keys());
    keys.sort_by_key(|k| k.0);
    keys.dedup();
    keys
}

/// Every key of the virtual mouse that the pad mouse and typing use up (the kernel passes on only the
/// ones that were down).
pub fn release_mouse_keys(out: &mut Out) {
    out.mouse.extend(MOUSE_DEVICE_KEYS.iter().map(|&k| Raw::key(k, Press::Up)));
    out.mouse.push(Raw::REPORT);
}

/// What the pad's events become.
#[derive(Debug, Default, PartialEq)]
pub struct Out {
    /// For the game's virtual pad.
    pub pad: Vec<Raw>,
    /// For the virtual mouse (pointer, buttons, wheel, and the keys the pad types or a keymap presses).
    pub mouse: Vec<Raw>,
    /// For the state.
    pub events: Vec<Event>,
    /// For the controller's motors.
    pub buzz: Option<super::rumble::Buzz>,
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

/// The sticks' and the triggers' ranges (read from the left stick's x axis and the right trigger: each
/// pair's axes share one).
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
        self.events_where(real, |_| true, out);
    }

    /// [`PadSnapshot::events`] for the keys and axes whose event `pick` takes.
    fn events_where(&self, real: bool, pick: impl Fn(Raw) -> bool, out: &mut Vec<Raw>) {
        let press = if real { Press::Down } else { Press::Up };
        let start = out.len();
        out.extend(self.keys.iter().map(|&k| Raw::key(k, press)).filter(|r| pick(*r)));
        out.extend(self.axes.iter().map(|a| Raw::abs(a.code, if real { a.value } else { a.rest })).filter(|r| pick(*r)));
        if out.len() > start {
            out.push(Raw::REPORT);
        }
    }

    /// Whether a key is held.
    pub fn held(&self, key: KeyCode) -> bool {
        self.keys.contains(&key)
    }

    /// An axis' value, if the controller has it.
    pub fn value(&self, code: AbsoluteAxisCode) -> Option<i32> {
        self.axes.iter().find(|a| a.code == code).map(|a| a.value)
    }
}

/// Away from the game, the virtual pad shows a neutral controller.
fn away(route: PadRoute) -> bool {
    route != PadRoute::Game
}

/// Home held with one of these buttons: ClamShell's system shortcut (the game never sees the button).
/// The D-pad as a hat (most controllers) is in [`PadRouter::event`]; as buttons, here.
const SYSTEM: [(KeyCode, DeviceKey); 5] = [
    (KeyCode::BTN_TR, DeviceKey::Screenshot),
    (KeyCode::BTN_SELECT, DeviceKey::BottomView),
    (KeyCode::BTN_START, DeviceKey::BottomPower),
    (KeyCode::BTN_DPAD_UP, DeviceKey::WindowUp),
    (KeyCode::BTN_DPAD_DOWN, DeviceKey::WindowDown),
];

/// Home with B: let go before this, the window in front closes; held this long, its program quits.
const QUIT_HOLD: std::time::Duration = std::time::Duration::from_millis(800);

/// The home button.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Home {
    Up,
    /// Held; whether a combination used it (then letting go isn't Home).
    Held {
        combined: bool,
    },
    /// Held, then the Back button with a game in front: the virtual pad's Guide is down until either is
    /// let go.
    Guide,
}

/// Routes the pad's events. The home button is ClamShell's: on its own it is [`Event::Home`] (on
/// release, so it can start a combination); held with a button of [`SYSTEM`], that shortcut; held, then
/// B, the window in front closed (let go at once) or its program quit (held); held, then
/// the Thor's Back button with a game in front, the game's Guide button (Steam's overlay;
/// buttons pressed meanwhile go to the game, for Steam's chords) while both are held. Held with any other
/// button, that button is Home's too (the game sees neither it nor its release), and letting Home go
/// isn't Home. Back on its own:
/// an input of the keymap in the game ([`remap`]), back ([`PadInput::Back`]) on ClamShell's screens.
/// With the pad mouse or typing, View is ClamShell's too (to move the pad to the second screen).
pub struct PadRouter {
    route: PadRoute,
    home: Home,
    /// Buttons a combination took: their release stays ClamShell's too.
    taken: Vec<KeyCode>,
    /// The D-pad's up or down taken by a Home shortcut: its events are ClamShell's until it is centred.
    hat_taken: bool,
    /// Home with B pressed: when holding it quits the program in front (let go before: its window
    /// closes).
    closing: Option<Instant>,
    mouse: PadMouse,
    navigator: Navigator,
    typing: Typing,
    listener: Listener,
    remap: Remap,
    /// Events passed through to the game's pad since the last report.
    passed: bool,
    /// The gyro's last sample's time, and its offset at rest.
    gyro_at: Option<std::time::Duration>,
    calibration: super::motion::Calibration,
    snapshot: PadSnapshot,
}

impl PadRouter {
    /// A controller just taken over (its ranges, its axes now), the pad going to `route`: away from the
    /// game, the virtual pad is blanked.
    /// `keymap`: the keymap in force.
    pub fn new(ranges: Ranges, snapshot: PadSnapshot, route: PadRoute, keymap: Keymap, out: &mut Out) -> PadRouter {
        let mut remap = Remap::new(ranges);
        remap.set_keymap(keymap, &snapshot, false, out);
        let mut router = PadRouter {
            route,
            home: Home::Up,
            taken: Vec::new(),
            hat_taken: false,
            closing: None,
            mouse: PadMouse::new(ranges),
            navigator: Navigator::new(ranges.stick, ranges.trigger),
            typing: Typing,
            listener: Listener::new(ranges),
            remap,
            passed: false,
            gyro_at: None,
            calibration: super::motion::Calibration::default(),
            snapshot,
        };
        if away(route) {
            router.snapshot.events(false, &mut out.pad);
        } else {
            // (The virtual pad is taken to show the controller as it is, as games saw it before.)
            router.remap.prime(&router.snapshot);
        }
        router
    }

    fn mapping(&mut self, route: PadRoute) -> Option<&mut dyn Mapping> {
        match route {
            PadRoute::Game => None,
            PadRoute::Ui => Some(&mut self.navigator),
            PadRoute::Keys => Some(&mut self.typing),
            PadRoute::Mouse => Some(&mut self.mouse),
            PadRoute::Listen => Some(&mut self.listener),
        }
    }

    /// The pad comes to the game: what the keymap makes of the controller now, and what it doesn't
    /// know as it is.
    fn enter_game(&mut self, out: &mut Out) {
        self.snapshot.events_where(true, |r| !remap::known(r), &mut out.pad);
        self.remap.update(&self.snapshot, Instant::now(), out);
    }

    /// A new keymap in force: applied at once while the game has the pad (what the old one held is
    /// let go).
    pub fn set_keymap(&mut self, keymap: Keymap, out: &mut Out) {
        let live = self.route == PadRoute::Game;
        self.remap.set_keymap(keymap, &self.snapshot, live, out);
    }

    /// A gyro sample: to the keymap while the game has the pad.
    pub fn motion(&mut self, sample: Sample, out: &mut Out) {
        let dt = interval(self.gyro_at, sample.at);
        self.gyro_at = Some(sample.at);
        // (Calibrated on every route: it learns at rest whatever the pad does.)
        let (rate, calibrated) = self.calibration.correct(sample.rate, dt);
        if let Some(ok) = calibrated {
            out.events.push(Event::GyroCalibrated(ok));
        }
        if self.route == PadRoute::Game {
            self.remap.motion(rate, dt, &self.snapshot, out);
        }
    }

    /// Calibrates the gyro from its next samples (the device held still); [`Event::GyroCalibrated`]
    /// says how it went.
    pub fn calibrate(&mut self) {
        self.calibration.ask();
    }

    /// The controller is gone: whatever the route holds down outside the game's pad is let go.
    pub fn lose(&mut self, out: &mut Out) {
        match self.mapping(self.route) {
            Some(mapping) => mapping.release(out),
            None => self.remap.release(out),
        }
    }

    /// The pad goes to `to`: the mapping it leaves lets go of what it holds, and the virtual pad is
    /// blanked when leaving the game (or shows the controller again when coming back).
    pub fn set_route(&mut self, to: PadRoute, out: &mut Out) {
        let from = self.route;
        if from == to {
            return;
        }
        match self.mapping(from) {
            Some(mapping) => mapping.release(out),
            None => self.remap.release(out),
        }
        if self.home == Home::Guide {
            self.guide(false, out);
            self.home = Home::Held { combined: true };
        }
        self.route = to;
        if away(to) && !away(from) {
            // (The keymap let go of its own outputs above; the rest as it is on the controller.)
            self.snapshot.events_where(false, |r| !remap::known(r), &mut out.pad);
        } else if away(from) && !away(to) {
            self.enter_game(out);
        }
        log::info!("pad: {to:?}");
    }

    /// Events that bring what the router holds to the controller as it is (`down`: its keys held,
    /// `axes`: its axes' values), after the kernel dropped some of its events: what was let go meanwhile
    /// is let go, what was pressed is pressed (Home's press before the others', its release after).
    pub fn resync(&self, down: &[KeyCode], axes: &[(AbsoluteAxisCode, i32)], out: &mut Vec<Raw>) {
        let start = out.len();
        let mut held: Vec<KeyCode> = self.snapshot.keys.iter().chain(&self.taken).copied().collect();
        held.dedup();
        out.extend(held.iter().filter(|k| !down.contains(k)).map(|&k| Raw::key(k, Press::Up)));
        let home = down.contains(&KeyCode::BTN_MODE);
        if home != (self.home != Home::Up) {
            out.push(Raw::key(KeyCode::BTN_MODE, if home { Press::Down } else { Press::Up }));
        }
        out.extend(down.iter().filter(|&&k| k != KeyCode::BTN_MODE && !held.contains(&k)).map(|&k| Raw::key(k, Press::Down)));
        out.extend(axes.iter().filter(|&&(code, value)| self.snapshot.value(code).is_some_and(|v| v != value)).map(|&(code, value)| Raw::abs(code, value)));
        if out.len() > start {
            out.push(Raw::REPORT);
        }
    }

    /// One controller event at `now`.
    pub fn event(&mut self, raw: Raw, now: Instant, out: &mut Out) {
        let key = raw.as_key();
        let held = matches!(self.home, Home::Held { .. });
        match key {
            Some((KeyCode::BTN_MODE, Press::Down)) => {
                self.home = Home::Held { combined: false };
                return;
            }
            Some((KeyCode::BTN_MODE, Press::Up)) => {
                match std::mem::replace(&mut self.home, Home::Up) {
                    Home::Held { combined: false } => out.events.push(Event::Home),
                    Home::Guide => self.guide(false, out),
                    Home::Held { .. } | Home::Up => {}
                }
                return;
            }
            Some((KeyCode::BTN_MODE, Press::Repeat)) => return,
            Some((code, press)) if self.taken.contains(&code) => {
                if press == Press::Up {
                    self.taken.retain(|&k| k != code);
                    if code == KeyCode::BTN_EAST && self.closing.take().is_some() {
                        out.events.push(Event::Device(DeviceKey::CloseWindow));
                    }
                    if code == BACK_KEY && self.home == Home::Guide {
                        self.guide(false, out);
                        self.home = Home::Held { combined: true };
                    }
                }
                return;
            }
            // Home, then Back: Guide while both are held (a game in front); Back isn't the keymap's
            // then.
            Some((code, Press::Down)) if held && code == BACK_KEY => {
                self.taken.push(code);
                if self.route == PadRoute::Game {
                    self.home = Home::Guide;
                    self.guide(true, out);
                } else {
                    self.home = Home::Held { combined: true };
                }
                return;
            }
            // Home with the D-pad up or down (a hat): a window between the screens.
            None if raw.as_abs().is_some_and(|(axis, _)| axis == AbsoluteAxisCode::ABS_HAT0Y) && (held || self.hat_taken) => {
                let value = raw.as_abs().map_or(0, |(_, v)| v);
                if self.hat_taken {
                    self.hat_taken = value != 0;
                } else if value != 0 {
                    self.home = Home::Held { combined: true };
                    self.hat_taken = true;
                    out.events.push(Event::Device(if value < 0 { DeviceKey::WindowUp } else { DeviceKey::WindowDown }));
                }
                return;
            }
            // Home with B: Close window when let go soon, Quit once held (see the tick).
            Some((KeyCode::BTN_EAST, Press::Down)) if held => {
                self.home = Home::Held { combined: true };
                self.taken.push(KeyCode::BTN_EAST);
                self.closing = Some(now + QUIT_HOLD);
                return;
            }
            Some((code, Press::Down)) if held && let Some(&(_, shortcut)) = SYSTEM.iter().find(|(k, _)| *k == code) => {
                self.home = Home::Held { combined: true };
                self.taken.push(code);
                out.events.push(Event::Device(shortcut));
                return;
            }
            // Any other button with Home held: Home's too, the game sees neither it nor its release.
            Some((code, Press::Down)) if held => {
                self.home = Home::Held { combined: true };
                self.taken.push(code);
                return;
            }
            // (The D-pad's left and right too.)
            None if held && raw.as_abs().is_some_and(|(axis, v)| axis == AbsoluteAxisCode::ABS_HAT0X && v != 0) => {
                self.home = Home::Held { combined: true };
                return;
            }
            _ => {}
        }
        self.snapshot.update(raw);
        // The Back button away from the game: back on ClamShell's screens, nothing with the pad mouse or
        // typing; while a keymap's edit screen listens, an input like the others (to remap it).
        if let Some((code, press)) = key
            && code == BACK_KEY
            && matches!(self.route, PadRoute::Ui | PadRoute::Mouse | PadRoute::Keys)
        {
            if press == Press::Down && self.route == PadRoute::Ui {
                out.events.push(Event::Pad(PadInput::Back));
            }
            return;
        }
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
            None if remap::known(raw) => self.remap.update(&self.snapshot, now, out),
            // What the keymap doesn't know passes through, with its report.
            None if raw.is_report() => {
                if std::mem::take(&mut self.passed) {
                    out.pad.push(raw);
                }
            }
            None => {
                self.passed = true;
                out.pad.push(raw);
            }
        }
    }

    /// Ticks the mapping in force, and Home with B held long enough (its program quits); when to tick
    /// next, if needed.
    pub fn tick(&mut self, now: Instant, out: &mut Out) -> Option<Instant> {
        if self.closing.is_some_and(|at| now >= at) {
            self.closing = None;
            out.events.push(Event::Device(DeviceKey::Quit));
        }
        let next = match self.mapping(self.route) {
            Some(m) => m.tick(now, out),
            None => self.remap.tick(now, &self.snapshot, out),
        };
        match (next, self.closing) {
            (Some(a), Some(b)) => Some(a.min(b)),
            (a, b) => a.or(b),
        }
    }

    /// The virtual pad's Guide button down or up.
    fn guide(&self, down: bool, out: &mut Out) {
        out.pad.extend([Raw::key(KeyCode::BTN_MODE, if down { Press::Down } else { Press::Up }), Raw::REPORT]);
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
        let router = PadRouter::new(ranges, PadSnapshot::new(axes), route, Keymap::default(), &mut out);
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
    fn events_dropped_are_made_up_from_the_controller_as_it_is() {
        let (mut r, mut out) = router(PadRoute::Game);
        let now = Instant::now();
        r.event(key(KeyCode::BTN_SOUTH, Press::Down), now, &mut out);
        let mut events = Vec::new();
        r.resync(&[KeyCode::BTN_EAST], &[(AbsoluteAxisCode::ABS_X, 1000), (AbsoluteAxisCode::ABS_RZ, 500)], &mut events);
        assert_eq!(events, [key(KeyCode::BTN_SOUTH, Press::Up), key(KeyCode::BTN_EAST, Press::Down), Raw::abs(AbsoluteAxisCode::ABS_RZ, 500), Raw::REPORT]);
        for e in events {
            r.event(e, now, &mut out);
        }
        let mut again = Vec::new();
        r.resync(&[KeyCode::BTN_EAST], &[(AbsoluteAxisCode::ABS_X, 1000), (AbsoluteAxisCode::ABS_RZ, 500)], &mut again);
        assert_eq!(again, [], "in step");
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
        assert_eq!(out.pad, vec![key(KeyCode::BTN_TL, Press::Down), Raw::REPORT], "passed through");
        out.pad.clear();
        r.set_route(PadRoute::Mouse, &mut out);
        let axis = |v| Raw::abs(AbsoluteAxisCode::ABS_X, v);
        let rz = Raw::abs(AbsoluteAxisCode::ABS_RZ, 0);
        assert_eq!(out.pad, vec![key(KeyCode::BTN_TL, Press::Up), axis(0), Raw::REPORT], "at rest (the trigger was)");
        out.pad.clear();
        r.set_route(PadRoute::Keys, &mut out);
        assert!(out.pad.is_empty(), "still away");
        r.set_route(PadRoute::Game, &mut out);
        let hat = |axis| Raw::abs(axis, 0);
        let (hx, hy) = (hat(AbsoluteAxisCode::ABS_HAT0X), hat(AbsoluteAxisCode::ABS_HAT0Y));
        assert_eq!(out.pad, vec![key(KeyCode::BTN_TL, Press::Down), axis(1000), rz, hx, hy, Raw::REPORT], "as it is (the D-pad's hat too)");
    }

    #[test]
    fn home_with_select_start_and_the_dpad_are_the_bottom_screens_and_the_game_sees_none() {
        // (A controller with a D-pad hat.)
        let ranges = Ranges { stick: Range::STICK, trigger: Range { min: 0, max: 1023 } };
        let axes = vec![Axis { code: AbsoluteAxisCode::ABS_HAT0Y, value: 0, rest: 0 }];
        let mut out = Out::default();
        let mut r = PadRouter::new(ranges, PadSnapshot::new(axes), PadRoute::Game, Keymap::default(), &mut out);
        let now = Instant::now();
        let hat = |v| Raw::abs(AbsoluteAxisCode::ABS_HAT0Y, v);
        r.event(key(KeyCode::BTN_MODE, Press::Down), now, &mut out);
        for code in [KeyCode::BTN_SELECT, KeyCode::BTN_START] {
            r.event(key(code, Press::Down), now, &mut out);
            r.event(key(code, Press::Up), now, &mut out);
        }
        // The D-pad as a hat: down, centred, up, centred; then Home let go.
        for v in [1, 0, -1, 0] {
            r.event(hat(v), now, &mut out);
            r.event(Raw::REPORT, now, &mut out);
        }
        r.event(key(KeyCode::BTN_MODE, Press::Up), now, &mut out);
        let device = |k| Event::Device(k);
        assert_eq!(
            out.events,
            vec![device(DeviceKey::BottomView), device(DeviceKey::BottomPower), device(DeviceKey::WindowDown), device(DeviceKey::WindowUp)],
            "not Home as well"
        );
        // (The game's pad has the D-pad as a hat.)
        let dpad = |pad: &[Raw]| pad.iter().any(|e| *e == Raw::abs(AbsoluteAxisCode::ABS_HAT0Y, 1) || *e == Raw::abs(AbsoluteAxisCode::ABS_HAT0Y, -1));
        assert!(!dpad(&out.pad), "the game saw no D-pad: {:?}", out.pad);
        // Home let go while the D-pad is still down: the rest of that press is ClamShell's too.
        out.events.clear();
        out.pad.clear();
        r.event(key(KeyCode::BTN_MODE, Press::Down), now, &mut out);
        r.event(hat(1), now, &mut out);
        r.event(key(KeyCode::BTN_MODE, Press::Up), now, &mut out);
        r.event(hat(0), now, &mut out);
        r.event(Raw::REPORT, now, &mut out);
        assert_eq!(out.events, vec![device(DeviceKey::WindowDown)]);
        assert!(!dpad(&out.pad), "{:?}", out.pad);
        // Without Home, the D-pad is the game's.
        r.event(hat(1), now, &mut out);
        r.event(Raw::REPORT, now, &mut out);
        assert!(out.pad.contains(&Raw::abs(AbsoluteAxisCode::ABS_HAT0Y, 1)), "{:?}", out.pad);
    }

    #[test]
    fn home_with_b_closes_the_window_held_quits_and_the_game_sees_no_b() {
        let (mut r, mut out) = router(PadRoute::Game);
        let now = Instant::now();
        let b = |out: &Out| out.pad.iter().any(|e| e.as_key().is_some_and(|(k, _)| k == KeyCode::BTN_EAST));
        // Let go soon: Close window.
        r.event(key(KeyCode::BTN_MODE, Press::Down), now, &mut out);
        r.event(key(KeyCode::BTN_EAST, Press::Down), now, &mut out);
        assert_eq!(r.tick(now, &mut out), Some(now + QUIT_HOLD), "ticked at the hold's end");
        r.event(key(KeyCode::BTN_EAST, Press::Up), now + QUIT_HOLD / 2, &mut out);
        r.event(key(KeyCode::BTN_MODE, Press::Up), now + QUIT_HOLD / 2, &mut out);
        assert_eq!(out.events, vec![Event::Device(DeviceKey::CloseWindow)], "not Home as well");
        // Held: Quit once the hold is reached; letting go then is nothing more.
        out.events.clear();
        r.event(key(KeyCode::BTN_MODE, Press::Down), now, &mut out);
        r.event(key(KeyCode::BTN_EAST, Press::Down), now, &mut out);
        r.tick(now + QUIT_HOLD, &mut out);
        r.event(key(KeyCode::BTN_EAST, Press::Up), now + QUIT_HOLD * 2, &mut out);
        r.event(key(KeyCode::BTN_MODE, Press::Up), now + QUIT_HOLD * 2, &mut out);
        assert_eq!(out.events, vec![Event::Device(DeviceKey::Quit)]);
        assert!(!b(&out), "the game saw no B: {:?}", out.pad);
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
        assert_eq!(out.pad, vec![key(KeyCode::BTN_TR, Press::Down), Raw::REPORT]);
    }

    const BACK: KeyCode = KeyCode::BTN_BACK;

    #[test]
    fn home_then_back_with_a_game_in_front_is_its_guide_while_both_are_held() {
        let (mut r, mut out) = router(PadRoute::Game);
        let t0 = Instant::now();
        r.event(key(KeyCode::BTN_MODE, Press::Down), t0, &mut out);
        assert_eq!(r.tick(t0 + std::time::Duration::from_secs(2), &mut out), None, "held long: nothing");
        assert!(out.pad.is_empty());
        r.event(key(BACK, Press::Down), t0, &mut out);
        assert_eq!(out.pad, vec![key(KeyCode::BTN_MODE, Press::Down), Raw::REPORT]);
        // RB meanwhile is the game's (a Steam chord), not a screenshot.
        r.event(key(KeyCode::BTN_TR, Press::Down), t0, &mut out);
        assert_eq!(held(&out.pad), vec![KeyCode::BTN_TR, KeyCode::BTN_MODE]);
        // Back let go first: Guide up; Home's release isn't Home; Back's release isn't the game's.
        r.event(key(BACK, Press::Up), t0, &mut out);
        assert_eq!(held(&out.pad), vec![KeyCode::BTN_TR]);
        r.event(key(KeyCode::BTN_MODE, Press::Up), t0, &mut out);
        assert!(out.events.is_empty(), "not Home: {:?}", out.events);
        assert!(!out.pad.iter().any(|e| e.as_key().is_some_and(|(k, _)| k == BACK)), "Back never reaches the game");
        // Home let go first: Guide up, and Back's release after stays ours.
        out = Out::default();
        r.event(key(KeyCode::BTN_MODE, Press::Down), t0, &mut out);
        r.event(key(BACK, Press::Down), t0, &mut out);
        r.event(key(KeyCode::BTN_MODE, Press::Up), t0, &mut out);
        assert!(!held(&out.pad).contains(&KeyCode::BTN_MODE));
        let before = out.pad.len();
        r.event(key(BACK, Press::Up), t0, &mut out);
        assert_eq!(out.pad.len(), before);
        assert!(out.events.is_empty());
    }

    #[test]
    fn home_with_another_button_is_a_combination_not_home() {
        let (mut r, mut out) = router(PadRoute::Game);
        let t0 = Instant::now();
        r.event(key(KeyCode::BTN_MODE, Press::Down), t0, &mut out);
        r.event(key(KeyCode::BTN_SOUTH, Press::Down), t0, &mut out);
        r.event(key(KeyCode::BTN_MODE, Press::Up), t0, &mut out);
        r.event(key(KeyCode::BTN_SOUTH, Press::Up), t0, &mut out);
        assert!(out.events.is_empty(), "not Home");
        assert!(
            held(&out.pad).is_empty() && !out.pad.iter().any(|e| e.as_key().is_some_and(|(k, _)| k == KeyCode::BTN_SOUTH)),
            "A, pressed with Home held, never reaches the game: {:?}",
            out.pad
        );
        // Pressed again after: the game's.
        r.event(key(KeyCode::BTN_SOUTH, Press::Down), t0, &mut out);
        assert_eq!(held(&out.pad), vec![KeyCode::BTN_SOUTH]);
        // Away from the game, Home then Back is neither Guide nor Home.
        let (mut r, mut out) = router(PadRoute::Ui);
        out.pad.clear();
        r.event(key(KeyCode::BTN_MODE, Press::Down), t0, &mut out);
        r.event(key(BACK, Press::Down), t0, &mut out);
        r.event(key(BACK, Press::Up), t0, &mut out);
        r.event(key(KeyCode::BTN_MODE, Press::Up), t0, &mut out);
        assert!(out.events.is_empty() && out.pad.is_empty(), "{out:?}");
        // Home alone, held long: Home on release.
        r.event(key(KeyCode::BTN_MODE, Press::Down), t0, &mut out);
        r.event(key(KeyCode::BTN_MODE, Press::Up), t0 + std::time::Duration::from_secs(2), &mut out);
        assert_eq!(out.events, vec![Event::Home]);
    }

    #[test]
    fn back_on_clamshells_screens_is_back_and_nothing_with_the_mouse() {
        let (mut r, mut out) = router(PadRoute::Ui);
        out.pad.clear();
        r.event(key(BACK, Press::Down), Instant::now(), &mut out);
        r.event(key(BACK, Press::Up), Instant::now(), &mut out);
        assert_eq!(out.events, vec![Event::Pad(PadInput::Back)]);
        assert!(out.pad.is_empty());
        // A keymap's edit screen listening: Back is an input to remap.
        let (mut r, mut out) = router(PadRoute::Listen);
        out.pad.clear();
        r.event(key(BACK, Press::Down), Instant::now(), &mut out);
        r.event(Raw::REPORT, Instant::now(), &mut out);
        assert_eq!(out.events, vec![Event::Listened(clamshell_core::keymap::Input::Button(clamshell_core::keymap::Button::Back))]);
        let (mut r, mut out) = router(PadRoute::Mouse);
        out = Out { pad: vec![], ..out };
        r.event(key(BACK, Press::Down), Instant::now(), &mut out);
        assert!(out.events.is_empty() && out.mouse.is_empty() && out.pad.is_empty());
    }

    #[test]
    fn guide_is_let_go_when_the_pad_leaves_the_game() {
        let (mut r, mut out) = router(PadRoute::Game);
        let t0 = Instant::now();
        r.event(key(KeyCode::BTN_MODE, Press::Down), t0, &mut out);
        r.event(key(BACK, Press::Down), t0, &mut out);
        r.set_route(PadRoute::Ui, &mut out);
        assert!(!held(&out.pad).contains(&KeyCode::BTN_MODE), "no stuck Guide");
        out.events.clear();
        r.event(key(KeyCode::BTN_MODE, Press::Up), t0, &mut out);
        r.event(key(BACK, Press::Up), t0, &mut out);
        assert!(out.events.is_empty(), "letting go afterwards isn't Home, nor back");
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
        assert_eq!(out.pad, vec![key(KeyCode::BTN_SELECT, Press::Down), Raw::REPORT], "View is the game's");
        r.set_route(PadRoute::Mouse, &mut out);
        out = Out::default();
        r.event(key(KeyCode::BTN_SELECT, Press::Down), now, &mut out);
        r.event(key(KeyCode::BTN_SELECT, Press::Up), now, &mut out);
        assert_eq!(out, Out { events: vec![Event::Pad(PadInput::Switch)], ..Out::default() });
    }
}
