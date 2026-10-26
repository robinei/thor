//! Wayland selections served to X11 requestors over a fake X11 peer.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::indexing_slicing)]
use super::*;
use std::{
    io::{Read, Write},
    os::{fd::OwnedFd, unix::net::UnixStream},
};

fn transfer(bytes: &[u8], latin1: bool, close: bool, now: Instant) -> (Transfer, UnixStream) {
    let (read, mut write) = UnixStream::pair().unwrap();
    read.set_nonblocking(true).unwrap();
    write.write_all(bytes).unwrap();
    if close {
        write.shutdown(std::net::Shutdown::Write).unwrap();
    }
    (
        Transfer {
            id: 1,
            selection: 0,
            request: SelectionRequestEvent { requestor: 10, selection: 100, target: if latin1 { 31 } else { 101 }, ..Default::default() },
            property: 102,
            pipe: File::from(OwnedFd::from(read)),
            format: Format { atom: 101, mime: "text/plain;charset=utf-8".into(), latin1 },
            data: Vec::with_capacity(CHUNK),
            encoding: Latin1::default(),
            eof: false,
            incremental: false,
            permission: false,
            deadline: now + TIMEOUT,
        },
        write,
    )
}
fn wire(now: Instant) -> (Transport, UnixStream) {
    let (socket, mut peer) = UnixStream::pair().unwrap();
    let mut wire = Transport::new(socket, now).unwrap();
    wire.turn(now).unwrap();
    peer.read_exact(&mut [0; 12]).unwrap();
    let setup = Setup { status: 1, length: 8, protocol_major_version: 11, maximum_request_length: u16::MAX, resource_id_mask: 0x1fffff, ..Default::default() };
    peer.write_all(&x11rb_protocol::x11_utils::Serialize::serialize(&setup)).unwrap();
    wire.turn(now).unwrap();
    assert!(wire.setup.is_some());
    peer.set_nonblocking(true).unwrap();
    (wire, peer)
}
fn packets(wire: &mut Transport, peer: &mut UnixStream, now: Instant) -> Vec<Vec<u8>> {
    wire.turn(now).unwrap();
    let mut bytes = Vec::new();
    match peer.read_to_end(&mut bytes) {
        Ok(_) => {}
        Err(error) => assert_eq!(error.kind(), io::ErrorKind::WouldBlock),
    }
    let mut result = Vec::new();
    while !bytes.is_empty() {
        let length = usize::from(u16::from_ne_bytes(bytes[2..4].try_into().unwrap())) * 4;
        result.push(bytes.drain(..length).collect());
    }
    result
}
fn content(packet: &[u8]) -> &[u8] {
    let count = u32::from_ne_bytes(packet[20..24].try_into().unwrap()) as usize;
    &packet[24..24 + count * usize::from(packet[16]) / 8]
}
fn atoms() -> Atoms {
    Atoms(vec![("INCR", 200), ("TEXT", 201), ("UTF8_STRING", 101)])
}

