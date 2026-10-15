//! Touchpad mode: the second screen as a laptop touchpad.
//!
//! While on, the touchscreen is grabbed (sway sees none of its touches). Each finger is decided at
//! touch-down: one that starts on a region the UI keeps (the bar) is a tap on that target (moving, on the
//! volume or brightness slider); any other is copied, multitouch events as they come, to a virtual
//! touchpad the size of the area below the bar (made again when that changes). sway's libinput then does what it does
//! for a laptop touchpad: pointer motion with acceleration, tap to click, two-finger scroll, gestures,
//! palm detection, all configurable with `input` lines. Our part is only this forwarding.

use std::io;
use std::sync::mpsc::{Receiver, TryRecvError};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::Instant;

use evdev::raw_stream::RawDevice;
use evdev::uinput::VirtualDevice;
use evdev::{AbsInfo, AbsoluteAxisCode, AttributeSet, EventType, InputEvent, KeyCode, PropType, UinputAbsSetup};
use clamshell_core::controls::{Control, SLIDE_AFTER, SLIDE_MOVE};
use clamshell_core::touch::TouchLayout;
use clamshell_core::{Event, Id};

use crate::Sink;

const EV_SYN: u16 = EventType::SYNCHRONIZATION.0;
const EV_KEY: u16 = EventType::KEY.0;
const EV_ABS: u16 = EventType::ABSOLUTE.0;
const SYN_REPORT: u16 = 0;
const ABS_X: u16 = AbsoluteAxisCode::ABS_X.0;
const ABS_Y: u16 = AbsoluteAxisCode::ABS_Y.0;
const MT_SLOT: u16 = AbsoluteAxisCode::ABS_MT_SLOT.0;
const MT_X: u16 = AbsoluteAxisCode::ABS_MT_POSITION_X.0;
const MT_Y: u16 = AbsoluteAxisCode::ABS_MT_POSITION_Y.0;
const MT_TRACKING_ID: u16 = AbsoluteAxisCode::ABS_MT_TRACKING_ID.0;
const BTN_TOUCH: u16 = KeyCode::BTN_TOUCH.0;
/// BTN_TOOL_FINGER, _DOUBLETAP, _TRIPLETAP, _QUADTAP: how many fingers are on the touchpad.
const TOOLS: [u16; 4] =
    [KeyCode::BTN_TOOL_FINGER.0, KeyCode::BTN_TOOL_DOUBLETAP.0, KeyCode::BTN_TOOL_TRIPLETAP.0, KeyCode::BTN_TOOL_QUADTAP.0];
const SLOTS: usize = 10;

/// What one input frame from the touchscreen becomes.
#[derive(Debug, Clone, PartialEq)]
pub enum Out {
    /// An event for the virtual touchpad (positions already in touchpad coordinates).
    Forward { kind: u16, code: u16, value: i32 },
    /// A finger went down on (or lifted from) a UI target, at this horizontal screen position.
    Tap { id: Id, down: bool, x: f32 },
    /// A finger on a UI target moved to this horizontal screen position.
    Move { x: f32 },
}

#[derive(Debug, Clone, Default, PartialEq)]
enum Owner {
    #[default]
    None,
    Pad,
    Ui(Id),
}

#[derive(Debug, Clone, Default)]
struct Touch {
    owner: Owner,
    x: i32,
    y: i32,
    began: bool,
    ended: bool,
    moved: bool,
}

/// The touchscreen's raw coordinate ranges, (min, max) per axis.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Axes {
    pub x: (i32, i32),
    pub y: (i32, i32),
}

/// Splits the touchscreen's multitouch stream (protocol B) between the UI and the virtual touchpad.
/// Pure: raw events in, `Out`s out.
#[derive(Debug)]
pub struct Router {
    axes: Axes,
    slot: usize,
    touches: Vec<Touch>,
    /// The events of the frame being read, until its SYN_REPORT.
    frame: Vec<(u16, u16, i32)>,
    fingers: usize,
    first: Option<(i32, i32)>,
}

impl Router {
    pub fn new(axes: Axes) -> Self {
        Router { axes, slot: 0, touches: vec![Touch::default(); SLOTS], frame: Vec::with_capacity(64), fingers: 0, first: None }
    }

