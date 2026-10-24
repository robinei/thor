#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::indexing_slicing)]
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
    let setup = Setup { status: 1, length: 8, protocol_major_version: 11, maximum_request_length: u16::MAX, resource_id_mask: 0x1fffff, ..Setup::default() };
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
