//! The DualSense's USB reports, as the kernel's hid-playstation driver reads and writes them: the HID
//! report descriptor, the input report (0x01: sticks, triggers, buttons, D-pad, motion), the output
//! report (0x02: of which only the rumble motors are used), and the feature reports the driver asks for
//! when it binds (0x05 calibration, 0x09 pairing info with the MAC address, 0x20 firmware info).
//! Written from the layout in drivers/hid/hid-playstation.c (Linux 7.2) and the public descriptions of
//! the controller. Pure: no I/O here.

use std::time::Duration;

use super::motion::Motion;
use super::state::{Button, PadState};
use super::{Answer, Bus, Identity, Motors, Protocol, put};

/// The gamepad's name (hid-playstation adds " Motion Sensors" and " Touchpad" for the other two).
pub const NAME: &str = "clamshell pad (DualSense)";
pub const MOTION_NAME: &str = "clamshell pad (DualSense) Motion Sensors";
pub const TOUCHPAD_NAME: &str = "clamshell pad (DualSense) Touchpad";

pub const VENDOR: u32 = 0x054c;
pub const PRODUCT: u32 = 0x0ce6;

pub const INPUT_REPORT: u8 = 0x01;
/// With its report ID.
pub const INPUT_SIZE: usize = 64;
pub const OUTPUT_REPORT: u8 = 0x02;
pub const CALIBRATION: u8 = 0x05;
const CALIBRATION_SIZE: usize = 41;
pub const PAIRING_INFO: u8 = 0x09;
const PAIRING_INFO_SIZE: usize = 20;
pub const FIRMWARE_INFO: u8 = 0x20;
const FIRMWARE_INFO_SIZE: usize = 64;

/// The report descriptor: a gamepad with report 1 in (6 axes, a sequence byte, the hat, 15 buttons,
/// then vendor bytes for the rest: motion, touch, status), report 2 out (47 bytes) and the three
/// feature reports. hid-playstation reads the reports by layout, not through the descriptor; it is
/// there so that the report sizes are right for anything that does parse it (hidraw users, hid core).
#[rustfmt::skip]
pub const DESCRIPTOR: &[u8] = &[
    0x05, 0x01,             // Usage Page (Generic Desktop)
    0x09, 0x05,             // Usage (Game Pad)
    0xa1, 0x01,             // Collection (Application)
    0x85, INPUT_REPORT,     //   Report ID 1
    0x09, 0x30, 0x09, 0x31, //   Usage (X), (Y): left stick
    0x09, 0x32, 0x09, 0x35, //   Usage (Z), (Rz): right stick
    0x09, 0x33, 0x09, 0x34, //   Usage (Rx), (Ry): L2, R2
    0x15, 0x00,             //   Logical Minimum (0)
    0x26, 0xff, 0x00,       //   Logical Maximum (255)
    0x75, 0x08,             //   Report Size (8)
    0x95, 0x06,             //   Report Count (6)
    0x81, 0x02,             //   Input (Data, Var, Abs)
    0x06, 0x00, 0xff,       //   Usage Page (Vendor 0xff00)
    0x09, 0x20,             //   Usage (0x20): the sequence number
    0x95, 0x01,             //   Report Count (1)
    0x81, 0x02,             //   Input (Data, Var, Abs)
    0x05, 0x01,             //   Usage Page (Generic Desktop)
    0x09, 0x39,             //   Usage (Hat Switch)
    0x15, 0x00,             //   Logical Minimum (0)
    0x25, 0x07,             //   Logical Maximum (7)
    0x35, 0x00,             //   Physical Minimum (0)
    0x46, 0x3b, 0x01,       //   Physical Maximum (315)
    0x65, 0x14,             //   Unit (Degrees)
    0x75, 0x04,             //   Report Size (4)
    0x95, 0x01,             //   Report Count (1)
    0x81, 0x42,             //   Input (Data, Var, Abs, Null State): 8 is centred
    0x45, 0x00,             //   Physical Maximum (0)
    0x65, 0x00,             //   Unit (None)
    0x05, 0x09,             //   Usage Page (Button)
    0x19, 0x01,             //   Usage Minimum (1)
    0x29, 0x0f,             //   Usage Maximum (15)
    0x15, 0x00,             //   Logical Minimum (0)
    0x25, 0x01,             //   Logical Maximum (1)
    0x75, 0x01,             //   Report Size (1)
    0x95, 0x0f,             //   Report Count (15)
    0x81, 0x02,             //   Input (Data, Var, Abs)
    0x06, 0x00, 0xff,       //   Usage Page (Vendor 0xff00)
    0x09, 0x21,             //   Usage (0x21): the rest of the button bytes
    0x95, 0x0d,             //   Report Count (13)
    0x81, 0x02,             //   Input (Data, Var, Abs)
    0x09, 0x22,             //   Usage (0x22): motion, touch, status
    0x26, 0xff, 0x00,       //   Logical Maximum (255)
    0x75, 0x08,             //   Report Size (8)
    0x95, 0x34,             //   Report Count (52)
    0x81, 0x02,             //   Input (Data, Var, Abs)
    0x85, OUTPUT_REPORT,    //   Report ID 2
    0x09, 0x23,             //   Usage (0x23)
    0x95, 0x2f,             //   Report Count (47)
    0x91, 0x02,             //   Output (Data, Var, Abs)
    0x85, CALIBRATION,      //   Report ID 5
    0x09, 0x24,             //   Usage (0x24)
    0x95, 0x28,             //   Report Count (40)
    0xb1, 0x02,             //   Feature (Data, Var, Abs)
    0x85, PAIRING_INFO,     //   Report ID 9
    0x09, 0x25,             //   Usage (0x25)
    0x95, 0x13,             //   Report Count (19)
    0xb1, 0x02,             //   Feature (Data, Var, Abs)
    0x85, FIRMWARE_INFO,    //   Report ID 0x20
    0x09, 0x26,             //   Usage (0x26)
    0x95, 0x3f,             //   Report Count (63)
    0xb1, 0x02,             //   Feature (Data, Var, Abs)
    0xc0,                   // End Collection
];

