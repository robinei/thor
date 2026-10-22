//! The sensor hub's wire formats: the little of protobuf the sensor API needs, and QMI's framing of its
//! messages.

/// QMI: the sensor client's request, report and large report; message types.
pub(super) const QMI_REQUEST: u16 = 0x20;
pub(super) const QMI_REPORT: u16 = 0x21;
pub(super) const QMI_JUMBO_REPORT: u16 = 0x22;
pub(super) const QMI_TYPE_REQUEST: u8 = 0;
pub(super) const QMI_TYPE_RESPONSE: u8 = 2;
pub(super) const QMI_TYPE_INDICATION: u8 = 4;

pub(super) fn pb_varint(out: &mut Vec<u8>, mut v: u64) {
    while v >= 0x80 {
        out.push((v as u8) | 0x80);
        v >>= 7;
    }
    out.push(v as u8);
}

pub(super) fn pb_key(out: &mut Vec<u8>, field: u32, wire: u8) {
    pb_varint(out, u64::from(field) << 3 | u64::from(wire));
}

pub(super) fn pb_varint_field(out: &mut Vec<u8>, field: u32, v: u64) {
    pb_key(out, field, 0);
    pb_varint(out, v);
}

pub(super) fn pb_fixed32(out: &mut Vec<u8>, field: u32, v: u32) {
    pb_key(out, field, 5);
    out.extend_from_slice(&v.to_le_bytes());
}

pub(super) fn pb_fixed64(out: &mut Vec<u8>, field: u32, v: u64) {
    pb_key(out, field, 1);
    out.extend_from_slice(&v.to_le_bytes());
}

pub(super) fn pb_bytes(out: &mut Vec<u8>, field: u32, b: &[u8]) {
    pb_key(out, field, 2);
    pb_varint(out, b.len() as u64);
    out.extend_from_slice(b);
}

/// One field: its number, and its value (a number, or the bytes of a length-delimited one).
#[derive(Debug, Clone, Copy, PartialEq)]
pub(super) enum Value<'a> {
    Num(u64),
    Bytes(&'a [u8]),
}

/// A protobuf message's fields, until the end or a malformed one.
pub(super) fn pb_fields(mut msg: &[u8]) -> impl Iterator<Item = (u32, u8, Value<'_>)> {
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

/// A QMI request carrying a sensor API message: TLV 1 holds its length and the message.
pub(super) fn qmi_request(txn: u16, pb: &[u8]) -> Vec<u8> {
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
pub(super) enum Qmi<'a> {
    /// A request refused: its error.
    Refused(u16),
    /// A report's sensor API message.
    Report(&'a [u8]),
}

pub(super) fn qmi_message(msg: &[u8]) -> Option<Qmi<'_>> {
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
