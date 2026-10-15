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
use super::remap::{self, Remap, is_layer_key, mouse_codes};
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
        self.events_where(real, |_| true, out);
    }

    /// [`PadSnapshot::events`] for the keys and axes whose event `pick` takes.
    pub fn events_where(&self, real: bool, pick: impl Fn(Raw) -> bool, out: &mut Vec<Raw>) {
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
const SYSTEM: [(KeyCode, DeviceKey); 1] = [(KeyCode::BTN_TR, DeviceKey::Screenshot)];

/// The home button.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Home {
    Up,
    /// Held; whether a combination used it (then letting go isn't Home).
    Held { combined: bool },
    /// Held, then the layer key with a game in front: the virtual pad's Guide is down until either is
    /// let go.
    Guide,
}

/// Routes the pad's events. The home button is ClamShell's: on its own it is [`Event::Home`] (on
/// release, so it can start a combination); held with a button of [`SYSTEM`], that shortcut; held, then
/// the layer key (the Thor's Back button) with a game in front, the game's Guide button (Steam's overlay;
/// buttons pressed meanwhile go to the game, for Steam's chords) while both are held. Held with any other
/// button, that button goes where it would, and letting Home go isn't Home. The layer key on its own:
/// the keymap's Back layer in the game ([`remap`]), back ([`PadInput::Back`]) on ClamShell's screens.
/// With the pad mouse or typing, View is ClamShell's too (to move the pad to the second screen).
pub struct PadRouter {
    route: PadRoute,
    home: Home,
    /// Buttons a combination took: their release stays ClamShell's too.
    taken: Vec<KeyCode>,
    mouse: PadMouse,
    navigator: Navigator,
    typing: Typing,
    listener: Listener,
    remap: Remap,
    /// Events passed through to the game's pad since the last report.
    passed: bool,
    /// The gyro's last sample's time.
    gyro_at: Option<std::time::Duration>,
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
            mouse: PadMouse::new(ranges),
            navigator: Navigator::new(ranges.stick),
            typing: Typing,
            listener: Listener::new(ranges),
            remap,
            passed: false,
            gyro_at: None,
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
        self.remap.update(&self.snapshot, out);
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
        if self.route == PadRoute::Game {
            self.remap.motion(sample.rate, dt, &self.snapshot, out);
        }
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
                    if is_layer_key(code) && self.home == Home::Guide {
                        self.guide(false, out);
                        self.home = Home::Held { combined: true };
                    }
                }
                return;
            }
            // Home, then the layer key: Guide while both are held (a game in front); the layer key
            // isn't the layer then.
            Some((code, Press::Down)) if held && is_layer_key(code) => {
                self.taken.push(code);
                if self.route == PadRoute::Game {
                    self.home = Home::Guide;
                    self.guide(true, out);
                } else {
                    self.home = Home::Held { combined: true };
                }
                return;
            }
            Some((code, Press::Down)) if held && let Some(&(_, shortcut)) = SYSTEM.iter().find(|(k, _)| *k == code) => {
                self.home = Home::Held { combined: true };
                self.taken.push(code);
                out.events.push(Event::Device(shortcut));
                return;
            }
            Some((_, Press::Down)) if held => self.home = Home::Held { combined: true },
            _ => {}
        }
        self.snapshot.update(raw);
        // The layer key away from the game: back on ClamShell's screens (the edit screen's layer while it
        // listens), nothing with the pad mouse or typing.
        if let Some((code, press)) = key
            && is_layer_key(code)
            && self.route != PadRoute::Game
        {
            if press == Press::Down && matches!(self.route, PadRoute::Ui | PadRoute::Listen) {
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
            None if remap::known(raw) => self.remap.update(&self.snapshot, out),
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

    /// Ticks the mapping in force; when to tick next, if needed.
    pub fn tick(&mut self, now: Instant, out: &mut Out) -> Option<Instant> {
        match self.mapping(self.route) {
            Some(m) => m.tick(now, out),
            None => self.remap.tick(now, out),
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
        assert_eq!(out.pad, vec![key(KeyCode::BTN_TL, Press::Down), axis(1000), rz, Raw::REPORT], "as it is");
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

    const BACK: KeyCode = KeyCode::KEY_BACK;

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
        assert!(out.events.is_empty(), "not Home");
        assert_eq!(held(&out.pad), vec![KeyCode::BTN_SOUTH], "A goes to the game as usual");
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
        for route in [PadRoute::Ui, PadRoute::Listen] {
            let (mut r, mut out) = router(route);
            out.pad.clear();
            r.event(key(BACK, Press::Down), Instant::now(), &mut out);
            r.event(key(BACK, Press::Up), Instant::now(), &mut out);
            assert_eq!(out.events, vec![Event::Pad(PadInput::Back)], "{route:?}");
            assert!(out.pad.is_empty());
        }
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