    /// The UI targets fingers are on now.
    pub fn ui_touches(&self) -> impl Iterator<Item = &Id> {
        self.touches.iter().filter_map(|t| match &t.owner {
            Owner::Ui(id) if !t.ended => Some(id),
            _ => None,
        })
    }

    /// Whether a finger is on the touchpad part.
    pub fn pad_fingers(&self) -> bool {
        self.fingers > 0
    }

    fn to_screen(&self, x: i32, y: i32, layout: &TouchLayout) -> (f32, f32) {
        let span = |(lo, hi): (i32, i32), v: i32| (v - lo) as f32 / (hi - lo).max(1) as f32;
        (span(self.axes.x, x) * layout.screen.0, span(self.axes.y, y) * layout.screen.1)
    }

    /// A raw position as touchpad coordinates: screen pixels from the pad area's corner, inside it.
    fn to_pad(&self, x: i32, y: i32, layout: &TouchLayout) -> (i32, i32) {
        let (sx, sy) = self.to_screen(x, y, layout);
        let (x0, y0, x1, y1) = layout.pad;
        ((sx.clamp(x0, x1) - x0).round() as i32, (sy.clamp(y0, y1) - y0).round() as i32)
    }

    /// Feeds one raw event; a SYN_REPORT completes the frame and appends what it becomes to `out`.
    pub fn feed(&mut self, kind: u16, code: u16, value: i32, layout: &TouchLayout, out: &mut Vec<Out>) {
        match (kind, code) {
            (EV_SYN, SYN_REPORT) => self.flush(layout, out),
            // Touch keys and the single-touch axes are recomputed for the fingers we forward.
            (EV_KEY, _) | (EV_ABS, ABS_X | ABS_Y) | (EV_SYN, _) => {}
            _ => self.frame.push((kind, code, value)),
        }
    }

