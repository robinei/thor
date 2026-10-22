//! The Thor's gyro ([`SensorHub`]). Its IMU (SH5001) belongs to Qualcomm's sensor hub (SSC) on the
//! ADSP, not to a bus Linux drives. When the ADSP's sensors process is up, the hub offers a QMI service
//! (400) over QRTR. That needs linux-thor's FastRPC patch and device tree, and thor-sensors' hexagonrpcd
//! serving its files. This module reads the service directly, as PB-OS's pbos-motiond does
//! (project-barry/pb-os), using QMI framing and the few protobuf messages of Qualcomm's sensor API.
//!
//! 1. The name service tells the socket where service 400 is, and when it goes and comes back, as on an
//!    ADSP restart ([`qrtr::lookup`]).
//! 2. A SUID lookup asks for the default "gyro" sensor.
//! 3. A std sensor config, which sets the rate, starts its stream. Each report carries x, y and z in
//!    rad/s in the sensor's frame, with the hub's time in 19.2 MHz ticks.
//!
//! The samples are turned into the frame of ClamShell's [`Rate`], which is the Steam Deck's (x right, y
//! up the screen, z toward the player), in degrees per second. The stream runs while the pad thread has
//! the controller, as an evdev motion sensor's does, but not over sleep. The source is dropped before
//! sleep, and its Drop stops the stream (see `crate::power::sleep::spawn_hold`). It is found again
//! after.

use std::io;
use std::os::fd::{AsFd, BorrowedFd, OwnedFd};
use std::time::Duration;

mod wire;

use super::motion::{MotionSource, Rate, Sample};
use crate::os::qrtr;
use wire::{Qmi, Value, pb_bytes, pb_fields, pb_fixed32, pb_fixed64, pb_varint_field, qmi_message, qmi_request};

/// The sensor hub's client service.
const SERVICE: u32 = 400;
/// The sensor API's message ids, from sns_suid.proto, sns_std_sensor.proto and sns_client.proto.
const MSG_SUID_REQUEST: u32 = 512;
const MSG_SUID_EVENT: u32 = 768;
const MSG_STD_SENSOR_CONFIG: u32 = 513;
const MSG_STD_SENSOR_EVENT: u32 = 1025;
const MSG_CLIENT_DISABLE: u32 = 10;
/// The SUID lookup's own SUID.
const SUID_LOOKUP: Suid = Suid { low: 0xabab_abab_abab_abab, high: 0xabab_abab_abab_abab };
/// Our processor, the application processor, with reports only while it is awake.
const PROCESSOR_APSS: u64 = 1;
const DELIVERY_NO_WAKEUP: u64 = 1;
/// Samples per second, as PB-OS uses.
const RATE_HZ: f32 = 200.0;
/// The hub's clock rate.
const TICKS_PER_SECOND: u64 = 19_200_000;

/// Turns the Thor's sensor frame into the Deck's: Deck x = -sensor y, y = sensor x. It is a quarter turn,
/// because the Thor's panels are portrait. The turn is PB-OS's, "the frame MaSi's and Batocera's bridges
/// validated on a Thor".
const THOR_TURN: [[f32; 3]; 3] = [[0.0, -1.0, 0.0], [1.0, 0.0, 0.0], [0.0, 0.0, 1.0]];

/// A sensor's id.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Suid {
    low: u64,
    high: u64,
}

/// The sensor hub's gyro, while there is one.
pub struct SensorHub {
    socket: OwnedFd,
    /// The hub's service node and port, once the name service has given them.
    service: Option<(u32, u32)>,
    /// The gyro that is streaming.
    gyro: Option<Suid>,
    txn: u16,
    /// The buffer for a datagram.
    buf: Vec<u8>,
}

impl SensorHub {
    /// The Thor's sensor hub, found by the device tree, or none on other machines. It waits for the hub's
    /// service, which may come up later than this, with thor-sensors' hexagonrpcd.
    pub fn find() -> Option<SensorHub> {
        let compatible = std::fs::read("/sys/firmware/devicetree/base/compatible")
            .inspect_err(|e| log::debug!("pad: reading /sys/firmware/devicetree/base/compatible: {e}"))
            .ok()?;
        if !compatible.split(|&b| b == 0).any(|c| c == b"ayn,thor") {
            return None;
        }
        let hub = (|| {
            let (socket, node) = qrtr::socket()?;
            qrtr::lookup(&socket, node, SERVICE)?;
            Ok::<_, io::Error>(SensorHub { socket, service: None, gyro: None, txn: 0, buf: vec![0; 65536] })
        })();
        hub.map_err(|e| log::warn!("pad: the sensor hub: {e}")).ok().inspect(|_| log::info!("pad: gyro from the sensor hub, once it is up"))
    }

