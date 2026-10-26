//! Where the controller's events go: the route the state chose ([`PadRoute`]) and the mapping that
//! carries it out. That is either the game's mapping, which is the keymap in force ([`Remap`]), or one of
//! ClamShell's: navigation, typing, the pad mouse or listening. There is no I/O here. Controller and gyro
//! events go in, and events for the virtual devices and the state come out, so switching routes is
//! tested here. Whatever a mapping holds down is let go when the pad leaves it.

use clamshell_state::device::buttons::DeviceKey;
use std::time::Instant;

use clamshell_state::{Event, PadInput, PadRoute};
use evdev::{AbsoluteAxisCode, KeyCode};

use crate::input::flick::Flick;
use clamshell_state::keymap::{Dir, Keymap};

use super::listen::Listener;
use super::motion::{Sample, interval};
use super::mouse::PadMouse;
use super::navigator::Navigator;
use super::remap::{self, BACK_KEY, Remap, mouse_codes};
use super::typing::Typing;
use crate::os::evdev::{Press, Range, Raw};

/// The virtual mouse's own keys: the pad mouse's buttons and the keys the pad types with. The device is
/// built with them, and leaving the pad mouse or typing lets go of them.
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

/// Every key the virtual mouse has: its own, for the pad mouse and typing, and those a keymap may press,
/// which are the keyboard's keys and the mouse's side buttons.
pub(crate) fn mouse_device_keys() -> Vec<KeyCode> {
    let mut keys = MOUSE_DEVICE_KEYS.to_vec();
    keys.extend(mouse_codes());
    keys.extend(crate::input::keyboard::all_keys());
    keys.sort_by_key(|k| k.0);
    keys.dedup();
    keys
}

/// Lets go of every key of the virtual mouse that the pad mouse and typing use. The kernel passes on only
/// the ones that were down.
pub(crate) fn release_mouse_keys(out: &mut Out) {
    out.mouse.extend(MOUSE_DEVICE_KEYS.iter().map(|&k| Raw::key(k, Press::Up)));
    out.mouse.push(Raw::REPORT);
}

/// What the pad's events become.
#[derive(Debug, Default, PartialEq)]
pub(crate) struct Out {
    /// For the game's virtual pad.
    pub(crate) pad: Vec<Raw>,
    /// For the virtual mouse: pointer, buttons, wheel, and the keys the pad types or a keymap presses.
    pub(crate) mouse: Vec<Raw>,
    /// For the state.
    pub(crate) events: Vec<Event>,
    /// For the controller's motors.
    pub(crate) buzz: Option<super::rumble::Buzz>,
}

/// A way of turning the pad's events into something else, while its route is in force.
pub(crate) trait Mapping {
    /// One controller event at `now`.
    fn event(&mut self, raw: Raw, now: Instant, out: &mut Out);

    /// Whatever time brings, such as pointer motion or a held direction's repeats. Returns when to be
    /// ticked next, if it needs to be.
    fn tick(&mut self, now: Instant, out: &mut Out) -> Option<Instant>;

    /// Called when the pad leaves this mapping. Lets go of whatever it holds down, and starts over.
    fn release(&mut self, out: &mut Out);
}

/// The sticks' and the triggers' ranges. They are read from the left stick's x axis and the right
/// trigger, since each pair's axes share one range.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct Ranges {
    pub(crate) stick: Range,
    pub(crate) trigger: Range,
}

/// The controller's state as last seen. It is used to blank the virtual pad while the pad is away from
/// the game, and to restore it when it comes back.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub(crate) struct PadSnapshot {
    keys: Vec<KeyCode>,
    axes: Vec<Axis>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Axis {
    pub(crate) code: AbsoluteAxisCode,
    pub(crate) value: i32,
    /// The axis' value at rest: the middle of a stick (0), or the bottom of a trigger.
    pub(crate) rest: i32,
}

