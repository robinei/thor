//! X11 selections read into Wayland pipes over a fake X11 peer.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::indexing_slicing, clippy::too_many_arguments)]
use super::*;
use std::{
    io::{Read, Write},
    os::unix::net::UnixStream,
};
use x11rb_protocol::x11_utils::Serialize;
fn atoms() -> Atoms {
    Atoms(vec![("CLIPBOARD", 100), ("UTF8_STRING", 101), ("_THOR_SELECTION", 102), ("TARGETS", 103), ("INCR", 200)])
}
fn wire(now: Instant) -> (Transport, UnixStream) {
    let (socket, mut peer) = UnixStream::pair().unwrap();
    let mut wire = Transport::new(socket, now).unwrap();
    wire.turn(now).unwrap();
    peer.read_exact(&mut [0; 12]).unwrap();
    let setup = Setup {
        status: 1,
        length: 18,
        protocol_major_version: 11,
        maximum_request_length: u16::MAX,
        resource_id_base: 0x200000,
        resource_id_mask: 0x1fffff,
        roots: vec![Screen { root: 1, ..Default::default() }],
        ..Default::default()
    };
    peer.write_all(&Serialize::serialize(&setup)).unwrap();
    wire.turn(now).unwrap();
    assert!(wire.setup.is_some());
    peer.set_nonblocking(true).unwrap();
    (wire, peer)
}
fn packets(wire: &mut Transport, peer: &mut UnixStream, now: Instant) -> Vec<Vec<u8>> {
    wire.turn(now).unwrap();
    let mut bytes = Vec::new();
    if let Err(error) = peer.read_to_end(&mut bytes) {
        assert_eq!(error.kind(), io::ErrorKind::WouldBlock);
    }
    let mut result = Vec::new();
    while !bytes.is_empty() {
        let length = usize::from(u16::from_ne_bytes(bytes[2..4].try_into().unwrap())) * 4;
        result.push(bytes.drain(..length).collect());
    }
    result
}
fn event<T: Serialize>(event: T) -> Vec<u8> {
    let mut bytes = Vec::new();
    event.serialize_into(&mut bytes);
    bytes.resize(32, 0);
    bytes
}
fn imports() -> Imports {
    let mut imports = Imports { allocator: Some(IdAllocator::new(0x200000, 0x1fffff).unwrap()), wm_owner: 0x200000, ..Default::default() };
    imports.allocator.as_mut().unwrap().generate_id();
    imports.offers.clipboard = Offer { owner: 40, origin: 1, ..Default::default() };
    imports
}
fn pipe() -> (File, File) {
    let (read, write) = rustix::pipe::pipe_with(rustix::pipe::PipeFlags::CLOEXEC | rustix::pipe::PipeFlags::NONBLOCK).unwrap();
    (read.into(), write.into())
}
fn begin(imports: &mut Imports, pipe: File, latin1: bool, wire: &mut Transport, now: Instant) -> u64 {
    imports
        .start(
            0,
            Format { atom: if latin1 { AtomEnum::STRING.into() } else { 101 }, mime: "text/plain;charset=utf-8".into(), latin1 },
            Some(pipe),
            wire,
            &atoms(),
            now,
        )
        .unwrap();
    let transfer = imports.transfers.last().unwrap();
    let id = transfer.id;
    let notify = SelectionNotifyEvent {
        response_type: SELECTION_NOTIFY_EVENT,
        requestor: transfer.window,
        selection: 100,
        target: transfer.format.atom,
        property: 102,
        ..Default::default()
    };
    imports.transfer_event(&event(notify), wire, &atoms(), now).unwrap();
    imports.pump(wire, &atoms(), now).unwrap();
    id
}
fn reply(imports: &mut Imports, id: u64, type_: u32, format: u8, bytes: Vec<u8>, after: u32, wire: &mut Transport, now: Instant) {
    imports.pending.retain(|pending| !matches!(pending.query, Query::Property(pending_id) if pending_id == id));
    imports
        .property(
            id,
            GetPropertyReply { type_, format, bytes_after: after, value_len: bytes.len() as u32 * 8 / u32::from(format), value: bytes, ..Default::default() },
            wire,
            &atoms(),
            now,
        )
        .unwrap();
}
fn chunk(imports: &mut Imports, id: u64, wire: &mut Transport, now: Instant) {
    let window = imports.transfers.iter().find(|transfer| transfer.id == id).unwrap().window;
    let changed = PropertyNotifyEvent { response_type: PROPERTY_NOTIFY_EVENT, window, atom: 102, state: Property::NEW_VALUE, ..Default::default() };
    imports.transfer_event(&event(changed), wire, &atoms(), now).unwrap();
    imports.pump(wire, &atoms(), now).unwrap();
}
fn drain(pipe: &mut File) -> Vec<u8> {
    let mut bytes = Vec::new();
    if let Err(error) = pipe.read_to_end(&mut bytes) {
        assert_eq!(error.kind(), io::ErrorKind::WouldBlock);
    }
    bytes
}
#[test]
fn incr_deletes_only_after_backpressure_and_all_property_slices_have_drained() {
    let now = Instant::now();
    let (mut wire, mut peer) = wire(now);
    let (mut reader, mut writer) = pipe();
    let mut prefix = 0;
    while let Ok(n) = writer.write(&[b'p'; 4096]) {
        prefix += n;
    }
    assert!(prefix > 0);
    let mut imports = imports();
    let id = begin(&mut imports, writer, false, &mut wire, now);
    assert_eq!(
        packets(&mut wire, &mut peer, now).iter().map(|p| p[0]).collect::<Vec<_>>(),
        [CREATE_WINDOW_REQUEST, CONVERT_SELECTION_REQUEST, GET_PROPERTY_REQUEST]
    );
    reply(&mut imports, id, 200, 32, (CHUNK as u32 * 2).to_ne_bytes().to_vec(), 0, &mut wire, now);
    assert_eq!(packets(&mut wire, &mut peer, now)[0][0], DELETE_PROPERTY_REQUEST);
    chunk(&mut imports, id, &mut wire, now);
    packets(&mut wire, &mut peer, now);
    reply(&mut imports, id, 101, 8, vec![b'a'; CHUNK], CHUNK as u32, &mut wire, now);
    imports.pump(&mut wire, &atoms(), now).unwrap();
    assert!(packets(&mut wire, &mut peer, now).is_empty(), "a blocked reader must not acknowledge the property");
    assert!(imports.transfers[0].writing());
    let event_loop = smithay::reexports::calloop::EventLoop::<Server>::try_new().unwrap();
    let mut sources = Vec::new();
    imports.sync_sources(&event_loop.handle(), &mut sources).unwrap();
    assert_eq!(sources.len(), 1);
    assert_eq!(drain(&mut reader), vec![b'p'; prefix]);
    let mut received = Vec::new();
    let mut requests = Vec::new();
    for _ in 0..8 {
        imports.pump(&mut wire, &atoms(), now).unwrap();
        requests.extend(packets(&mut wire, &mut peer, now));
        received.extend(drain(&mut reader));
        if imports.transfers[0].phase == Phase::Reply {
            break;
        }
    }
    assert_eq!(received, vec![b'a'; CHUNK]);
    assert_eq!(requests.len(), 1);
    assert_eq!(requests[0][0], GET_PROPERTY_REQUEST, "partial reads do not delete or acknowledge the X11 chunk");
    assert_eq!(u32::from_ne_bytes(requests[0][16..20].try_into().unwrap()), CHUNK as u32 / 4);
    reply(&mut imports, id, 101, 8, vec![b'b'; CHUNK], 0, &mut wire, now);
    requests.clear();
    for _ in 0..8 {
        imports.pump(&mut wire, &atoms(), now).unwrap();
        requests.extend(packets(&mut wire, &mut peer, now));
        received.extend(drain(&mut reader));
        if imports.transfers[0].phase == Phase::Chunk {
            break;
        }
    }
    assert_eq!(received, [vec![b'a'; CHUNK], vec![b'b'; CHUNK]].concat());
    assert_eq!(requests.iter().map(|p| p[0]).collect::<Vec<_>>(), [DELETE_PROPERTY_REQUEST]);
    chunk(&mut imports, id, &mut wire, now);
    packets(&mut wire, &mut peer, now);
    reply(&mut imports, id, 101, 8, Vec::new(), 0, &mut wire, now);
    imports.pump(&mut wire, &atoms(), now).unwrap();
    // The final zero property ends INCR; its deletion and requestor destruction follow all payload.
    assert_eq!(packets(&mut wire, &mut peer, now).iter().map(|p| p[0]).collect::<Vec<_>>(), [DELETE_PROPERTY_REQUEST, DESTROY_WINDOW_REQUEST]);
    imports.sync_sources(&event_loop.handle(), &mut sources).unwrap();
    assert!(sources.is_empty());
    assert!(imports.transfers.is_empty());
    assert_eq!(imports.deadline(), None);
    assert!(drain(&mut reader).is_empty());
}
#[test]
fn normal_latin1_empty_and_invalid_properties_finish_only_their_own_conversion() {
    let now = Instant::now();
    let (mut wire, mut peer) = wire(now);
    let mut imports = imports();
    for (latin1, type_, format, bytes, expected) in [
        (false, 101, 8, b"native".to_vec(), b"native".to_vec()),
        (true, 31, 8, b"caf\xe9".to_vec(), "café".as_bytes().to_vec()),
        (false, 101, 8, Vec::new(), Vec::new()),
        (false, 101, 16, vec![1, 2], Vec::new()),
        (false, 999, 8, b"wrong type".to_vec(), Vec::new()),
        (false, 101, 8, vec![1; CHUNK + 1], Vec::new()),
    ] {
        let (mut reader, writer) = pipe();
        let id = begin(&mut imports, writer, latin1, &mut wire, now);
        packets(&mut wire, &mut peer, now);
        reply(&mut imports, id, type_, format, bytes, 0, &mut wire, now);
        imports.pump(&mut wire, &atoms(), now).unwrap();
        assert_eq!(drain(&mut reader), expected);
        assert!(imports.transfers.is_empty());
        assert_eq!(imports.deadline(), None);
    }
}
#[test]
fn refusal_timeout_dead_reader_and_late_reply_do_not_affect_a_replacement() {
    let now = Instant::now();
    let (mut wire, mut peer) = wire(now);
    let mut imports = imports();
    let (reader, writer) = pipe();
    let old = begin(&mut imports, writer, false, &mut wire, now);
    packets(&mut wire, &mut peer, now);
    reply(&mut imports, old, 101, 8, b"dead reader".to_vec(), 0, &mut wire, now);
    drop(reader);
    imports.pump(&mut wire, &atoms(), now).unwrap();
    assert!(imports.transfers.is_empty());
    let (_reader, writer) = pipe();
    imports.start(0, Format { atom: 101, mime: "text/plain".into(), latin1: false }, Some(writer), &mut wire, &atoms(), now).unwrap();
    let replacement = imports.transfers[0].id;
    assert_ne!(old, replacement);
    imports.property(old, GetPropertyReply { type_: 101, format: 8, value: b"late".to_vec(), ..Default::default() }, &mut wire, &atoms(), now).unwrap();
    assert_eq!(imports.transfers[0].id, replacement);
    assert!(imports.transfers[0].data.is_empty());
    imports.pump(&mut wire, &atoms(), now + TIMEOUT).unwrap();
    assert!(imports.transfers.is_empty());
    assert_eq!(imports.deadline(), None);
    let (_reader, writer) = pipe();
    imports.start(0, Format { atom: 101, mime: "text/plain".into(), latin1: false }, Some(writer), &mut wire, &atoms(), now).unwrap();
    let transfer = &imports.transfers[0];
    let refused = SelectionNotifyEvent {
        response_type: SELECTION_NOTIFY_EVENT,
        requestor: transfer.window,
        selection: 100,
        target: 101,
        property: 0,
        ..Default::default()
    };
    imports.transfer_event(&event(refused), &mut wire, &atoms(), now).unwrap();
    assert!(imports.transfers.is_empty());
}

