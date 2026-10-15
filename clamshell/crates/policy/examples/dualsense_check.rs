//! Development: checks the virtual DualSense against the running kernel, for a few seconds. Makes the
//! device through /dev/uhid (the user needs write access: group input on most desktops), checks that
//! hid-playstation binds it (its driver in sysfs, the gamepad, "Motion Sensors" and "Touchpad" evdev
//! nodes), sends a few input reports and reads them back from the gamepad (grabbed meanwhile, so no
//! other evdev reader acts on them) and motion nodes, plays rumble through the gamepad's force feedback
//! and reads it back from the output report, then removes the device. Touch is never sent; the only
//! buttons pressed are L1 and D-pad up, for a moment. hidraw readers (Steam) still see the reports.
//!
//!     cargo run -p clamshell-policy --example dualsense_check

use std::error::Error;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::thread::sleep;
use std::time::{Duration, Instant};

use clamshell_policy::input::{Press, Range, Raw};
use clamshell_policy::pad::Ranges;
use clamshell_policy::pad::dualsense::{MOTION_NAME, NAME, TOUCHPAD_NAME, VirtualDualSense, motion, report};
use evdev::raw_stream::RawDevice;
use evdev::{AbsoluteAxisCode, FFEffectData, FFEffectKind, FFReplay, FFTrigger, KeyCode};

type Result<T> = std::result::Result<T, Box<dyn Error>>;

/// The evdev node with this exact name.
fn node(name: &str) -> Option<(PathBuf, RawDevice)> {
    evdev::raw_stream::enumerate().find(|(_, d)| d.name() == Some(name))
}

/// Our HID device in sysfs and the driver bound to it.
fn hid_driver() -> Option<(String, String)> {
    for entry in std::fs::read_dir("/sys/bus/hid/devices").ok()?.flatten() {
        let dir = entry.path();
        let id = entry.file_name().to_string_lossy().into_owned();
        let ours = std::fs::read_to_string(dir.join("uevent")).is_ok_and(|u| u.lines().any(|l| l == format!("HID_NAME={NAME}")));
        if id.contains("054C:0CE6") && ours {
            let driver = std::fs::read_link(dir.join("driver")).ok()?;
            return Some((id, driver.file_name()?.to_string_lossy().into_owned()));
        }
    }
    None
}

fn abs(d: &RawDevice, axis: AbsoluteAxisCode) -> Result<i32> {
    let state = d.get_abs_state()?;
    Ok(state.get(usize::from(axis.0)).ok_or("no such axis")?.value)
}

fn check(what: &str, ok: bool) -> Result<()> {
    println!("{} {what}", if ok { "ok  " } else { "FAIL" });
    if ok { Ok(()) } else { Err(format!("failed: {what}").into()) }
}

fn main() -> Result<()> {
    if std::process::Command::new("pgrep").arg("-x").arg("steam").output().is_ok_and(|o| o.status.success()) {
        println!("note: Steam is running; it may open the pad's hidraw node and see the reports too");
    }
    let ranges = Ranges { stick: Range::STICK, trigger: Range { min: 0, max: 1023 } };
    let mut ds = VirtualDualSense::new(ranges, motion::default_source(), Arc::new(Mutex::new(None)))?;
    let result = run(&mut ds);
    drop(ds);
    sleep(Duration::from_millis(300));
    let gone = node(NAME).is_none() && node(MOTION_NAME).is_none() && hid_driver().is_none();
    check("removed: no evdev nodes or HID device left", gone)?;
    result
}