#[test]
fn small_and_empty_transfers_set_the_property_before_notifying_and_close_the_pipe() {
    for bytes in [b"".as_slice(), b"a small selection".as_slice()] {
        let now = Instant::now();
        let (transfer, _writer) = transfer(bytes, false, true, now);
        let mut bridge = Bridge { transfers: vec![transfer], ..Default::default() };
        let (mut wire, mut peer) = wire(now);
        bridge.pump(&mut wire, &atoms(), now).unwrap();
        let requests = packets(&mut wire, &mut peer, now);
        // Publishing the property before SelectionNotify prevents a reader from seeing missing data.
        assert_eq!(requests.iter().map(|packet| packet[0]).collect::<Vec<_>>(), [CHANGE_PROPERTY_REQUEST, SEND_EVENT_REQUEST]);
        assert_eq!(content(&requests[0]), bytes);
        assert_eq!(u32::from_ne_bytes(requests[1][32..36].try_into().unwrap()), 102);
        assert!(bridge.transfers.is_empty());
        assert_eq!(bridge.deadline(), None);
    }
}
#[test]
fn incr_waits_for_each_delete_keeps_one_bounded_chunk_and_finishes_with_zero_length() {
    let now = Instant::now();
    let bytes: Vec<u8> = (0..CHUNK * 3 + 7).map(|index| (index % 251) as u8).collect();
    let (transfer, _writer) = transfer(&bytes, false, true, now);
    let mut bridge = Bridge { transfers: vec![transfer], ..Default::default() };
    let (mut wire, mut peer) = wire(now);
    bridge.pump(&mut wire, &atoms(), now).unwrap();
    let marker = packets(&mut wire, &mut peer, now);
    // The INCR marker and its notification precede all data, even if the entire source is readable.
    assert_eq!(marker.iter().map(|packet| packet[0]).collect::<Vec<_>>(), [CHANGE_PROPERTY_REQUEST, SEND_EVENT_REQUEST]);
    assert_eq!(u32::from_ne_bytes(marker[0][12..16].try_into().unwrap()), 200);
    assert_eq!(marker[0][16], 32);
    assert_eq!(u32::from_ne_bytes(content(&marker[0]).try_into().unwrap()), CHUNK as u32);
    assert_eq!(bridge.transfers[0].data.len(), CHUNK);
    for _ in 0..3 {
        bridge.pump(&mut wire, &atoms(), now).unwrap();
        assert!(packets(&mut wire, &mut peer, now).is_empty(), "no data before the deletion acknowledgement");
    }
    let mut received = Vec::new();
    let mut sizes = Vec::new();
    while !bridge.transfers.is_empty() {
        bridge.transfers[0].permission = true;
        bridge.pump(&mut wire, &atoms(), now).unwrap();
        for packet in packets(&mut wire, &mut peer, now) {
            assert_eq!(packet[0], CHANGE_PROPERTY_REQUEST);
            let chunk = content(&packet);
            assert!(chunk.len() <= CHUNK);
            sizes.push(chunk.len());
            received.extend_from_slice(chunk);
        }
        assert!(bridge.transfers.iter().all(|transfer| transfer.data.len() <= CHUNK));
    }
    assert_eq!(sizes, [CHUNK, CHUNK, CHUNK, 7, 0]);
    assert_eq!(received, bytes);
    assert_eq!(bridge.deadline(), None);
}
#[test]
fn timeout_and_replacement_cancel_only_their_transfers_without_a_fake_incr_eof() {
    let now = Instant::now();
    let (small, _small_writer) = transfer(b"unfinished", false, false, now);
    let (large, _large_writer) = transfer(&vec![b'x'; CHUNK], false, false, now);
    let mut bridge = Bridge { transfers: vec![small, large], ..Default::default() };
    bridge.transfers[1].selection = 1;
    let (mut wire, mut peer) = wire(now);
    bridge.pump(&mut wire, &atoms(), now).unwrap();
    assert_eq!(packets(&mut wire, &mut peer, now).len(), 2);
    bridge.cancel(0, &mut wire, now).unwrap();
    let refused = packets(&mut wire, &mut peer, now);
    assert_eq!(refused.len(), 1);
    assert_eq!(refused[0][0], SEND_EVENT_REQUEST);
    assert_eq!(u32::from_ne_bytes(refused[0][32..36].try_into().unwrap()), 0);
    assert_eq!(bridge.transfers.len(), 1);
    assert_eq!(bridge.deadline(), Some(now + TIMEOUT));
    bridge.pump(&mut wire, &atoms(), now + TIMEOUT).unwrap();
    assert!(packets(&mut wire, &mut peer, now).is_empty(), "an aborted INCR must not look like successful truncated content");
    assert!(bridge.transfers.is_empty());
    assert_eq!(bridge.deadline(), None);
}
#[test]
fn latin1_translation_preserves_split_scalars_and_bounds_incomplete_input() {
    let mut encoder = Latin1::default();
    let mut out = Vec::new();
    for byte in "café €".as_bytes() {
        encoder.append(&[*byte], false, &mut out);
        assert!(encoder.pending.len() <= 3);
    }
    encoder.append(&[0xff, 0xe2], true, &mut out);
    assert_eq!(out, b"caf\xe9 ???");
    assert!(encoder.pending.is_empty());
}