// Input report bytes (with the report ID at 0).
const STICKS: usize = 1; // left x, left y, right x, right y
const TRIGGERS: usize = 5; // L2, R2
const SEQUENCE: usize = 7;
const BUTTONS: usize = 8; // 4 bytes: hat and face buttons, shoulders and the rest, PS/touchpad/mic
const GYRO: usize = 16; // 3 x i16: pitch (x), yaw (y), roll (z)
const ACCEL: usize = 22; // 3 x i16: x, y, z
const TIMESTAMP: usize = 28; // u32, in 1/3 microseconds
const TOUCH: usize = 33; // 2 x 4 bytes: contact (bit 7 set: no touch), x and y
const STATUS: usize = 53; // battery, jack, mic

/// Bit 7 of a touch point's contact byte: not touching.
const NO_TOUCH: u8 = 0x80;
/// The D-pad centred (0-7 are N, NE, E, SE, S, SW, W, NW).
const HAT_CENTRED: u8 = 8;
/// Battery status: charging complete (high nibble 2), which hid-playstation reads as full. The pad is
/// part of the device; its charge is the device's, shown elsewhere.
const STATUS_FULL: u8 = 0x20;

/// Raw motion units, which the calibration report below makes exact: 16 per degree per second
/// (±2048 °/s in an i16), 8192 per g (±4 g).
pub const GYRO_PER_DEG_S: f32 = 16.0;
pub const ACCEL_PER_G: f32 = 8192.0;

/// Each button's (byte of the four button bytes, bit). Byte 0's low nibble is the hat. The PS button
/// (byte 2, bit 0) is never pressed: the home button is ClamShell's.
const BUTTONS_AT: [(Button, usize, u8); 12] = [
    (Button::West, 0, 4),  // Square
    (Button::South, 0, 5), // Cross
    (Button::East, 0, 6),  // Circle
    (Button::North, 0, 7), // Triangle
    (Button::L1, 1, 0),
    (Button::R1, 1, 1),
    (Button::L2, 1, 2),
    (Button::R2, 1, 3),
    (Button::Select, 1, 4), // Create
    (Button::Start, 1, 5),  // Options
    (Button::L3, 1, 6),
    (Button::R3, 1, 7),
];

