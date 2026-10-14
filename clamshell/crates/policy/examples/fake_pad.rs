//! Development: a fake Xbox-style controller for testing the controller path on a desktop. Creates the
//! pad, waits for ClamShell to grab it, pushes the left stick fully right for 0.5 s and down for
//! 0.25 s (no buttons: they would click whatever is under the pointer), and reports what came out: the
//! virtual mouse's motion and the virtual pad's stick.
//!
//!     cargo run -p clamshell-policy --example fake_pad &
//!     CLAMSHELL_GAMEPAD="clamshell fake pad" [CLAMSHELL_PAD_MOUSE=1] cargo run

use std::io;
use std::thread::{sleep, spawn};
use std::time::Duration;

use evdev::raw_stream::RawDevice;
use evdev::uinput::VirtualDevice;
use evdev::{AbsInfo, AbsoluteAxisCode, AttributeSet, EventType, InputEvent, KeyCode, RelativeAxisCode, UinputAbsSetup};

fn stick(dev: &mut VirtualDevice, x: i32, y: i32) -> io::Result<()> {
    dev.emit(&[
        InputEvent::new(EventType::ABSOLUTE.0, AbsoluteAxisCode::ABS_X.0, x),
        InputEvent::new(EventType::ABSOLUTE.0, AbsoluteAxisCode::ABS_Y.0, y),
        InputEvent::new(EventType::SYNCHRONIZATION.0, 0, 0),
    ])
}

fn main() -> io::Result<()> {
    let stick_axis = |code| UinputAbsSetup::new(code, AbsInfo::new(0, -32768, 32767, 16, 128, 0));
    let trigger = |code| UinputAbsSetup::new(code, AbsInfo::new(0, 0, 1023, 0, 0, 0));
    let hat = |code| UinputAbsSetup::new(code, AbsInfo::new(0, -1, 1, 0, 0, 0));
    let keys: AttributeSet<KeyCode> =
        [KeyCode::BTN_SOUTH, KeyCode::BTN_EAST, KeyCode::BTN_NORTH, KeyCode::BTN_WEST, KeyCode::BTN_START].into_iter().collect();
    let mut pad = VirtualDevice::builder()?
        .name("clamshell fake pad")
        .with_keys(&keys)?
        .with_absolute_axis(&stick_axis(AbsoluteAxisCode::ABS_X))?
        .with_absolute_axis(&stick_axis(AbsoluteAxisCode::ABS_Y))?
        .with_absolute_axis(&stick_axis(AbsoluteAxisCode::ABS_RX))?
        .with_absolute_axis(&stick_axis(AbsoluteAxisCode::ABS_RY))?
        .with_absolute_axis(&trigger(AbsoluteAxisCode::ABS_Z))?
        .with_absolute_axis(&trigger(AbsoluteAxisCode::ABS_RZ))?
        .with_absolute_axis(&hat(AbsoluteAxisCode::ABS_HAT0X))?
        .with_absolute_axis(&hat(AbsoluteAxisCode::ABS_HAT0Y))?
        .build()?;
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
    stick(&mut pad, 32767, 0)?;
    sleep(Duration::from_millis(500));
    stick(&mut pad, 0, 32767)?;
    sleep(Duration::from_millis(250));
    stick(&mut pad, 0, 0)?;
    let (x, y, stick) = reader.join().map_err(|_| io::Error::other("reader panicked"))?;
    eprintln!("pointer moved by x {x}, y {y}; the virtual pad's left stick reached x {stick}");
    eprintln!("(pad mouse on: about 900 and 450, stick 0; off: no motion, stick 32767)");
    Ok(())
}
