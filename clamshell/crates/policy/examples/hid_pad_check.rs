//! Development: checks the virtual HID pads (the DualSense, the Switch Pro Controller) against the
//! running kernel, a few seconds each. Makes the device through /dev/uhid (the user needs write
//! access: group input on most desktops), checks that the kernel's driver binds it (hid-playstation,
//! hid-nintendo: its driver in sysfs, its gamepad and motion evdev nodes), sends a few input reports
//! and reads them back from the gamepad (grabbed meanwhile, so no other evdev reader acts on them) and
//! motion nodes, plays rumble through the gamepad's force feedback and reads it back from the output
//! reports, then removes the device. Touch is never sent; the only buttons pressed are L1 and D-pad
//! up, for a moment. hidraw readers (Steam) still see the reports.
//!
//!     cargo run -p clamshell-policy --example hid_pad_check [playstation|nintendo]

use std::error::Error;
use std::sync::{Arc, Mutex};
use std::thread::sleep;
use std::time::{Duration, Instant};

use clamshell_policy::input::{Press, Range, Raw};
use clamshell_policy::pad::Ranges;
use clamshell_policy::pad::hid::{Motors, Protocol, VirtualHid, dualsense, motion, procon};
use evdev::raw_stream::RawDevice;
use evdev::{AbsoluteAxisCode, FFEffectCode, FFEffectData, FFEffectKind, FFReplay, FFTrigger, KeyCode};

type Result<T> = std::result::Result<T, Box<dyn Error>>;

/// What is checked for one pad.
struct Pad {
    protocol: fn() -> Box<dyn Protocol>,
    /// The evdev nodes' names: gamepad, motion, and any others.
    names: &'static [&'static str],
    /// HID_ID's vendor and product in sysfs, and the driver.
    sysfs_id: &'static str,
    driver: &'static str,
    /// The motion node at rest, level: accelerometer x, y, z and gyro.
    level: [i32; 6],
    /// The left stick for an input of 2000 (of ±32767).
    stick: fn(i32) -> bool,
}

const PADS: [(&str, Pad); 2] = [
    (
        "playstation",
        Pad {
            protocol: || Box::new(dualsense::DualSense::default()),
            names: &[dualsense::NAME, dualsense::MOTION_NAME, dualsense::TOUCHPAD_NAME],
            sysfs_id: "054C:0CE6",
            driver: "playstation",
            level: [0, 8192, 0, 0, 0, 0],
            stick: |x| x == 135,
        },
    ),
    (
        "nintendo",
        Pad {
            protocol: || Box::new(procon::ProController::default()),
            names: &[procon::NAME, procon::IMU_NAME],
            sysfs_id: "057E:2009",
            driver: "nintendo",
            level: [0, 0, 4096, 0, 0, 0],
            stick: |x| (1950..2050).contains(&x),
        },
    ),
];

/// The evdev node with this exact name.
fn node(name: &str) -> Option<RawDevice> {
    evdev::raw_stream::enumerate().map(|(_, d)| d).find(|d| d.name() == Some(name))
}

