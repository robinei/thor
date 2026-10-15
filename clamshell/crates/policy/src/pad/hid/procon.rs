//! The Nintendo Switch Pro Controller over Bluetooth, as the kernel's hid-nintendo driver talks to it
//! (drivers/hid/hid-nintendo.c, Linux 7.2) and as the public reverse-engineering notes describe it
//! (dekuNukem's Nintendo_Switch_Reverse_Engineering). Bluetooth because over USB the driver first
//! wants the 0x80 handshake; over Bluetooth it goes straight to subcommands.
//!
//! The host sends output report 0x01 (a packet number, 8 bytes of HD rumble, a subcommand and its
//! arguments) or 0x10 (rumble only). Every subcommand is answered with input report 0x21: the usual
//! header (timer, battery, buttons, sticks) then an ack, the subcommand's id and its data. The ones
//! with data: device info (0x02: the controller type and MAC address) and SPI flash reads (0x10: the
//! factory stick and IMU calibration, the colours; everything else reads as erased, 0xff, so there
//! is no user calibration). The rest (report mode, IMU, vibration, player and home lights) are
//! acknowledged. Otherwise the pad streams the full report 0x30: the header and three IMU samples.
//! hid-nintendo only sends subcommands and rumble when reports have been coming every 8 to 17 ms, so
//! the stream runs all the time, every [`EVERY`]. Pure: no I/O here.

use std::time::Duration;

use super::motion::Motion;
use super::state::{Button, PadState};
use super::{Answer, Bus, Identity, Motors, Protocol, put};

pub const NAME: &str = "clamshell pad (Pro Controller)";
/// hid-nintendo's name for the motion node.
pub const IMU_NAME: &str = "clamshell pad (Pro Controller) (IMU)";
pub const VENDOR: u32 = 0x057e;
pub const PRODUCT: u32 = 0x2009;

pub const SUBCMD_REPLY: u8 = 0x21;
pub const FULL_REPORT: u8 = 0x30;
pub const RUMBLE_AND_SUBCMD: u8 = 0x01;
pub const RUMBLE_ONLY: u8 = 0x10;
/// Input reports, with the report ID.
const INPUT_SIZE: usize = 64;

/// How often the pad reports: within hid-nintendo's 8-17 ms window, with room for jitter.
pub const EVERY: Duration = Duration::from_millis(12);

/// The report descriptor: vendor-defined reports of the real controller's sizes, input 0x21 and 0x30
/// (63 bytes), output 0x01 (48) and 0x10 (9). hid-nintendo reads the reports by layout.
#[rustfmt::skip]
pub const DESCRIPTOR: &[u8] = &[
    0x05, 0x01,             // Usage Page (Generic Desktop)
    0x09, 0x05,             // Usage (Game Pad)
    0xa1, 0x01,             // Collection (Application)
    0x06, 0x00, 0xff,       //   Usage Page (Vendor 0xff00)
    0x15, 0x00,             //   Logical Minimum (0)
    0x26, 0xff, 0x00,       //   Logical Maximum (255)
    0x75, 0x08,             //   Report Size (8)
    0x85, SUBCMD_REPLY,     //   Report ID 0x21: subcommand replies
    0x09, 0x21,             //   Usage (0x21)
    0x95, 0x3f,             //   Report Count (63)
    0x81, 0x02,             //   Input (Data, Var, Abs)
    0x85, FULL_REPORT,      //   Report ID 0x30: the full report
    0x09, 0x30,             //   Usage (0x30)
    0x95, 0x3f,             //   Report Count (63)
    0x81, 0x02,             //   Input (Data, Var, Abs)
    0x85, RUMBLE_AND_SUBCMD, //  Report ID 0x01: rumble and a subcommand
    0x09, 0x01,             //   Usage (0x01)
    0x95, 0x30,             //   Report Count (48)
    0x91, 0x02,             //   Output (Data, Var, Abs)
    0x85, RUMBLE_ONLY,      //   Report ID 0x10: rumble
    0x09, 0x10,             //   Usage (0x10)
    0x95, 0x09,             //   Report Count (9)
    0x91, 0x02,             //   Output (Data, Var, Abs)
    0xc0,                   // End Collection
];

