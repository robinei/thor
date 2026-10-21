//! The Thor's gyro ([`SensorHub`]): its IMU (SH5001) belongs to Qualcomm's sensor hub (SSC) on the ADSP,
//! not to a bus Linux drives. With the ADSP's sensors process up (linux-thor's FastRPC patch and device
//! tree, thor-sensors' hexagonrpcd serving its files), the hub offers a QMI service (400) over QRTR, which
//! this reads directly, as PB-OS's pbos-motiond does (project-barry/pb-os): QMI framing and the few
//! protobuf messages of Qualcomm's sensor API.
//!
//! 1. The name service tells the socket where service 400 is, and when it goes and comes back (an ADSP
//!    restart): [`qrtr::lookup`].
//! 2. A SUID lookup asks for the default "gyro" sensor.
//! 3. A std sensor config (the rate) starts its stream; each report carries x, y, z in rad/s in the
//!    sensor's frame, with the hub's time (19.2 MHz ticks).
//!
//! Turned into the frame ClamShell's [`Rate`] is in (the Steam Deck's: x right, y up the screen, z
//! toward the player), in degrees per second. The stream runs while the pad thread has the controller,
//! as an evdev motion sensor's does, but not over sleep: the source is dropped before it (its Drop
//! stops the stream; `crate::sleep::spawn_hold`) and found again after.

use std::io;
use std::os::fd::{AsFd, BorrowedFd, OwnedFd};
use std::time::Duration;

use super::motion::{MotionSource, Rate, Sample};
use super::qrtr;

/// The sensor hub's client service.
const SERVICE: u32 = 400;
/// QMI: the sensor client's request, report and large report; message types.
const QMI_REQUEST: u16 = 0x20;
const QMI_REPORT: u16 = 0x21;
const QMI_JUMBO_REPORT: u16 = 0x22;
const QMI_TYPE_REQUEST: u8 = 0;
const QMI_TYPE_RESPONSE: u8 = 2;
const QMI_TYPE_INDICATION: u8 = 4;
/// The sensor API's message ids (sns_suid.proto, sns_std_sensor.proto, sns_client.proto).
const MSG_SUID_REQUEST: u32 = 512;
const MSG_SUID_EVENT: u32 = 768;
const MSG_STD_SENSOR_CONFIG: u32 = 513;
const MSG_STD_SENSOR_EVENT: u32 = 1025;
const MSG_CLIENT_DISABLE: u32 = 10;
/// The SUID lookup's own SUID.
const SUID_LOOKUP: Suid = Suid { low: 0xabab_abab_abab_abab, high: 0xabab_abab_abab_abab };
/// Ours, the application processor; reports while it is awake only.
const PROCESSOR_APSS: u64 = 1;
const DELIVERY_NO_WAKEUP: u64 = 1;
/// Samples per second (PB-OS's).
const RATE_HZ: f32 = 200.0;
/// The hub's clock.
const TICKS_PER_SECOND: u64 = 19_200_000;

/// The Thor's sensor frame turned into the Deck's (a quarter turn: its panels are portrait; PB-OS's,
/// "the frame MaSi's and Batocera's bridges validated on a Thor"): Deck x = -sensor y, y = sensor x.
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
    /// The hub's service: node and port, once the name service has said.
    service: Option<(u32, u32)>,
    /// The gyro streaming.
    gyro: Option<Suid>,
    txn: u16,
    turn: [[f32; 3]; 3],
    /// For a datagram.
    buf: Vec<u8>,
}

impl SensorHub {
    /// The Thor's sensor hub (by its device tree; elsewhere none): waits for the hub's service, which
    /// may come later than this (thor-sensors' hexagonrpcd).
    pub fn find() -> Option<SensorHub> {
        let compatible = std::fs::read("/sys/firmware/devicetree/base/compatible").ok()?;
        if !compatible.split(|&b| b == 0).any(|c| c == b"ayn,thor") {
            return None;
        }
        let hub = (|| {
            let (socket, node) = qrtr::socket()?;
            qrtr::lookup(&socket, node, SERVICE)?;
            Ok::<_, io::Error>(SensorHub { socket, service: None, gyro: None, txn: 0, turn: THOR_TURN, buf: vec![0; 65536] })
        })();
        hub.map_err(|e| log::warn!("pad: the sensor hub: {e}")).ok().inspect(|_| log::info!("pad: gyro from the sensor hub, once it is up"))
    }

    /// Sends a request to the hub. A send that fails (the ADSP gone before the name service said so)
    /// drops the hub until it is announced again, rather than the gyro for good, as PB-OS's.
    fn request(&mut self, suid: Suid, msg_id: u32, payload: &[u8]) -> io::Result<()> {
        let Some((node, port)) = self.service else { return Ok(()) };
        self.txn = self.txn.wrapping_add(1);
        if let Err(e) = qrtr::send_to(&self.socket, node, port, &qmi_request(self.txn, &client_request(suid, msg_id, payload))) {
            log::warn!("pad: the sensor hub: {e} (until it is back)");
            self.service = None;
            self.gyro = None;
        }
        Ok(())
    }

