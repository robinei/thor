//! Development: a fake Xbox-style controller for testing the controller path without one.
//!
//! Without arguments it creates the pad, waits for ClamShell to grab it, pushes the left stick fully
//! right for 0.5 s and down for 0.25 s (no buttons: they would click whatever is under the pointer), and
//! reports what came out: the virtual mouse's motion and the virtual pad's stick.
//!
//!     cargo run -p clamshell-system --example fake_pad &
//!     CLAMSHELL_GAMEPAD="clamshell fake pad" [CLAMSHELL_PAD_MOUSE=1] cargo run
//!
//! With `--control PATH` it creates the pad and then does what lines read from PATH (a named pipe, made
//! if missing; `-` for standard input) say, until `quit` or until it is killed (which unplugs the pad):
//!
//! - `press BUTTON`, `release BUTTON`, `tap BUTTON [MS]` (pressed for MS, default 80): a, b, x, y, lb,
//!   rb, view, menu, home, ls, rs, back
//! - `dpad up|down|left|right|none`: the D-pad (a hat), held until the next `dpad`
//! - `stick left|right X Y`: a stick at X, Y (-1.0 to 1.0, y down), held until changed
//! - `trigger left|right V`: a trigger at V (0.0 to 1.0)
//! - `gyro X Y Z`: the gyro turning at X, Y, Z degrees per second (pitch, yaw, roll: ABS_RX, RY, RZ),
//!   held until changed (`gyro 0 0 0`)
//! - `sleep MS`
//! - `quit`
//!
//! Each line is answered on standard error. `tools/vm/vm.py pad` runs it in the virtual Thor. Controlled, it
//! has rumble (FF_RUMBLE): it answers effect uploads and says on standard error what it is asked to play
//! (`rumble STRONG WEAK`, `rumble stop`).
//!
//! Beside the pad it makes a motion sensor as the kernel's controller drivers do ("clamshell fake pad
//! Motion Sensors": INPUT_PROP_ACCELEROMETER, the accelerometer on ABS_X/Y/Z, the gyro on ABS_RX/RY/RZ
//! with its resolution, units per degree per second, and MSC_TIMESTAMP), reporting at 250 Hz.

use std::fs::File;
use std::io::{self, BufRead, BufReader};
use std::path::Path;
use std::sync::{Arc, Mutex};
use std::thread::{sleep, spawn};
use std::time::{Duration, Instant};

use evdev::raw_stream::RawDevice;
use evdev::uinput::VirtualDevice;
use evdev::{
    AbsInfo, AbsoluteAxisCode, AttributeSet, EventSummary, EventType, FFEffectCode, FFEffectKind, InputEvent, KeyCode, MiscCode, PropType, RelativeAxisCode,
    UInputCode, UinputAbsSetup,
};

const STICK_MAX: i32 = 32767;
const TRIGGER_MAX: i32 = 1023;
/// The gyro's units per degree per second, and its range in degrees per second.
const GYRO_RES: i32 = 1024;
const GYRO_RANGE: i32 = 2000;
/// The accelerometer's units per g.
const ACCEL_RES: i32 = 8192;
/// How often the motion sensor reports.
const MOTION_EVERY: Duration = Duration::from_millis(4);

/// The buttons by name, as an Xbox controller's (Linux's xpad codes), and the Thor's Back button (its
/// code assumed until the device test).
const BUTTONS: [(&str, KeyCode); 12] = [
    ("a", KeyCode::BTN_SOUTH),
    ("b", KeyCode::BTN_EAST),
    ("x", KeyCode::BTN_WEST),
    ("y", KeyCode::BTN_NORTH),
    ("lb", KeyCode::BTN_TL),
    ("rb", KeyCode::BTN_TR),
    ("view", KeyCode::BTN_SELECT),
    ("menu", KeyCode::BTN_START),
    ("home", KeyCode::BTN_MODE),
    ("ls", KeyCode::BTN_THUMBL),
    ("rs", KeyCode::BTN_THUMBR),
    ("back", KeyCode::KEY_BACK),
];

fn syn() -> InputEvent {
    InputEvent::new(EventType::SYNCHRONIZATION.0, 0, 0)
}

fn abs(code: AbsoluteAxisCode, value: i32) -> InputEvent {
    InputEvent::new(EventType::ABSOLUTE.0, code.0, value)
}

fn key(code: KeyCode, down: bool) -> InputEvent {
    InputEvent::new(EventType::KEY.0, code.0, i32::from(down))
}

fn stick(dev: &mut VirtualDevice, x: i32, y: i32) -> io::Result<()> {
    dev.emit(&[abs(AbsoluteAxisCode::ABS_X, x), abs(AbsoluteAxisCode::ABS_Y, y), syn()])
}