    fn flush(&mut self, layout: &TouchLayout, out: &mut Vec<Out>) {
        let start = out.len();
        // Apply the frame to the touches.
        let mut slot = self.slot;
        for &(kind, code, value) in &self.frame {
            if kind != EV_ABS {
                continue;
            }
            if code == MT_SLOT {
                slot = usize::try_from(value).unwrap_or(0);
                continue;
            }
            let Some(t) = self.touches.get_mut(slot) else {
                continue;
            };
            match code {
                MT_TRACKING_ID if value >= 0 => *t = Touch { began: true, x: t.x, y: t.y, ..Touch::default() },
                MT_TRACKING_ID => t.ended = true,
                MT_X => {
                    t.x = value;
                    t.moved = true;
                }
                MT_Y => t.y = value,
                _ => {}
            }
        }
        // A new finger belongs to the UI target under it, else to the touchpad (its position comes in
        // the same frame as its tracking id).
        for i in 0..self.touches.len() {
            let Some(t) = self.touches.get(i) else {
                continue;
            };
            if !t.began {
                continue;
            }
            let (sx, sy) = self.to_screen(t.x, t.y, layout);
            let owner = match layout.region_at(sx, sy) {
                Some(id) => {
                    out.push(Out::Tap { id: id.clone(), down: true, x: sx });
                    Owner::Ui(id.clone())
                }
                None => Owner::Pad,
            };
            if let Some(t) = self.touches.get_mut(i) {
                t.owner = owner;
                t.began = false;
                t.moved = false;
            }
        }
        // UI fingers that moved.
        for t in &mut self.touches {
            if std::mem::take(&mut t.moved) && matches!(t.owner, Owner::Ui(_)) && !t.ended {
                let span = |(lo, hi): (i32, i32), v: i32| (v - lo) as f32 / (hi - lo).max(1) as f32;
                out.push(Out::Move { x: span(self.axes.x, t.x) * layout.screen.0 });
            }
        }
        // Forward the touchpad fingers' events, in touchpad coordinates.
        let mut slot = self.slot;
        let mut forwarded_slot = None;
        for &(kind, code, value) in &self.frame {
            if kind == EV_ABS && code == MT_SLOT {
                slot = usize::try_from(value).unwrap_or(0);
                continue;
            }
            let Some(t) = self.touches.get(slot).filter(|t| t.owner == Owner::Pad) else {
                continue;
            };
            let value = match code {
                MT_X => self.to_pad(value, t.y, layout).0,
                MT_Y => self.to_pad(t.x, value, layout).1,
                _ => value,
            };
            if forwarded_slot != Some(slot) {
                out.push(Out::Forward { kind: EV_ABS, code: MT_SLOT, value: slot as i32 });
                forwarded_slot = Some(slot);
            }
            out.push(Out::Forward { kind, code, value });
        }
        self.slot = slot;
        self.frame.clear();
        // Lifted fingers.
        for t in &mut self.touches {
            if t.ended {
                if let Owner::Ui(id) = &t.owner {
                    out.push(Out::Tap { id: id.clone(), down: false, x: 0.0 });
                }
                *t = Touch::default();
            }
        }
        // BTN_TOUCH and the tool key for the number of touchpad fingers; ABS_X/Y follow the first one.
        let fingers = self.touches.iter().filter(|t| t.owner == Owner::Pad).count();
        if fingers != self.fingers {
            if (fingers > 0) != (self.fingers > 0) {
                out.push(Out::Forward { kind: EV_KEY, code: BTN_TOUCH, value: i32::from(fingers > 0) });
            }
            let tool = |n: usize| n.checked_sub(1).and_then(|i| TOOLS.get(i.min(TOOLS.len() - 1)).copied());
            if let Some(code) = tool(self.fingers) {
                out.push(Out::Forward { kind: EV_KEY, code, value: 0 });
            }
            if let Some(code) = tool(fingers) {
                out.push(Out::Forward { kind: EV_KEY, code, value: 1 });
            }
            self.fingers = fingers;
        }
        let first = self.touches.iter().find(|t| t.owner == Owner::Pad).map(|t| self.to_pad(t.x, t.y, layout));
        if let Some((x, y)) = first
            && self.first != first
        {
            out.push(Out::Forward { kind: EV_ABS, code: ABS_X, value: x });
            out.push(Out::Forward { kind: EV_ABS, code: ABS_Y, value: y });
        }
        self.first = first;
        if out.get(start..).is_some_and(|new| new.iter().any(|o| matches!(o, Out::Forward { .. }))) {
            out.push(Out::Forward { kind: EV_SYN, code: SYN_REPORT, value: 0 });
        }
    }
}

/// Starting and stopping touchpad mode (from ClamShell's mode changes).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Command {
    On,
    Off,
}

#[derive(Debug, Clone)]
pub struct Config {
    /// The second screen's touchscreen: a /dev/input path, or part of its name; `None`: the first
    /// direct-touch multitouch device.
    pub device: Option<String>,
    /// The second screen's pixels per millimetre, so libinput's speeds and thresholds match its size.
    pub px_per_mm: f32,
}

/// Runs touchpad mode whenever `On` arrives, until `Off`; taps on UI regions go to `sink`.
pub fn spawn(
    config: Config,
    layout: Arc<Mutex<TouchLayout>>,
    commands: Receiver<Command>,
    sink: impl Sink,
) -> io::Result<JoinHandle<()>> {
    thread::Builder::new().name("touchpad".into()).spawn(move || {
        loop {
            match commands.recv() {
                Ok(Command::On) => {}
                Ok(Command::Off) => continue,
                Err(_) => return,
            }
            log::info!("touchpad mode on");
            let mut session = Session::default();
            let result = run(&config, &layout, &commands, &sink, &mut session);
            // What the UI had under fingers ends (no tap): the targets let go, a slide ended.
            let mut events: Vec<Event> = session.router.iter().flat_map(Router::ui_touches).map(|id| Event::Up(id.clone())).collect();
            if session.bar_press.is_some_and(|p| p.sliding) {
                events.push(Event::SlideEnd);
            }
            // Whether it worked, for the touchpad tab to say.
            events.push(Event::TouchpadProblem(result.as_ref().err().map(|e| {
                log::warn!("touchpad mode: {e}");
                format!("No touchpad: {e}")
            })));
            if !events.into_iter().all(|e| sink.send(e)) {
                return;
            }
            log::info!("touchpad mode off");
        }
    })
}