    /// One datagram from the name service: the hub came (look the gyro up) or went.
    fn control(&mut self, pkt: &[u8]) -> io::Result<()> {
        match qrtr::control(pkt) {
            Some((qrtr::NEW_SERVER, SERVICE, node, port)) if node != 0 || port != 0 => {
                log::info!("pad: the sensor hub is up");
                self.service = Some((node, port));
                self.gyro = None;
                let mut lookup = Vec::new();
                pb_bytes(&mut lookup, 1, b"gyro");
                pb_varint_field(&mut lookup, 2, 1); // register_updates
                pb_varint_field(&mut lookup, 3, 1); // default_only
                self.request(SUID_LOOKUP, MSG_SUID_REQUEST, &lookup)
            }
            Some((qrtr::DEL_SERVER, SERVICE, node, port)) if self.service == Some((node, port)) => {
                log::info!("pad: the sensor hub is gone (until it is back)");
                self.service = None;
                self.gyro = None;
                Ok(())
            }
            _ => Ok(()),
        }
    }

    /// One QMI datagram from the hub: a refusal logged; the gyro's SUID (start it); its samples.
    fn message(&mut self, msg: &[u8], out: &mut Vec<Sample>) -> io::Result<()> {
        match qmi_message(msg) {
            Some(Qmi::Refused(error)) => {
                // (Its config refused: started again from the lookup's next report.)
                log::warn!("pad: the sensor hub refused a request (error {error})");
                self.gyro = None;
            }
            Some(Qmi::Report(report)) => match self.gyro {
                None => {
                    if let Some(suid) = suid_of(report, b"gyro") {
                        let mut config = Vec::new();
                        pb_fixed32(&mut config, 1, RATE_HZ.to_bits());
                        self.request(suid, MSG_STD_SENSOR_CONFIG, &config)?;
                        self.gyro = Some(suid);
                        log::info!("pad: the gyro streams at {RATE_HZ} Hz");
                    }
                }
                Some(_) => samples(report, |[x, y, z], ticks| {
                    let d = turned(self.turn, [x, y, z]).map(f32::to_degrees);
                    let at = Duration::from_nanos(u64::try_from(u128::from(ticks) * 1_000_000_000 / u128::from(TICKS_PER_SECOND)).unwrap_or(u64::MAX));
                    out.push(Sample { rate: Rate { pitch: d[0], yaw: d[1], roll: d[2] }, at });
                }),
            },
            None => {}
        }
        Ok(())
    }
}

