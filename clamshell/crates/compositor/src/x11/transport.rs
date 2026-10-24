//! Bounded non-blocking X11 transport. x11rb supplies wire types and sequence tracking, never I/O.

use std::{
    collections::VecDeque,
    io::{self, Read, Write},
    os::unix::net::UnixStream,
    time::{Duration, Instant},
};
use x11rb_protocol::{
    connect::Connect,
    connection::{Connection, ReplyFdKind},
    protocol::xproto::Setup,
    x11_utils::Request,
};

const LIMIT: usize = 1024 * 1024;
const TIMEOUT: Duration = Duration::from_secs(3);
const BUDGET: usize = 64;

pub(super) struct Transport {
    stream: UnixStream,
    handshake: Option<Connect>,
    pub setup: Option<Setup>,
    wire: Connection,
    outgoing: VecDeque<u8>,
    incoming: Vec<u8>,
    packet_size: usize,
    pub setup_deadline: Instant,
    write_deadline: Option<Instant>,
    last_request: u64,
    last_reply: u64,
}

impl Transport {
    pub fn new(stream: UnixStream, now: Instant) -> io::Result<Self> {
        stream.set_nonblocking(true)?;
        // The dedicated -wm socket is trusted by Xwayland; it does not use Xauthority.
        let (handshake, bytes) = Connect::with_authorization(Vec::new(), Vec::new());
        Ok(Self {
            stream,
            handshake: Some(handshake),
            setup: None,
            wire: Connection::new(),
            outgoing: bytes.into(),
            incoming: Vec::with_capacity(32),
            packet_size: 32,
            setup_deadline: now + TIMEOUT,
            write_deadline: Some(now + TIMEOUT),
            last_request: 0,
            last_reply: 0,
        })
    }

    pub fn turn(&mut self, now: Instant) -> io::Result<()> {
        if self.handshake.is_some() && now >= self.setup_deadline {
            return Err(io::Error::new(io::ErrorKind::TimedOut, "X11 setup timed out"));
        }
        if self.write_deadline.is_some_and(|at| now >= at) && self.writing() {
            return Err(io::Error::new(io::ErrorKind::TimedOut, "X11 write timed out"));
        }
        for _ in 0..BUDGET {
            let (bytes, _) = self.outgoing.as_slices();
            if bytes.is_empty() {
                break;
            }
            match self.stream.write(bytes) {
                Ok(0) => return Err(io::Error::new(io::ErrorKind::WriteZero, "X11 socket stopped writing")),
                Ok(n) => {
                    self.outgoing.drain(..n);
                }
                Err(e) if e.kind() == io::ErrorKind::WouldBlock => break,
                Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
                Err(e) => return Err(e),
            }
        }
        if !self.writing() {
            self.write_deadline = None;
        }
        if let Some(handshake) = self.handshake.as_mut() {
            for _ in 0..BUDGET {
                match self.stream.read(handshake.buffer()) {
                    Ok(0) => return Err(io::Error::new(io::ErrorKind::UnexpectedEof, "X11 setup socket closed")),
                    Ok(n) if handshake.advance(n) => {
                        if let Some(handshake) = self.handshake.take() {
                            self.setup = Some(handshake.into_setup().map_err(io::Error::other)?);
                        }
                        break;
                    }
                    Ok(_) => {}
                    Err(e) if e.kind() == io::ErrorKind::WouldBlock => return Ok(()),
                    Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
                    Err(e) => return Err(e),
                }
            }
            if self.handshake.is_some() {
                return Ok(());
            }
        }
        for _ in 0..BUDGET {
            let left = self.packet_size.saturating_sub(self.incoming.len());
            let mut buffer = [0u8; 4096];
            let capacity = left.min(buffer.len());
            let bytes = buffer.get_mut(..capacity).ok_or_else(|| io::Error::other("invalid X11 packet size"))?;
            match self.stream.read(bytes) {
                Ok(0) => return Err(io::Error::new(io::ErrorKind::UnexpectedEof, "X11 socket closed")),
                Ok(n) => self.incoming.extend(buffer.get(..n).ok_or_else(|| io::Error::other("invalid X11 read size"))?),
                Err(e) if e.kind() == io::ErrorKind::WouldBlock => break,
                Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
                Err(e) => return Err(e),
            }
            if self.incoming.len() == 32 && matches!(self.incoming.first().map(|v| v & 0x7f), Some(1 | 35)) {
                let length =
                    self.incoming.get(4..8).and_then(|b| b.try_into().ok()).map(u32::from_ne_bytes).ok_or_else(|| io::Error::other("invalid X11 length"))?;
                self.packet_size = usize::try_from(length)
                    .ok()
                    .and_then(|n| n.checked_mul(4))
                    .and_then(|n| n.checked_add(32))
                    .filter(|n| *n <= LIMIT)
                    .ok_or_else(|| io::Error::other("X11 packet exceeds limit"))?;
            }
            if self.incoming.len() == self.packet_size {
                self.wire.enqueue_packet(std::mem::replace(&mut self.incoming, Vec::with_capacity(32)));
                self.packet_size = 32;
            }
        }
        Ok(())
    }

    pub fn fd(&self) -> &UnixStream {
        &self.stream
    }
    pub fn writing(&self) -> bool {
        !self.outgoing.is_empty()
    }
    pub fn deadline(&self) -> Option<Instant> {
        self.handshake.as_ref().map(|_| self.setup_deadline).into_iter().chain(self.write_deadline).min()
    }
    pub fn request(&mut self, request: impl Request, opcode: u8, reply: bool) -> io::Result<u64> {
        let (bytes, fds) = request.serialize(opcode);
        if !fds.is_empty() {
            return Err(io::Error::other("XWM requests cannot carry descriptors"));
        }
        let kind = if reply { ReplyFdKind::ReplyWithoutFDs } else { ReplyFdKind::NoReply };
        let sequence = self.wire.send_request(kind).ok_or_else(|| io::Error::other("X11 sequence exhausted"))?;
        self.queue(&bytes)?;
        self.last_request = sequence;
        Ok(sequence)
    }

    pub fn reply(&mut self, sequence: u64) -> Option<Vec<u8>> {
        let (bytes, _) = self.wire.poll_for_reply_or_error(sequence)?;
        self.last_reply = self.last_reply.max(sequence);
        Some(bytes)
    }

    pub fn needs_sequence_anchor(&self) -> bool {
        self.last_request.saturating_sub(self.last_reply) >= 32_768
    }

    pub fn event(&mut self) -> Option<Vec<u8>> {
        self.wire.poll_for_event_with_sequence().map(|(bytes, _)| bytes)
    }

    fn queue(&mut self, bytes: &[u8]) -> io::Result<()> {
        if self.outgoing.len().saturating_add(bytes.len()) > LIMIT {
            return Err(io::Error::other("X11 outgoing queue exceeds limit"));
        }
        if self.outgoing.is_empty() && !bytes.is_empty() {
            self.write_deadline = Some(Instant::now() + TIMEOUT);
        }
        self.outgoing.extend(bytes);
        Ok(())
    }
}

#[cfg(test)]
#[path = "transport_tests.rs"]
mod tests;
