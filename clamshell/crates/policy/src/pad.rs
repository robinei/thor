//! The controller path: the real Xbox-style controller is grabbed for good, and games get a virtual Xbox
//! 360 pad that is always there. Where its events go is the state's choice ([`PadRoute`]): the game's
//! virtual pad (copied straight through); the pad mouse (a virtual mouse: left stick moves the pointer,
//! right stick scrolls, A or the right trigger clicks left, B right, X middle, the D-pad sends arrow
//! keys); ClamShell's screens ([`Event::Pad`]: D-pad or left stick move focus, repeated while held; A,
//! B, Y, the shoulders, View); or typing into an app (D-pad arrows, A Enter, B Backspace). With the pad
//! mouse or typing, View goes to ClamShell, to move the pad to the second screen. Away from the game the
//! virtual pad shows a neutral controller, so games never see it disappear. The home button (BTN_MODE)
//! is ours: games never see it, it goes to the program as [`Event::Home`]. The virtual devices outlive
//! the controller: unplugged (or failing), the pad stays, at rest, until one is back. Button layers
//! (remaps) go in this same path later.
//!
//! Not yet: forwarding rumble from the virtual pad to the real one (uinput force-feedback uploads).

use std::io;
use std::sync::mpsc::{Receiver, TryRecvError};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use evdev::raw_stream::RawDevice;
use evdev::uinput::VirtualDevice;
use clamshell_core::{Event, PadInput, PadRoute};
use evdev::{AbsInfo, AbsoluteAxisCode, AttributeSet, BusType, EventType, InputEvent, InputId, KeyCode, RelativeAxisCode, UinputAbsSetup};

use crate::Sink;

/// Stick deflection below this (of full) is ignored: sticks don't rest exactly at zero.
const DEADZONE: f32 = 0.15;
/// Pointer speed at full deflection, pixels per second; the curve is quadratic below that, so small
/// deflections give fine control.
const MAX_SPEED: f32 = 1800.0;
/// Scroll speed at full deflection, wheel notches per second.
const MAX_SCROLL: f32 = 18.0;
/// High-resolution wheel units per notch (the kernel's convention).
const HI_RES_NOTCH: f32 = 120.0;
/// A trigger pressed past this (of full) is a held button.
const TRIGGER: f32 = 0.5;
/// How often motion is sent while a stick is deflected (125 Hz, a common mouse rate).
const MOTION_EVERY: Duration = Duration::from_millis(8);

const EV_SYN: u16 = EventType::SYNCHRONIZATION.0;
const EV_KEY: u16 = EventType::KEY.0;
const EV_REL: u16 = EventType::RELATIVE.0;
const EV_ABS: u16 = EventType::ABSOLUTE.0;

/// A raw event for the virtual mouse.
pub type Out = (u16, u16, i32);

/// Normalises an axis to -1..1 (sticks) or 0..1 (triggers) from its raw range.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Range {
    pub min: i32,
    pub max: i32,
}

impl Range {
    fn centered(self, v: i32) -> f32 {
        let mid = (self.min as f32 + self.max as f32) / 2.0;
        let half = ((self.max - self.min) as f32 / 2.0).max(1.0);
        ((v as f32 - mid) / half).clamp(-1.0, 1.0)
    }

    fn unit(self, v: i32) -> f32 {
        ((v - self.min) as f32 / (self.max - self.min).max(1) as f32).clamp(0.0, 1.0)
    }
}

/// What games get from the pad: the face buttons in Nintendo layout (A/B and X/Y swapped), a stick
/// deadzone (percent; the rest of the range stretched to full).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Shaping {
    pub nintendo: bool,
    pub deadzone: u16,
}