// Report bytes (with the report ID at 0).
const TIMER: usize = 1;
const BATTERY: usize = 2;
const BUTTONS: usize = 3; // 3 bytes: right side, middle, left side
const LEFT_STICK: usize = 6; // 2 x 12 bits: x, y (up is larger)
const RIGHT_STICK: usize = 9;
const VIBRATOR: usize = 12;
const IMU: usize = 13; // 3 samples of accel x, y, z, gyro x, y, z (i16)
const ACK: usize = 13;
const REPLY_TO: usize = 14;
const REPLY_DATA: usize = 15;

/// Battery full (level 4 in the top three bits), powered by the host (bit 0): the pad is part of
/// the device, its charge is the device's.
const BATTERY_FULL: u8 = 0x81;
/// The vibrator byte: non-zero, or hid-nintendo never sends its periodic rumble packets.
const VIBRATOR_READY: u8 = 0x80;

/// Each button's (byte, bit) in the three button bytes. Positions as on the controller: B is the
/// bottom face button, A the right one, Y the left, X the top. Home (1, 4) and Capture (1, 5) are never
/// pressed: the home button is ClamShell's.
const BUTTONS_AT: [(Button, usize, u8); 12] = [
    (Button::West, 0, 0),  // Y
    (Button::North, 0, 1), // X
    (Button::South, 0, 2), // B
    (Button::East, 0, 3),  // A
    (Button::R1, 0, 6),
    (Button::R2, 0, 7), // ZR
    (Button::Select, 1, 0), // Minus
    (Button::Start, 1, 1),  // Plus
    (Button::R3, 1, 2),
    (Button::L3, 1, 3),
    (Button::L1, 2, 6),
    (Button::L2, 2, 7), // ZL
];
/// The D-pad's bits in byte 2: down, up, right, left.
const DPAD: [u8; 4] = [0, 1, 2, 3];
/// A trigger pressed this far is ZL or ZR (they are buttons on this pad).
const TRIGGER_PRESSED: f32 = 0.3;

/// The stick calibration in the flash: centre, and how far it goes either way (12-bit values).
const STICK_CENTRE: u16 = 2048;
const STICK_RANGE: u16 = 1800;

/// Raw IMU units, which the calibration below makes exact for hid-nintendo: 4096 per g, and
/// 14.247 per degree per second (its 13371 gyro scale).
pub const ACCEL_PER_G: f32 = 4096.0;
pub const GYRO_PER_DEG_S: f32 = 14.247;

/// The MAC address the pad reports, as it sends it (most significant byte first); locally
/// administered, so it can't be a real one's.
const MAC: [u8; 6] = [0x02, 0xa5, 0xc1, 0x5e, 0x00, 0x02];

/// Two 12-bit values packed in 3 bytes, as the sticks and their calibration are.
fn pack12(a: u16, b: u16) -> [u8; 3] {
    [(a & 0xff) as u8, ((a >> 8) & 0x0f) as u8 | ((b & 0x0f) << 4) as u8, (b >> 4) as u8]
}

/// A stick (-1 to 1, y positive down) as raw values: up is larger.
fn stick(x: f32, y: f32) -> [u8; 3] {
    let raw = |v: f32| (f32::from(STICK_CENTRE) + v.clamp(-1.0, 1.0) * f32::from(STICK_RANGE)).round() as u16;
    pack12(raw(x), raw(-y))
}

fn i16_bytes(v: f32, per_unit: f32) -> [u8; 2] {
    ((v * per_unit).round().clamp(f32::from(i16::MIN), f32::from(i16::MAX)) as i16).to_le_bytes()
}