/// What a run of touchpad mode leaves to clean up after it.
#[derive(Default)]
struct Session {
    router: Option<Router>,
    bar_press: Option<BarPress>,
}

/// The touchpad area (pixels), once the UI has laid it out (at start it may not have yet).
fn pad_area(layout: &Mutex<TouchLayout>) -> Option<(f32, f32, f32, f32)> {
    layout.lock().ok().map(|l| l.pad).filter(|(x0, y0, x1, y1)| x1 > x0 && y1 > y0)
}

fn run(config: &Config, layout: &Mutex<TouchLayout>, commands: &Receiver<Command>, sink: &impl Sink, session: &mut Session) -> io::Result<()> {
    let mut device = find(config.device.as_deref())?;
    let abs = device.get_abs_state()?;
    let range = |code: AbsoluteAxisCode| abs.get(usize::from(code.0)).map_or((0, 1), |a| (a.minimum, a.maximum));
    // The area, once drawn (a few frames at most).
    let waited = Instant::now();
    let mut pad = loop {
        if let Some(pad) = pad_area(layout) {
            break pad;
        }
        if waited.elapsed() > std::time::Duration::from_secs(3) {
            return Err(io::Error::other("the touchpad area wasn't laid out"));
        }
        if matches!(commands.try_recv(), Ok(Command::Off) | Err(TryRecvError::Disconnected)) {
            return Ok(());
        }
        thread::sleep(std::time::Duration::from_millis(20));
    };
    let mut touchpad = virtual_touchpad(pad, config.px_per_mm)?;
    device.grab()?;
    let flags = rustix::fs::fcntl_getfl(&device)?;
    rustix::fs::fcntl_setfl(&device, flags | rustix::fs::OFlags::NONBLOCK)?;
    if !sink.send(Event::TouchpadProblem(None)) {
        return Ok(());
    }
    let router = session.router.insert(Router::new(Axes { x: range(AbsoluteAxisCode::ABS_MT_POSITION_X), y: range(AbsoluteAxisCode::ABS_MT_POSITION_Y) }));
    let bar_press = &mut session.bar_press;

    let mut out = Vec::new();
    let mut batch = Vec::new();
    let mut events = Vec::new();
    let tick = rustix::event::Timespec { tv_sec: 0, tv_nsec: 100_000_000 };
    loop {
        match commands.try_recv() {
            Ok(Command::Off) | Err(TryRecvError::Disconnected) => return Ok(()),
            Ok(Command::On) | Err(TryRecvError::Empty) => {}
        }
        // The area changed (a new layout): a touchpad its new size, between touches.
        if let Some(now) = pad_area(layout).filter(|now| *now != pad)
            && !router.pad_fingers()
        {
            log::info!("touchpad mode: the area changed, the touchpad follows");
            touchpad = virtual_touchpad(now, config.px_per_mm)?;
            pad = now;
        }
        // Wakes for touches at once; the timeout only bounds how long an Off waits (interrupted: again).
        let mut fds = [rustix::event::PollFd::new(&device, rustix::event::PollFlags::IN)];
        match rustix::event::poll(&mut fds, Some(&tick)) {
            Ok(_) | Err(rustix::io::Errno::INTR) => {}
            Err(e) => return Err(e.into()),
        }
        {
            let layout = layout.lock().map_err(|_| io::Error::other("touch layout lock poisoned"))?;
            match device.fetch_events() {
                Ok(events) => {
                    for e in events {
                        router.feed(e.event_type().0, e.code(), e.value(), &layout, &mut out);
                    }
                }
                Err(e) if e.kind() == io::ErrorKind::WouldBlock => {}
                Err(e) => return Err(e),
            }
        }
        // Where the layout is, for positions on the sliders.
        let geometry = layout.lock().map(|l| TouchLayout { scale: l.scale, origin_x: l.origin_x, width: l.width, ..TouchLayout::default() }).ok();
        let width = geometry.as_ref().map_or(0.0, |g| g.width);
        let layout_x = geometry.as_ref().map(|g| |x: f32| g.layout_x(x));
        let now = Instant::now();
        // A finger held still on volume or brightness: the slider comes up.
        if let Some(press) = bar_press.as_mut() {
            press.start(now, false, width, &mut events);
        }
        for o in out.drain(..) {
            match o {
                Out::Forward { kind, code, value } => batch.push(InputEvent::new(kind, code, value)),
                Out::Tap { id, down: true, x } => {
                    log::debug!("touchpad mode: tap on {id:?}");
                    if !(sink.send(Event::HideFocus) && sink.send(Event::Down(id.clone()))) {
                        return Ok(());
                    }
                    if let (Id::Control(control), Some(at)) = (id, layout_x) {
                        *bar_press = Some(BarPress::new(control, at(x), now));
                    }
                }
                Out::Move { x } => {
                    if let (Some(press), Some(at)) = (bar_press.as_mut(), layout_x) {
                        press.moved(at(x), now, width, &mut events);
                    }
                }
                Out::Tap { id, down: false, .. } => {
                    let tap = bar_press.take().is_none_or(|p| p.tap(now));
                    if tap {
                        events.push(Event::Activate(id.clone()));
                    }
                    events.push(Event::Up(id));
                }
            }
        }
        for event in events.drain(..) {
            if !sink.send(event) {
                return Ok(());
            }
        }
        if !batch.is_empty() {
            touchpad.emit(&batch)?;
            batch.clear();
        }
    }
}

