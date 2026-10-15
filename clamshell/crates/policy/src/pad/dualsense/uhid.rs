//! The /dev/uhid protocol (include/uapi/linux/uhid.h): a HID device made in userspace by writing
//! `struct uhid_event`s (create, input reports, replies to report requests, destroy) and reading the
//! kernel's (start, open, output reports, report requests). Each event is a 32-bit type and a packed
//! payload; short writes are padded with zeros by the kernel. Encoding and decoding only, no I/O.

pub const VERSION: u32 = 0x0100;
/// `struct uhid_event`: the type and the largest payload (UHID_CREATE2's).
pub const EVENT_SIZE: usize = 4 + 4372;
/// The most a report carries.
const DATA_MAX: usize = 4096;
const BUS_USB: u16 = 0x03;

const DESTROY: u32 = 1;
const START: u32 = 2;
const STOP: u32 = 3;
const OPEN: u32 = 4;
const CLOSE: u32 = 5;
const OUTPUT: u32 = 6;
const GET_REPORT: u32 = 9;
const GET_REPORT_REPLY: u32 = 10;
const CREATE2: u32 = 11;
const INPUT2: u32 = 12;
const SET_REPORT: u32 = 13;
const SET_REPORT_REPLY: u32 = 14;

/// What the kernel's events say.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum KernelEvent {
    /// A driver bound the device.
    Start,
    Stop,
    /// Someone opened it (the driver, a hidraw reader); input reports are read.
    Open,
    Close,
    /// An output report (rumble, LEDs).
    Output(Vec<u8>),
    /// A report wanted: answer with [`get_report_reply`] and the same id.
    GetReport { id: u32, report: u8 },
    /// A report sent through SET_REPORT: answer with [`set_report_reply`].
    SetReport { id: u32, report: u8 },
    Other(u32),
}

fn header(kind: u32, size: usize) -> Vec<u8> {
    let mut e = Vec::with_capacity(size);
    e.extend_from_slice(&kind.to_le_bytes());
    e
}

/// A fixed-size text field, zero-padded (cut if too long, keeping the terminating zero).
fn text(e: &mut Vec<u8>, s: &str, size: usize) {
    let bytes = s.as_bytes();
    let n = bytes.len().min(size - 1);
    e.extend_from_slice(bytes.get(..n).unwrap_or_default());
    e.resize(e.len() + size - n, 0);
}

/// UHID_CREATE2: a USB device named `name` with this report descriptor.
pub fn create2(name: &str, uniq: &str, vendor: u32, product: u32, descriptor: &[u8]) -> Vec<u8> {
    let mut e = header(CREATE2, EVENT_SIZE);
    text(&mut e, name, 128);
    text(&mut e, "clamshell", 64); // phys
    text(&mut e, uniq, 64);
    let len = descriptor.len().min(DATA_MAX);
    e.extend_from_slice(&(len as u16).to_le_bytes());
    e.extend_from_slice(&BUS_USB.to_le_bytes());
    for v in [vendor, product, VERSION, 0] {
        e.extend_from_slice(&v.to_le_bytes());
    }
    e.extend_from_slice(descriptor.get(..len).unwrap_or_default());
    e
}

/// UHID_INPUT2: an input report.
pub fn input2(report: &[u8]) -> Vec<u8> {
    let len = report.len().min(DATA_MAX);
    let mut e = header(INPUT2, 6 + len);
    e.extend_from_slice(&(len as u16).to_le_bytes());
    e.extend_from_slice(report.get(..len).unwrap_or_default());
    e
}

/// UHID_GET_REPORT_REPLY: the report asked for, or an errno (`Err`).
pub fn get_report_reply(id: u32, reply: Result<&[u8], u16>) -> Vec<u8> {
    let (err, data) = match reply {
        Ok(data) => (0, data.get(..data.len().min(DATA_MAX)).unwrap_or_default()),
        Err(errno) => (errno, &[][..]),
    };
    let mut e = header(GET_REPORT_REPLY, 12 + data.len());
    e.extend_from_slice(&id.to_le_bytes());
    e.extend_from_slice(&err.to_le_bytes());
    e.extend_from_slice(&(data.len() as u16).to_le_bytes());
    e.extend_from_slice(data);
    e
}