/// A stick (-1 to 1) or trigger (0 to 1) as 0-255.
fn byte(v: f32, stick: bool) -> u8 {
    let unit = if stick { (v + 1.0) / 2.0 } else { v };
    (unit.clamp(0.0, 1.0) * 255.0).round() as u8
}

/// The D-pad as the hat's value.
fn hat(pad: &PadState) -> u8 {
    match pad.dpad() {
        (0, -1) => 0,
        (1, -1) => 1,
        (1, 0) => 2,
        (1, 1) => 3,
        (0, 1) => 4,
        (-1, 1) => 5,
        (-1, 0) => 6,
        (-1, -1) => 7,
        _ => HAT_CENTRED,
    }
}

/// The button bytes: the hat in byte 0's low nibble, L2 and R2 also pressed as buttons as soon as
/// their trigger moves (as the real pad does).
fn button_bytes(pad: &PadState) -> [u8; 4] {
    let mut b = [hat(pad), 0, 0, 0];
    let [l2, r2] = pad.triggers.map(|t| byte(t, false) > 0);
    let pressed = |button| pad.pressed(button) || (button == Button::L2 && l2) || (button == Button::R2 && r2);
    for (button, i, bit) in BUTTONS_AT {
        if let Some(byte) = b.get_mut(i)
            && pressed(button)
        {
            *byte |= 1 << bit;
        }
    }
    b
}

fn raw_i16(v: f32, per_unit: f32) -> [u8; 2] {
    ((v * per_unit).round().clamp(f32::from(i16::MIN), f32::from(i16::MAX)) as i16).to_le_bytes()
}

/// Input report 0x01 for the pad and motion as they are: `sequence` counts reports, `ticks` is the
/// sensor clock in 1/3 microseconds (both wrap).
pub fn input_report(pad: &PadState, motion: &Motion, sequence: u8, ticks: u32) -> [u8; INPUT_SIZE] {
    let mut r = [0u8; INPUT_SIZE];
    put(&mut r, 0, &[INPUT_REPORT]);
    put(&mut r, STICKS, &pad.sticks.map(|s| byte(s, true)));
    put(&mut r, TRIGGERS, &pad.triggers.map(|t| byte(t, false)));
    put(&mut r, SEQUENCE, &[sequence]);
    put(&mut r, BUTTONS, &button_bytes(pad));
    for (i, (g, a)) in motion.gyro.iter().zip(motion.accel).enumerate() {
        put(&mut r, GYRO + 2 * i, &raw_i16(*g, GYRO_PER_DEG_S));
        put(&mut r, ACCEL + 2 * i, &raw_i16(a, ACCEL_PER_G));
    }
    put(&mut r, TIMESTAMP, &ticks.to_le_bytes());
    // Both touch points lifted: an all-zero point would be a finger at the corner.
    put(&mut r, TOUCH, &[NO_TOUCH]);
    put(&mut r, TOUCH + 4, &[NO_TOUCH]);
    put(&mut r, STATUS, &[STATUS_FULL]);
    r
}

/// The rumble in an output report (0x02), if it sets one: games and hid-playstation flag the motor
/// bytes as valid with "compatible vibration" (flag 0 bit 0, or flag 2 bit 2 for the newer method)
/// or "haptics select" (flag 0 bit 1). Lightbar, LED, audio and trigger-effect fields are ignored.
pub fn rumble(report: &[u8]) -> Option<Motors> {
    // Report ID, flag 0, flag 1, right motor, left motor; flag 2 is byte 39.
    let (&id, rest) = report.split_first()?;
    if id != OUTPUT_REPORT {
        return None;
    }
    let flag0 = rest.first().copied().unwrap_or(0);
    let flag2 = rest.get(38).copied().unwrap_or(0);
    if flag0 & 0b11 == 0 && flag2 & 0b100 == 0 {
        return None;
    }
    Some(Motors { weak: *rest.get(2)?, strong: *rest.get(3)? })
}

/// The MAC address the pad reports (locally administered, so it can't be a real one's), as the
/// report carries it: least significant byte first.
const MAC: [u8; 6] = [0x01, 0x00, 0x5e, 0xc1, 0xa5, 0x02];