fn server() -> (smithay::reexports::wayland_server::Display<Server>, Server) {
    crate::tests::ServerBuilder::new().build()
}
#[test]
fn unsupported_duplicate_excess_stale_and_missing_source_requests_are_refused() {
    let now = Instant::now();
    let (_display, mut server) = server();
    let (mut wire, mut peer) = wire(now);
    let mut atoms = atoms();
    atoms.0.push(("CLIPBOARD", 100));
    let (one, _writer) = transfer(b"", false, false, now);
    let mut request = one.request;
    request.property = 102;
    let mut bridge = Bridge { transfers: vec![one], ..Default::default() };
    bridge.exports.clipboard =
        Export { owned: true, revision: Some(0), formats: vec![Format { atom: 101, mime: "text/plain".into(), latin1: false }], ..Default::default() };
    let mut unsupported = request;
    unsupported.target = 999;
    bridge.request(unsupported, &mut wire, &atoms, &mut server, now).unwrap();
    bridge.request(request, &mut wire, &atoms, &mut server, now).unwrap();
    let mut writers = Vec::new();
    for index in 1..MAX_TRANSFERS {
        let (mut next, writer) = transfer(b"", false, false, now);
        next.request.requestor += index as u32;
        bridge.transfers.push(next);
        writers.push(writer);
    }
    let mut excess = request;
    excess.requestor = 1000;
    bridge.request(excess, &mut wire, &atoms, &mut server, now).unwrap();
    bridge.transfers.clear();
    server.x11.selections.entries.clipboard.revision = 1;
    bridge.request(request, &mut wire, &atoms, &mut server, now).unwrap();
    bridge.exports.clipboard.revision = Some(1);
    bridge.request(request, &mut wire, &atoms, &mut server, now).unwrap();
    let refused = packets(&mut wire, &mut peer, now);
    assert_eq!(refused.len(), 5);
    for packet in refused {
        assert_eq!(packet[0], SEND_EVENT_REQUEST);
        assert_eq!(u32::from_ne_bytes(packet[32..36].try_into().unwrap()), 0);
    }
    assert!(bridge.transfers.is_empty());
    assert_eq!(bridge.deadline(), None);
}
#[test]
fn server_timestamp_precedes_ownership_and_stale_clears_cannot_cancel_a_new_offer() {
    use x11rb_protocol::x11_utils::Serialize;
    let now = Instant::now();
    let (_display, mut server) = server();
    let (mut wire, mut peer) = wire(now);
    let mut atoms = atoms();
    atoms.0.extend([("CLIPBOARD", 100), ("TIMESTAMP", 202)]);
    let mut bridge = Bridge::default();
    bridge.exports.clipboard =
        Export { revision: Some(0), claim: true, formats: vec![Format { atom: 101, mime: "text/plain".into(), latin1: false }], ..Default::default() };
    bridge.exports.primary.revision = Some(0);
    bridge.turn(&mut wire, &atoms, 5, &mut server, now).unwrap();
    let clock = packets(&mut wire, &mut peer, now);
    assert_eq!(clock.len(), 1);
    assert_eq!(clock[0][0], CHANGE_PROPERTY_REQUEST);
    assert!(!bridge.exports.clipboard.owned);
    assert_eq!(bridge.deadline(), Some(now + Duration::from_secs(3)));
    // A timestamp requested for an earlier generation must not acquire a replacement offer.
    bridge.exports.clipboard.revision = Some(1);
    server.x11.selections.entries.clipboard.revision = 1;
    let old_clock =
        PropertyNotifyEvent { response_type: PROPERTY_NOTIFY_EVENT, window: 5, atom: 202, time: 150, state: Property::NEW_VALUE, ..Default::default() };
    bridge.event(&event_bytes(old_clock.serialize()), &mut wire, &atoms, 5, &mut server, now).unwrap();
    assert_eq!(bridge.exports.clipboard.timestamp, 0);
    bridge.turn(&mut wire, &atoms, 5, &mut server, now).unwrap();
    let new_clock = packets(&mut wire, &mut peer, now);
    assert_eq!(new_clock.len(), 1);
    assert_eq!(new_clock[0][0], CHANGE_PROPERTY_REQUEST);
    let event = PropertyNotifyEvent { response_type: PROPERTY_NOTIFY_EVENT, window: 5, atom: 202, time: 200, state: Property::NEW_VALUE, ..Default::default() };
    bridge.event(&event_bytes(event.serialize()), &mut wire, &atoms, 5, &mut server, now).unwrap();
    bridge.turn(&mut wire, &atoms, 5, &mut server, now).unwrap();
    let owned = packets(&mut wire, &mut peer, now);
    // Ownership uses a confirmed server time, which TIMESTAMP must later report exactly.
    assert_eq!(owned.len(), 1);
    assert_eq!(owned[0][0], SET_SELECTION_OWNER_REQUEST);
    assert_eq!(u32::from_ne_bytes(owned[0][12..16].try_into().unwrap()), 200);
    assert!(bridge.exports.clipboard.owned);
    assert_eq!(bridge.exports.clipboard.timestamp, 200);
    assert_eq!(bridge.deadline(), None);
    let clear = SelectionClearEvent { response_type: SELECTION_CLEAR_EVENT, owner: 5, selection: 100, time: 199, ..Default::default() };
    bridge.event(&event_bytes(clear.serialize()), &mut wire, &atoms, 5, &mut server, now).unwrap();
    assert!(bridge.exports.clipboard.owned);
    let clear = SelectionClearEvent { time: 201, ..clear };
    for _ in 0..2 {
        bridge.event(&event_bytes(clear.serialize()), &mut wire, &atoms, 5, &mut server, now).unwrap();
    }
    bridge.turn(&mut wire, &atoms, 5, &mut server, now).unwrap();
    assert!(!bridge.exports.clipboard.owned);
    assert!(bridge.exports.clipboard.formats.is_empty());
    assert!(packets(&mut wire, &mut peer, now).is_empty(), "an unchanged revision cannot reclaim ownership");
    assert_eq!(bridge.deadline(), None);
}

