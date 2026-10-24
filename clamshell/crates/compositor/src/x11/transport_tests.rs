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
    assert!(!transport.ready);
    (transport, peer, now)
}

fn reply(sequence: u16) -> [u8; 32] {
    let mut bytes = [0; 32];
    bytes[0] = 1;
    bytes[2..4].copy_from_slice(&sequence.to_ne_bytes());
    bytes
}

#[test]
fn partial_replies_and_a_silent_peer_never_block_and_heartbeat_times_out() {
    let (mut transport, mut peer, now) = connected();
    transport.turn(now).unwrap();
    let mut request = [0; 4];
    peer.read_exact(&mut request).unwrap();
    assert_eq!(request[0], 43); // GetInputFocus is a read-only round trip.
    peer.write_all(&reply(1)[..9]).unwrap();
    transport.turn(now).unwrap();
    assert!(!transport.ready);
    peer.write_all(&reply(1)[9..]).unwrap();
    transport.turn(now).unwrap();
    assert!(transport.ready);
    assert!(transport.probe.is_none());
    transport.turn(now + Duration::from_secs(1)).unwrap();
    assert_eq!(transport.probe, Some(2));
    for _ in 0..100 {
        transport.turn(now + Duration::from_secs(2)).unwrap();
    }
    assert_eq!(transport.turn(now + Duration::from_secs(4)).unwrap_err().kind(), io::ErrorKind::TimedOut);
}

#[test]
fn full_write_buffers_keep_the_queue_bounded_and_still_return_to_the_loop() {
    let (mut transport, _peer, now) = connected();
    transport.next_probe = now + Duration::from_secs(100);
    transport.queue(&vec![0; LIMIT]).unwrap();
    assert!(transport.queue(&[1]).is_err());
    transport.turn(now).unwrap();
    assert!(!transport.outgoing.is_empty(), "the peer does not drain the socket");
    let left = transport.outgoing.len();
    for _ in 0..100 {
        transport.turn(now).unwrap();
    }
    assert_eq!(transport.outgoing.len(), left, "WouldBlock retains unsent bytes");
    assert_eq!(transport.turn(now + TIMEOUT).unwrap_err().kind(), io::ErrorKind::TimedOut);
}

#[test]
fn setup_timeout_eof_and_oversized_packets_end_only_the_connection() {
    let (stream, _peer) = UnixStream::pair().unwrap();
    let now = Instant::now();
    let mut transport = Transport::new(stream, now).unwrap();
    transport.turn(now).unwrap();
    assert_eq!(transport.turn(now + TIMEOUT).unwrap_err().kind(), io::ErrorKind::TimedOut);
    let (mut transport, peer, now) = connected();
    drop(peer);
    assert!(transport.turn(now).is_err());
    let (mut transport, mut peer, now) = connected();
    let mut oversized = reply(1);
    oversized[4..8].copy_from_slice(&u32::MAX.to_ne_bytes());
    peer.write_all(&oversized).unwrap();
    assert!(transport.turn(now).unwrap_err().to_string().contains("exceeds limit"));
    assert_eq!(transport.incoming.len(), 32, "a malicious length never allocates a huge packet");
}
