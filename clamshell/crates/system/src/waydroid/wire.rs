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
/// take. A message cut off at the end is left for the next read.
pub(super) fn messages(buf: &[u8]) -> (Vec<(u32, u16, std::ops::Range<usize>)>, usize) {
    let mut out = Vec::new();
    let mut at = 0;
    let word = |i: usize| buf.get(i..i + 4).and_then(|b| b.try_into().ok()).map(u32::from_ne_bytes);
    while let (Some(object), Some(word)) = (word(at), word(at + 4)) {
        let (size, opcode) = ((word >> 16) as usize, (word & 0xffff) as u16);
        if size < 8 || at + size > buf.len() {
            break;
        }
        out.push((object, opcode, at + 8..at + size));
        at += size;
    }
    (out, at)
}

/// Passes messages from `from` to `to` as `pass` says, with the file descriptors that come with them,
/// until either end closes. Then both ends are shut. `after` is called after each read. `closed` is
/// called before both ends are shut, with true if `from` closed.
pub(super) fn relay(
    from: &UnixStream,
    to: &UnixStream,
    mut pass: impl FnMut(u32, u16, &[u8], Instant) -> Pass,
    mut after: impl FnMut(Instant),
    closed: impl FnOnce(bool),
) {
    let mut buf: Vec<u8> = Vec::new();
    let mut chunk = vec![0u8; 16384];
    let mut out = Vec::new();
    let from_closed = loop {
        let mut space = [MaybeUninit::uninit(); rustix::cmsg_space!(ScmRights(MAX_FDS))];
        let mut ancillary = RecvAncillaryBuffer::new(&mut space);
        let n = match rustix::net::recvmsg(from, &mut [IoSliceMut::new(&mut chunk)], &mut ancillary, RecvFlags::CMSG_CLOEXEC) {
            Ok(r) if r.bytes > 0 => r.bytes,
            Err(rustix::io::Errno::INTR) => continue,
            Ok(_) => break true,
            Err(e) => {
                log::debug!("wayland filter: receiving: {e}");
                break true;
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
        let (found, used) = messages(&buf);
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
        after(now);
        // The descriptors go with the bytes they came with, since the receiver takes them in order, by
        // message.
        if (!out.is_empty() || !received.is_empty())
            && let Err(e) = send(to, &out, &received)
        {
            log::debug!("wayland filter: sending: {e}");
            break false;
        }
    };
    closed(from_closed);
    for end in [from, to] {
        if let Err(e) = end.shutdown(std::net::Shutdown::Both)
            && e.kind() != io::ErrorKind::NotConnected
        {
            log::debug!("wayland filter: closing: {e}");
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