fn run(ds: &mut VirtualDualSense) -> Result<()> {
    // The driver binds in the background: wait for its three nodes.
    let deadline = Instant::now() + Duration::from_secs(3);
    let (pad, motion, touchpad) = loop {
        if let (Some(p), Some(m), Some(t)) = (node(NAME), node(MOTION_NAME), node(TOUCHPAD_NAME)) {
            break (p, m, t);
        }
        if Instant::now() > deadline {
            return Err("hid-playstation didn't make the evdev nodes within 3 s".into());
        }
        sleep(Duration::from_millis(50));
    };
    let elapsed = 3.0 - deadline.saturating_duration_since(Instant::now()).as_secs_f32();
    println!("     nodes after {elapsed:.2} s: {} (pad), {} (motion), {} (touchpad)", pad.0.display(), motion.0.display(), touchpad.0.display());
    check("started by the driver", ds.started())?;
    let (id, driver) = hid_driver().ok_or("no HID device with a driver")?;
    check(&format!("{id} bound to {driver}"), driver == "playstation")?;
    let (_, mut gamepad) = pad;
    let (_, motion) = motion;
    gamepad.grab()?;
    let ff = gamepad.supported_ff().is_some_and(|f| f.contains(evdev::FFEffectCode::FF_RUMBLE));
    check("the gamepad has FF_RUMBLE", ff)?;
    // Reports sent while the driver was binding were dropped; it settles within a second.
    sleep(Duration::from_millis(1100));
    ds.resend()?;
    sleep(Duration::from_millis(50));
    check("sticks centred (128)", abs(&gamepad, AbsoluteAxisCode::ABS_X)? == 128 && abs(&gamepad, AbsoluteAxisCode::ABS_RY)? == 128)?;
    let accel = [AbsoluteAxisCode::ABS_X, AbsoluteAxisCode::ABS_Y, AbsoluteAxisCode::ABS_Z].map(|a| abs(&motion, a));
    let gyro = [AbsoluteAxisCode::ABS_RX, AbsoluteAxisCode::ABS_RY, AbsoluteAxisCode::ABS_RZ].map(|a| abs(&motion, a));
    let (accel, gyro): (Vec<i32>, Vec<i32>) = (accel.into_iter().collect::<Result<_>>()?, gyro.into_iter().collect::<Result<_>>()?);
    println!("     motion: accel {accel:?} (8192 per g), gyro {gyro:?} (1024 per °/s)");
    check("level at rest: accel (0, 8192, 0), no rotation", accel == [0, 8192, 0] && gyro == [0, 0, 0])?;

    // L1 and D-pad up, the left stick a little right: for 200 ms.
    ds.emit(&[
        Raw::key(KeyCode::BTN_TL, Press::Down),
        Raw::abs(AbsoluteAxisCode::ABS_HAT0Y, -1),
        Raw::abs(AbsoluteAxisCode::ABS_X, 2000),
        Raw::REPORT,
    ])?;
    sleep(Duration::from_millis(200));
    let keys = gamepad.get_key_state()?;
    let (x, hat) = (abs(&gamepad, AbsoluteAxisCode::ABS_X)?, abs(&gamepad, AbsoluteAxisCode::ABS_HAT0Y)?);
    ds.emit(&[Raw::key(KeyCode::BTN_TL, Press::Up), Raw::abs(AbsoluteAxisCode::ABS_HAT0Y, 0), Raw::abs(AbsoluteAxisCode::ABS_X, 0), Raw::REPORT])?;
    println!("     pressed: L1 {}, hat y {hat}, left x {x}", keys.contains(KeyCode::BTN_TL));
    check("L1 down, D-pad up, left stick at 135", keys.contains(KeyCode::BTN_TL) && hat == -1 && x == 135)?;
    sleep(Duration::from_millis(100));
    let keys = gamepad.get_key_state()?;
    check("all let go", keys.iter().next().is_none() && abs(&gamepad, AbsoluteAxisCode::ABS_HAT0Y)? == 0 && abs(&gamepad, AbsoluteAxisCode::ABS_X)? == 128)?;

    // Rumble from the gamepad's force feedback: hid-playstation sends it as an output report.
    let effect = FFEffectData {
        direction: 0,
        trigger: FFTrigger { button: 0, interval: 0 },
        replay: FFReplay { length: 500, delay: 0 },
        kind: FFEffectKind::Rumble { strong_magnitude: 0x8000, weak_magnitude: 0x4000 },
    };
    let mut effect = gamepad.upload_ff_effect(effect)?;
    effect.play(1)?;
    sleep(Duration::from_millis(150));
    let playing = ds.motors();
    effect.stop()?;
    sleep(Duration::from_millis(150));
    println!("     rumble: {playing:?} while playing, then {:?}", ds.motors());
    check("rumble read back: strong 128, weak 64, then off", playing == Some(report::Motors { strong: 128, weak: 64 }) && ds.motors() == Some(report::Motors::default()))?;
    drop(effect);
    gamepad.ungrab()?;
    Ok(())
}