/// UHID_SET_REPORT_REPLY: done (0) or an errno.
pub fn set_report_reply(id: u32, err: u16) -> Vec<u8> {
    let mut e = header(SET_REPORT_REPLY, 10);
    e.extend_from_slice(&id.to_le_bytes());
    e.extend_from_slice(&err.to_le_bytes());
    e
}

pub fn destroy() -> Vec<u8> {
    header(DESTROY, 4)
}

fn u16_at(b: &[u8], at: usize) -> Option<u16> {
    Some(u16::from_le_bytes([*b.get(at)?, *b.get(at + 1)?]))
}

fn u32_at(b: &[u8], at: usize) -> Option<u32> {
    Some(u32::from_le_bytes([*b.get(at)?, *b.get(at + 1)?, *b.get(at + 2)?, *b.get(at + 3)?]))
}

/// An event read from the kernel (`None`: too short to be one).
pub fn parse(b: &[u8]) -> Option<KernelEvent> {
    let event = match u32_at(b, 0)? {
        START => KernelEvent::Start,
        STOP => KernelEvent::Stop,
        OPEN => KernelEvent::Open,
        CLOSE => KernelEvent::Close,
        OUTPUT => {
            // struct uhid_output_req: data[4096], size, rtype.
            let size = usize::from(u16_at(b, 4 + DATA_MAX)?).min(DATA_MAX);
            KernelEvent::Output(b.get(4..4 + size)?.to_vec())
        }
        // struct uhid_get_report_req and uhid_set_report_req: id, rnum, rtype, ...
        GET_REPORT => KernelEvent::GetReport { id: u32_at(b, 4)?, report: *b.get(8)? },
        SET_REPORT => KernelEvent::SetReport { id: u32_at(b, 4)?, report: *b.get(8)? },
        other => KernelEvent::Other(other),
    };
    Some(event)
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::indexing_slicing)]
mod tests {
    use super::*;

    #[test]
    fn create2_lays_out_the_request() {
        let e = create2("pad", "aa:bb", 0x054c, 0x0ce6, &[1, 2, 3]);
        assert_eq!(u32_at(&e, 0), Some(CREATE2));
        assert_eq!(&e[4..8], b"pad\0");
        assert_eq!(&e[132..142], b"clamshell\0");
        assert_eq!(&e[196..202], b"aa:bb\0");
        assert_eq!((u16_at(&e, 260), u16_at(&e, 262)), (Some(3), Some(BUS_USB)));
        assert_eq!((u32_at(&e, 264), u32_at(&e, 268), u32_at(&e, 272)), (Some(0x054c), Some(0x0ce6), Some(VERSION)));
        assert_eq!(&e[280..], &[1, 2, 3]);
        let long = create2(&"x".repeat(300), "", 0, 0, &[]);
        assert_eq!((long[130], long[131]), (b'x', 0), "cut, still terminated");
    }

    #[test]
    fn writes_and_reads() {
        assert_eq!(input2(&[1, 9]), vec![12, 0, 0, 0, 2, 0, 1, 9]);
        assert_eq!(get_report_reply(7, Ok(&[5, 6])), vec![10, 0, 0, 0, 7, 0, 0, 0, 0, 0, 2, 0, 5, 6]);
        assert_eq!(get_report_reply(7, Err(5)), vec![10, 0, 0, 0, 7, 0, 0, 0, 5, 0, 0, 0]);
        assert_eq!(set_report_reply(3, 0), vec![14, 0, 0, 0, 3, 0, 0, 0, 0, 0]);
        assert_eq!(destroy(), vec![1, 0, 0, 0]);

        let mut event = vec![0u8; EVENT_SIZE];
        event[0] = OUTPUT as u8;
        event[4..7].copy_from_slice(&[2, 3, 4]);
        event[4 + DATA_MAX] = 3;
        assert_eq!(parse(&event), Some(KernelEvent::Output(vec![2, 3, 4])));
        let mut event = vec![0u8; EVENT_SIZE];
        event[0] = GET_REPORT as u8;
        event[4] = 42;
        event[8] = 0x20;
        assert_eq!(parse(&event), Some(KernelEvent::GetReport { id: 42, report: 0x20 }));
        assert_eq!(parse(&[START as u8, 0, 0, 0]), Some(KernelEvent::Start));
        assert_eq!(parse(&[1, 0]), None);
    }
}
