//! Development: a fake touchscreen for testing touchpad mode on a desktop. Creates a direct-touch
//! multitouch device the size of the second screen's buffer (1240x1080), waits for the system program
//! to grab it, then plays: a one-finger swipe on the pad, a two-finger scroll, and a tap on the bar's
//! touchpad icon (which ends touchpad mode).
//!
//!     cargo run -p sysprog-policy --example fake_touchscreen
//!     SYSPROG_TOUCHSCREEN="sysprog fake touchscreen" SYSPROG_START_MODE=touchpad cargo run

use std::io;
use std::thread::sleep;
use std::time::Duration;

use evdev::uinput::VirtualDevice;
use evdev::{AbsInfo, AbsoluteAxisCode, AttributeSet, EventType, InputEvent, KeyCode, PropType, UinputAbsSetup};

const W: i32 = 1240;
const H: i32 = 1080;

fn abs(code: AbsoluteAxisCode, value: i32) -> InputEvent {
    InputEvent::new(EventType::ABSOLUTE.0, code.0, value)
}

fn key(code: KeyCode, value: i32) -> InputEvent {
    InputEvent::new(EventType::KEY.0, code.0, value)
}

fn syn() -> InputEvent {
    InputEvent::new(EventType::SYNCHRONIZATION.0, 0, 0)
}

/// Fingers as (slot, x, y); `None` for a lifted one.
fn frame(dev: &mut VirtualDevice, fingers: &[(i32, Option<(i32, i32)>)]) -> io::Result<()> {
    let mut events = Vec::new();
    for &(slot, pos) in fingers {
        events.push(abs(AbsoluteAxisCode::ABS_MT_SLOT, slot));
        match pos {
            Some((x, y)) => {
                events.push(abs(AbsoluteAxisCode::ABS_MT_POSITION_X, x));
                events.push(abs(AbsoluteAxisCode::ABS_MT_POSITION_Y, y));
            }
            None => events.push(abs(AbsoluteAxisCode::ABS_MT_TRACKING_ID, -1)),
        }
    }
    events.push(syn());
    dev.emit(&events)
}

fn touch_down(dev: &mut VirtualDevice, slot: i32, id: i32, (x, y): (i32, i32)) -> io::Result<()> {
    dev.emit(&[
        abs(AbsoluteAxisCode::ABS_MT_SLOT, slot),
        abs(AbsoluteAxisCode::ABS_MT_TRACKING_ID, id),
        abs(AbsoluteAxisCode::ABS_MT_POSITION_X, x),
        abs(AbsoluteAxisCode::ABS_MT_POSITION_Y, y),
        key(KeyCode::BTN_TOUCH, 1),
        syn(),
    ])
}

fn main() -> io::Result<()> {
    let axis = |code, max| UinputAbsSetup::new(code, AbsInfo::new(0, 0, max, 0, 0, 16));
    let keys: AttributeSet<KeyCode> = [KeyCode::BTN_TOUCH].into_iter().collect();
    let props: AttributeSet<PropType> = [PropType::DIRECT].into_iter().collect();
    let mut dev = VirtualDevice::builder()?
        .name("sysprog fake touchscreen")
        .with_properties(&props)?
        .with_keys(&keys)?
        .with_absolute_axis(&axis(AbsoluteAxisCode::ABS_X, W - 1))?
        .with_absolute_axis(&axis(AbsoluteAxisCode::ABS_Y, H - 1))?
        .with_absolute_axis(&axis(AbsoluteAxisCode::ABS_MT_SLOT, 9))?
        .with_absolute_axis(&axis(AbsoluteAxisCode::ABS_MT_TRACKING_ID, 65535))?
        .with_absolute_axis(&axis(AbsoluteAxisCode::ABS_MT_POSITION_X, W - 1))?
        .with_absolute_axis(&axis(AbsoluteAxisCode::ABS_MT_POSITION_Y, H - 1))?
        .build()?;
    eprintln!("fake touchscreen ready; start the system program in touchpad mode now (8 s)");
    sleep(Duration::from_secs(8));
    let step = Duration::from_millis(12);

    eprintln!("one-finger swipe on the pad");
    touch_down(&mut dev, 0, 1, (400, 500))?;
    for i in 1..=40 {
        sleep(step);
        frame(&mut dev, &[(0, Some((400 + i * 8, 500 + i * 3)))])?;
    }
    sleep(step);
    dev.emit(&[abs(AbsoluteAxisCode::ABS_MT_SLOT, 0), abs(AbsoluteAxisCode::ABS_MT_TRACKING_ID, -1), key(KeyCode::BTN_TOUCH, 0), syn()])?;
    sleep(Duration::from_millis(500));

    eprintln!("two-finger scroll on the pad");
    touch_down(&mut dev, 0, 2, (500, 400))?;
    touch_down(&mut dev, 1, 3, (650, 400))?;
    for i in 1..=30 {
        sleep(step);
        frame(&mut dev, &[(0, Some((500, 400 + i * 6))), (1, Some((650, 400 + i * 6)))])?;
    }
    sleep(step);
    frame(&mut dev, &[(0, None), (1, None)])?;
    dev.emit(&[key(KeyCode::BTN_TOUCH, 0), syn()])?;
    sleep(Duration::from_millis(500));

    eprintln!("tap on the bar's touchpad icon (ends touchpad mode)");
    touch_down(&mut dev, 0, 4, (324, 44))?;
    sleep(Duration::from_millis(80));
    dev.emit(&[abs(AbsoluteAxisCode::ABS_MT_SLOT, 0), abs(AbsoluteAxisCode::ABS_MT_TRACKING_ID, -1), key(KeyCode::BTN_TOUCH, 0), syn()])?;
    sleep(Duration::from_secs(1));
    eprintln!("done");
    Ok(())
}