/// A finger on volume or brightness in the bar, as the UI treats one: a tap opens the tab; held
/// ([`SLIDE_AFTER`]) or moved sideways ([`SLIDE_MOVE`]) it becomes a slide, which doesn't end in a tap.
/// Positions are in layout units.
#[derive(Debug)]
struct BarPress {
    control: Control,
    down_x: f32,
    x: f32,
    at: Instant,
    sliding: bool,
}

impl BarPress {
    fn new(control: Control, x: f32, now: Instant) -> Self {
        BarPress { control, down_x: x, x, at: now, sliding: false }
    }

    /// The slide starting, if it is due (held long enough, or `force`d by a move).
    fn start(&mut self, now: Instant, force: bool, width: f32, out: &mut Vec<Event>) {
        if !self.sliding && (force || now.duration_since(self.at) >= SLIDE_AFTER) {
            self.sliding = true;
            out.push(Event::SlideStart { control: self.control, x: self.x, width });
        }
    }

    fn moved(&mut self, x: f32, now: Instant, width: f32, out: &mut Vec<Event>) {
        self.x = x;
        let was = self.sliding;
        self.start(now, (x - self.down_x).abs() >= SLIDE_MOVE, width, out);
        if was {
            out.push(Event::SlideTo { x });
        }
    }

    /// Lifted: whether it was a tap.
    fn tap(&self, now: Instant) -> bool {
        !self.sliding && now.duration_since(self.at) < SLIDE_AFTER
    }
}

/// The configured touchscreen, or the first direct-touch multitouch device.
fn find(wanted: Option<&str>) -> io::Result<RawDevice> {
    if let Some(path) = wanted.filter(|w| w.starts_with('/')) {
        return RawDevice::open(path);
    }
    let touchscreen = |d: &RawDevice| {
        d.properties().contains(PropType::DIRECT)
            && d.supported_absolute_axes().is_some_and(|a| a.contains(AbsoluteAxisCode::ABS_MT_POSITION_X))
    };
    evdev::raw_stream::enumerate()
        .map(|(_, d)| d)
        .find(|d| match wanted {
            Some(name) => d.name().is_some_and(|n| n.contains(name)),
            None => touchscreen(d),
        })
        .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, format!("no touchscreen ({})", wanted.unwrap_or("auto"))))
}