/// The answer to a feature report request, `None` for one the pad doesn't have.
pub fn feature_report(id: u8) -> Option<Vec<u8>> {
    match id {
        CALIBRATION => {
            // Gyro: bias 0, ±16000 raw at ±1000 °/s for each axis; accelerometer: ±8192 at ±1 g. With
            // these hid-playstation reports raw values times 64 for the gyro (1024 per °/s) and as
            // they are for the accelerometer (8192 per g).
            let mut r = vec![0u8; CALIBRATION_SIZE];
            put(&mut r, 0, &[CALIBRATION]);
            let words: [i16; 17] = [
                0, 0, 0, // pitch, yaw, roll bias
                16000, -16000, // pitch plus, minus
                16000, -16000, // yaw
                16000, -16000, // roll
                1000, 1000, // speed plus, minus (°/s)
                8192, -8192, // accelerometer x plus, minus
                8192, -8192, // y
                8192, -8192, // z
            ];
            for (i, w) in words.iter().enumerate() {
                put(&mut r, 1 + 2 * i, &w.to_le_bytes());
            }
            Some(r)
        }
        PAIRING_INFO => {
            let mut r = vec![0u8; PAIRING_INFO_SIZE];
            put(&mut r, 0, &[PAIRING_INFO]);
            put(&mut r, 1, &MAC);
            Some(r)
        }
        FIRMWARE_INFO => {
            let mut r = vec![0u8; FIRMWARE_INFO_SIZE];
            put(&mut r, 0, &[FIRMWARE_INFO]);
            // Build date and time, as text.
            put(&mut r, 1, b"Oct 15 2026");
            put(&mut r, 12, b"12:00:00");
            // Hardware and firmware versions (logged by the driver).
            put(&mut r, 24, &0x0000_0617u32.to_le_bytes());
            put(&mut r, 28, &0x0110_0002u32.to_le_bytes());
            // Feature ("update") version 2.36: 2.21 or later has the driver use the newer rumble
            // method, as current firmware does.
            put(&mut r, 44, &0x0224u16.to_le_bytes());
            Some(r)
        }
        _ => None,
    }
}

/// The DualSense's side of the device: counts its reports.
#[derive(Debug, Default)]
pub struct DualSense {
    sequence: u8,
}

impl Protocol for DualSense {
    fn identity(&self) -> Identity {
        Identity { name: NAME, bus: Bus::Usb, vendor: VENDOR, product: PRODUCT, descriptor: DESCRIPTOR }
    }

    fn input(&mut self, pad: &PadState, motion: &Motion, at: Duration) -> Vec<u8> {
        // The sensor clock counts thirds of a microsecond and wraps.
        let ticks = at.as_micros().wrapping_mul(3) as u32;
        let report = input_report(pad, motion, self.sequence, ticks);
        self.sequence = self.sequence.wrapping_add(1);
        report.to_vec()
    }

    fn feature(&mut self, id: u8) -> Option<Vec<u8>> {
        feature_report(id)
    }

    fn output(&mut self, report: &[u8], _: &PadState, _: Duration) -> Answer {
        Answer { motors: rumble(report), reports: Vec::new() }
    }

    fn every(&self) -> Option<Duration> {
        None
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::indexing_slicing)]
mod tests {
    use super::*;
    use crate::input::{Press, Range, Raw};
    use crate::pad::route::Ranges;
    use evdev::{AbsoluteAxisCode, KeyCode};

    #[test]
    fn the_descriptor_declares_the_reports_hid_playstation_uses() {
        assert!(DESCRIPTOR.len() < 4096, "fits UHID_CREATE2");
        let bits = crate::pad::hid::descriptor::report_bytes(DESCRIPTOR);
        let bytes = |kind: u8, id: u8| bits.iter().find(|(k, _)| *k == (kind, id)).map(|(_, b)| *b);
        assert_eq!(bytes(0x80, INPUT_REPORT), Some(INPUT_SIZE as u32 - 1));
        assert_eq!(bytes(0x90, OUTPUT_REPORT), Some(47));
        assert_eq!(bytes(0xb0, CALIBRATION), Some(CALIBRATION_SIZE as u32 - 1));
        assert_eq!(bytes(0xb0, PAIRING_INFO), Some(PAIRING_INFO_SIZE as u32 - 1));
        assert_eq!(bytes(0xb0, FIRMWARE_INFO), Some(FIRMWARE_INFO_SIZE as u32 - 1));
        assert_eq!(bits.len(), 5);
    }