fn make_pad() -> io::Result<VirtualDevice> {
    make_pad_with(false)
}

/// The pad, with rumble if `rumble` (then someone must answer its effect uploads: [`serve_rumble`]).
fn make_pad_with(rumble: bool) -> io::Result<VirtualDevice> {
    let stick_axis = |code| UinputAbsSetup::new(code, AbsInfo::new(0, -STICK_MAX - 1, STICK_MAX, 16, 128, 0));
    let trigger = |code| UinputAbsSetup::new(code, AbsInfo::new(0, 0, TRIGGER_MAX, 0, 0, 0));
    let hat = |code| UinputAbsSetup::new(code, AbsInfo::new(0, -1, 1, 0, 0, 0));
    let keys: AttributeSet<KeyCode> = BUTTONS.iter().map(|&(_, code)| code).collect();
    let builder = VirtualDevice::builder()?
        .name("clamshell fake pad")
        .with_keys(&keys)?
        .with_absolute_axis(&stick_axis(AbsoluteAxisCode::ABS_X))?
        .with_absolute_axis(&stick_axis(AbsoluteAxisCode::ABS_Y))?
        .with_absolute_axis(&stick_axis(AbsoluteAxisCode::ABS_RX))?
        .with_absolute_axis(&stick_axis(AbsoluteAxisCode::ABS_RY))?
        .with_absolute_axis(&trigger(AbsoluteAxisCode::ABS_Z))?
        .with_absolute_axis(&trigger(AbsoluteAxisCode::ABS_RZ))?
        .with_absolute_axis(&hat(AbsoluteAxisCode::ABS_HAT0X))?
        .with_absolute_axis(&hat(AbsoluteAxisCode::ABS_HAT0Y))?;
    let builder = if rumble {
        let ff: AttributeSet<FFEffectCode> = [FFEffectCode::FF_RUMBLE].into_iter().collect();
        builder.with_ff(&ff)?.with_ff_effects_max(4)
    } else {
        builder
    };
    let pad = builder.build()?;
    let flags = rustix::fs::fcntl_getfl(&pad)?;
    rustix::fs::fcntl_setfl(&pad, flags | rustix::fs::OFlags::NONBLOCK)?;
    Ok(pad)
}

/// Answers the pad's effect uploads and says what it is asked to play, until the pad is gone.
fn serve_rumble(pad: Arc<Mutex<VirtualDevice>>) {
    let mut effects: Vec<(i16, (u16, u16))> = Vec::new();
    loop {
        sleep(Duration::from_millis(2));
        let Ok(mut pad) = pad.lock() else {
            return;
        };
        let events: Vec<InputEvent> = match pad.fetch_events() {
            Ok(events) => events.collect(),
            Err(e) if e.kind() == io::ErrorKind::WouldBlock => continue,
            Err(_) => return,
        };
        for event in events {
            match event.destructure() {
                EventSummary::UInput(request, UInputCode::UI_FF_UPLOAD, _) => {
                    if let Ok(mut upload) = pad.process_ff_upload(request) {
                        if let FFEffectKind::Rumble { strong_magnitude, weak_magnitude } = upload.effect().kind {
                            let id = upload.effect_id();
                            effects.retain(|(e, _)| *e != id);
                            effects.push((id, (strong_magnitude, weak_magnitude)));
                        }
                        upload.set_retval(0);
                    }
                }
                EventSummary::UInput(request, UInputCode::UI_FF_ERASE, _) => {
                    if let Ok(mut erase) = pad.process_ff_erase(request) {
                        erase.set_retval(0);
                    }
                }
                EventSummary::ForceFeedback(_, FFEffectCode(id), value) => {
                    let level = effects.iter().find(|(e, _)| i32::from(*e) == i32::from(id)).map(|(_, l)| *l);
                    match (value > 0, level) {
                        (true, Some((strong, weak))) => eprintln!("rumble {strong} {weak}"),
                        _ => eprintln!("rumble stop"),
                    }
                }
                _ => {}
            }
        }
    }
}

/// The motion sensor beside the pad.
fn make_motion() -> io::Result<VirtualDevice> {
    let gyro = |code| UinputAbsSetup::new(code, AbsInfo::new(0, -GYRO_RANGE * GYRO_RES, GYRO_RANGE * GYRO_RES, 16, 0, GYRO_RES));
    let accel = |code| UinputAbsSetup::new(code, AbsInfo::new(0, -4 * ACCEL_RES, 4 * ACCEL_RES, 4, 0, ACCEL_RES));
    let props: AttributeSet<PropType> = [PropType::ACCELEROMETER].into_iter().collect();
    let msc: AttributeSet<MiscCode> = [MiscCode::MSC_TIMESTAMP].into_iter().collect();
    VirtualDevice::builder()?
        .name("clamshell fake pad Motion Sensors")
        .with_properties(&props)?
        .with_msc(&msc)?
        .with_absolute_axis(&accel(AbsoluteAxisCode::ABS_X))?
        .with_absolute_axis(&accel(AbsoluteAxisCode::ABS_Y))?
        .with_absolute_axis(&accel(AbsoluteAxisCode::ABS_Z))?
        .with_absolute_axis(&gyro(AbsoluteAxisCode::ABS_RX))?
        .with_absolute_axis(&gyro(AbsoluteAxisCode::ABS_RY))?
        .with_absolute_axis(&gyro(AbsoluteAxisCode::ABS_RZ))?
        .build()
}