impl Drop for SensorHub {
    fn drop(&mut self) {
        if let Some(gyro) = self.gyro.take() {
            let _ = self.request(gyro, MSG_CLIENT_DISABLE, &[]);
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
                if port == qrtr::PORT_CTRL {
                    self.control(msg)?;
                } else {
                    self.message(msg, out)?;
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

// --- protobuf: the little the sensor API needs

fn pb_varint(out: &mut Vec<u8>, mut v: u64) {
    while v >= 0x80 {
        out.push((v as u8) | 0x80);
        v >>= 7;
    }
    out.push(v as u8);
}

fn pb_key(out: &mut Vec<u8>, field: u32, wire: u8) {
    pb_varint(out, u64::from(field) << 3 | u64::from(wire));
}

fn pb_varint_field(out: &mut Vec<u8>, field: u32, v: u64) {
    pb_key(out, field, 0);
    pb_varint(out, v);
}

fn pb_fixed32(out: &mut Vec<u8>, field: u32, v: u32) {
    pb_key(out, field, 5);
    out.extend_from_slice(&v.to_le_bytes());
}

fn pb_fixed64(out: &mut Vec<u8>, field: u32, v: u64) {
    pb_key(out, field, 1);
    out.extend_from_slice(&v.to_le_bytes());
}

fn pb_bytes(out: &mut Vec<u8>, field: u32, b: &[u8]) {
    pb_key(out, field, 2);
    pb_varint(out, b.len() as u64);
    out.extend_from_slice(b);
}

/// One field: its number, and its value (a number, or the bytes of a length-delimited one).
#[derive(Debug, Clone, Copy, PartialEq)]
enum Value<'a> {
    Num(u64),
    Bytes(&'a [u8]),
}

/// A protobuf message's fields, until the end or a malformed one.
fn pb_fields(mut msg: &[u8]) -> impl Iterator<Item = (u32, u8, Value<'_>)> {
    fn varint(b: &mut &[u8]) -> Option<u64> {
        let mut v = 0u64;
        for shift in (0..64).step_by(7) {
            let (&c, rest) = b.split_first()?;
            *b = rest;
            v |= u64::from(c & 0x7f) << shift;
            if c & 0x80 == 0 {
                return Some(v);
            }
        }
        None
    }
    fn fixed(b: &mut &[u8], n: usize) -> Option<u64> {
        let (bytes, rest) = b.split_at_checked(n)?;
        *b = rest;
        Some(bytes.iter().rev().fold(0, |v, &c| v << 8 | u64::from(c)))
    }
    std::iter::from_fn(move || {
        let key = varint(&mut msg)?;
        let wire = (key & 7) as u8;
        let value = match wire {
            0 => Value::Num(varint(&mut msg)?),
            1 => Value::Num(fixed(&mut msg, 8)?),
            5 => Value::Num(fixed(&mut msg, 4)?),
            2 => {
                let n = usize::try_from(varint(&mut msg)?).ok()?;
                let (bytes, rest) = msg.split_at_checked(n)?;
                msg = rest;
                Value::Bytes(bytes)
            }
            _ => return None,
        };
        Some(((key >> 3) as u32, wire, value))
    })
}

fn pb_suid(s: Suid) -> Vec<u8> {
    let mut out = Vec::new();
    pb_fixed64(&mut out, 1, s.low);
    pb_fixed64(&mut out, 2, s.high);
    out
}

/// sns_client_request_msg: to `suid`, message `msg_id` with `payload`, for this processor while awake.
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

/// A report's events (sns_client_event_msg: its field 2): each one's message id, time and payload.
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

/// Calls `on` with each std sensor event's x, y, z and its time (the hub's ticks) in a report.
fn samples(report: &[u8], mut on: impl FnMut([f32; 3], u64)) {
    for (_, at, payload) in events(report).filter(|(id, _, _)| *id == MSG_STD_SENSOR_EVENT) {
        let mut v = Vec::with_capacity(3);
        for (field, wire, value) in pb_fields(payload) {
            match (field, wire, value) {
                (1, 5, Value::Num(n)) => v.push(f32::from_bits(n as u32)),
                // (Packed.)
                (1, 2, Value::Bytes(b)) => v.extend(b.chunks_exact(4).filter_map(|c| c.try_into().ok()).map(f32::from_le_bytes)),
                _ => {}
            }
        }
        if let [x, y, z, ..] = v[..] {
            on([x, y, z], at);
        }
    }
}

// --- QMI framing

/// A QMI request carrying a sensor API message: TLV 1 holds its length and the message.
fn qmi_request(txn: u16, pb: &[u8]) -> Vec<u8> {
    let len = |n: usize| u16::try_from(n).unwrap_or(u16::MAX).to_le_bytes();
    let mut tlv = vec![0x01];
    tlv.extend_from_slice(&len(pb.len() + 2));
    tlv.extend_from_slice(&len(pb.len()));
    tlv.extend_from_slice(pb);
    let mut msg = vec![QMI_TYPE_REQUEST];
    msg.extend_from_slice(&txn.to_le_bytes());
    msg.extend_from_slice(&QMI_REQUEST.to_le_bytes());
    msg.extend_from_slice(&len(tlv.len()));
    msg.extend_from_slice(&tlv);
    msg
}

/// What a QMI message from the hub says.
#[derive(Debug, PartialEq)]
enum Qmi<'a> {
    /// A request refused: its error.
    Refused(u16),
    /// A report's sensor API message.
    Report(&'a [u8]),
}

fn qmi_message(msg: &[u8]) -> Option<Qmi<'_>> {
    let u16_at = |b: &[u8], i: usize| b.get(i..i + 2).and_then(|b| b.try_into().ok()).map(u16::from_le_bytes);
    let kind = *msg.first()?;
    let id = u16_at(msg, 3)?;
    let len = usize::from(u16_at(msg, 5)?);
    let mut tlvs = msg.get(7..7 + len)?;
    let mut tlv2 = None;
    while let Some(&t) = tlvs.first() {
        let n = usize::from(u16_at(tlvs, 1)?);
        let value = tlvs.get(3..3 + n)?;
        if t == 0x02 {
            tlv2 = Some(value);
        }
        tlvs = tlvs.get(3 + n..)?;
    }
    let tlv2 = tlv2?;
    match (kind, id) {
        (QMI_TYPE_RESPONSE, QMI_REQUEST) => match (u16_at(tlv2, 0)?, u16_at(tlv2, 2)?) {
            (0, _) => None,
            (_, error) => Some(Qmi::Refused(error)),
        },
        (QMI_TYPE_INDICATION, QMI_REPORT | QMI_JUMBO_REPORT) => {
            let n = usize::from(u16_at(tlv2, 0)?);
            tlv2.get(2..2 + n).map(Qmi::Report)
        }
        _ => None,
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::indexing_slicing, clippy::panic)]
mod tests {
    use super::*;

    /// A report (sns_client_event_msg) with these events (message id, time, payload).
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
