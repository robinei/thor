//! The Wayland wire format as the proxy needs it: splitting and making messages, and passing them on with
//! the file descriptors that come with them.

use std::io::{self, IoSlice, IoSliceMut};
use std::mem::MaybeUninit;
use std::os::fd::{AsFd, BorrowedFd, OwnedFd};
use std::os::unix::net::UnixStream;
use std::time::Instant;

use rustix::net::{RecvAncillaryBuffer, RecvAncillaryMessage, RecvFlags, SendAncillaryBuffer, SendAncillaryMessage, SendFlags};

use super::objects::Pass;

/// The most file descriptors a message may carry, as in libwayland.
pub(super) const MAX_FDS: usize = 28;
/// The size field is 16 bits and every message occupies whole 32-bit words.
const MAX_MESSAGE_BYTES: usize = (u16::MAX as usize) & !3;
type Messages = (Vec<(u32, u16, std::ops::Range<usize>)>, usize);

/// Makes a message, with its header, from its object, opcode and arguments.
pub(super) fn message(object: u32, opcode: u16, args: &[u8]) -> Vec<u8> {
    let mut m = object.to_ne_bytes().to_vec();
    m.extend((((args.len() + 8) as u32) << 16 | u32::from(opcode)).to_ne_bytes());
    m.extend_from_slice(args);
    m
}

pub(super) fn ints(values: &[i32]) -> Vec<u8> {
    values.iter().flat_map(|v| v.to_ne_bytes()).collect()
}

/// The whole messages at the start of `buf`, each as `(object, opcode, args)`, and how many bytes they
/// take. A message cut off at the end is left for the next read; an impossible size is an error.
pub(super) fn messages(buf: &[u8]) -> io::Result<Messages> {
    let mut out = Vec::new();
    let mut at = 0;
    let word = |i: usize| buf.get(i..i + 4).and_then(|b| b.try_into().ok()).map(u32::from_ne_bytes);
    while let (Some(object), Some(word)) = (word(at), word(at + 4)) {
        let (size, opcode) = ((word >> 16) as usize, (word & 0xffff) as u16);
        if size < 8 || size % 4 != 0 {
            return Err(io::Error::new(io::ErrorKind::InvalidData, format!("invalid Wayland message size: {size}")));
        }
        if at + size > buf.len() {
            break;
        }
        out.push((object, opcode, at + 8..at + size));
        at += size;
    }
    Ok((out, at))
}

/// How a relay ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum End {
    /// The end it reads from closed.
    From,
    /// Sending to the other end failed.
    To,
    /// Its input contained an impossible message header.
    Invalid,
    /// It panicked (the panic hook has logged it).
    Panicked,
}

/// Passes messages from `from` to `to` as `pass` says, with the file descriptors that come with them,
/// until either end closes or the relay panics. Then both ends are shut, so the relay the other way ends
/// too. `closed` is told how it ended before the ends are shut.
pub(super) fn relay(from: &UnixStream, to: &UnixStream, pass: impl FnMut(u32, u16, &[u8], Instant) -> Pass, closed: impl FnOnce(End)) {
    let end = crate::os::worker::or_on_panic(
        || pass_on(from, to, pass),
        || {
            log::error!("wayland filter: a relay panicked: its connection ends");
            End::Panicked
        },
    );
    closed(end);
    for end in [from, to] {
        if let Err(e) = end.shutdown(std::net::Shutdown::Both)
            && e.kind() != io::ErrorKind::NotConnected
        {
            log::debug!("wayland filter: closing: {e}");
        }
    }
}

/// The relay's work, until an end closes.
fn pass_on(from: &UnixStream, to: &UnixStream, mut pass: impl FnMut(u32, u16, &[u8], Instant) -> Pass) -> End {
    let mut buf: Vec<u8> = Vec::new();
    let mut chunk = vec![0u8; 16384];
    let mut out = Vec::new();
    loop {
        let mut space = [MaybeUninit::uninit(); rustix::cmsg_space!(ScmRights(MAX_FDS))];
        let mut ancillary = RecvAncillaryBuffer::new(&mut space);
        // Keep at most one maximum-sized incomplete message, regardless of how the peer fragments it.
        let available = MAX_MESSAGE_BYTES.saturating_sub(buf.len()).min(chunk.len());
        if available == 0 {
            log::warn!("wayland filter: pending message exceeds the wire size limit: its connection ends");
            return End::Invalid;
        }
        let read = chunk.get_mut(..available).unwrap_or_default();
        let n = match rustix::net::recvmsg(from, &mut [IoSliceMut::new(read)], &mut ancillary, RecvFlags::CMSG_CLOEXEC) {
            Ok(r) if r.bytes > 0 => r.bytes,
            Err(rustix::io::Errno::INTR) => continue,
            Ok(_) => return End::From,
            Err(e) => {
                log::debug!("wayland filter: receiving: {e}");
                return End::From;
            }
        };
        let mut received: Vec<OwnedFd> = Vec::new();
        for message in ancillary.drain() {
            if let RecvAncillaryMessage::ScmRights(fds) = message {
                received.extend(fds);
            }
        }
        let now = Instant::now();
        buf.extend_from_slice(chunk.get(..n).unwrap_or_default());
        let (found, used) = match messages(&buf) {
            Ok(parsed) => parsed,
            Err(e) => {
                log::warn!("wayland filter: {e}: its connection ends");
                return End::Invalid;
            }
        };
        out.clear();
        for (object, opcode, args) in found {
            let whole = buf.get(args.start - 8..args.end).unwrap_or_default();
            match pass(object, opcode, buf.get(args).unwrap_or_default(), now) {
                Pass::Keep => out.extend_from_slice(whole),
                Pass::Drop => {}
                Pass::Replace(m) => out.extend(m),
            }
        }
        buf.drain(..used);
        // The descriptors go with the bytes they came with, since the receiver takes them in order, by
        // message.
        if (!out.is_empty() || !received.is_empty())
            && let Err(e) = send(to, &out, &received)
        {
            log::debug!("wayland filter: sending: {e}");
            return End::To;
        }
    }
}