/// One IMU sample, in the controller's frame: x towards the triggers (away from the player), y to
/// the left, z up out of the face. [`Motion`] is in the DualSense's frame (x right, y up out of the
/// face, z towards the player).
fn imu_sample(m: &Motion) -> [u8; 12] {
    let frame = |v: [f32; 3]| [-v[2], -v[0], v[1]];
    let (accel, gyro) = (frame(m.accel), frame(m.gyro));
    let mut out = [0u8; 12];
    for i in 0..3 {
        put(&mut out, 2 * i, &i16_bytes(accel.get(i).copied().unwrap_or(0.0), ACCEL_PER_G));
        put(&mut out, 6 + 2 * i, &i16_bytes(gyro.get(i).copied().unwrap_or(0.0), GYRO_PER_DEG_S));
    }
    out
}

/// The header every input report starts with: ID, timer, battery, buttons, sticks, vibrator.
fn header(id: u8, timer: u8, pad: &PadState) -> [u8; INPUT_SIZE] {
    let mut r = [0u8; INPUT_SIZE];
    put(&mut r, 0, &[id]);
    put(&mut r, TIMER, &[timer]);
    put(&mut r, BATTERY, &[BATTERY_FULL]);
    let mut b = [0u8; 3];
    let [zl, zr] = pad.triggers.map(|t| t > TRIGGER_PRESSED);
    for (button, i, bit) in BUTTONS_AT {
        let down = pad.pressed(button) || (button == Button::L2 && zl) || (button == Button::R2 && zr);
        if let Some(byte) = b.get_mut(i)
            && down
        {
            *byte |= 1 << bit;
        }
    }
    let (x, y) = pad.dpad();
    let [down, up, right, left] = DPAD.map(|bit| 1u8 << bit);
    let mut dpad = 0;
    if y > 0 {
        dpad |= down;
    }
    if y < 0 {
        dpad |= up;
    }
    if x > 0 {
        dpad |= right;
    }
    if x < 0 {
        dpad |= left;
    }
    if let Some(byte) = b.get_mut(2) {
        *byte |= dpad;
    }
    put(&mut r, BUTTONS, &b);
    let [lx, ly, rx, ry] = pad.sticks;
    put(&mut r, LEFT_STICK, &stick(lx, ly));
    put(&mut r, RIGHT_STICK, &stick(rx, ry));
    put(&mut r, VIBRATOR, &[VIBRATOR_READY]);
    r
}

/// The full report (0x30): the header and three IMU samples (all the same reading).
pub fn full_report(pad: &PadState, motion: &Motion, timer: u8) -> [u8; INPUT_SIZE] {
    let mut r = header(FULL_REPORT, timer, pad);
    let sample = imu_sample(motion);
    for i in 0..3 {
        put(&mut r, IMU + 12 * i, &sample);
    }
    r
}

/// The flash, as far as anything reads it: `size` bytes from `addr`.
pub fn spi_flash(addr: u32, size: u8) -> Vec<u8> {
    let mut regions: Vec<(u32, Vec<u8>)> = Vec::new();
    // Factory IMU calibration: accelerometer offsets (0) and scales (16384), gyro offsets (0) and
    // scales (13371), each x, y, z as i16.
    let mut imu = Vec::new();
    for (value, n) in [(0i16, 3), (16384, 3), (0, 3), (13371, 3)] {
        for _ in 0..n {
            imu.extend_from_slice(&value.to_le_bytes());
        }
    }
    regions.push((0x6020, imu));
    // Factory stick calibration. Left: above centre x, y; centre x, y; below centre x, y. Right:
    // centre; below; above.
    let (c, r) = (STICK_CENTRE, STICK_RANGE);
    regions.push((0x603d, [pack12(r, r), pack12(c, c), pack12(r, r)].concat()));
    regions.push((0x6046, [pack12(c, c), pack12(r, r), pack12(r, r)].concat()));
    // Colours: body, buttons, left and right grips (RGB).
    regions.push((0x6050, vec![0x32, 0x32, 0x32, 0xe6, 0xe6, 0xe6, 0x32, 0x32, 0x32, 0x32, 0x32, 0x32]));
    (0..u32::from(size))
        .map(|i| {
            let a = addr.wrapping_add(i);
            regions
                .iter()
                .find_map(|(start, bytes)| a.checked_sub(*start).and_then(|o| bytes.get(o as usize)).copied())
                .unwrap_or(0xff)
        })
        .collect()
}