    /// Sends a request to the hub and returns whether it was sent. A send can fail when the ADSP is gone
    /// before the name service says so. Then the hub is dropped until it is announced again, rather than
    /// dropping the gyro for good as PB-OS does.
    fn request(&mut self, suid: Suid, msg_id: u32, payload: &[u8]) -> bool {
        let Some((node, port)) = self.service else { return false };
        self.txn = self.txn.wrapping_add(1);
        if let Err(e) = qrtr::send_to(&self.socket, node, port, &qmi_request(self.txn, &client_request(suid, msg_id, payload))) {
            log::warn!("pad: the sensor hub: {e} (until it is back)");
            self.service = None;
            self.gyro = None;
            return false;
        }
        true
    }

    /// Handles one datagram from the name service, saying the hub came, so the gyro is looked up, or went.
    fn control(&mut self, pkt: &[u8]) {
        match qrtr::control(pkt) {
            Some((qrtr::NEW_SERVER, SERVICE, node, port)) if node != 0 || port != 0 => {
                log::info!("pad: the sensor hub is up");
                self.service = Some((node, port));
                self.gyro = None;
                let mut lookup = Vec::new();
                pb_bytes(&mut lookup, 1, b"gyro");
                pb_varint_field(&mut lookup, 2, 1); // register_updates
                pb_varint_field(&mut lookup, 3, 1); // default_only
                self.request(SUID_LOOKUP, MSG_SUID_REQUEST, &lookup);
            }
            Some((qrtr::DEL_SERVER, SERVICE, node, port)) if self.service == Some((node, port)) => {
                log::info!("pad: the sensor hub is gone (until it is back)");
                self.service = None;
                self.gyro = None;
            }
            _ => {}
        }
    }

    /// Handles one QMI datagram from the hub. A refusal is logged. The gyro's SUID starts the gyro, and
    /// after that the reports carry its samples.
    fn message(&mut self, msg: &[u8], out: &mut Vec<Sample>) {
        match qmi_message(msg) {
            Some(Qmi::Refused(error)) => {
                // The config was refused, so start again from the lookup's next report.
                log::warn!("pad: the sensor hub refused a request (error {error})");
                self.gyro = None;
            }
            Some(Qmi::Report(report)) => match self.gyro {
                None => {
                    if let Some(suid) = suid_of(report, b"gyro") {
                        let mut config = Vec::new();
                        pb_fixed32(&mut config, 1, RATE_HZ.to_bits());
                        if self.request(suid, MSG_STD_SENSOR_CONFIG, &config) {
                            self.gyro = Some(suid);
                            log::info!("pad: the gyro streams at {RATE_HZ} Hz");
                        }
                    }
                }
                Some(_) => samples(report, |[x, y, z], ticks| {
                    let d = turned(THOR_TURN, [x, y, z]).map(f32::to_degrees);
                    let at = Duration::from_nanos(u64::try_from(u128::from(ticks) * 1_000_000_000 / u128::from(TICKS_PER_SECOND)).unwrap_or(u64::MAX));
                    out.push(Sample { rate: Rate { pitch: d[0], yaw: d[1], roll: d[2] }, at });
                }),
            },
            None => {}
        }
    }
}

impl Drop for SensorHub {
    fn drop(&mut self) {
        if let Some(gyro) = self.gyro.take() {
            self.request(gyro, MSG_CLIENT_DISABLE, &[]);
        }
    }
}

impl MotionSource for SensorHub {
    fn fd(&self) -> BorrowedFd<'_> {
        self.socket.as_fd()
    }

    fn fetch(&mut self, out: &mut Vec<Sample>) -> io::Result<()> {
        let mut buf = std::mem::take(&mut self.buf);
        let result = (|| {
            while let Some((n, _, port)) = qrtr::recv_from(&self.socket, &mut buf)? {
                let msg = buf.get(..n).unwrap_or_default();
                // Log the datagrams as they come, for the tests' fixtures. See docs/thor-gameplan.md.
                if log::log_enabled!(log::Level::Trace) {
                    log::trace!("port {port:#x}: {}", msg.iter().map(|b| format!("{b:02x}")).collect::<String>());
                }
                if port == qrtr::PORT_CTRL {
                    self.control(msg);
                } else {
                    self.message(msg, out);
                }
            }
            Ok(())
        })();
        self.buf = buf;
        result
    }
}