/// A touchpad the size of the pad area (in screen pixels, `px_per_mm` of them per millimetre), with
/// one dummy button so libinput takes it as a touchpad with buttons and taps for clicks.
fn virtual_touchpad((x0, y0, x1, y1): (f32, f32, f32, f32), px_per_mm: f32) -> io::Result<VirtualDevice> {
    let (w, h) = (((x1 - x0).max(1.0)) as i32, ((y1 - y0).max(1.0)) as i32);
    let res = px_per_mm.round().max(1.0) as i32;
    let axis = |code, max, res| UinputAbsSetup::new(code, AbsInfo::new(0, 0, max, 0, 0, res));
    let keys: AttributeSet<KeyCode> = [
        KeyCode::BTN_LEFT,
        KeyCode::BTN_TOUCH,
        KeyCode::BTN_TOOL_FINGER,
        KeyCode::BTN_TOOL_DOUBLETAP,
        KeyCode::BTN_TOOL_TRIPLETAP,
        KeyCode::BTN_TOOL_QUADTAP,
    ]
    .into_iter()
    .collect();
    let props: AttributeSet<PropType> = [PropType::POINTER].into_iter().collect();
    VirtualDevice::builder()?
        .name("clamshell touchpad")
        .with_properties(&props)?
        .with_keys(&keys)?
        .with_absolute_axis(&axis(AbsoluteAxisCode::ABS_X, w, res))?
        .with_absolute_axis(&axis(AbsoluteAxisCode::ABS_Y, h, res))?
        .with_absolute_axis(&axis(AbsoluteAxisCode::ABS_MT_SLOT, SLOTS as i32 - 1, 0))?
        .with_absolute_axis(&axis(AbsoluteAxisCode::ABS_MT_TRACKING_ID, 65535, 0))?
        .with_absolute_axis(&axis(AbsoluteAxisCode::ABS_MT_POSITION_X, w, res))?
        .with_absolute_axis(&axis(AbsoluteAxisCode::ABS_MT_POSITION_Y, h, res))?
        .build()
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::indexing_slicing)]
mod tests {
    use super::*;

    #[test]
    fn on_volume_a_tap_opens_the_tab_and_a_hold_or_slide_does_not() {
        let t0 = Instant::now();
        let ms = |n| t0 + std::time::Duration::from_millis(n);
        let mut out = Vec::new();
        // A quick tap.
        let press = BarPress::new(Control::Volume, 280.0, t0);
        assert!(press.tap(ms(120)));
        // Held still: the slider comes up, lifting isn't a tap.
        let mut press = BarPress::new(Control::Volume, 280.0, t0);
        press.start(ms(100), false, 620.0, &mut out);
        assert!(out.is_empty(), "not yet");
        press.start(ms(200), false, 620.0, &mut out);
        assert_eq!(out, vec![Event::SlideStart { control: Control::Volume, x: 280.0, width: 620.0 }]);
        assert!(!press.tap(ms(250)));
        // Slid sideways at once: the slider comes up where the finger is, and follows it.
        out.clear();
        let mut press = BarPress::new(Control::Brightness, 330.0, t0);
        press.moved(333.0, ms(20), 620.0, &mut out);
        assert!(out.is_empty(), "a small wobble is still a tap");
        press.moved(345.0, ms(40), 620.0, &mut out);
        press.moved(360.0, ms(60), 620.0, &mut out);
        assert_eq!(out, vec![Event::SlideStart { control: Control::Brightness, x: 345.0, width: 620.0 }, Event::SlideTo { x: 360.0 }]);
        assert!(!press.tap(ms(80)), "a quick slide isn't a tap either");
    }
    use clamshell_core::Mode;
    use clamshell_core::touch::Region;

    /// A 1000x1000 touchscreen on a 1000x800 screen: a bar region along the top, the pad below it.
    fn setup() -> (Router, TouchLayout) {
        let layout = TouchLayout {
            screen: (1000.0, 800.0),
            pad: (0.0, 100.0, 1000.0, 800.0),
            regions: vec![Region { x0: 0.0, y0: 0.0, x1: 100.0, y1: 100.0, id: Id::Mode(Mode::Touchpad) }],
            ..TouchLayout::default()
        };
        (Router::new(Axes { x: (0, 1000), y: (0, 1000) }), layout)
    }

    fn frame(r: &mut Router, l: &TouchLayout, events: &[(u16, u16, i32)]) -> Vec<Out> {
        let mut out = Vec::new();
        for &(k, c, v) in events {
            r.feed(k, c, v, l, &mut out);
        }
        r.feed(EV_SYN, SYN_REPORT, 0, l, &mut out);
        out
    }