/// The answer to subcommand `id` with arguments `args`: the ack byte and data.
fn subcommand(id: u8, args: &[u8]) -> (u8, Vec<u8>) {
    match id {
        // Device info: firmware 4.33, a Pro Controller (3), then the MAC address, then "colours from
        // the flash".
        0x02 => (0x82, [&[0x04, 0x21, 0x03, 0x02][..], &MAC, &[0x01, 0x01]].concat()),
        // SPI flash read: the address and size echoed, then the bytes (at most 0x1d).
        0x10 => {
            let addr = u32::from_le_bytes([0, 1, 2, 3].map(|i| args.get(i).copied().unwrap_or(0)));
            let size = args.get(4).copied().unwrap_or(0).min(0x1d);
            let mut data = addr.to_le_bytes().to_vec();
            data.push(size);
            data.extend(spi_flash(addr, size));
            (0x90, data)
        }
        _ => (0x80, Vec::new()),
    }
}

/// The strength (0-255) one side's 4 bytes of HD rumble ask for: the larger of the high and the
/// low band's amplitude, by its step in hid-nintendo's amplitude table (0-100). Rough: the bands'
/// frequencies and the table's curve are left out.
fn side(d: &[u8]) -> u8 {
    let byte = |i: usize| d.get(i).copied().unwrap_or(0);
    let high = u32::from(byte(1) & 0xfe) / 2;
    let low = match byte(3) {
        b @ 0x40..=0x72 => u32::from(b - 0x40) * 2 + u32::from(byte(2) >> 7),
        _ => 0,
    };
    (high.max(low).min(100) * 255 / 100) as u8
}

/// The motors an output report's rumble data asks for: the left side is the strong motor, the right
/// the weak one (as hid-nintendo maps force feedback).
pub fn rumble(report: &[u8]) -> Option<Motors> {
    let data = report.get(2..10)?;
    Some(Motors { strong: side(data.get(..4)?), weak: side(data.get(4..)?) })
}

/// The Pro Controller's side of the device: its timer.
#[derive(Debug, Default)]
pub struct ProController {
    timer: u8,
}

impl ProController {
    fn tick(&mut self) -> u8 {
        self.timer = self.timer.wrapping_add(1);
        self.timer
    }
}

impl Protocol for ProController {
    fn identity(&self) -> Identity {
        Identity { name: NAME, bus: Bus::Bluetooth, vendor: VENDOR, product: PRODUCT, descriptor: DESCRIPTOR }
    }

    fn input(&mut self, pad: &PadState, motion: &Motion, _: Duration) -> Vec<u8> {
        full_report(pad, motion, self.tick()).to_vec()
    }

    fn feature(&mut self, _: u8) -> Option<Vec<u8>> {
        None
    }

    fn output(&mut self, report: &[u8], pad: &PadState, _: Duration) -> Answer {
        let motors = match report.first() {
            Some(&(RUMBLE_AND_SUBCMD | RUMBLE_ONLY)) => rumble(report),
            _ => None,
        };
        let mut reports = Vec::new();
        if report.first() == Some(&RUMBLE_AND_SUBCMD)
            && let Some(&id) = report.get(10)
        {
            let (ack, data) = subcommand(id, report.get(11..).unwrap_or_default());
            let mut r = header(SUBCMD_REPLY, self.tick(), pad);
            put(&mut r, ACK, &[ack]);
            put(&mut r, REPLY_TO, &[id]);
            put(&mut r, REPLY_DATA, &data);
            reports.push(r.to_vec());
        }
        Answer { motors, reports }
    }

    fn every(&self) -> Option<Duration> {
        Some(EVERY)
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::indexing_slicing)]
mod tests {
    use super::*;
    use crate::input::{Press, Range, Raw};
    use crate::pad::route::Ranges;
    use evdev::{AbsoluteAxisCode, KeyCode};