fn server() -> (smithay::reexports::wayland_server::Display<Server>, Server) {
    let (display, mut server) = crate::tests::ServerBuilder::new().build();
    server.x11.selections.next_origin = 10;
    (display, server)
}
#[test]
fn offers_wait_for_names_are_tagged_and_do_not_echo_or_clear_newer_wayland_selections() {
    let now = Instant::now();
    let (mut wire, mut peer) = wire(now);
    let (_display, mut server) = server();
    let mut imports = imports();
    imports.owner(0, (41, 1000), &mut wire, &atoms(), &mut server, now).unwrap();
    let id = imports.transfers[0].id;
    let origin = imports.offers.clipboard.origin;
    imports.transfers[0].targets = [101u32, 300].into_iter().flat_map(u32::to_ne_bytes).collect();
    imports.targets(id, &mut wire, &atoms(), now).unwrap();
    imports.finish(id, &mut wire).unwrap();
    imports.publish(0, &mut server);
    assert!(!imports.offers.clipboard.published);
    assert_eq!(server.x11.selections.entries.clipboard.origin, None);
    let name = b"application/x-thor-bytes".to_vec();
    let reply = GetAtomNameReply { length: (name.len() as u32).div_ceil(4), name, ..Default::default() };
    let mut bytes = Vec::new();
    reply.serialize_into(&mut bytes);
    imports.reply(Query::Name(0, origin, 300), &bytes, &mut wire, &atoms(), &mut server, now).unwrap();
    imports.publish(0, &mut server);
    assert!(imports.offers.clipboard.published);
    assert_eq!(server.x11.selections.entries.clipboard.origin, Some(origin));
    assert_eq!(server.x11.selections.entries.clipboard.mime_types, ["text/plain;charset=utf-8", "text/plain", "application/x-thor-bytes"]);
    assert_eq!(*data_device::current_data_device_selection_userdata::<Server>(&server.seat).unwrap(), origin);
    imports.pending.clear();
    packets(&mut wire, &mut peer, now);
    let mut bridge = super::super::super::Bridge { imports, ..Default::default() };
    bridge.turn(&mut wire, &atoms(), 0x200000, &mut server, now).unwrap();
    assert!(!bridge.exports.clipboard.claim);
    assert!(!bridge.exports.clipboard.owned);
    assert!(packets(&mut wire, &mut peer, now).is_empty(), "an X11-derived offer cannot acquire X11 ownership again");
    server.x11.selections.changed(SelectionTarget::Clipboard, None);
    server.x11.selections.entries.clipboard.mime_types = vec!["application/x-newer-wayland".into()];
    let revision = server.x11.selections.entries.clipboard.revision;
    bridge.clear(&mut server);
    assert_eq!(server.x11.selections.entries.clipboard.revision, revision);
    assert_eq!(server.x11.selections.entries.clipboard.mime_types, ["application/x-newer-wayland"]);
    assert_eq!(bridge.deadline(), None);
}
#[test]
fn replacement_owner_cancels_old_requests_without_publishing_an_empty_selection_between_offers() {
    let now = Instant::now();
    let (mut wire, _peer) = wire(now);
    let (_display, mut server) = server();
    let mut imports = imports();
    imports.offers.clipboard.targets_done = true;
    imports.offers.clipboard.formats.push(Format { atom: 101, mime: "text/plain".into(), latin1: false });
    imports.publish(0, &mut server);
    let old_origin = imports.offers.clipboard.origin;
    let old_revision = server.x11.selections.entries.clipboard.revision;
    let (mut reader, writer) = pipe();
    let old_id = begin(&mut imports, writer, false, &mut wire, now);
    imports.owner(0, (42, 2000), &mut wire, &atoms(), &mut server, now).unwrap();
    assert!(drain(&mut reader).is_empty());
    assert_eq!(imports.transfers.len(), 1);
    assert_ne!(imports.transfers[0].id, old_id);
    assert_ne!(imports.offers.clipboard.origin, old_origin);
    assert_eq!(server.x11.selections.entries.clipboard.origin, Some(old_origin));
    assert_eq!(server.x11.selections.entries.clipboard.revision, old_revision, "replacement metadata must not cause an empty offer and keeper restore");
    let new_origin = imports.offers.clipboard.origin;
    imports.offers.clipboard.formats.push(Format { atom: 101, mime: "text/plain".into(), latin1: false });
    imports.offers.clipboard.targets_done = true;
    imports.publish(0, &mut server);
    assert_eq!(server.x11.selections.entries.clipboard.origin, Some(new_origin));
    imports.owner(0, (0, 2001), &mut wire, &atoms(), &mut server, now).unwrap();
    assert_eq!(server.x11.selections.entries.clipboard.origin, None);
    assert!(data_device::current_data_device_selection_userdata::<Server>(&server.seat).is_none());
}