    fn ranges() -> Ranges {
        Ranges { stick: Range::STICK, trigger: Range { min: 0, max: 1023 } }
    }

    fn report(pad: &PadState) -> [u8; INPUT_SIZE] {
        input_report(pad, &Motion::LEVEL, 0, 0)
    }

    #[test]
    fn an_untouched_pad() {
        let r = report(&PadState::default());
        assert_eq!(r[0], INPUT_REPORT);
        assert_eq!(&r[STICKS..STICKS + 4], &[128; 4], "sticks centred");
        assert_eq!(&r[TRIGGERS..TRIGGERS + 2], &[0, 0]);
        assert_eq!(&r[BUTTONS..BUTTONS + 4], &[HAT_CENTRED, 0, 0, 0], "no buttons, the hat centred");
        assert_eq!((r[TOUCH], r[TOUCH + 4]), (NO_TOUCH, NO_TOUCH), "no touch");
    }

    #[test]
    fn buttons_hat_sticks_and_triggers() {
        let mut pad = PadState::default();
        let r = ranges();
        for key in [KeyCode::BTN_SOUTH, KeyCode::BTN_NORTH, KeyCode::BTN_TR, KeyCode::BTN_START, KeyCode::BTN_THUMBR, KeyCode::BTN_MODE] {
            pad.apply(Raw::key(key, Press::Down), r);
        }
        pad.apply(Raw::abs(AbsoluteAxisCode::ABS_HAT0X, 1), r);
        pad.apply(Raw::abs(AbsoluteAxisCode::ABS_HAT0Y, -1), r);
        pad.apply(Raw::abs(AbsoluteAxisCode::ABS_X, -32768), r);
        pad.apply(Raw::abs(AbsoluteAxisCode::ABS_Y, 32767), r);
        pad.apply(Raw::abs(AbsoluteAxisCode::ABS_RX, 0), r);
        pad.apply(Raw::abs(AbsoluteAxisCode::ABS_Z, 1023), r);
        let out = report(&pad);
        // Cross and Square, the hat north-east.
        assert_eq!(out[BUTTONS], 0b0011_0000 | 1);
        // R1, Options, R3; L2 as a button since its trigger is pressed.
        assert_eq!(out[BUTTONS + 1], 0b1010_0110);
        assert_eq!(out[BUTTONS + 2], 0, "no PS button");
        assert_eq!(&out[STICKS..STICKS + 4], &[0, 255, 128, 128]);
        assert_eq!(&out[TRIGGERS..TRIGGERS + 2], &[255, 0]);
        pad.apply(Raw::key(KeyCode::BTN_SOUTH, Press::Up), r);
        pad.apply(Raw::abs(AbsoluteAxisCode::ABS_HAT0X, 0), r);
        pad.apply(Raw::abs(AbsoluteAxisCode::ABS_Z, 0), r);
        let out = report(&pad);
        assert_eq!(out[BUTTONS], 0b0001_0000, "Square, the hat north");
        assert_eq!(out[BUTTONS + 1], 0b1010_0010);
    }

    #[test]
    fn dpad_buttons_make_the_hat_too() {
        let mut pad = PadState::default();
        pad.apply(Raw::key(KeyCode::BTN_DPAD_DOWN, Press::Down), ranges());
        pad.apply(Raw::key(KeyCode::BTN_DPAD_LEFT, Press::Down), ranges());
        assert_eq!(report(&pad)[BUTTONS], 5, "south-west");
        pad.apply(Raw::key(KeyCode::BTN_DPAD_DOWN, Press::Up), ranges());
        assert_eq!(report(&pad)[BUTTONS], 6, "west");
    }