/// `turn` × `v`.
fn turned(turn: [[f32; 3]; 3], v: [f32; 3]) -> [f32; 3] {
    turn.map(|row| row.iter().zip(v).map(|(a, b)| a * b).sum())
}

fn pb_suid(s: Suid) -> Vec<u8> {
    let mut out = Vec::new();
    pb_fixed64(&mut out, 1, s.low);
    pb_fixed64(&mut out, 2, s.high);
    out
}

/// An sns_client_request_msg to `suid`: message `msg_id` with `payload`, for this processor while awake.
fn client_request(suid: Suid, msg_id: u32, payload: &[u8]) -> Vec<u8> {
    let mut susp = Vec::new();
    pb_varint_field(&mut susp, 1, PROCESSOR_APSS);
    pb_varint_field(&mut susp, 2, DELIVERY_NO_WAKEUP);
    let mut request = Vec::new();
    if !payload.is_empty() {
        pb_bytes(&mut request, 2, payload);
    }
    let mut m = Vec::new();
    pb_bytes(&mut m, 1, &pb_suid(suid));
    pb_fixed32(&mut m, 2, msg_id);
    pb_bytes(&mut m, 3, &susp);
    pb_bytes(&mut m, 4, &request);
    m
}

/// A report's events, which are field 2 of sns_client_event_msg: each one's message id, time and payload.
fn events(report: &[u8]) -> impl Iterator<Item = (u32, u64, &[u8])> {
    pb_fields(report).filter_map(|(field, _, v)| match (field, v) {
        (2, Value::Bytes(event)) => {
            let (mut id, mut at, mut payload) = (0, 0, &[][..]);
            for (f, _, v) in pb_fields(event) {
                match (f, v) {
                    (1, Value::Num(n)) => id = n as u32,
                    (2, Value::Num(n)) => at = n,
                    (3, Value::Bytes(b)) => payload = b,
                    _ => {}
                }
            }
            Some((id, at, payload))
        }
        _ => None,
    })
}

/// The SUID a lookup report gives for `data_type`, if it is one.
fn suid_of(report: &[u8], data_type: &[u8]) -> Option<Suid> {
    events(report).filter(|(id, _, _)| *id == MSG_SUID_EVENT).find_map(|(_, _, payload)| {
        let mut of_type = false;
        let mut first = None;
        for (field, _, v) in pb_fields(payload) {
            match (field, v) {
                (1, Value::Bytes(t)) => of_type = t == data_type,
                (2, Value::Bytes(s)) if first.is_none() => {
                    let (mut low, mut high) = (0, 0);
                    for (f, _, v) in pb_fields(s) {
                        match (f, v) {
                            (1, Value::Num(n)) => low = n,
                            (2, Value::Num(n)) => high = n,
                            _ => {}
                        }
                    }
                    first = Some(Suid { low, high });
                }
                _ => {}
            }
        }
        first.filter(|_| of_type)
    })
}

/// Calls `on` with the x, y and z of each std sensor event in a report, and its time in the hub's ticks.
fn samples(report: &[u8], mut on: impl FnMut([f32; 3], u64)) {
    for (_, at, payload) in events(report).filter(|(id, _, _)| *id == MSG_STD_SENSOR_EVENT) {
        let mut v = Vec::with_capacity(3);
        for (field, wire, value) in pb_fields(payload) {
            match (field, wire, value) {
                (1, 5, Value::Num(n)) => v.push(f32::from_bits(n as u32)),
                // Packed.
                (1, 2, Value::Bytes(b)) => v.extend(b.chunks_exact(4).filter_map(|c| c.try_into().ok()).map(f32::from_le_bytes)),
                _ => {}
            }
        }
        if let [x, y, z, ..] = v[..] {
            on([x, y, z], at);
        }
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::indexing_slicing, clippy::panic)]
mod tests {
    use super::wire::*;
    use super::*;

    /// A report (sns_client_event_msg) with these events, each as (message id, time, payload).
    fn report(events: &[(u32, u64, Vec<u8>)]) -> Vec<u8> {
        let mut r = Vec::new();
        pb_bytes(&mut r, 1, &pb_suid(Suid { low: 1, high: 2 }));
        for (id, at, payload) in events {
            let mut e = Vec::new();
            pb_fixed32(&mut e, 1, *id);
            pb_fixed64(&mut e, 2, *at);
            pb_bytes(&mut e, 3, payload);
            pb_bytes(&mut r, 2, &e);
        }
        r
    }