impl PadSnapshot {
    /// The axes as they are when the controller is taken over. Keys are assumed up.
    pub(crate) fn new(axes: Vec<Axis>) -> PadSnapshot {
        PadSnapshot { keys: Vec::new(), axes }
    }

    pub(crate) fn update(&mut self, raw: Raw) {
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

    /// Events that make a pad look like this state if `real`, or else like an untouched pad.
    fn events(&self, real: bool, out: &mut Vec<Raw>) {
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
    pub(crate) fn held(&self, key: KeyCode) -> bool {
        self.keys.contains(&key)
    }

    /// An axis' value, if the controller has it.
    pub(crate) fn value(&self, code: AbsoluteAxisCode) -> Option<i32> {
        self.axes.iter().find(|a| a.code == code).map(|a| a.value)
    }
}

/// Away from the game, the virtual pad shows a neutral controller.
fn away(route: PadRoute) -> bool {
    route != PadRoute::Game
}

/// Home held with one of these buttons gives ClamShell's system shortcut, and the game never sees the
/// button. Directional flicks are recognised separately from normalised stick axes.
const SYSTEM: [(KeyCode, DeviceKey); 3] =
    [(KeyCode::BTN_TR, DeviceKey::Screenshot), (KeyCode::BTN_THUMBR, DeviceKey::BottomView), (KeyCode::BTN_START, DeviceKey::BottomPower)];

/// Home with B: if B is let go before this, the window in front closes. If held this long, its program
/// quits.
const QUIT_HOLD: std::time::Duration = std::time::Duration::from_millis(800);

/// The home button.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Home {
    Up,
    /// Held, with whether a combination used it. If one did, letting go isn't Home.
    Held {
        combined: bool,
    },
    /// Held, then the Back button pressed with a game in front. The virtual pad's Guide is down until
    /// either is let go.
    Guide,
}

/// Routes the pad's events. The home button is ClamShell's:
///
/// - On its own it is [`Event::Home`], sent on release so it can start a combination.
/// - Held with a button of [`SYSTEM`], it gives that shortcut.
/// - Held, then B: the window in front closes if B is let go at once, or its program quits if B is held.
/// - Held, then the Thor's Back button with a game in front: the game's Guide button while both are held.
///   That opens Steam's overlay, and buttons pressed in the meantime go to the game, for Steam's chords.
/// - Held with any other button, that button is Home's too. The game sees neither it nor its release,
///   and letting Home go isn't Home.
///
/// Back on its own is an input of the keymap in the game ([`remap`]), and back ([`PadInput::Back`]) on
/// ClamShell's screens. With the pad mouse or typing, View is ClamShell's too, to move the pad to the
/// second screen.
pub(crate) struct PadRouter {
    route: PadRoute,
    watch_activity: bool,
    home: Home,
    /// Buttons a combination took. Their release stays ClamShell's too.
    taken: Vec<KeyCode>,
    /// Whether the D-pad's up or down was taken by a Home shortcut. If so, its events are ClamShell's until
    /// it is centred.
    stick_range: Range,
    flick: Flick,
    stick_taken: bool,
    /// With Home and B pressed, the time at which holding them quits the program in front. If let go
    /// before, its window closes instead.
    closing: Option<Instant>,
    mouse: PadMouse,
    navigator: Navigator,
    typing: Typing,
    listener: Listener,
    remap: Remap,
    /// Events passed through to the game's pad since the last report.
    passed: bool,
    /// The time of the gyro's last sample, and the gyro's offset at rest.
    gyro_at: Option<std::time::Duration>,
    calibration: super::motion::Calibration,
    snapshot: PadSnapshot,
}

impl PadRouter {
    /// A router for a controller just taken over, with its ranges and current axes, and the pad going to
    /// `route`. Away from the game, the virtual pad is blanked. `keymap` is the keymap in force.
    pub(crate) fn new(ranges: Ranges, snapshot: PadSnapshot, route: PadRoute, keymap: Keymap, out: &mut Out) -> PadRouter {
        let mut remap = Remap::new(ranges);
        remap.set_keymap(keymap, &snapshot, false, out);
        let mut router = PadRouter {
            route,
            watch_activity: false,
            home: Home::Up,
            taken: Vec::new(),
            stick_range: ranges.stick,
            flick: Flick::default(),
            stick_taken: false,
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
            // The virtual pad is taken to show the controller as it is, as games saw it before.
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

    /// The pad comes to the game. Sends what the keymap makes of the controller now, and passes on as
    /// they are the inputs the keymap doesn't know.
    fn enter_game(&mut self, out: &mut Out) {
        self.snapshot.events_where(true, |r| !remap::known(r), &mut out.pad);
        self.with_game_snapshot(|remap, snapshot| remap.update(snapshot, Instant::now(), out));
    }

    /// Puts a new keymap in force. It applies at once while the game has the pad, and what the old one
    /// held is let go.
    pub(crate) fn set_keymap(&mut self, keymap: Keymap, out: &mut Out) {
        let live = self.route == PadRoute::Game;
        self.with_game_snapshot(|remap, snapshot| remap.set_keymap(keymap, snapshot, live, out));
    }

    /// Handles a gyro sample, which goes to the keymap while the game has the pad.
    pub(crate) fn motion(&mut self, sample: Sample, out: &mut Out) {
        let dt = interval(self.gyro_at, sample.at);
        self.gyro_at = Some(sample.at);
        // Calibrate on every route, so it learns at rest whatever the pad does.
        let (rate, calibrated) = self.calibration.correct(sample.rate, dt);
        if let Some(ok) = calibrated {
            out.events.push(Event::GyroCalibrated(ok));
        }
        if self.route == PadRoute::Game {
            self.with_game_snapshot(|remap, snapshot| remap.motion(rate, dt, snapshot, out));
        }
    }

    /// Calibrates the gyro from its next samples, with the device held still. [`Event::GyroCalibrated`]
    /// says how it went.
    pub(crate) fn calibrate(&mut self) {
        self.calibration.ask();
    }

    /// Called when the controller is gone. Whatever the route holds down outside the game's pad is let
    /// go.
    pub(crate) fn lose(&mut self, out: &mut Out) {
        if self.home != Home::Up {
            out.events.push(Event::HomeHeld(false));
        }
        match self.mapping(self.route) {
            Some(mapping) => mapping.release(out),
            None => self.remap.release(out),
        }
    }

    /// Arms one activity report for the next game input after keyboard focus leaves the top window.
    pub(crate) fn watch_activity(&mut self, watch: bool) {
        self.watch_activity = watch;
    }

    /// Moves the pad to route `to`. The mapping it leaves lets go of what it holds. The virtual pad is
    /// blanked when the pad leaves the game, and shows the controller again when it comes back.
    pub(crate) fn set_route(&mut self, to: PadRoute, out: &mut Out) {
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
            // The keymap let go of its own outputs above. Blank the rest, which is as it is on the
            // controller.
            self.snapshot.events_where(false, |r| !remap::known(r), &mut out.pad);
        } else if away(from) && !away(to) {
            self.enter_game(out);
        }
        log::info!("pad: {to:?}");
    }

    /// Events that bring what the router holds in line with the controller as it is, after the kernel
    /// dropped some of its events. `down` is the keys held and `axes` the axes' values. What was let go in
    /// the meantime is let go and what was pressed is pressed. Home's press comes before the others and
    /// its release after.
    pub(crate) fn resync(&self, down: &[KeyCode], axes: &[(AbsoluteAxisCode, i32)], out: &mut Vec<Raw>) {
        let start = out.len();
        let mut held: Vec<KeyCode> = self.snapshot.keys.iter().chain(&self.taken).copied().collect();
        held.sort_by_key(|k| k.0);
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
    pub(crate) fn event(&mut self, raw: Raw, now: Instant, out: &mut Out) {
        let key = raw.as_key();
        let held = matches!(self.home, Home::Held { .. });
        match key {
            Some((KeyCode::BTN_MODE, Press::Down)) => {
                self.home = Home::Held { combined: false };
                self.flick.begin(self.right_stick());
                self.stick_taken = true;
                out.events.push(Event::HomeHeld(true));
                if self.route == PadRoute::Game {
                    self.with_game_snapshot(|remap, snapshot| remap.update(snapshot, now, out));
                } else {
                    let axes = [AbsoluteAxisCode::ABS_RX, AbsoluteAxisCode::ABS_RY]
                        .map(|code| (code, self.snapshot.axes.iter().find(|axis| axis.code == code).map_or(0, |axis| axis.rest)));
                    if let Some(mapping) = self.mapping(self.route) {
                        for (code, rest) in axes {
                            mapping.event(Raw::abs(code, rest), now, out);
                        }
                    }
                }
                return;
            }
            Some((KeyCode::BTN_MODE, Press::Up)) => {
                out.events.push(Event::HomeHeld(false));
                match std::mem::replace(&mut self.home, Home::Up) {
                    Home::Held { combined: false } => out.events.push(Event::HomeTap),
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
            // Home, then Back, with a game in front, is Guide while both are held. Back isn't the keymap's
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
            None if raw.as_abs().is_some_and(|(axis, _)| matches!(axis, AbsoluteAxisCode::ABS_RX | AbsoluteAxisCode::ABS_RY)) && (held || self.stick_taken) => {
                self.snapshot.update(raw);
                let [x, y] = self.right_stick();
                if Flick::centered([x, y]) && !held {
                    self.stick_taken = false;
                }
                if held && let Some(direction) = self.flick.update([x, y]) {
                    self.home = Home::Held { combined: true };
                    let action = match direction {
                        Dir::Left => DeviceKey::WindowPrevious,
                        Dir::Right => DeviceKey::WindowNext,
                        Dir::Up => DeviceKey::WindowUp,
                        Dir::Down => DeviceKey::WindowDown,
                    };
                    out.events.push(Event::Device(action));
                }
                return;
            }
            // Home with B closes the window if let go soon, and quits once held (see the tick).
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
            // Any other button with Home held is Home's too. The game sees neither it nor its release.
            Some((code, Press::Down)) if held => {
                self.home = Home::Held { combined: true };
                self.taken.push(code);
                return;
            }
            // The same for the D-pad's left and right.
            None if held && raw.as_abs().is_some_and(|(axis, v)| matches!(axis, AbsoluteAxisCode::ABS_HAT0X | AbsoluteAxisCode::ABS_HAT0Y) && v != 0) => {
                self.home = Home::Held { combined: true };
                return;
            }
            _ => {}
        }
        self.snapshot.update(raw);
        if self.watch_activity
            && self.route == PadRoute::Game
            && (key.is_some_and(|(_, press)| press == Press::Down) || raw.as_abs().is_some_and(|(_, value)| value.unsigned_abs() > 4096))
        {
            self.watch_activity = false;
            out.events.push(Event::PadActive);
        }
        // Away from the game, the Back button means back on ClamShell's screens and nothing with the pad
        // mouse or typing. While a keymap's edit screen listens, it is an input like the others, so it can
        // be remapped.
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
            None if remap::known(raw) => self.with_game_snapshot(|remap, snapshot| remap.update(snapshot, now, out)),
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

    fn with_game_snapshot<T>(&mut self, run: impl FnOnce(&mut Remap, &PadSnapshot) -> T) -> T {
        let axes = [AbsoluteAxisCode::ABS_RX, AbsoluteAxisCode::ABS_RY];
        let saved = axes.map(|code| self.snapshot.value(code));
        if self.stick_taken || self.home != Home::Up {
            for axis in &mut self.snapshot.axes {
                if axes.contains(&axis.code) {
                    axis.value = axis.rest;
                }
            }
        }
        let result = run(&mut self.remap, &self.snapshot);
        for (code, value) in axes.into_iter().zip(saved) {
            if let Some(value) = value
                && let Some(axis) = self.snapshot.axes.iter_mut().find(|a| a.code == code)
            {
                axis.value = value;
            }
        }
        result
    }

    fn right_stick(&self) -> [f32; 2] {
        [AbsoluteAxisCode::ABS_RX, AbsoluteAxisCode::ABS_RY].map(|axis| self.stick_range.centered(self.snapshot.value(axis).unwrap_or(0)))
    }

    /// Ticks the mapping in force, and quits the program in front once Home with B is held long enough.
    /// Returns when to tick next, if needed.
    pub(crate) fn tick(&mut self, now: Instant, out: &mut Out) -> Option<Instant> {
        if self.closing.is_some_and(|at| now >= at) {
            self.closing = None;
            out.events.push(Event::Device(DeviceKey::Quit));
        }
        let next = match self.mapping(self.route) {
            Some(m) => m.tick(now, out),
            None => self.with_game_snapshot(|remap, snapshot| remap.tick(now, snapshot, out)),
        };
        [next, self.closing].into_iter().flatten().min()
    }

    /// The virtual pad's Guide button down or up.
    fn guide(&self, down: bool, out: &mut Out) {
        out.pad.extend([Raw::key(KeyCode::BTN_MODE, if down { Press::Down } else { Press::Up }), Raw::REPORT]);
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    // These older tests assert shortcut actions. Gesture tests below also assert held/activity events.
    fn actions(out: &super::Out) -> Vec<clamshell_state::Event> {
        out.events.iter().filter(|e| !matches!(e, clamshell_state::Event::HomeHeld(_) | clamshell_state::Event::PadActive)).cloned().collect()
    }

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
    fn game_activity_is_reported_once_per_keyboard_focus_loss() {
        let (mut r, mut out) = router(PadRoute::Game);
        let now = Instant::now();
        out.events.clear();
        for _ in 0..100 {
            r.event(key(KeyCode::BTN_SOUTH, Press::Down), now, &mut out);
            r.event(key(KeyCode::BTN_SOUTH, Press::Up), now, &mut out);
            r.event(Raw::abs(AbsoluteAxisCode::ABS_X, 30000), now, &mut out);
        }
        assert!(out.events.is_empty(), "normal play sends no state activity events");
        r.watch_activity(true);
        for _ in 0..100 {
            r.event(Raw::abs(AbsoluteAxisCode::ABS_X, 30000), now, &mut out);
            r.event(key(KeyCode::BTN_SOUTH, Press::Down), now, &mut out);
            r.event(key(KeyCode::BTN_SOUTH, Press::Up), now, &mut out);
        }
        assert_eq!(out.events, vec![Event::PadActive]);
        r.watch_activity(false);
        r.watch_activity(true);
        r.event(key(KeyCode::BTN_SOUTH, Press::Up), now, &mut out);
        r.event(Raw::abs(AbsoluteAxisCode::ABS_X, 0), now, &mut out);
        assert_eq!(out.events, vec![Event::PadActive], "release and rest do not take focus");
        r.event(key(KeyCode::BTN_SOUTH, Press::Down), now, &mut out);
        assert_eq!(out.events, vec![Event::PadActive, Event::PadActive]);
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
        // Back on the pad mouse, the trigger is still held, but it counts only once pressed again.
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
        assert_eq!(actions(&out), vec![Event::Pad(PadInput::Down)]);
        assert!(r.tick(t0, &mut out).is_some(), "repeats while held");
        r.set_route(PadRoute::Mouse, &mut out);
        r.set_route(PadRoute::Ui, &mut out);
        out.events.clear();
        assert_eq!(r.tick(t0 + std::time::Duration::from_secs(1), &mut out), None);
        assert!(actions(&out).is_empty(), "no repeat after coming back");
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
    fn home_chords_use_the_right_stick_and_not_select_or_the_dpad() {
        let (mut r, mut out) = router(PadRoute::Game);
        let now = Instant::now();
        r.event(key(KeyCode::BTN_MODE, Press::Down), now, &mut out);
        for code in [KeyCode::BTN_SELECT, KeyCode::BTN_DPAD_UP, KeyCode::BTN_DPAD_DOWN] {
            r.event(key(code, Press::Down), now, &mut out);
            r.event(key(code, Press::Up), now, &mut out);
        }
        for code in [KeyCode::BTN_THUMBR, KeyCode::BTN_START] {
            r.event(key(code, Press::Down), now, &mut out);
            r.event(key(code, Press::Up), now, &mut out);
        }
        r.event(key(KeyCode::BTN_MODE, Press::Up), now, &mut out);
        assert_eq!(actions(&out), vec![Event::Device(DeviceKey::BottomView), Event::Device(DeviceKey::BottomPower)]);
        assert!(!out.pad.iter().any(|raw| {
            raw.as_key().is_some_and(|(key, _)| matches!(key, KeyCode::BTN_SELECT | KeyCode::BTN_THUMBR | KeyCode::BTN_DPAD_UP | KeyCode::BTN_DPAD_DOWN))
        }));
    }

    #[test]
    fn home_with_b_closes_the_window_held_quits_and_the_game_sees_no_b() {
        let (mut r, mut out) = router(PadRoute::Game);
        let now = Instant::now();
        let b = |out: &Out| out.pad.iter().any(|e| e.as_key().is_some_and(|(k, _)| k == KeyCode::BTN_EAST));
        // Letting go soon closes the window.
        r.event(key(KeyCode::BTN_MODE, Press::Down), now, &mut out);
        r.event(key(KeyCode::BTN_EAST, Press::Down), now, &mut out);
        assert_eq!(r.tick(now, &mut out), Some(now + QUIT_HOLD), "ticked at the hold's end");
        r.event(key(KeyCode::BTN_EAST, Press::Up), now + QUIT_HOLD / 2, &mut out);
        r.event(key(KeyCode::BTN_MODE, Press::Up), now + QUIT_HOLD / 2, &mut out);
        assert_eq!(actions(&out), vec![Event::Device(DeviceKey::CloseWindow)], "not Home as well");
        // Holding quits once the hold is reached, and letting go after that does nothing more.
        out.events.clear();
        r.event(key(KeyCode::BTN_MODE, Press::Down), now, &mut out);
        r.event(key(KeyCode::BTN_EAST, Press::Down), now, &mut out);
        r.tick(now + QUIT_HOLD, &mut out);
        r.event(key(KeyCode::BTN_EAST, Press::Up), now + QUIT_HOLD * 2, &mut out);
        r.event(key(KeyCode::BTN_MODE, Press::Up), now + QUIT_HOLD * 2, &mut out);
        assert_eq!(actions(&out), vec![Event::Device(DeviceKey::Quit)]);
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
        assert_eq!(actions(&out), vec![Event::Device(DeviceKey::Screenshot)], "not Home as well");
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
        // RB pressed in the meantime is the game's, as a Steam chord, not a screenshot.
        r.event(key(KeyCode::BTN_TR, Press::Down), t0, &mut out);
        assert_eq!(held(&out.pad), vec![KeyCode::BTN_TR, KeyCode::BTN_MODE]);
        // Back let go first puts Guide up. Home's release isn't Home, and Back's release isn't the game's.
        r.event(key(BACK, Press::Up), t0, &mut out);
        assert_eq!(held(&out.pad), vec![KeyCode::BTN_TR]);
        r.event(key(KeyCode::BTN_MODE, Press::Up), t0, &mut out);
        assert!(actions(&out).is_empty(), "not Home: {:?}", out.events);
        assert!(!out.pad.iter().any(|e| e.as_key().is_some_and(|(k, _)| k == BACK)), "Back never reaches the game");
        // Home let go first puts Guide up, and Back's release after that stays ours.
        out = Out::default();
        r.event(key(KeyCode::BTN_MODE, Press::Down), t0, &mut out);
        r.event(key(BACK, Press::Down), t0, &mut out);
        r.event(key(KeyCode::BTN_MODE, Press::Up), t0, &mut out);
        assert!(!held(&out.pad).contains(&KeyCode::BTN_MODE));
        let before = out.pad.len();
        r.event(key(BACK, Press::Up), t0, &mut out);
        assert_eq!(out.pad.len(), before);
        assert!(actions(&out).is_empty());
    }

    #[test]
    fn home_with_another_button_is_a_combination_not_home() {
        let (mut r, mut out) = router(PadRoute::Game);
        let t0 = Instant::now();
        r.event(key(KeyCode::BTN_MODE, Press::Down), t0, &mut out);
        r.event(key(KeyCode::BTN_SOUTH, Press::Down), t0, &mut out);
        r.event(key(KeyCode::BTN_MODE, Press::Up), t0, &mut out);
        r.event(key(KeyCode::BTN_SOUTH, Press::Up), t0, &mut out);
        assert!(actions(&out).is_empty(), "not Home");
        assert!(
            held(&out.pad).is_empty() && !out.pad.iter().any(|e| e.as_key().is_some_and(|(k, _)| k == KeyCode::BTN_SOUTH)),
            "A, pressed with Home held, never reaches the game: {:?}",
            out.pad
        );
        // Pressed again afterwards, it is the game's.
        r.event(key(KeyCode::BTN_SOUTH, Press::Down), t0, &mut out);
        assert_eq!(held(&out.pad), vec![KeyCode::BTN_SOUTH]);
        // Away from the game, Home then Back is neither Guide nor Home.
        let (mut r, mut out) = router(PadRoute::Ui);
        out.pad.clear();
        r.event(key(KeyCode::BTN_MODE, Press::Down), t0, &mut out);
        r.event(key(BACK, Press::Down), t0, &mut out);
        r.event(key(BACK, Press::Up), t0, &mut out);
        r.event(key(KeyCode::BTN_MODE, Press::Up), t0, &mut out);
        assert!(actions(&out).is_empty() && out.pad.is_empty(), "{out:?}");
        // Home alone, held long, is Home on release.
        r.event(key(KeyCode::BTN_MODE, Press::Down), t0, &mut out);
        r.event(key(KeyCode::BTN_MODE, Press::Up), t0 + std::time::Duration::from_secs(2), &mut out);
        assert_eq!(actions(&out), vec![Event::HomeTap]);
    }

    #[test]
    fn back_on_clamshells_screens_is_back_and_nothing_with_the_mouse() {
        let (mut r, mut out) = router(PadRoute::Ui);
        out.pad.clear();
        r.event(key(BACK, Press::Down), Instant::now(), &mut out);
        r.event(key(BACK, Press::Up), Instant::now(), &mut out);
        assert_eq!(actions(&out), vec![Event::Pad(PadInput::Back)]);
        assert!(out.pad.is_empty());
        // While a keymap's edit screen listens, Back is an input to remap.
        let (mut r, mut out) = router(PadRoute::Listen);
        out.pad.clear();
        r.event(key(BACK, Press::Down), Instant::now(), &mut out);
        r.event(Raw::REPORT, Instant::now(), &mut out);
        assert_eq!(actions(&out), vec![Event::Listened(clamshell_state::keymap::Input::Button(clamshell_state::keymap::Button::Back))]);
        let (mut r, mut out) = router(PadRoute::Mouse);
        out = Out { pad: vec![], ..out };
        r.event(key(BACK, Press::Down), Instant::now(), &mut out);
        assert!(actions(&out).is_empty() && out.mouse.is_empty() && out.pad.is_empty());
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
        assert!(actions(&out).is_empty(), "letting go afterwards isn't Home, nor back");
    }

    #[test]
    fn home_is_always_clamshells_and_view_with_the_mouse_or_typing() {
        let (mut r, mut out) = router(PadRoute::Game);
        let now = Instant::now();
        r.event(key(KeyCode::BTN_MODE, Press::Down), now, &mut out);
        assert!(actions(&out).is_empty(), "Home acts on release");
        r.event(key(KeyCode::BTN_MODE, Press::Up), now, &mut out);
        r.event(key(KeyCode::BTN_SELECT, Press::Down), now, &mut out);
        assert_eq!(actions(&out), vec![Event::HomeTap]);
        assert_eq!(out.pad, vec![key(KeyCode::BTN_SELECT, Press::Down), Raw::REPORT], "View is the game's");
        r.set_route(PadRoute::Mouse, &mut out);
        out = Out::default();
        r.event(key(KeyCode::BTN_SELECT, Press::Down), now, &mut out);
        r.event(key(KeyCode::BTN_SELECT, Press::Up), now, &mut out);
        assert_eq!(out, Out { events: vec![Event::Pad(PadInput::Switch)], ..Out::default() });
    }

    #[test]
    fn home_flicks_once_and_neutralises_the_games_stick_until_it_returns() {
        let (mut r, mut out) = router(PadRoute::Game);
        for code in [AbsoluteAxisCode::ABS_RX, AbsoluteAxisCode::ABS_RY] {
            r.snapshot.axes.push(Axis { code, value: 0, rest: 0 });
        }
        let now = Instant::now();
        r.event(Raw::abs(AbsoluteAxisCode::ABS_RX, 30000), now, &mut out);
        out = Out::default();
        r.event(key(KeyCode::BTN_MODE, Press::Down), now, &mut out);
        assert_eq!(out.events, vec![Event::HomeHeld(true)]);
        assert!(out.pad.contains(&Raw::abs(AbsoluteAxisCode::ABS_RX, 0)), "Home must centre the stick the game already saw: {:?}", out.pad);
        out.pad.clear();
        // Already off-centre when Home went down: it must return before this can be a flick.
        for value in [32000, 0, -30000, -32000, 10000, -30000, 0, 30000] {
            r.event(Raw::abs(AbsoluteAxisCode::ABS_RX, value), now, &mut out);
        }
        assert_eq!(out.events, vec![Event::HomeHeld(true), Event::Device(DeviceKey::WindowPrevious), Event::Device(DeviceKey::WindowNext)]);
        assert!(!out.pad.iter().any(|raw| raw.as_abs().is_some_and(|(axis, value)| axis == AbsoluteAxisCode::ABS_RX && value != 0)));
        r.event(key(KeyCode::BTN_MODE, Press::Up), now, &mut out);
        r.event(Raw::abs(AbsoluteAxisCode::ABS_RX, 31000), now, &mut out);
        assert!(!out.pad.iter().any(|raw| raw.as_abs().is_some_and(|(axis, value)| axis == AbsoluteAxisCode::ABS_RX && value != 0)));
        assert_eq!(out.events.last(), Some(&Event::HomeHeld(false)));
        r.event(Raw::abs(AbsoluteAxisCode::ABS_RX, 0), now, &mut out);
        r.event(Raw::abs(AbsoluteAxisCode::ABS_RX, 30000), now, &mut out);
        assert!(out.pad.contains(&Raw::abs(AbsoluteAxisCode::ABS_RX, 30000)));
        assert_eq!(out.events.last(), Some(&Event::HomeHeld(false)));
    }

    #[test]
    fn losing_the_controller_releases_home_for_the_state() {
        let (mut r, mut out) = router(PadRoute::Game);
        r.event(key(KeyCode::BTN_MODE, Press::Down), Instant::now(), &mut out);
        r.lose(&mut out);
        assert_eq!(out.events, vec![Event::HomeHeld(true), Event::HomeHeld(false)]);
    }
}