fn event_bytes(bytes: impl AsRef<[u8]>) -> Vec<u8> {
    let mut bytes = bytes.as_ref().to_vec();
    bytes.resize(32, 0);
    bytes
}

#[test]
fn late_atom_replies_cannot_restore_an_old_generation_and_missing_replies_have_a_deadline() {
    let now = Instant::now();
    let (_display, mut server) = server();
    server.x11.selections.entries.clipboard.mime_types.push("application/x-thor-old".into());
    let mut atoms = atoms();
    atoms.0.extend([("CLIPBOARD", 100), ("TIMESTAMP", 202)]);
    let (mut wire, mut peer) = wire(now);
    let mut bridge = Bridge::default();
    bridge.turn(&mut wire, &atoms, 5, &mut server, now).unwrap();
    let old = bridge.pending[0].sequence;
    assert_eq!(packets(&mut wire, &mut peer, now)[0][0], INTERN_ATOM_REQUEST);
    server.x11.selections.entries.clipboard.revision = 1;
    server.x11.selections.entries.clipboard.mime_types[0] = "application/x-thor-new".into();
    bridge.turn(&mut wire, &atoms, 5, &mut server, now).unwrap();
    let new = bridge.pending[1].sequence;
    assert_eq!(packets(&mut wire, &mut peer, now)[0][0], INTERN_ATOM_REQUEST);
    for (sequence, atom) in [(old, 300), (new, 301)] {
        let reply = InternAtomReply { sequence: sequence as u16, atom, ..Default::default() };
        peer.write_all(&event_bytes(x11rb_protocol::x11_utils::Serialize::serialize(&reply))).unwrap();
    }
    wire.turn(now).unwrap();
    bridge.turn(&mut wire, &atoms, 5, &mut server, now).unwrap();
    assert!(bridge.pending.is_empty());
    assert_eq!(
        bridge.exports.clipboard.formats.iter().map(|format| (format.atom, format.mime.as_str())).collect::<Vec<_>>(),
        [(301, "application/x-thor-new")]
    );
    assert!(!bridge.exports.clipboard.owned);
    assert_eq!(packets(&mut wire, &mut peer, now)[0][0], CHANGE_PROPERTY_REQUEST);
    assert_eq!(bridge.turn(&mut wire, &atoms, 5, &mut server, now + Duration::from_secs(3)).unwrap_err().kind(), io::ErrorKind::TimedOut);
    bridge.clear(&mut server);
    assert!(bridge.transfers.is_empty());
    assert_eq!(bridge.deadline(), None);
    bridge.turn(&mut wire, &atoms, 5, &mut server, now).unwrap();
    assert!(!bridge.pending.is_empty());
    assert_eq!(bridge.turn(&mut wire, &atoms, 5, &mut server, now + Duration::from_secs(3)).unwrap_err().kind(), io::ErrorKind::TimedOut);
}
#[test]
fn clearing_a_source_releases_ownership_at_its_acquisition_time_and_removes_read_watches() {
    let now = Instant::now();
    let (_display, mut server) = server();
    let mut atoms = atoms();
    atoms.0.push(("CLIPBOARD", 100));
    let (mut wire, mut peer) = wire(now);
    let (transfer, mut writer) = transfer(b"", false, false, now);
    let mut bridge = Bridge { transfers: vec![transfer], ..Default::default() };
    bridge.exports.clipboard = Export { revision: Some(0), owned: true, timestamp: 50, ..Default::default() };
    let event_loop = smithay::reexports::calloop::EventLoop::<Server>::try_new().unwrap();
    let handle = event_loop.handle();
    let mut sources = Vec::new();
    bridge.sync_sources(&handle, &mut sources).unwrap();
    assert_eq!(sources.len(), 1);
    server.x11.selections.changed(SelectionTarget::Clipboard, None);
    bridge.turn(&mut wire, &atoms, 5, &mut server, now).unwrap();
    bridge.sync_sources(&handle, &mut sources).unwrap();
    assert!(sources.is_empty());
    crate::tests::wait_closed(&writer);
    assert!(writer.write_all(b"closed reader").is_err());
    let requests = packets(&mut wire, &mut peer, now);
    assert_eq!(requests.iter().map(|packet| packet[0]).collect::<Vec<_>>(), [SEND_EVENT_REQUEST, SET_SELECTION_OWNER_REQUEST]);
    assert_eq!(u32::from_ne_bytes(requests[1][4..8].try_into().unwrap()), 0);
    assert_eq!(u32::from_ne_bytes(requests[1][12..16].try_into().unwrap()), 50);
    assert_eq!(bridge.deadline(), None);
}

#[test]
fn split_invalid_utf8_at_a_chunk_boundary_cannot_exceed_the_payload_budget() {
    let now = Instant::now();
    let mut bytes = vec![b'a'; CHUNK - 1];
    bytes.push(0xe2);
    let (mut transfer, mut writer) = transfer(&bytes, true, false, now);
    transfer.read(now).unwrap();
    assert_eq!(transfer.data.len(), CHUNK - 1);
    assert_eq!(transfer.buffered(), CHUNK);
    assert!(!transfer.reading());
    transfer.data.clear();
    writer.write_all(&[0xff]).unwrap();
    writer.shutdown(std::net::Shutdown::Write).unwrap();
    transfer.read(now).unwrap();
    assert_eq!(transfer.data, b"??");
    assert!(transfer.eof);
    assert_eq!(transfer.buffered(), 2);
}