/// Our HID device in sysfs and the driver bound to it.
fn hid_driver(name: &str, id: &str) -> Option<(String, String)> {
    for entry in std::fs::read_dir("/sys/bus/hid/devices").ok()?.flatten() {
        let dir = entry.path();
        let dev = entry.file_name().to_string_lossy().into_owned();
        let ours = std::fs::read_to_string(dir.join("uevent")).is_ok_and(|u| u.lines().any(|l| l == format!("HID_NAME={name}")));
        if dev.contains(id) && ours {
            let driver = std::fs::read_link(dir.join("driver")).map(|d| d.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default());
            return Some((dev, driver.unwrap_or_else(|_| "none".into())));
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
        println!("note: Steam is running; it may open the pads' hidraw nodes and see the reports too");
    }
    let only = std::env::args().nth(1);
    let mut failed = false;
    for (name, pad) in PADS.iter().filter(|(n, _)| only.as_deref().is_none_or(|o| o == *n)) {
        println!("{name}:");
        let ranges = Ranges { stick: Range::STICK, trigger: Range { min: 0, max: 1023 } };
        let mut hid = VirtualHid::new((pad.protocol)(), ranges, motion::default_source(), Arc::new(Mutex::new(None)))?;
        let result = run(pad, &mut hid);
        drop(hid);
        sleep(Duration::from_millis(300));
        let gone = pad.names.iter().all(|n| node(n).is_none()) && hid_driver(pad.names.first().copied().unwrap_or_default(), pad.sysfs_id).is_none();
        failed |= check("removed: no evdev nodes or HID device left", gone).is_err();
        if let Err(e) = result {
            println!("     {e}");
            failed = true;
        }
    }
    if failed { Err("some checks failed".into()) } else { Ok(()) }
}

fn run(pad: &Pad, hid: &mut VirtualHid) -> Result<()> {
    // The driver binds in the background: wait for its nodes.
    let started = Instant::now();
    let mut nodes = loop {
        let found: Vec<RawDevice> = pad.names.iter().filter_map(|n| node(n)).collect();
        if found.len() == pad.names.len() {
            break found;
        }
        if started.elapsed() > Duration::from_secs(5) {
            let driver = hid_driver(pad.names.first().copied().unwrap_or_default(), pad.sysfs_id);
            return Err(format!("the driver didn't make the evdev nodes within 5 s (sysfs: {driver:?})").into());
        }
        sleep(Duration::from_millis(50));
    };
    println!("     nodes after {:.2} s", started.elapsed().as_secs_f32());
    check("started by the driver", hid.started())?;
    let name = pad.names.first().copied().unwrap_or_default();
    let (id, driver) = hid_driver(name, pad.sysfs_id).ok_or("no HID device")?;
    check(&format!("{id} bound to {driver}"), driver == pad.driver)?;
    let mut gamepad = nodes.remove(0);
    let motion = nodes.remove(0);
    gamepad.grab()?;
    check("the gamepad has FF_RUMBLE", gamepad.supported_ff().is_some_and(|f| f.contains(FFEffectCode::FF_RUMBLE)))?;
    // Reports sent while the driver was binding may have been dropped; it settles within a second.
    sleep(Duration::from_millis(1100));
    hid.resend()?;
    sleep(Duration::from_millis(50));
    let axes = [
        AbsoluteAxisCode::ABS_X,
        AbsoluteAxisCode::ABS_Y,
        AbsoluteAxisCode::ABS_Z,
        AbsoluteAxisCode::ABS_RX,
        AbsoluteAxisCode::ABS_RY,
        AbsoluteAxisCode::ABS_RZ,
    ];
    let level: Vec<i32> = axes.iter().map(|&a| abs(&motion, a)).collect::<Result<_>>()?;
    println!("     motion: accel {:?}, gyro {:?}", level.get(..3), level.get(3..));
    check("level at rest", level == pad.level)?;
    let centre = abs(&gamepad, AbsoluteAxisCode::ABS_X)?;

    // L1 and D-pad up, the left stick a little right: for 200 ms.
    let press = [Raw::key(KeyCode::BTN_TL, Press::Down), Raw::abs(AbsoluteAxisCode::ABS_HAT0Y, -1), Raw::abs(AbsoluteAxisCode::ABS_X, 2000), Raw::REPORT];
    hid.emit(&press)?;
    sleep(Duration::from_millis(200));
    let keys = gamepad.get_key_state()?;
    let (x, hat) = (abs(&gamepad, AbsoluteAxisCode::ABS_X)?, abs(&gamepad, AbsoluteAxisCode::ABS_HAT0Y)?);
    hid.emit(&[Raw::key(KeyCode::BTN_TL, Press::Up), Raw::abs(AbsoluteAxisCode::ABS_HAT0Y, 0), Raw::abs(AbsoluteAxisCode::ABS_X, 0), Raw::REPORT])?;
    println!("     pressed: L1 {}, hat y {hat}, left x {x} (centred: {centre})", keys.contains(KeyCode::BTN_TL));
    check("L1 down, D-pad up, left stick a little right", keys.contains(KeyCode::BTN_TL) && hat == -1 && (pad.stick)(x))?;
    sleep(Duration::from_millis(100));
    let keys = gamepad.get_key_state()?;
    check("all let go", keys.iter().next().is_none() && abs(&gamepad, AbsoluteAxisCode::ABS_HAT0Y)? == 0 && abs(&gamepad, AbsoluteAxisCode::ABS_X)? == centre)?;

    // Rumble from the gamepad's force feedback: the driver sends it as output reports.
    let effect = FFEffectData {
        direction: 0,
        trigger: FFTrigger { button: 0, interval: 0 },
        replay: FFReplay { length: 1000, delay: 0 },
        kind: FFEffectKind::Rumble { strong_magnitude: 0x8000, weak_magnitude: 0x4000 },
    };
    let mut effect = gamepad.upload_ff_effect(effect)?;
    effect.play(1)?;
    sleep(Duration::from_millis(300));
    let playing = hid.motors();
    effect.stop()?;
    sleep(Duration::from_millis(400));
    let after = hid.motors();
    println!("     rumble: {playing:?} while playing, then {after:?}");
    let ok = playing.is_some_and(|m| m.strong > m.weak && m.weak > 0) && after == Some(Motors::default());
    check("rumble read back: strong over weak, then off", ok)?;
    drop(effect);
    gamepad.ungrab()?;
    Ok(())
}
