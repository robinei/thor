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
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::indexing_slicing)]
mod tests {
    use super::*;
    use x11rb_protocol::x11_utils::Serialize;

    fn connected() -> (Transport, UnixStream, Instant) {
        let (stream, mut peer) = UnixStream::pair().unwrap();
        let now = Instant::now();
        let mut transport = Transport::new(stream, now).unwrap();
        transport.turn(now).unwrap();
        let mut setup_request = [0; 12];
        peer.read_exact(&mut setup_request).unwrap();
        assert_eq!(setup_request[0], if cfg!(target_endian = "little") { b'l' } else { b'B' });
        let setup =
            Setup { status: 1, length: 8, protocol_major_version: 11, maximum_request_length: u16::MAX, resource_id_mask: 0x1fffff, ..Setup::default() };
        let bytes = Serialize::serialize(&setup);
        // A partial header is not a reason to block waiting for the rest of setup.
        peer.write_all(&bytes[..3]).unwrap();
        transport.turn(now).unwrap();
        assert!(transport.handshake.is_some());
        peer.write_all(&bytes[3..]).unwrap();
        transport.turn(now).unwrap();
        assert!(transport.setup.is_some());
        assert!(transport.reply(1).is_none());
        (transport, peer, now)
    }

    fn reply(sequence: u16) -> [u8; 32] {
        let mut bytes = [0; 32];
        bytes[0] = 1;
        bytes[2..4].copy_from_slice(&sequence.to_ne_bytes());
        bytes
    }

    #[test]
    fn partial_replies_are_polled_and_idle_connections_never_probe() {
        use x11rb_protocol::protocol::xproto::{GetInputFocusReply, GetInputFocusRequest};
        use x11rb_protocol::x11_utils::TryParse;
        let (mut transport, mut peer, now) = connected();
        let sequence = transport.request(GetInputFocusRequest {}, 0, true).unwrap();
        transport.turn(now).unwrap();
        let mut request = [0; 4];
        peer.read_exact(&mut request).unwrap();
        assert_eq!(request[0], 43);
        peer.write_all(&reply(1)[..9]).unwrap();
        transport.turn(now).unwrap();
        assert!(transport.reply(sequence).is_none());
        peer.write_all(&reply(1)[9..]).unwrap();
        transport.turn(now).unwrap();
        GetInputFocusReply::try_parse(&transport.reply(sequence).unwrap()).unwrap();
        assert!(transport.reply(sequence).is_none());
        assert_eq!(transport.deadline(), None);
        for i in 0..100 {
            transport.turn(now + Duration::from_secs(i)).unwrap();
        }
        peer.set_nonblocking(true).unwrap();
        assert_eq!(peer.read(&mut request).unwrap_err().kind(), io::ErrorKind::WouldBlock);
        assert_eq!(transport.deadline(), None, "an idle connection creates no timer or traffic");
    }

    #[test]
    fn full_write_buffers_keep_the_queue_bounded_and_still_return_to_the_loop() {
        let (mut transport, _peer, now) = connected();

        transport.queue(&vec![0; LIMIT]).unwrap();
        assert!(transport.queue(&[1]).is_err());
        transport.turn(now).unwrap();
        assert!(!transport.outgoing.is_empty(), "the peer does not drain the socket");
        let left = transport.outgoing.len();
        for _ in 0..100 {
            transport.turn(now).unwrap();
        }
        assert_eq!(transport.outgoing.len(), left, "WouldBlock retains unsent bytes");
        assert_eq!(transport.turn(now + TIMEOUT + Duration::from_secs(1)).unwrap_err().kind(), io::ErrorKind::TimedOut);
    }

    #[test]
    fn setup_timeout_eof_and_oversized_packets_end_only_the_connection() {
        let (stream, _peer) = UnixStream::pair().unwrap();
        let now = Instant::now();
        let mut transport = Transport::new(stream, now).unwrap();
        transport.turn(now).unwrap();
        assert_eq!(transport.turn(now + TIMEOUT).unwrap_err().kind(), io::ErrorKind::TimedOut);
        let (mut transport, peer, now) = connected();
        peer.shutdown(std::net::Shutdown::Both).unwrap();
        drop(peer);
        assert!(transport.turn(now).is_err());
        let (mut transport, mut peer, now) = connected();
        let mut oversized = reply(1);
        oversized[4..8].copy_from_slice(&u32::MAX.to_ne_bytes());
        peer.write_all(&oversized).unwrap();
        assert!(transport.turn(now).unwrap_err().to_string().contains("exceeds limit"));
        assert_eq!(transport.incoming.len(), 32, "a malicious length never allocates a huge packet");
    }

    #[test]
    fn sequence_anchors_follow_actual_traffic_across_wire_wrap() {
        use x11rb_protocol::protocol::xproto::{GetInputFocusRequest, NoOperationRequest};
        let (mut transport, _peer, _now) = connected();
        for _ in 0..3 {
            assert!(!transport.needs_sequence_anchor());
            for _ in 0..32_768 {
                transport.request(NoOperationRequest {}, 0, false).unwrap();
            }
            assert!(transport.needs_sequence_anchor());
            let sequence = transport.request(GetInputFocusRequest {}, 0, true).unwrap();
            transport.wire.enqueue_packet(reply(sequence as u16).to_vec());
            assert!(transport.reply(sequence).is_some());
            assert!(!transport.needs_sequence_anchor());
        }
        assert!(transport.last_reply > u64::from(u16::MAX), "the low 16 bits have wrapped without losing the reply");
    }
}