/// Sends all of `bytes`, with `fds` attached to the first part sent.
pub(super) fn send(to: &UnixStream, bytes: &[u8], fds: &[OwnedFd]) -> io::Result<()> {
    let borrowed: Vec<BorrowedFd> = fds.iter().map(AsFd::as_fd).collect();
    let mut sent = 0;
    let mut first = true;
    while sent < bytes.len() || (first && !borrowed.is_empty()) {
        let mut space = [MaybeUninit::uninit(); rustix::cmsg_space!(ScmRights(MAX_FDS))];
        let mut ancillary = SendAncillaryBuffer::new(&mut space);
        if first && !borrowed.is_empty() {
            ancillary.push(SendAncillaryMessage::ScmRights(&borrowed));
        }
        let rest = bytes.get(sent..).unwrap_or_default();
        let n = match rustix::net::sendmsg(to, &[IoSlice::new(rest)], &mut ancillary, SendFlags::NOSIGNAL) {
            Err(rustix::io::Errno::INTR) => continue,
            sent => sent?,
        };
        if n == 0 && rest.is_empty() {
            break;
        }
        sent += n;
        first = false;
    }
    Ok(())
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    use std::io::{Read, Write};
    use std::time::Duration;

    #[test]
    fn incomplete_headers_and_bodies_are_distinct_from_invalid_sizes() {
        let valid = message(1, 0, &ints(&[4, 5]));
        for prefix in 0..valid.len() {
            let (found, used) = messages(valid.get(..prefix).unwrap()).unwrap();
            assert!(found.is_empty());
            assert_eq!(used, 0);
        }
        assert_eq!(messages(&valid).unwrap().1, valid.len());
        for size in [0u32, 4, 7, 9, 65535] {
            let mut invalid = 1u32.to_ne_bytes().to_vec();
            invalid.extend((size << 16).to_ne_bytes());
            assert_eq!(messages(&invalid).unwrap_err().kind(), io::ErrorKind::InvalidData);
            let mut after_valid = valid.clone();
            after_valid.extend(invalid);
            assert!(messages(&after_valid).is_err(), "invalid size is rejected after a valid prefix too");
        }
    }

    #[test]
    fn malformed_input_ends_both_sockets_and_releases_received_descriptors() {
        for size in [0u32, 4, 9, 65535] {
            let (writer, from) = UnixStream::pair().unwrap();
            let (to, mut reader) = UnixStream::pair().unwrap();
            let (descriptor, mut observer) = UnixStream::pair().unwrap();
            let (done, ended) = std::sync::mpsc::channel();
            let relay = std::thread::spawn(move || super::relay(&from, &to, |_, _, _, _| Pass::Keep, |how| done.send(how).unwrap()));
            let mut invalid = 1u32.to_ne_bytes().to_vec();
            invalid.extend((size << 16).to_ne_bytes());
            send(&writer, &invalid, &[descriptor.into()]).unwrap();
            assert_eq!(ended.recv_timeout(Duration::from_secs(2)).unwrap(), End::Invalid);
            relay.join().unwrap();
            for socket in [&mut reader, &mut observer] {
                socket.set_read_timeout(Some(Duration::from_secs(2))).unwrap();
                assert_eq!(socket.read(&mut [0]).unwrap(), 0, "relay and descriptor sockets closed");
            }
            let mut writer = writer;
            writer.set_read_timeout(Some(Duration::from_secs(2))).unwrap();
            assert_eq!(writer.read(&mut [0]).unwrap(), 0, "input socket also closed");
        }
    }

    #[test]
    fn a_maximum_sized_valid_message_passes_through_the_bounded_buffer() {
        let valid = message(1, 0, &vec![0; MAX_MESSAGE_BYTES - 8]);
        assert_eq!(valid.len(), MAX_MESSAGE_BYTES);
        assert_eq!(messages(valid.get(..valid.len() - 1).unwrap()).unwrap().1, 0);
        let (mut writer, from) = UnixStream::pair().unwrap();
        let (to, mut reader) = UnixStream::pair().unwrap();
        let relay = std::thread::spawn(move || super::relay(&from, &to, |_, _, _, _| Pass::Keep, |_| {}));
        writer.set_write_timeout(Some(Duration::from_secs(2))).unwrap();
        reader.set_read_timeout(Some(Duration::from_secs(2))).unwrap();
        writer.write_all(&valid).unwrap();
        let mut received = vec![0; valid.len()];
        reader.read_exact(&mut received).unwrap();
        assert_eq!(received, valid);
        writer.shutdown(std::net::Shutdown::Both).unwrap();
        relay.join().unwrap();
    }
}