#[test]
fn conversion_and_mailbox_limits_close_only_new_readers_and_old_owner_snapshots_are_ignored() {
    let now = Instant::now();
    let (mut wire, _peer) = wire(now);
    let (_display, mut server) = server();
    let mut imports = imports();
    imports.offers.clipboard.serial = 2;
    let snapshot = GetSelectionOwnerReply { owner: 999, ..Default::default() };
    imports.reply(Query::Owner(0, 1), &event(snapshot), &mut wire, &atoms(), &mut server, now).unwrap();
    assert_eq!(imports.offers.clipboard.owner, 40);
    assert!(imports.transfers.is_empty());
    let mut readers = Vec::new();
    for _ in 0..MAX_TRANSFERS {
        let (reader, writer) = pipe();
        imports.start(0, Format { atom: 101, mime: "text/plain".into(), latin1: false }, Some(writer), &mut wire, &atoms(), now).unwrap();
        readers.push(reader);
    }
    let (mut excess, writer) = pipe();
    imports.start(0, Format { atom: 101, mime: "text/plain".into(), latin1: false }, Some(writer), &mut wire, &atoms(), now).unwrap();
    assert_eq!(imports.transfers.len(), MAX_TRANSFERS);
    crate::tests::wait_closed(&excess);
    assert_eq!(excess.read(&mut [0]).unwrap(), 0);
    let windows: std::collections::BTreeSet<u32> = imports.transfers.iter().map(|transfer| transfer.window).collect();
    assert_eq!(windows.len(), MAX_TRANSFERS, "each conversion has a unique requestor");
    server.x11.selections.entries.clipboard.origin = Some(77);
    let (mut stale, writer) = pipe();
    server.x11.selections.read(SelectionTarget::Clipboard, "text/plain".into(), writer.into(), 76);
    assert!(server.x11.selections.requests.is_empty());
    crate::tests::wait_closed(&stale);
    assert_eq!(stale.read(&mut [0]).unwrap(), 0);
    for _ in 0..MAX_TRANSFERS {
        let (reader, writer) = pipe();
        server.x11.selections.read(SelectionTarget::Clipboard, "text/plain".into(), writer.into(), 77);
        readers.push(reader);
    }
    let (mut excess, writer) = pipe();
    server.x11.selections.read(SelectionTarget::Clipboard, "text/plain".into(), writer.into(), 77);
    assert_eq!(server.x11.selections.requests.len(), MAX_TRANSFERS);
    crate::tests::wait_closed(&excess);
    assert_eq!(excess.read(&mut [0]).unwrap(), 0);
    imports.clear(&mut server);
    assert!(server.x11.selections.requests.is_empty());
    assert_eq!(imports.deadline(), None);
    assert!(readers.iter_mut().all(|reader| {
        crate::tests::wait_closed(reader);
        reader.read(&mut [0]).unwrap() == 0
    }));
}