/// The gyro's rate as last set (degrees per second), reported every [`MOTION_EVERY`] with gravity
/// straight down, as a handheld held level.
type Gyro = Arc<Mutex<[f32; 3]>>;

fn report_motion(mut device: VirtualDevice, gyro: Gyro) {
    let start = Instant::now();
    loop {
        let rate = gyro.lock().map(|g| *g).unwrap_or_default();
        let micros = start.elapsed().as_micros() as u32;
        let mut events: Vec<InputEvent> = [AbsoluteAxisCode::ABS_RX, AbsoluteAxisCode::ABS_RY, AbsoluteAxisCode::ABS_RZ]
            .into_iter()
            .zip(rate)
            .map(|(code, deg)| abs(code, (deg * GYRO_RES as f32).round() as i32))
            .collect();
        events.push(abs(AbsoluteAxisCode::ABS_Y, ACCEL_RES));
        events.push(InputEvent::new(EventType::MISC.0, MiscCode::MSC_TIMESTAMP.0, micros as i32));
        events.push(syn());
        if device.emit(&events).is_err() {
            return;
        }
        sleep(MOTION_EVERY);
    }
}

fn main() -> io::Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.iter().map(String::as_str).collect::<Vec<_>>().as_slice() {
        [] => demo(),
        ["--control", path] => controlled(path),
        _ => Err(io::Error::other("usage: fake_pad [--control PATH|-]")),
    }
}

/// Reads commands from `path` (made as a named pipe if missing; reopened after each writer) or stdin.
fn controlled(path: &str) -> io::Result<()> {
    let pad = Arc::new(Mutex::new(make_pad_with(true)?));
    let gyro = Gyro::default();
    let motion = make_motion()?;
    let reporting = gyro.clone();
    spawn(move || report_motion(motion, reporting));
    let serving = pad.clone();
    spawn(move || serve_rumble(serving));
    let mut pad = Fake { pad, gyro };
    let pad = &mut pad;
    eprintln!("fake pad ready; commands from {path}");
    if path == "-" {
        for line in io::stdin().lock().lines() {
            if !run_line(pad, &line?) {
                break;
            }
        }
        return Ok(());
    }
    if !Path::new(path).exists() {
        rustix::fs::mkfifoat(rustix::fs::CWD, path, rustix::fs::Mode::from_raw_mode(0o600))?;
    }
    loop {
        for line in BufReader::new(File::open(path)?).lines() {
            if !run_line(pad, &line?) {
                return Ok(());
            }
        }
    }
}

/// The fake controller: its pad (shared with the thread answering its rumble), and its gyro's rate.
struct Fake {
    pad: Arc<Mutex<VirtualDevice>>,
    gyro: Gyro,
}

/// Carries out one command line; false for `quit`.
fn run_line(pad: &mut Fake, line: &str) -> bool {
    let words: Vec<&str> = line.split_whitespace().collect();
    if words.is_empty() {
        return true;
    }
    if words == ["quit"] {
        eprintln!("quit");
        return false;
    }
    match command(pad, &words) {
        Ok(()) => eprintln!("ok: {line}"),
        Err(e) => eprintln!("error: {line}: {e}"),
    }
    true
}

fn invalid(what: &str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidInput, what.to_owned())
}

fn button(name: &str) -> io::Result<KeyCode> {
    BUTTONS.iter().find(|(n, _)| *n == name).map(|&(_, code)| code).ok_or_else(|| invalid("no such button"))
}

fn number(word: &str) -> io::Result<f32> {
    word.parse().map_err(|_| invalid("not a number"))
}

fn millis(word: &str) -> io::Result<Duration> {
    Ok(Duration::from_millis(number(word)?.max(0.0) as u64))
}

/// A value from -1.0 to 1.0 on an axis running to `max`.
fn scaled(value: f32, max: i32) -> i32 {
    (value.clamp(-1.0, 1.0) * max as f32).round() as i32
}

