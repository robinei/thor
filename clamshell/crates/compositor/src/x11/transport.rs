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
    protocol::xproto::{GetInputFocusReply, GetInputFocusRequest, Setup},
    x11_utils::{Request, TryParse},
};

const LIMIT: usize = 1024 * 1024;
const TIMEOUT: Duration = Duration::from_secs(3);
const BUDGET: usize = 64;

pub(super) struct Transport {
    stream: UnixStream,
    handshake: Option<Connect>,
    setup: Option<Setup>,
    wire: Connection,
    outgoing: VecDeque<u8>,
    incoming: Vec<u8>,
    packet_size: usize,
    probe: Option<u64>,
    deadline: Instant,
    next_probe: Instant,
    pub ready: bool,
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
            probe: None,
            deadline: now + TIMEOUT,
            next_probe: now,
            ready: false,
        })
    }

    pub fn turn(&mut self, now: Instant) -> io::Result<()> {
        if now >= self.deadline && (!self.outgoing.is_empty() || self.handshake.is_some() || self.probe.is_some()) {
            return Err(io::Error::new(io::ErrorKind::TimedOut, "Xwayland stopped responding"));
        }
        if self.setup.is_some() && self.probe.is_none() && now >= self.next_probe {
            let sequence = self.wire.send_request(ReplyFdKind::ReplyWithoutFDs).ok_or_else(|| io::Error::other("X11 sequence exhausted"))?;
            let (bytes, _) = Request::serialize(GetInputFocusRequest {}, 0);
            self.queue(&bytes)?;
            self.probe = Some(sequence);
            self.deadline = now + TIMEOUT;
            self.next_probe = now + Duration::from_secs(1);
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
                if let Some(sequence) = self.probe
                    && let Some((bytes, _)) = self.wire.poll_for_reply_or_error(sequence)
                {
                    GetInputFocusReply::try_parse(&bytes).map_err(io::Error::other)?;
                    self.probe = None;
                    self.ready = true;
                }
                // Window events are consumed by the adapter in the next implementation step.
                while self.wire.poll_for_event_with_sequence().is_some() {}
            }
        }
        Ok(())
    }

    fn queue(&mut self, bytes: &[u8]) -> io::Result<()> {
        if self.outgoing.len().saturating_add(bytes.len()) > LIMIT {
            return Err(io::Error::other("X11 outgoing queue exceeds limit"));
        }
        self.outgoing.extend(bytes);
        Ok(())
    }
}

#[cfg(test)]
#[path = "transport_tests.rs"]
mod tests;