    fn unpack12(b: &[u8]) -> (u16, u16) {
        (u16::from(b[0]) | (u16::from(b[1] & 0x0f) << 8), u16::from(b[1] >> 4) | (u16::from(b[2]) << 4))
    }

    #[test]
    fn the_descriptor_declares_the_reports_hid_nintendo_uses() {
        let bytes = crate::pad::hid::descriptor::report_bytes(DESCRIPTOR);
        assert_eq!(bytes, vec![((0x80, 0x21), 63), ((0x80, 0x30), 63), ((0x90, 0x01), 48), ((0x90, 0x10), 9)]);
    }

    #[test]
    fn the_full_report() {
        let ranges = Ranges { stick: Range::STICK, trigger: Range { min: 0, max: 1023 } };
        let mut pad = PadState::default();
        let r = full_report(&pad, &Motion::LEVEL, 5);
        assert_eq!(&r[..3], &[FULL_REPORT, 5, BATTERY_FULL]);
        assert_eq!(&r[BUTTONS..BUTTONS + 3], &[0, 0, 0]);
        assert_eq!(unpack12(&r[LEFT_STICK..]), (STICK_CENTRE, STICK_CENTRE), "centred");
        assert_ne!(r[VIBRATOR], 0);
        // Level at rest: 1 g up out of the face (z), no rotation; three samples.
        let sample: Vec<i16> = (0..6).map(|i| i16::from_le_bytes([r[IMU + 2 * i], r[IMU + 2 * i + 1]])).collect();
        assert_eq!(sample, vec![0, 0, 4096, 0, 0, 0]);
        assert_eq!(&r[IMU..IMU + 12], &r[IMU + 24..IMU + 36]);

        for key in [KeyCode::BTN_SOUTH, KeyCode::BTN_NORTH, KeyCode::BTN_TL, KeyCode::BTN_START, KeyCode::BTN_MODE] {
            pad.apply(Raw::key(key, Press::Down), ranges);
        }
        pad.apply(Raw::abs(AbsoluteAxisCode::ABS_HAT0X, -1), ranges);
        pad.apply(Raw::abs(AbsoluteAxisCode::ABS_HAT0Y, 1), ranges);
        pad.apply(Raw::abs(AbsoluteAxisCode::ABS_RZ, 1023), ranges);
        pad.apply(Raw::abs(AbsoluteAxisCode::ABS_X, 32767), ranges);
        pad.apply(Raw::abs(AbsoluteAxisCode::ABS_Y, -32768), ranges);
        let r = full_report(&pad, &Motion::LEVEL, 6);
        // B (south) and Y (west: xpad's X), ZR from the trigger; Plus; L, D-pad down and left.
        assert_eq!(&r[BUTTONS..BUTTONS + 3], &[0b1000_0101, 0b0000_0010, 0b0100_1001]);
        assert_eq!(unpack12(&r[LEFT_STICK..]), (STICK_CENTRE + STICK_RANGE, STICK_CENTRE + STICK_RANGE), "right and up");
    }

    #[test]
    fn motion_in_the_controllers_frame() {
        // Rolling right (about the DualSense's z, towards the player) is rolling about the Pro
        // Controller's x (away from the player) the other way; tipped onto its right side, gravity
        // reads along its y.
        let m = Motion { gyro: [0.0, 0.0, 10.0], accel: [1.0, 0.0, 0.0] };
        let s = imu_sample(&m);
        let v: Vec<i16> = (0..6).map(|i| i16::from_le_bytes([s[2 * i], s[2 * i + 1]])).collect();
        assert_eq!(v, vec![0, -4096, 0, -142, 0, 0]);
    }

    fn subcmd(id: u8, args: &[u8]) -> Vec<u8> {
        let mut out = vec![RUMBLE_AND_SUBCMD, 0, 0x00, 0x01, 0x40, 0x40, 0x00, 0x01, 0x40, 0x40, id];
        out.extend_from_slice(args);
        out
    }