    /// A QMI indication carrying `pb` as a report.
    fn indication(pb: &[u8]) -> Vec<u8> {
        let mut tlv = vec![0x02];
        tlv.extend_from_slice(&((pb.len() + 2) as u16).to_le_bytes());
        tlv.extend_from_slice(&(pb.len() as u16).to_le_bytes());
        tlv.extend_from_slice(pb);
        let mut msg = vec![QMI_TYPE_INDICATION, 0, 0];
        msg.extend_from_slice(&QMI_REPORT.to_le_bytes());
        msg.extend_from_slice(&(tlv.len() as u16).to_le_bytes());
        msg.extend_from_slice(&tlv);
        msg
    }

    #[test]
    fn a_request_is_framed_as_the_hub_wants() {
        let pb = client_request(SUID_LOOKUP, MSG_SUID_REQUEST, b"x");
        let msg = qmi_request(7, &pb);
        assert_eq!(&msg[..7], &[QMI_TYPE_REQUEST, 7, 0, 0x20, 0, (pb.len() + 5) as u8, 0]);
        assert_eq!(msg[7], 0x01);
        assert_eq!(u16::from_le_bytes([msg[8], msg[9]]) as usize, pb.len() + 2);
        assert_eq!(&msg[12..], &pb[..]);
        let fields: Vec<_> = pb_fields(&pb).map(|(f, w, _)| (f, w)).collect();
        assert_eq!(fields, vec![(1, 2), (2, 5), (3, 2), (4, 2)]);
    }

    #[test]
    fn a_lookup_report_gives_the_gyros_suid_and_no_other() {
        let mut gyro = Vec::new();
        pb_bytes(&mut gyro, 1, b"gyro");
        pb_bytes(&mut gyro, 2, &pb_suid(Suid { low: 0x11, high: 0x22 }));
        pb_bytes(&mut gyro, 2, &pb_suid(Suid { low: 0x33, high: 0x44 }));
        let r = report(&[(MSG_SUID_EVENT, 0, gyro)]);
        let msg = indication(&r);
        let Some(Qmi::Report(pb)) = qmi_message(&msg) else { panic!("a report") };
        assert_eq!(suid_of(pb, b"gyro"), Some(Suid { low: 0x11, high: 0x22 }), "the first");
        assert_eq!(suid_of(pb, b"accel"), None);
    }

    #[test]
    fn samples_come_with_their_time_packed_or_not() {
        let mut plain = Vec::new();
        for v in [1.0f32, 2.0, 3.0] {
            pb_fixed32(&mut plain, 1, v.to_bits());
        }
        let packed_values: Vec<u8> = [4.0f32, 5.0, 6.0].iter().flat_map(|v| v.to_le_bytes()).collect();
        let mut packed = Vec::new();
        pb_bytes(&mut packed, 1, &packed_values);
        let r = report(&[(MSG_STD_SENSOR_EVENT, 100, plain), (MSG_SUID_EVENT, 0, vec![]), (MSG_STD_SENSOR_EVENT, 200, packed)]);
        let mut got = Vec::new();
        samples(&r, |v, at| got.push((v, at)));
        assert_eq!(got, vec![([1.0, 2.0, 3.0], 100), ([4.0, 5.0, 6.0], 200)]);
    }

    #[test]
    fn a_refusal_is_told_and_an_ok_response_is_nothing() {
        let response = |result: u16, error: u16| {
            let mut v = result.to_le_bytes().to_vec();
            v.extend_from_slice(&error.to_le_bytes());
            let mut msg = vec![QMI_TYPE_RESPONSE, 1, 0];
            msg.extend_from_slice(&QMI_REQUEST.to_le_bytes());
            msg.extend_from_slice(&7u16.to_le_bytes());
            msg.extend_from_slice(&[0x02, 4, 0]);
            msg.extend_from_slice(&v);
            msg
        };
        assert_eq!(qmi_message(&response(1, 0x30)), Some(Qmi::Refused(0x30)));
        assert_eq!(qmi_message(&response(0, 0)), None);
        assert_eq!(qmi_message(&[1, 2]), None, "malformed");
    }

    #[test]
    fn the_thors_frame_is_turned_a_quarter() {
        assert_eq!(turned(THOR_TURN, [1.0, 2.0, 3.0]), [-2.0, 1.0, 3.0]);
    }
}