impl Shaping {
    /// The code and value a game gets for this event.
    pub fn event(self, stick: Range, kind: u16, code: u16, value: i32) -> (u16, i32) {
        if kind == EV_KEY && self.nintendo {
            let swapped = match KeyCode(code) {
                KeyCode::BTN_SOUTH => KeyCode::BTN_EAST,
                KeyCode::BTN_EAST => KeyCode::BTN_SOUTH,
                KeyCode::BTN_NORTH => KeyCode::BTN_WEST,
                KeyCode::BTN_WEST => KeyCode::BTN_NORTH,
                other => other,
            };
            return (swapped.0, value);
        }
        let is_stick = [AbsoluteAxisCode::ABS_X, AbsoluteAxisCode::ABS_Y, AbsoluteAxisCode::ABS_RX, AbsoluteAxisCode::ABS_RY]
            .iter()
            .any(|a| a.0 == code);
        if kind != EV_ABS || !is_stick || self.deadzone == 0 {
            return (code, value);
        }
        let dz = f32::from(self.deadzone.min(90)) / 100.0;
        let c = stick.centered(value);
        let shaped = if c.abs() < dz { 0.0 } else { c.signum() * (c.abs() - dz) / (1.0 - dz) };
        // The resting value: 0 for -32768..32767, 128 for 0..255.
        let mid = (stick.min + stick.max + 1) / 2;
        let half = if shaped < 0.0 { mid - stick.min } else { stick.max - mid };
        (code, mid + (shaped * half as f32).round() as i32)
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Ranges {
    pub stick: Range,
    pub trigger: Range,
}

/// The pad-to-mouse mapping. Pure: pad events in, mouse events out, motion on `tick`.
#[derive(Debug)]
pub struct PadMouse {
    ranges: Ranges,
    left: (f32, f32),
    right: (f32, f32),
    trigger_down: bool,
    /// Fractions of a pixel and of a wheel unit not yet sent.
    carry: (f32, f32),
    scroll_carry: (f32, f32),
}

/// A stick vector after the radial deadzone and the curve: 0 at the deadzone's edge, 1 at full.
fn shape((x, y): (f32, f32)) -> (f32, f32) {
    let len = x.hypot(y);
    if len <= DEADZONE {
        return (0.0, 0.0);
    }
    let scaled = ((len.min(1.0) - DEADZONE) / (1.0 - DEADZONE)).powi(2);
    (x / len * scaled, y / len * scaled)
}

impl PadMouse {
    pub fn new(ranges: Ranges) -> Self {
        PadMouse { ranges, left: (0.0, 0.0), right: (0.0, 0.0), trigger_down: false, carry: (0.0, 0.0), scroll_carry: (0.0, 0.0) }
    }

    /// Whether a stick is out of its deadzone (motion needs ticks).
    pub fn moving(&self) -> bool {
        shape(self.left) != (0.0, 0.0) || shape(self.right) != (0.0, 0.0)
    }

    /// One pad event; buttons map at once.
    pub fn event(&mut self, kind: u16, code: u16, value: i32, out: &mut Vec<Out>) {
        let button = |out: &mut Vec<Out>, code: KeyCode| {
            out.push((EV_KEY, code.0, value.clamp(0, 1)));
            out.push((EV_SYN, 0, 0));
        };
        match (kind, code) {
            (EV_ABS, c) if c == AbsoluteAxisCode::ABS_X.0 => self.left.0 = self.ranges.stick.centered(value),
            (EV_ABS, c) if c == AbsoluteAxisCode::ABS_Y.0 => self.left.1 = self.ranges.stick.centered(value),
            (EV_ABS, c) if c == AbsoluteAxisCode::ABS_RX.0 => self.right.0 = self.ranges.stick.centered(value),
            (EV_ABS, c) if c == AbsoluteAxisCode::ABS_RY.0 => self.right.1 = self.ranges.stick.centered(value),
            (EV_ABS, c) if c == AbsoluteAxisCode::ABS_RZ.0 => {
                let down = self.ranges.trigger.unit(value) > TRIGGER;
                if down != self.trigger_down {
                    self.trigger_down = down;
                    out.push((EV_KEY, KeyCode::BTN_LEFT.0, i32::from(down)));
                    out.push((EV_SYN, 0, 0));
                }
            }
            (EV_ABS, c) if c == AbsoluteAxisCode::ABS_HAT0X.0 || c == AbsoluteAxisCode::ABS_HAT0Y.0 => {
                let (minus, plus) = if c == AbsoluteAxisCode::ABS_HAT0X.0 {
                    (KeyCode::KEY_LEFT, KeyCode::KEY_RIGHT)
                } else {
                    (KeyCode::KEY_UP, KeyCode::KEY_DOWN)
                };
                // The hat reports -1, 0 or 1; releasing reports 0 for both directions.
                out.push((EV_KEY, minus.0, i32::from(value < 0)));
                out.push((EV_KEY, plus.0, i32::from(value > 0)));
                out.push((EV_SYN, 0, 0));
            }
            (EV_KEY, c) if c == KeyCode::BTN_SOUTH.0 => button(out, KeyCode::BTN_LEFT),
            (EV_KEY, c) if c == KeyCode::BTN_EAST.0 => button(out, KeyCode::BTN_RIGHT),
            (EV_KEY, c) if c == KeyCode::BTN_NORTH.0 => button(out, KeyCode::BTN_MIDDLE),
            _ => {}
        }
    }

    /// Releases everything the mapping holds down (when the pad mouse turns off).
    pub fn reset(&mut self, out: &mut Vec<Out>) {
        for code in [KeyCode::BTN_LEFT, KeyCode::BTN_RIGHT, KeyCode::BTN_MIDDLE, KeyCode::KEY_UP, KeyCode::KEY_DOWN, KeyCode::KEY_LEFT, KeyCode::KEY_RIGHT] {
            out.push((EV_KEY, code.0, 0));
        }
        out.push((EV_SYN, 0, 0));
        *self = PadMouse::new(self.ranges);
    }

    /// Motion for `dt` of stick deflection: pointer from the left stick, scrolling from the right.
    pub fn tick(&mut self, dt: Duration, out: &mut Vec<Out>) {
        let secs = dt.as_secs_f32();
        let (mx, my) = shape(self.left);
        self.carry.0 += mx * MAX_SPEED * secs;
        self.carry.1 += my * MAX_SPEED * secs;
        let (dx, dy) = (self.carry.0.trunc(), self.carry.1.trunc());
        self.carry = (self.carry.0 - dx, self.carry.1 - dy);

        let (sx, sy) = shape(self.right);
        // Stick down (positive y) scrolls down, which is a negative wheel value.
        self.scroll_carry.0 += sx * MAX_SCROLL * HI_RES_NOTCH * secs;
        self.scroll_carry.1 -= sy * MAX_SCROLL * HI_RES_NOTCH * secs;
        let (hx, hy) = (self.scroll_carry.0.trunc(), self.scroll_carry.1.trunc());
        self.scroll_carry = (self.scroll_carry.0 - hx, self.scroll_carry.1 - hy);

        let start = out.len();
        if dx != 0.0 {
            out.push((EV_REL, RelativeAxisCode::REL_X.0, dx as i32));
        }
        if dy != 0.0 {
            out.push((EV_REL, RelativeAxisCode::REL_Y.0, dy as i32));
        }
        if hy != 0.0 {
            out.push((EV_REL, RelativeAxisCode::REL_WHEEL_HI_RES.0, hy as i32));
        }
        if hx != 0.0 {
            out.push((EV_REL, RelativeAxisCode::REL_HWHEEL_HI_RES.0, hx as i32));
        }
        if out.len() > start {
            out.push((EV_SYN, 0, 0));
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Command {
    Route(PadRoute),
    Shape(Shaping),
}

/// A direction held this long starts repeating, this often.
const REPEAT_AFTER: Duration = Duration::from_millis(350);
const REPEAT_EVERY: Duration = Duration::from_millis(90);
/// The left stick counts as a direction past this (of full).
const STICK_DIRECTION: f32 = 0.6;

/// The pad driving ClamShell: buttons as they come, the D-pad and left stick as directions that repeat
/// while held.
#[derive(Debug)]
pub struct Navigator {
    stick_range: Range,
    hat: (i32, i32),
    stick: (f32, f32),
    held: Option<PadInput>,
    next: Option<Instant>,
}

impl Navigator {
    pub fn new(stick_range: Range) -> Self {
        Navigator { stick_range, hat: (0, 0), stick: (0.0, 0.0), held: None, next: None }
    }

    /// The direction held now: the D-pad's, else the stick's (its larger axis).
    fn direction(&self) -> Option<PadInput> {
        let from = |(x, y): (f32, f32), threshold: f32| match (x.abs() >= y.abs(), x, y) {
            (true, x, _) if x <= -threshold => Some(PadInput::Left),
            (true, x, _) if x >= threshold => Some(PadInput::Right),
            (false, _, y) if y <= -threshold => Some(PadInput::Up),
            (false, _, y) if y >= threshold => Some(PadInput::Down),
            _ => None,
        };
        from((self.hat.0 as f32, self.hat.1 as f32), 0.5).or_else(|| from(self.stick, STICK_DIRECTION))
    }

    /// One pad event at `now`; what it means goes to `out`.
    pub fn event(&mut self, kind: u16, code: u16, value: i32, now: Instant, out: &mut Vec<PadInput>) {
        match (kind, code) {
            (EV_ABS, c) if c == AbsoluteAxisCode::ABS_HAT0X.0 => self.hat.0 = value.signum(),
            (EV_ABS, c) if c == AbsoluteAxisCode::ABS_HAT0Y.0 => self.hat.1 = value.signum(),
            (EV_ABS, c) if c == AbsoluteAxisCode::ABS_X.0 => self.stick.0 = self.stick_range.centered(value),
            (EV_ABS, c) if c == AbsoluteAxisCode::ABS_Y.0 => self.stick.1 = self.stick_range.centered(value),
            (EV_KEY, c) if c == KeyCode::BTN_SOUTH.0 && value != 2 => out.push(PadInput::A(value == 1)),
            // The other buttons act when pressed (not on release or autorepeat).
            (EV_KEY, _) if value != 1 => {}
            (EV_KEY, c) if c == KeyCode::BTN_EAST.0 => out.push(PadInput::B),
            (EV_KEY, c) if c == KeyCode::BTN_NORTH.0 => out.push(PadInput::Y),
            (EV_KEY, c) if c == KeyCode::BTN_TL.0 => out.push(PadInput::Shoulder(-1)),
            (EV_KEY, c) if c == KeyCode::BTN_TR.0 => out.push(PadInput::Shoulder(1)),
            (EV_KEY, c) if c == KeyCode::BTN_SELECT.0 => out.push(PadInput::Switch),
            _ => {}
        }
        let direction = self.direction();
        if direction != self.held {
            self.held = direction;
            self.next = direction.map(|d| {
                out.push(d);
                now + REPEAT_AFTER
            });
        }
    }

    /// Repeats a held direction when due; returns when to call again, if one is held.
    pub fn tick(&mut self, now: Instant, out: &mut Vec<PadInput>) -> Option<Instant> {
        let (Some(direction), Some(next)) = (self.held, self.next) else {
            return None;
        };
        if now >= next {
            out.push(direction);
            self.next = Some(now + REPEAT_EVERY);
        }
        self.next
    }

    /// Lets go of everything (the pad goes elsewhere).
    pub fn reset(&mut self) {
        *self = Navigator::new(self.stick_range);
    }
}

/// The keys the pad types with (on the pad mouse's device).
const TYPING: [KeyCode; 6] = [KeyCode::KEY_UP, KeyCode::KEY_DOWN, KeyCode::KEY_LEFT, KeyCode::KEY_RIGHT, KeyCode::KEY_ENTER, KeyCode::KEY_BACKSPACE];

/// One pad event while the pad types: the D-pad as arrows, A as Enter, B as Backspace.
pub fn typing(kind: u16, code: u16, value: i32, out: &mut Vec<Out>) {
    match (kind, code) {
        (EV_ABS, c) if c == AbsoluteAxisCode::ABS_HAT0X.0 || c == AbsoluteAxisCode::ABS_HAT0Y.0 => {
            let (minus, plus) = if c == AbsoluteAxisCode::ABS_HAT0X.0 { (KeyCode::KEY_LEFT, KeyCode::KEY_RIGHT) } else { (KeyCode::KEY_UP, KeyCode::KEY_DOWN) };
            out.push((EV_KEY, minus.0, i32::from(value < 0)));
            out.push((EV_KEY, plus.0, i32::from(value > 0)));
            out.push((EV_SYN, 0, 0));
        }
        (EV_KEY, c) if c == KeyCode::BTN_SOUTH.0 || c == KeyCode::BTN_EAST.0 => {
            let key = if c == KeyCode::BTN_SOUTH.0 { KeyCode::KEY_ENTER } else { KeyCode::KEY_BACKSPACE };
            // 2 is the kernel's autorepeat: passed on, so a held B keeps deleting.
            out.push((EV_KEY, key.0, value.clamp(0, 2)));
            out.push((EV_SYN, 0, 0));
        }
        _ => {}
    }
}

/// What a route means for the pad's events.
trait Route {
    /// Away from games: the virtual pad shows a neutral controller.
    fn away(self) -> bool;
    fn mouse(self) -> bool;
    fn typing(self) -> bool;
    fn navigating(self) -> bool;
}

impl Route for PadRoute {
    fn away(self) -> bool {
        self != PadRoute::Game
    }

    fn mouse(self) -> bool {
        self == PadRoute::Mouse
    }

    fn typing(self) -> bool {
        self == PadRoute::Keys
    }

    fn navigating(self) -> bool {
        self == PadRoute::Ui
    }
}

/// The virtual devices, kept across the controller's comings and goings (an unplug, an error), so games
/// never see their pad go: made for the first controller, made again only for one with other buttons.
struct Virtual {
    /// The controller they were made for: its buttons and axes.
    made_for: (Vec<u16>, Vec<(AbsoluteAxisCode, AbsInfo)>),
    pad: VirtualDevice,
    mouse: VirtualDevice,
    /// The pad untouched: every button up, every axis at rest (shown while no controller is there).
    neutral: Vec<InputEvent>,
}

/// Owns the controller whenever one is connected (`wanted`: its /dev/input path or part of its name;
/// `None`: the first device that looks like a gamepad), looking again every second when there is none.
pub fn spawn(wanted: Option<String>, commands: Receiver<Command>, sink: impl Sink) -> io::Result<JoinHandle<()>> {
    thread::Builder::new().name("pad".into()).spawn(move || {
        let mut route = PadRoute::default();
        let mut shaping = Shaping::default();
        let mut devices: Option<Virtual> = None;
        // The first failure in a row is worth a warning; the retries each second aren't.
        let mut warned = false;
        loop {
            let mut grabbed = false;
            match run(wanted.as_deref(), &commands, &sink, &mut route, &mut shaping, &mut devices, &mut grabbed) {
                Ok(()) => return,
                // (No controller is a normal state, not a failure.)
                Err(e) if e.kind() != io::ErrorKind::NotFound && (grabbed || !warned) => {
                    log::warn!("pad: {e}");
                    warned = true;
                }
                Err(e) => log::debug!("pad: {e}"),
            }
            if grabbed {
                warned = false;
                // The controller is gone: the virtual pad stays, at rest.
                if let Some(v) = devices.as_mut()
                    && let Err(e) = v.pad.emit(&v.neutral)
                {
                    log::warn!("pad: {e}");
                }
            }
            match commands.recv_timeout(Duration::from_secs(1)) {
                Ok(Command::Shape(s)) => shaping = s,
                Ok(Command::Route(r)) => route = r,
                Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {}
                Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => return,
            }
        }
    })
}

/// The controller's current state, to blank the virtual pad (pad mouse on) and restore it (off).
struct Held {
    keys: Vec<u16>,
    /// (code, value, neutral value) per axis.
    axes: Vec<(u16, i32, i32)>,
}

impl Held {
    fn update(&mut self, kind: u16, code: u16, value: i32) {
        if kind == EV_KEY {
            self.keys.retain(|&k| k != code);
            if value != 0 {
                self.keys.push(code);
            }
        } else if kind == EV_ABS
            && let Some(axis) = self.axes.iter_mut().find(|a| a.0 == code)
        {
            axis.1 = value;
        }
    }

    /// Events that make a pad look like this state (`real`) or like an untouched one.
    fn events(&self, real: bool, out: &mut Vec<InputEvent>) {
        for &k in &self.keys {
            out.push(InputEvent::new(EV_KEY, k, i32::from(real)));
        }
        for &(code, value, neutral) in &self.axes {
            out.push(InputEvent::new(EV_ABS, code, if real { value } else { neutral }));
        }
        out.push(InputEvent::new(EV_SYN, 0, 0));
    }
}

/// Runs while the controller is connected: Ok when the program is going away, Err when the controller
/// is gone (or none was found).
fn run(
    wanted: Option<&str>,
    commands: &Receiver<Command>,
    sink: &impl Sink,
    routes: &mut PadRoute,
    shaping: &mut Shaping,
    devices: &mut Option<Virtual>,
    grabbed: &mut bool,
) -> io::Result<()> {
    let mut real = find(wanted)?;
    let abs = real.get_abs_state()?;
    // Each supported axis with its current value and range.
    let axes: Vec<(AbsoluteAxisCode, AbsInfo)> = real
        .supported_absolute_axes()
        .map(|set| {
            set.iter()
                .filter_map(|code| {
                    abs.get(usize::from(code.0))
                        .map(|a| (code, AbsInfo::new(a.value, a.minimum, a.maximum, a.fuzz, a.flat, a.resolution)))
                })
                .collect()
        })
        .unwrap_or_default();
    let range = |code: AbsoluteAxisCode| {
        axes.iter()
            .find(|(c, _)| *c == code)
            .map_or(Range { min: -32768, max: 32767 }, |(_, a)| Range { min: a.minimum(), max: a.maximum() })
    };
    let ranges = Ranges { stick: range(AbsoluteAxisCode::ABS_X), trigger: range(AbsoluteAxisCode::ABS_RZ) };
    let mut held = Held {
        keys: Vec::new(),
        axes: axes
            .iter()
            .map(|(code, a)| (code.0, a.value(), if a.minimum() < 0 { 0 } else { a.minimum() }))
            .collect(),
    };
    let keys: Vec<u16> = real.supported_keys().map(|k| k.iter().map(|c| c.0).collect()).unwrap_or_default();
    let made_for = (keys, axes.clone());
    if devices.as_ref().is_none_or(|v| v.made_for != made_for) {
        let mut neutral: Vec<InputEvent> = made_for.0.iter().map(|&k| InputEvent::new(EV_KEY, k, 0)).collect();
        neutral.extend(held.axes.iter().map(|&(code, _, rest)| InputEvent::new(EV_ABS, code, rest)));
        neutral.push(InputEvent::new(EV_SYN, 0, 0));
        *devices = Some(Virtual { pad: virtual_pad(&real, &axes)?, mouse: virtual_mouse()?, made_for, neutral });
    }
    let Some(Virtual { pad: virtual_pad, mouse, .. }) = devices.as_mut() else {
        return Err(io::Error::other("no virtual devices"));
    };
    real.grab()?;
    *grabbed = true;
    let flags = rustix::fs::fcntl_getfl(&real)?;
    rustix::fs::fcntl_setfl(&real, flags | rustix::fs::OFlags::NONBLOCK)?;
    log::info!("pad: {} grabbed; games see the virtual pad", real.name().unwrap_or("controller"));

    let mut mapping = PadMouse::new(ranges);
    let mut navigator = Navigator::new(ranges.stick);
    let mut inputs = Vec::new();
    let mut out = Vec::new();
    let mut to_pad = Vec::new();
    let mut to_mouse = Vec::new();
    let mut last = Instant::now();
    if routes.away() {
        held.events(false, &mut to_pad);
    }
    loop {
        loop {
            match commands.try_recv() {
                Ok(Command::Shape(s)) => *shaping = s,
                Ok(Command::Route(route)) => {
                    let before = *routes;
                    *routes = route;
                    if before.mouse() && !routes.mouse() {
                        mapping.reset(&mut out);
                    }
                    if before.typing() && !routes.typing() {
                        out.extend(TYPING.iter().map(|k| (EV_KEY, k.0, 0)));
                        out.push((EV_SYN, 0, 0));
                    }
                    if before.navigating() && !routes.navigating() {
                        navigator.reset();
                    }
                    if before.away() != routes.away() {
                        held.events(!routes.away(), &mut to_pad);
                    }
                    if before != *routes {
                        log::info!("pad: {routes:?}");
                    }
                }
                Err(TryRecvError::Empty) => break,
                Err(TryRecvError::Disconnected) => return Ok(()),
            }
        }
        // Wake at the motion rate while a stick is deflected; otherwise for events (or to check commands).
        let repeat_in = navigator.next.map(|t| t.saturating_duration_since(Instant::now()));
        let wait = if routes.mouse() && mapping.moving() { MOTION_EVERY } else { repeat_in.unwrap_or(Duration::from_millis(100)).min(Duration::from_millis(100)) };
        let timeout = rustix::event::Timespec { tv_sec: 0, tv_nsec: i64::from(wait.subsec_nanos()) };
        let mut fds = [rustix::event::PollFd::new(&real, rustix::event::PollFlags::IN)];
        match rustix::event::poll(&mut fds, Some(&timeout)) {
            // (Interrupted by a signal: just again.)
            Ok(_) | Err(rustix::io::Errno::INTR) => {}
            Err(e) => return Err(e.into()),
        }
        match real.fetch_events() {
            Ok(events) => {
                for e in events {
                    let (kind, code, value) = (e.event_type().0, e.code(), e.value());
                    if kind == EV_KEY && code == KeyCode::BTN_MODE.0 {
                        if value == 1 && !sink.send(Event::Home) {
                            return Ok(());
                        }
                        continue;
                    }
                    held.update(kind, code, value);
                    // View, with the pad mouse or typing: to ClamShell (the pad to the second screen).
                    if kind == EV_KEY && code == KeyCode::BTN_SELECT.0 && (routes.mouse() || routes.typing()) {
                        if value == 1 && !sink.send(Event::Pad(PadInput::Switch)) {
                            return Ok(());
                        }
                        continue;
                    }
                    if routes.mouse() {
                        mapping.event(kind, code, value, &mut out);
                    } else if routes.typing() {
                        typing(kind, code, value, &mut out);
                    } else if routes.navigating() {
                        navigator.event(kind, code, value, Instant::now(), &mut inputs);
                    } else {
                        to_pad.push(e);
                    }
                }
            }
            Err(e) if e.kind() == io::ErrorKind::WouldBlock => {}
            Err(e) => return Err(e),
        }
        let now = Instant::now();
        if routes.navigating() {
            navigator.tick(now, &mut inputs);
        }
        for input in inputs.drain(..) {
            if !sink.send(Event::Pad(input)) {
                return Ok(());
            }
        }
        if routes.mouse() && now.duration_since(last) >= MOTION_EVERY {
            mapping.tick(now.duration_since(last), &mut out);
        }
        if now.duration_since(last) >= MOTION_EVERY {
            last = now;
        }
        to_mouse.extend(out.drain(..).map(|(k, c, v)| InputEvent::new(k, c, v)));
        if !to_pad.is_empty() {
            for e in &mut to_pad {
                let (code, value) = shaping.event(ranges.stick, e.event_type().0, e.code(), e.value());
                if (code, value) != (e.code(), e.value()) {
                    *e = InputEvent::new(e.event_type().0, code, value);
                }
            }
            virtual_pad.emit(&to_pad)?;
            to_pad.clear();
        }
        if !to_mouse.is_empty() {
            mouse.emit(&to_mouse)?;
            to_mouse.clear();
        }
    }
}

/// The configured controller, or the first device with gamepad buttons and two sticks (not our own).
fn find(wanted: Option<&str>) -> io::Result<RawDevice> {
    if let Some(path) = wanted.filter(|w| w.starts_with('/')) {
        return RawDevice::open(path);
    }
    let gamepad = |d: &RawDevice| {
        d.name() != Some(VIRTUAL_PAD)
            && d.supported_keys().is_some_and(|k| k.contains(KeyCode::BTN_SOUTH))
            && d.supported_absolute_axes().is_some_and(|a| a.contains(AbsoluteAxisCode::ABS_RX))
    };
    evdev::raw_stream::enumerate()
        .map(|(_, d)| d)
        .find(|d| match wanted {
            Some(name) => d.name().is_some_and(|n| n.contains(name)) && d.name() != Some(VIRTUAL_PAD),
            None => gamepad(d),
        })
        .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "no controller"))
}

const VIRTUAL_PAD: &str = "clamshell pad (Xbox 360)";

/// A pad with the real one's buttons and axes, identified as a wired Xbox 360 controller so SDL, Wine
/// and Steam map it without configuration.
fn virtual_pad(real: &RawDevice, axes: &[(AbsoluteAxisCode, AbsInfo)]) -> io::Result<VirtualDevice> {
    let mut builder = VirtualDevice::builder()?.name(VIRTUAL_PAD).input_id(InputId::new(BusType::BUS_USB, 0x045e, 0x028e, 0x0110));
    if let Some(keys) = real.supported_keys() {
        builder = builder.with_keys(keys)?;
    }
    for &(code, info) in axes {
        builder = builder.with_absolute_axis(&UinputAbsSetup::new(code, info))?;
    }
    builder.build()
}

fn virtual_mouse() -> io::Result<VirtualDevice> {
    let keys: AttributeSet<KeyCode> = [
        KeyCode::BTN_LEFT,
        KeyCode::BTN_RIGHT,
        KeyCode::BTN_MIDDLE,
        KeyCode::KEY_UP,
        KeyCode::KEY_DOWN,
        KeyCode::KEY_LEFT,
        KeyCode::KEY_RIGHT,
        KeyCode::KEY_ENTER,
        KeyCode::KEY_BACKSPACE,
    ]
    .into_iter()
    .collect();
    let rel: AttributeSet<RelativeAxisCode> = [
        RelativeAxisCode::REL_X,
        RelativeAxisCode::REL_Y,
        RelativeAxisCode::REL_WHEEL,
        RelativeAxisCode::REL_HWHEEL,
        RelativeAxisCode::REL_WHEEL_HI_RES,
        RelativeAxisCode::REL_HWHEEL_HI_RES,
    ]
    .into_iter()
    .collect();
    VirtualDevice::builder()?.name("clamshell pad mouse").with_keys(&keys)?.with_relative_axes(&rel)?.build()
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::indexing_slicing)]
mod tests {
    use super::*;

    fn mouse() -> PadMouse {
        PadMouse::new(Ranges { stick: Range { min: -32768, max: 32767 }, trigger: Range { min: 0, max: 1023 } })
    }

    fn sum(out: &[Out], code: RelativeAxisCode) -> i32 {
        out.iter().filter(|(k, c, _)| *k == EV_REL && *c == code.0).map(|(_, _, v)| v).sum()
    }

    #[test]
    fn a_resting_stick_does_not_move_the_pointer() {
        let mut m = mouse();
        let mut out = Vec::new();
        m.event(EV_ABS, AbsoluteAxisCode::ABS_X.0, 3000, &mut out);
        assert!(!m.moving());
        m.tick(Duration::from_secs(1), &mut out);
        assert!(out.is_empty());
    }

    #[test]
    fn full_deflection_moves_at_max_speed() {
        let mut m = mouse();
        let mut out = Vec::new();
        m.event(EV_ABS, AbsoluteAxisCode::ABS_X.0, 32767, &mut out);
        for _ in 0..125 {
            m.tick(MOTION_EVERY, &mut out);
        }
        let moved = sum(&out, RelativeAxisCode::REL_X);
        assert!((moved - MAX_SPEED as i32).abs() <= 2, "moved {moved}");
        assert_eq!(sum(&out, RelativeAxisCode::REL_Y), 0);
    }

    #[test]
    fn half_deflection_is_much_slower_than_full() {
        let mut m = mouse();
        let mut out = Vec::new();
        m.event(EV_ABS, AbsoluteAxisCode::ABS_Y.0, 16384, &mut out);
        m.tick(Duration::from_secs(1), &mut out);
        let moved = sum(&out, RelativeAxisCode::REL_Y);
        assert!(moved > 0 && moved < MAX_SPEED as i32 / 3, "moved {moved}");
    }

    #[test]
    fn right_stick_down_scrolls_down() {
        let mut m = mouse();
        let mut out = Vec::new();
        m.event(EV_ABS, AbsoluteAxisCode::ABS_RY.0, 32767, &mut out);
        m.tick(Duration::from_secs(1), &mut out);
        assert_eq!(sum(&out, RelativeAxisCode::REL_WHEEL_HI_RES), -(MAX_SCROLL * HI_RES_NOTCH) as i32);
    }

    #[test]
    fn buttons_and_trigger_click() {
        let mut m = mouse();
        let mut out = Vec::new();
        m.event(EV_KEY, KeyCode::BTN_SOUTH.0, 1, &mut out);
        m.event(EV_KEY, KeyCode::BTN_EAST.0, 1, &mut out);
        assert_eq!(out[0], (EV_KEY, KeyCode::BTN_LEFT.0, 1));
        assert_eq!(out[2], (EV_KEY, KeyCode::BTN_RIGHT.0, 1));
        out.clear();
        m.event(EV_ABS, AbsoluteAxisCode::ABS_RZ.0, 900, &mut out);
        m.event(EV_ABS, AbsoluteAxisCode::ABS_RZ.0, 950, &mut out);
        assert_eq!(out, vec![(EV_KEY, KeyCode::BTN_LEFT.0, 1), (EV_SYN, 0, 0)], "one press, not one per report");
    }

    #[test]
    fn dpad_sends_arrows() {
        let mut m = mouse();
        let mut out = Vec::new();
        m.event(EV_ABS, AbsoluteAxisCode::ABS_HAT0Y.0, -1, &mut out);
        assert!(out.contains(&(EV_KEY, KeyCode::KEY_UP.0, 1)));
        out.clear();
        m.event(EV_ABS, AbsoluteAxisCode::ABS_HAT0Y.0, 0, &mut out);
        assert!(out.contains(&(EV_KEY, KeyCode::KEY_UP.0, 0)));
    }

    #[test]
    fn typing_maps_the_dpad_a_and_b() {
        let mut out = Vec::new();
        typing(EV_ABS, AbsoluteAxisCode::ABS_HAT0Y.0, -1, &mut out);
        assert_eq!(out, vec![(EV_KEY, KeyCode::KEY_UP.0, 1), (EV_KEY, KeyCode::KEY_DOWN.0, 0), (EV_SYN, 0, 0)]);
        out.clear();
        typing(EV_KEY, KeyCode::BTN_SOUTH.0, 1, &mut out);
        typing(EV_KEY, KeyCode::BTN_EAST.0, 2, &mut out);
        typing(EV_KEY, KeyCode::BTN_NORTH.0, 1, &mut out);
        assert_eq!(out, vec![(EV_KEY, KeyCode::KEY_ENTER.0, 1), (EV_SYN, 0, 0), (EV_KEY, KeyCode::KEY_BACKSPACE.0, 2), (EV_SYN, 0, 0)]);
        assert!(PadRoute::Keys.away() && PadRoute::Keys.typing());
        assert!(PadRoute::Mouse.away() && PadRoute::Mouse.mouse() && !PadRoute::Mouse.typing());
        assert!(!PadRoute::Game.away() && PadRoute::Ui.navigating());
    }

    #[test]
    fn navigating_repeats_held_directions_and_passes_buttons() {
        let mut nav = Navigator::new(Range { min: -32768, max: 32767 });
        let t0 = Instant::now();
        let mut out = Vec::new();
        nav.event(EV_ABS, AbsoluteAxisCode::ABS_HAT0Y.0, 1, t0, &mut out);
        assert_eq!(out, vec![PadInput::Down]);
        out.clear();
        assert!(nav.tick(t0 + Duration::from_millis(100), &mut out).is_some());
        assert!(out.is_empty(), "not yet");
        nav.tick(t0 + REPEAT_AFTER, &mut out);
        nav.tick(t0 + REPEAT_AFTER + REPEAT_EVERY, &mut out);
        assert_eq!(out, vec![PadInput::Down, PadInput::Down]);
        out.clear();
        nav.event(EV_ABS, AbsoluteAxisCode::ABS_HAT0Y.0, 0, t0, &mut out);
        assert_eq!(nav.tick(t0 + Duration::from_secs(5), &mut out), None);
        // The stick past its threshold, sideways.
        nav.event(EV_ABS, AbsoluteAxisCode::ABS_X.0, 30000, t0, &mut out);
        nav.event(EV_ABS, AbsoluteAxisCode::ABS_X.0, 0, t0, &mut out);
        for (code, value) in [(KeyCode::BTN_SOUTH, 1), (KeyCode::BTN_SOUTH, 0), (KeyCode::BTN_EAST, 1), (KeyCode::BTN_EAST, 0), (KeyCode::BTN_SELECT, 1), (KeyCode::BTN_TR, 1)] {
            nav.event(EV_KEY, code.0, value, t0, &mut out);
        }
        assert_eq!(out, vec![PadInput::Right, PadInput::A(true), PadInput::A(false), PadInput::B, PadInput::Switch, PadInput::Shoulder(1)]);
    }

    #[test]
    fn shaping_swaps_face_buttons_and_applies_the_deadzone() {
        let stick = Range { min: -32768, max: 32767 };
        let off = Shaping::default();
        assert_eq!(off.event(stick, EV_KEY, KeyCode::BTN_SOUTH.0, 1), (KeyCode::BTN_SOUTH.0, 1));
        let n = Shaping { nintendo: true, deadzone: 10 };
        assert_eq!(n.event(stick, EV_KEY, KeyCode::BTN_SOUTH.0, 1), (KeyCode::BTN_EAST.0, 1));
        assert_eq!(n.event(stick, EV_KEY, KeyCode::BTN_WEST.0, 0), (KeyCode::BTN_NORTH.0, 0));
        assert_eq!(n.event(stick, EV_KEY, KeyCode::BTN_START.0, 1), (KeyCode::BTN_START.0, 1));
        let x = AbsoluteAxisCode::ABS_X.0;
        assert_eq!(n.event(stick, EV_ABS, x, 3000).1, 0, "inside the deadzone: centred");
        assert_eq!(n.event(stick, EV_ABS, x, 32767).1, 32767, "full stays full");
        let half = n.event(stick, EV_ABS, x, 18000).1;
        assert!(half > 0 && half < 18000, "{half}: stretched from the deadzone's edge");
        assert_eq!(n.event(stick, EV_ABS, AbsoluteAxisCode::ABS_Z.0, 100), (AbsoluteAxisCode::ABS_Z.0, 100), "triggers as they are");
    }
}