    #[test]
    fn motion_sequence_and_timestamp() {
        let motion = Motion { gyro: [1.0, -2.5, 0.0], accel: [0.0, 1.0, -0.5] };
        let r = input_report(&PadState::default(), &motion, 7, 0x0102_0304);
        let i16_at = |at: usize| i16::from_le_bytes([r[at], r[at + 1]]);
        assert_eq!([i16_at(GYRO), i16_at(GYRO + 2), i16_at(GYRO + 4)], [16, -40, 0]);
        assert_eq!([i16_at(ACCEL), i16_at(ACCEL + 2), i16_at(ACCEL + 4)], [0, 8192, -4096]);
        assert_eq!(r[SEQUENCE], 7);
        assert_eq!(&r[TIMESTAMP..TIMESTAMP + 4], &[4, 3, 2, 1]);
        let wild = Motion { gyro: [5000.0, -5000.0, 0.0], accel: [9.0, 0.0, 0.0] };
        let r = input_report(&PadState::default(), &wild, 0, 0);
        let i16_at = |at: usize| i16::from_le_bytes([r[at], r[at + 1]]);
        assert_eq!([i16_at(GYRO), i16_at(GYRO + 2), i16_at(ACCEL)], [i16::MAX, i16::MIN, i16::MAX], "clamped");
    }

    #[test]
    fn rumble_from_output_reports() {
        // As hid-playstation sends it: haptics select and the newer vibration flag, 63 bytes.
        let mut kernel = [0u8; 63];
        kernel[0] = OUTPUT_REPORT;
        kernel[1] = 0b10;
        kernel[39] = 0b100;
        kernel[3] = 40; // right: weak
        kernel[4] = 200; // left: strong
        assert_eq!(rumble(&kernel), Some(Motors { strong: 200, weak: 40 }));
        // As a game sends it over hidraw: 48 bytes, the older flag.
        let mut game = [0u8; 48];
        game[0] = OUTPUT_REPORT;
        game[1] = 0b01;
        assert_eq!(rumble(&game), Some(Motors::default()), "motors off");
        // Lightbar only: no rumble change.
        let mut lightbar = [0u8; 63];
        lightbar[0] = OUTPUT_REPORT;
        lightbar[2] = 0b100;
        lightbar[3] = 99;
        assert_eq!(rumble(&lightbar), None);
        assert_eq!(rumble(&[0x31, 0b11, 0, 1, 2]), None, "not the USB report");
        assert_eq!(rumble(&[OUTPUT_REPORT, 0b11]), None, "too short");
    }

    #[test]
    fn feature_reports_as_the_driver_reads_them() {
        let cal = feature_report(CALIBRATION).unwrap();
        assert_eq!((cal.len(), cal[0]), (CALIBRATION_SIZE, CALIBRATION));
        let w = |at: usize| i32::from(i16::from_le_bytes([cal[at], cal[at + 1]]));
        // hid-playstation's gyro scale for each axis: speed_2x * 1024 / (|plus - bias| + |minus - bias|).
        let speed_2x = w(19) + w(21);
        for (bias, plus, minus) in [(1, 7, 9), (3, 11, 13), (5, 15, 17)] {
            let denom = (w(plus) - w(bias)).abs() + (w(minus) - w(bias)).abs();
            assert_eq!(speed_2x * 1024 / denom, 1024 / GYRO_PER_DEG_S as i32, "1 °/s raw as 1024");
        }
        // Accelerometer: bias plus - range/2 = 0, scale 2 * 8192 / range = 1.
        for (plus, minus) in [(23, 25), (27, 29), (31, 33)] {
            let range = w(plus) - w(minus);
            assert_eq!((w(plus) - range / 2, 2 * 8192 / range), (0, 1));
        }
        let pairing = feature_report(PAIRING_INFO).unwrap();
        assert_eq!((pairing.len(), pairing[0], &pairing[1..7]), (PAIRING_INFO_SIZE, PAIRING_INFO, &MAC[..]));
        let fw = feature_report(FIRMWARE_INFO).unwrap();
        assert_eq!((fw.len(), fw[0]), (FIRMWARE_INFO_SIZE, FIRMWARE_INFO));
        assert!(u16::from_le_bytes([fw[44], fw[45]]) >= 0x0215, "the newer rumble method");
        assert_eq!(feature_report(0x08), None);
    }
}
