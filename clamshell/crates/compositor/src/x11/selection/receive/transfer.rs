//! Read a property in bounded slices and delete it only after the reader accepts the entire chunk.
use super::*;
use std::io::Write;

#[derive(PartialEq, Eq)]
enum Phase {
    Notify,
    Read,
    Reply,
    Drain,
    Chunk,
}
pub(super) struct Transfer {
    pub id: u64,
    selection: usize,
    origin: u64,
    window: u32,
    format: Format,
    pub pipe: Option<File>,
    phase: Phase,
    incremental: bool,
    offset: u32,
    more: bool,
    empty: bool,
    data: Vec<u8>,
    written: usize,
    targets: Vec<u8>,
    pub deadline: Instant,
}
impl Transfer {
    pub fn writing(&self) -> bool {
        self.pipe.is_some() && self.phase == Phase::Drain && self.written < self.data.len()
    }
}
impl Imports {
    pub(super) fn start(&mut self, selection: usize, format: Format, pipe: Option<File>, wire: &mut Transport, atoms: &Atoms, now: Instant) -> io::Result<()> {
        if pipe.is_some() && self.transfers.iter().filter(|transfer| transfer.pipe.is_some()).count() >= MAX_TRANSFERS {
            return Ok(());
        }
        let Some(window) = self.allocator.as_mut().and_then(|allocator| allocator.generate_id()) else {
            log::warn!("compositor: no X11 selection resource ids");
            if pipe.is_none() {
                self.offers.get_mut(selection).targets_done = true;
            }
            return Ok(());
        };
        let root = wire.setup.as_ref().and_then(|setup| setup.roots.first()).map_or(0, |screen| screen.root);
        wire.request(
            CreateWindowRequest {
                depth: 0,
                wid: window,
                parent: root,
                x: 0,
                y: 0,
                width: 1,
                height: 1,
                border_width: 0,
                class: WindowClass::INPUT_ONLY,
                visual: 0,
                value_list: Cow::Owned(CreateWindowAux::new().event_mask(EventMask::PROPERTY_CHANGE)),
            },
            0,
            false,
        )?;
        let offer = self.offers.get(selection);
        wire.request(
            ConvertSelectionRequest {
                requestor: window,
                selection: selection_atom(atoms, selection),
                target: format.atom,
                property: atoms.get("_THOR_SELECTION"),
                time: offer.timestamp,
            },
            0,
            false,
        )?;
        self.next_id = self.next_id.wrapping_add(1);
        self.transfers.push(Transfer {
            id: self.next_id,
            selection,
            origin: offer.origin,
            window,
            format,
            pipe,
            phase: Phase::Notify,
            incremental: false,
            offset: 0,
            more: false,
            empty: false,
            data: Vec::new(),
            written: 0,
            targets: Vec::new(),
            deadline: now + TIMEOUT,
        });
        Ok(())
    }
    pub(super) fn cancel(&mut self, selection: usize, wire: &mut Transport) -> io::Result<()> {
        while let Some(id) = self.transfers.iter().find(|transfer| transfer.selection == selection).map(|transfer| transfer.id) {
            self.finish(id, wire)?;
        }
        Ok(())
    }
    pub(super) fn finish(&mut self, id: u64, wire: &mut Transport) -> io::Result<()> {
        if let Some(index) = self.transfers.iter().position(|transfer| transfer.id == id) {
            let transfer = self.transfers.remove(index);
            if transfer.pipe.is_none() && self.offers.get(transfer.selection).origin == transfer.origin {
                self.offers.get_mut(transfer.selection).targets_done = true;
            }
            wire.request(DestroyWindowRequest { window: transfer.window }, 0, false)?;
        }
        Ok(())
    }
    pub(super) fn transfer_event(&mut self, bytes: &[u8], wire: &mut Transport, atoms: &Atoms, now: Instant) -> io::Result<()> {
        match bytes.first().map(|byte| byte & 0x7f) {
            Some(SELECTION_NOTIFY_EVENT) => {
                let event = SelectionNotifyEvent::try_parse(bytes).map_err(io::Error::other)?.0;
                if let Some(transfer) = self.transfers.iter_mut().find(|transfer| transfer.window == event.requestor && transfer.phase == Phase::Notify) {
                    if event.selection != selection_atom(atoms, transfer.selection)
                        || event.target != transfer.format.atom
                        || event.property != atoms.get("_THOR_SELECTION")
                    {
                        let id = transfer.id;
                        log::warn!("compositor: an X11 client refused or mismatched a selection transfer: not pasted");
                        self.finish(id, wire)?;
                    } else {
                        transfer.phase = Phase::Read;
                        transfer.deadline = now + TIMEOUT;
                    }
                }
            }
            Some(PROPERTY_NOTIFY_EVENT) => {
                let event = PropertyNotifyEvent::try_parse(bytes).map_err(io::Error::other)?.0;
                if event.state == Property::NEW_VALUE
                    && event.atom == atoms.get("_THOR_SELECTION")
                    && let Some(transfer) = self.transfers.iter_mut().find(|transfer| transfer.window == event.window && transfer.phase == Phase::Chunk)
                {
                    transfer.phase = Phase::Read;
                    transfer.deadline = now + TIMEOUT;
                }
            }
            _ => {}
        }
        Ok(())
    }
    pub(super) fn property(&mut self, id: u64, reply: GetPropertyReply, wire: &mut Transport, atoms: &Atoms, now: Instant) -> io::Result<()> {
        let Some(transfer) = self.transfers.iter_mut().find(|transfer| transfer.id == id && transfer.phase == Phase::Reply) else {
            return Ok(());
        };
        if reply.type_ == atoms.get("INCR") && !transfer.incremental && transfer.offset == 0 {
            if reply.format != 32 || reply.value.len() != 4 || reply.bytes_after != 0 {
                return self.invalid(id, wire);
            }
            transfer.incremental = true;
            transfer.phase = Phase::Chunk;
            transfer.deadline = now + TIMEOUT;
            wire.request(DeletePropertyRequest { window: transfer.window, property: atoms.get("_THOR_SELECTION") }, 0, false)?;
            return Ok(());
        }
        let metadata = transfer.pipe.is_none();
        let (expected, format) = if metadata { (u32::from(AtomEnum::ATOM), 32) } else { (transfer.format.atom, 8) };
        let limit = if metadata {
            4096
        } else if transfer.format.latin1 {
            CHUNK / 2
        } else {
            CHUNK
        };
        if reply.type_ != expected
            || reply.format != format
            || reply.value.len() > limit
            || (reply.bytes_after != 0 && (reply.value.is_empty() || !reply.value.len().is_multiple_of(4)))
        {
            return self.invalid(id, wire);
        }
        transfer.empty = reply.value.is_empty();
        transfer.more = reply.bytes_after != 0;
        transfer.offset = transfer.offset.saturating_add((reply.value.len() as u32).div_ceil(4));
        transfer.written = 0;
        if metadata {
            if transfer.targets.len() + reply.value.len() > 4096 {
                return self.invalid(id, wire);
            }
            transfer.targets.extend_from_slice(&reply.value);
        } else if transfer.format.latin1 {
            transfer.data.clear();
            for byte in reply.value {
                let mut text = [0; 4];
                transfer.data.extend_from_slice(char::from(byte).encode_utf8(&mut text).as_bytes());
            }
        } else {
            transfer.data = reply.value;
        }
        transfer.phase = Phase::Drain;
        transfer.deadline = now + TIMEOUT;
        Ok(())
    }
    fn invalid(&mut self, id: u64, wire: &mut Transport) -> io::Result<()> {
        log::warn!("compositor: an X11 selection transfer's property is invalid: not pasted");
        self.finish(id, wire)
    }
    pub(super) fn pump(&mut self, wire: &mut Transport, atoms: &Atoms, now: Instant) -> io::Result<()> {
        let mut index = 0;
        while index < self.transfers.len() {
            let Some(transfer) = self.transfers.get_mut(index) else { break };
            let id = transfer.id;
            if now >= transfer.deadline {
                log::warn!("compositor: an X11 selection transfer timed out: not pasted");
                self.finish(id, wire)?;
                continue;
            }
            if transfer.phase == Phase::Drain {
                let mut failed = false;
                if let Some(pipe) = &mut transfer.pipe {
                    for _ in 0..4 {
                        let Some(bytes) = transfer.data.get(transfer.written..).filter(|bytes| !bytes.is_empty()) else { break };
                        match pipe.write(bytes) {
                            Ok(0) => {
                                failed = true;
                                break;
                            }
                            Ok(n) => {
                                transfer.written += n;
                                transfer.deadline = now + TIMEOUT;
                            }
                            Err(error) if error.kind() == io::ErrorKind::WouldBlock => break,
                            Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
                            Err(error) => {
                                log::debug!("compositor: selection reader: {error}");
                                failed = true;
                                break;
                            }
                        }
                    }
                }
                if failed {
                    self.finish(id, wire)?;
                    continue;
                }
                if transfer.pipe.is_none() || transfer.written == transfer.data.len() {
                    transfer.data.clear();
                    if transfer.more {
                        transfer.phase = Phase::Read;
                    } else {
                        // This deletion acknowledges the entire property, including any partial reads.
                        wire.request(DeletePropertyRequest { window: transfer.window, property: atoms.get("_THOR_SELECTION") }, 0, false)?;
                        transfer.offset = 0;
                        if transfer.incremental && !transfer.empty {
                            transfer.phase = Phase::Chunk;
                        } else {
                            if transfer.pipe.is_none() {
                                self.targets(id, wire, atoms, now)?;
                            }
                            self.finish(id, wire)?;
                            continue;
                        }
                    }
                }
            }
            let Some(transfer) = self.transfers.get_mut(index) else { continue };
            if transfer.phase == Phase::Read && self.pending.len() < 256 {
                transfer.phase = Phase::Reply;
                let request = GetPropertyRequest {
                    delete: false,
                    window: transfer.window,
                    property: atoms.get("_THOR_SELECTION"),
                    type_: 0,
                    long_offset: transfer.offset,
                    long_length: if transfer.pipe.is_none() {
                        1024
                    } else if transfer.format.latin1 {
                        CHUNK as u32 / 8
                    } else {
                        CHUNK as u32 / 4
                    },
                };
                self.query(wire, request, 0, Query::Property(id), now)?;
            }
            index += 1;
        }
        Ok(())
    }
    fn targets(&mut self, id: u64, wire: &mut Transport, atoms: &Atoms, now: Instant) -> io::Result<()> {
        let Some(transfer) = self.transfers.iter().find(|transfer| transfer.id == id) else {
            return Ok(());
        };
        let (selection, origin) = (transfer.selection, transfer.origin);
        let mut targets: Vec<u32> = transfer.targets.chunks_exact(4).filter_map(|bytes| bytes.try_into().ok().map(u32::from_ne_bytes)).collect();
        targets.sort_unstable();
        targets.dedup();
        let offer = self.offers.get_mut(selection);
        if offer.origin != origin {
            return Ok(());
        }
        let text = if targets.contains(&atoms.get("UTF8_STRING")) {
            Some((atoms.get("UTF8_STRING"), false))
        } else if targets.contains(&u32::from(AtomEnum::STRING)) {
            Some((AtomEnum::STRING.into(), true))
        } else {
            None
        };
        if let Some((atom, latin1)) = text {
            for mime in ["text/plain;charset=utf-8", "text/plain"] {
                offer.formats.push(Format { atom, mime: mime.into(), latin1 });
            }
        }
        for atom in targets
            .into_iter()
            .filter(|atom| {
                *atom != 0
                    && ![atoms.get("UTF8_STRING"), AtomEnum::STRING.into(), atoms.get("TEXT"), atoms.get("TARGETS"), atoms.get("TIMESTAMP"), atoms.get("INCR")]
                        .contains(atom)
            })
            .take(62.min(256usize.saturating_sub(self.pending.len())))
        {
            self.query(wire, GetAtomNameRequest { atom }, 0, Query::Name(selection, origin, atom), now)?;
            self.offers.get_mut(selection).pending_names += 1;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests;