    #[test]
    fn subcommands_are_answered() {
        let mut pro = ProController::default();
        let pad = PadState::default();
        let answer = pro.output(&subcmd(0x02, &[]), &pad, Duration::ZERO);
        assert_eq!(answer.motors, Some(Motors::default()), "neutral rumble: off");
        let r = &answer.reports[0];
        assert_eq!((r[0], r[ACK], r[REPLY_TO]), (SUBCMD_REPLY, 0x82, 0x02));
        assert_eq!(r[REPLY_DATA + 2], 0x03, "a Pro Controller");
        assert_eq!(&r[REPLY_DATA + 4..REPLY_DATA + 10], &MAC);
        for id in [0x03, 0x30, 0x38, 0x40, 0x48] {
            let r = &pro.output(&subcmd(id, &[1]), &pad, Duration::ZERO).reports[0];
            assert_eq!((r[ACK], r[REPLY_TO]), (0x80, id));
        }
        assert!(pro.output(&[RUMBLE_ONLY, 0, 0, 1, 0x40, 0x40, 0, 1, 0x40, 0x40], &pad, Duration::ZERO).reports.is_empty());
    }

    #[test]
    fn calibration_from_the_flash_as_the_driver_reads_it() {
        let mut pro = ProController::default();
        let read = |pro: &mut ProController, addr: u32, size: u8| {
            let mut args = addr.to_le_bytes().to_vec();
            args.push(size);
            let r = pro.output(&subcmd(0x10, &args), &PadState::default(), Duration::ZERO).reports.remove(0);
            assert_eq!((r[ACK], r[REPLY_TO], &r[REPLY_DATA..REPLY_DATA + 4], r[REPLY_DATA + 4]), (0x90, 0x10, &addr.to_le_bytes()[..], size));
            r[REPLY_DATA + 5..REPLY_DATA + 5 + usize::from(size)].to_vec()
        };
        // No user calibration: no magic (0xb2 0xa1) before it.
        for addr in [0x8010, 0x801b, 0x8026] {
            assert_eq!(read(&mut pro, addr, 2), vec![0xff, 0xff]);
        }
        // Left stick: above x, y; centre x, y; below x, y.
        let left = read(&mut pro, 0x603d, 9);
        assert_eq!([unpack12(&left[0..]), unpack12(&left[3..]), unpack12(&left[6..])], [(1800, 1800), (2048, 2048), (1800, 1800)]);
        let right = read(&mut pro, 0x6046, 9);
        assert_eq!(unpack12(&right[0..]), (2048, 2048), "the right stick starts with its centre");
        // IMU: offsets 0, accelerometer scale 16384, gyro 13371.
        let imu = read(&mut pro, 0x6020, 24);
        let w = |i: usize| i16::from_le_bytes([imu[2 * i], imu[2 * i + 1]]);
        assert_eq!((0..12).map(w).collect::<Vec<_>>(), vec![0, 0, 0, 16384, 16384, 16384, 0, 0, 0, 13371, 13371, 13371]);
        assert_eq!(read(&mut pro, 0x601f, 2), vec![0xff, 0x00], "regions end where they end");
    }

    #[test]
    fn hd_rumble_as_a_rough_strength() {
        // hid-nintendo's encoding: a left (strong) amplitude of step 0x64/2 = 50 in the high band,
        // the low band's 0x8059 (step 51); the right (weak) side off.
        let report = [RUMBLE_ONLY, 0, 0x74, 0x64, 0x8f, 0x59, 0x00, 0x01, 0x40, 0x40];
        assert_eq!(rumble(&report), Some(Motors { strong: 130, weak: 0 }));
        let full = [RUMBLE_ONLY, 0, 0x00, 0xc9, 0x40, 0x72, 0x00, 0xc8, 0x40, 0x40];
        assert_eq!(rumble(&full), Some(Motors { strong: 255, weak: 255 }));
        assert_eq!(rumble(&[RUMBLE_ONLY, 0, 1]), None, "too short");
    }
}