fn command(fake: &mut Fake, words: &[&str]) -> io::Result<()> {
    let mut held = fake.pad.lock().map_err(|_| io::Error::other("the pad's lock"))?;
    let pad = &mut *held;
    match words {
        ["gyro", x, y, z] => {
            let rate = [number(x)?, number(y)?, number(z)?];
            if let Ok(mut g) = fake.gyro.lock() {
                *g = rate;
            }
            Ok(())
        }
        ["press", name] => pad.emit(&[key(button(name)?, true), syn()]),
        ["release", name] => pad.emit(&[key(button(name)?, false), syn()]),
        ["tap", name, rest @ ..] => {
            let code = button(name)?;
            let held = match rest {
                [] => Duration::from_millis(80),
                [ms] => millis(ms)?,
                _ => return Err(invalid("tap BUTTON [MS]")),
            };
            pad.emit(&[key(code, true), syn()])?;
            sleep(held);
            pad.emit(&[key(code, false), syn()])
        }
        ["dpad", direction] => {
            let (x, y) = match *direction {
                "up" => (0, -1),
                "down" => (0, 1),
                "left" => (-1, 0),
                "right" => (1, 0),
                "none" => (0, 0),
                _ => return Err(invalid("dpad up|down|left|right|none")),
            };
            pad.emit(&[abs(AbsoluteAxisCode::ABS_HAT0X, x), abs(AbsoluteAxisCode::ABS_HAT0Y, y), syn()])
        }
        ["stick", side, x, y] => {
            let (cx, cy) = match *side {
                "left" => (AbsoluteAxisCode::ABS_X, AbsoluteAxisCode::ABS_Y),
                "right" => (AbsoluteAxisCode::ABS_RX, AbsoluteAxisCode::ABS_RY),
                _ => return Err(invalid("stick left|right X Y")),
            };
            pad.emit(&[abs(cx, scaled(number(x)?, STICK_MAX)), abs(cy, scaled(number(y)?, STICK_MAX)), syn()])
        }
        ["trigger", side, v] => {
            let code = match *side {
                "left" => AbsoluteAxisCode::ABS_Z,
                "right" => AbsoluteAxisCode::ABS_RZ,
                _ => return Err(invalid("trigger left|right V")),
            };
            pad.emit(&[abs(code, scaled(number(v)?.max(0.0), TRIGGER_MAX)), syn()])
        }
        ["sleep", ms] => {
            sleep(millis(ms)?);
            Ok(())
        }
        _ => Err(invalid("not a command")),
    }
}

fn demo() -> io::Result<()> {
    let mut pad = make_pad()?;
    eprintln!("fake pad ready; start ClamShell with the pad mouse on now (6 s)");
    sleep(Duration::from_secs(6));

    // Read what ClamShell makes of it: the virtual mouse's motion, and the largest stick value
    // the virtual pad passes on.
    let open = |name: &str| -> io::Result<RawDevice> {
        let d = evdev::raw_stream::enumerate()
            .map(|(_, d)| d)
            .find(|d| d.name() == Some(name))
            .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, format!("no '{name}': is ClamShell running?")))?;
        let flags = rustix::fs::fcntl_getfl(&d)?;
        rustix::fs::fcntl_setfl(&d, flags | rustix::fs::OFlags::NONBLOCK)?;
        Ok(d)
    };
    let mut mouse = open("clamshell pad mouse")?;
    let mut virtual_pad = open("clamshell pad (Xbox 360)")?;
    let reader = spawn(move || -> (i32, i32, i32) {
        let (mut x, mut y, mut stick) = (0, 0, 0);
        for _ in 0..150 {
            sleep(Duration::from_millis(10));
            if let Ok(events) = mouse.fetch_events() {
                for e in events.filter(|e| e.event_type() == EventType::RELATIVE) {
                    if e.code() == RelativeAxisCode::REL_X.0 {
                        x += e.value();
                    } else if e.code() == RelativeAxisCode::REL_Y.0 {
                        y += e.value();
                    }
                }
            }
            if let Ok(events) = virtual_pad.fetch_events() {
                for e in events.filter(|e| e.event_type() == EventType::ABSOLUTE && e.code() == AbsoluteAxisCode::ABS_X.0) {
                    stick = stick.max(e.value());
                }
            }
        }
        (x, y, stick)
    });

    sleep(Duration::from_millis(100));
    eprintln!("left stick right 0.5 s, then down 0.25 s");
    stick(&mut pad, STICK_MAX, 0)?;
    sleep(Duration::from_millis(500));
    stick(&mut pad, 0, STICK_MAX)?;
    sleep(Duration::from_millis(250));
    stick(&mut pad, 0, 0)?;
    let (x, y, stick) = reader.join().map_err(|_| io::Error::other("reader panicked"))?;
    eprintln!("pointer moved by x {x}, y {y}; the virtual pad's left stick reached x {stick}");
    eprintln!("(pad mouse on: about 900 and 450, stick 0; off: no motion, stick 32767)");
    Ok(())
}