    fn fwd(kind: u16, code: u16, value: i32) -> Out {
        Out::Forward { kind, code, value }
    }

    #[test]
    fn a_touch_on_the_bar_is_a_tap_not_forwarded() {
        let (mut r, l) = setup();
        // Raw (50, 50) is screen (50, 40): inside the bar region.
        let down = frame(&mut r, &l, &[(EV_ABS, MT_SLOT, 0), (EV_ABS, MT_TRACKING_ID, 7), (EV_ABS, MT_X, 50), (EV_ABS, MT_Y, 50), (EV_KEY, BTN_TOUCH, 1)]);
        assert_eq!(down, vec![Out::Tap { id: Id::Mode(Mode::Touchpad), down: true, x: 50.0 }]);
        let moved = frame(&mut r, &l, &[(EV_ABS, MT_X, 400)]);
        assert_eq!(moved, vec![Out::Move { x: 400.0 }], "a finger that started on the bar stays the bar's (a slider's)");
        let up = frame(&mut r, &l, &[(EV_ABS, MT_TRACKING_ID, -1), (EV_KEY, BTN_TOUCH, 0)]);
        assert_eq!(up, vec![Out::Tap { id: Id::Mode(Mode::Touchpad), down: false, x: 0.0 }]);
    }

    #[test]
    fn a_touch_on_the_pad_is_forwarded_in_pad_coordinates() {
        let (mut r, l) = setup();
        // Raw (500, 500) is screen (500, 400), pad (500, 300).
        let down = frame(&mut r, &l, &[(EV_ABS, MT_SLOT, 0), (EV_ABS, MT_TRACKING_ID, 7), (EV_ABS, MT_X, 500), (EV_ABS, MT_Y, 500)]);
        assert_eq!(
            down,
            vec![
                fwd(EV_ABS, MT_SLOT, 0),
                fwd(EV_ABS, MT_TRACKING_ID, 7),
                fwd(EV_ABS, MT_X, 500),
                fwd(EV_ABS, MT_Y, 300),
                fwd(EV_KEY, BTN_TOUCH, 1),
                fwd(EV_KEY, TOOLS[0], 1),
                fwd(EV_ABS, ABS_X, 500),
                fwd(EV_ABS, ABS_Y, 300),
                fwd(EV_SYN, SYN_REPORT, 0),
            ]
        );
        let up = frame(&mut r, &l, &[(EV_ABS, MT_TRACKING_ID, -1)]);
        assert_eq!(
            up,
            vec![
                fwd(EV_ABS, MT_SLOT, 0),
                fwd(EV_ABS, MT_TRACKING_ID, -1),
                fwd(EV_KEY, BTN_TOUCH, 0),
                fwd(EV_KEY, TOOLS[0], 0),
                fwd(EV_SYN, SYN_REPORT, 0),
            ]
        );
    }

    #[test]
    fn two_pad_fingers_are_a_doubletap_and_a_bar_finger_does_not_count() {
        let (mut r, l) = setup();
        frame(&mut r, &l, &[(EV_ABS, MT_SLOT, 0), (EV_ABS, MT_TRACKING_ID, 1), (EV_ABS, MT_X, 500), (EV_ABS, MT_Y, 500)]);
        let second = frame(&mut r, &l, &[(EV_ABS, MT_SLOT, 1), (EV_ABS, MT_TRACKING_ID, 2), (EV_ABS, MT_X, 600), (EV_ABS, MT_Y, 600)]);
        assert!(second.contains(&fwd(EV_KEY, TOOLS[0], 0)) && second.contains(&fwd(EV_KEY, TOOLS[1], 1)));
        let bar = frame(&mut r, &l, &[(EV_ABS, MT_SLOT, 2), (EV_ABS, MT_TRACKING_ID, 3), (EV_ABS, MT_X, 10), (EV_ABS, MT_Y, 10)]);
        assert!(matches!(bar.as_slice(), [Out::Tap { id: Id::Mode(Mode::Touchpad), down: true, .. }]), "{bar:?}");
    }
}
