//! From Wayland to X11: an X11 client's request for a selection a Wayland client offers is answered from
//! that client's pipe into a property of the requestor's, in bounded chunks; with INCR, for large data, each
//! next chunk is written only after the requestor deleted the last.

use super::*;
use std::{fs::File, io::Read};

pub(super) struct Transfer {
    pub id: u64,
    selection: usize,
    pub request: SelectionRequestEvent,
    pub property: u32,
    pub pipe: File,
    format: Format,
    data: Vec<u8>,
    encoding: Latin1,
    eof: bool,
    pub incremental: bool,
    pub permission: bool,
    pub deadline: Instant,
}
impl Transfer {
    fn buffered(&self) -> usize {
        self.data.len() + self.encoding.pending.len()
    }
    pub fn reading(&self) -> bool {
        !self.eof && self.buffered() < CHUNK
    }
    fn read(&mut self, now: Instant) -> io::Result<()> {
        for _ in 0..4 {
            if !self.reading() {
                break;
            }
            let mut bytes = [0; 4096];
            let capacity = bytes.len().min(CHUNK - self.buffered());
            let buffer = bytes.get_mut(..capacity).ok_or_else(|| io::Error::other("clipboard buffer size"))?;
            match self.pipe.read(buffer) {
                Ok(n) => {
                    if self.format.latin1 {
                        self.encoding.append(buffer.get(..n).unwrap_or_default(), n == 0, &mut self.data);
                    } else {
                        self.data.extend_from_slice(buffer.get(..n).unwrap_or_default());
                    }
                    self.eof = n == 0;
                    if n != 0 {
                        self.deadline = now + TIMEOUT;
                    }
                }
                Err(error) if error.kind() == io::ErrorKind::WouldBlock => break,
                Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
                Err(error) => return Err(error),
            }
        }
        Ok(())
    }
}

impl Bridge {
    pub(super) fn request(&mut self, request: SelectionRequestEvent, wire: &mut Transport, atoms: &Atoms, server: &mut Server, now: Instant) -> io::Result<()> {
        let property = if request.property == 0 { request.target } else { request.property };
        let selection = (0..2).find(|selection| selection_atom(atoms, *selection) == request.selection && self.exports.get(*selection).owned);
        let Some(selection) = selection else {
            log::debug!("compositor: refused an X11 selection request: the selection isn't ours");
            return notify(wire, &request, 0);
        };
        let export = self.exports.get(selection);
        if export.revision != Some(server.x11.selections.entries.get(selection).revision) {
            log::debug!("compositor: refused an X11 selection request: the selection changed and X11 isn't told yet");
            return notify(wire, &request, 0);
        }
        if request.target == atoms.get("TARGETS") {
            let mut targets: Vec<u32> = export.formats.iter().map(|format| format.atom).collect();
            targets.extend([atoms.get("TARGETS"), atoms.get("TIMESTAMP")]);
            targets.sort_unstable();
            targets.dedup();
            property32(wire, request.requestor, property, AtomEnum::ATOM.into(), &targets)?;
            return notify(wire, &request, property);
        }
        if request.target == atoms.get("TIMESTAMP") {
            property32(wire, request.requestor, property, AtomEnum::INTEGER.into(), &[export.timestamp])?;
            return notify(wire, &request, property);
        }
        let format = export.formats.iter().find(|format| format.atom == request.target).cloned();
        let Some(format) = format else {
            log::debug!("compositor: refused an X11 selection request: target {} isn't offered", request.target);
            return notify(wire, &request, 0);
        };
        if self.transfers.len() >= MAX_TRANSFERS {
            log::warn!("compositor: refused an X11 selection request: {MAX_TRANSFERS} transfers already under way");
            return notify(wire, &request, 0);
        }
        if self.transfers.iter().any(|transfer| transfer.request.requestor == request.requestor && transfer.property == property) {
            log::warn!("compositor: refused an X11 selection request: one into the same property is under way");
            return notify(wire, &request, 0);
        }
        let (read, write) = match rustix::pipe::pipe_with(rustix::pipe::PipeFlags::CLOEXEC) {
            Ok(pipe) => pipe,
            Err(error) => {
                log::warn!("compositor: clipboard pipe: {error}");
                return notify(wire, &request, 0);
            }
        };
        if let Err(error) = rustix::fs::fcntl_setfl(&read, rustix::fs::OFlags::NONBLOCK) {
            log::warn!("compositor: clipboard pipe flags: {error}");
            return notify(wire, &request, 0);
        }
        let accepted = match target(selection) {
            SelectionTarget::Clipboard => {
                smithay::wayland::selection::data_device::request_data_device_client_selection(&server.seat, format.mime.clone(), write).is_ok()
            }
            SelectionTarget::Primary => {
                smithay::wayland::selection::primary_selection::request_primary_client_selection(&server.seat, format.mime.clone(), write).is_ok()
            }
        };
        if !accepted {
            log::warn!("compositor: refused an X11 selection request: the Wayland selection can't be asked for its data");
            return notify(wire, &request, 0);
        }
        wire.request(
            ChangeWindowAttributesRequest {
                window: request.requestor,
                value_list: std::borrow::Cow::Owned(ChangeWindowAttributesAux::new().event_mask(EventMask::PROPERTY_CHANGE | EventMask::STRUCTURE_NOTIFY)),
            },
            0,
            false,
        )?;
        self.next_id = self.next_id.wrapping_add(1);
        self.transfers.push(Transfer {
            id: self.next_id,
            selection,
            request,
            property,
            pipe: read.into(),
            format,
            data: Vec::with_capacity(CHUNK),
            encoding: Latin1::default(),
            eof: false,
            incremental: false,
            permission: false,
            deadline: now + TIMEOUT,
        });
        Ok(())
    }
    pub(super) fn cancel(&mut self, selection: usize, wire: &mut Transport, _now: Instant) -> io::Result<()> {
        let mut index = 0;
        while index < self.transfers.len() {
            if self.transfers.get(index).is_some_and(|transfer| transfer.selection == selection) {
                let transfer = self.transfers.remove(index);
                log::info!("compositor: X11 clipboard transfer {} ended: the selection changed during it", transfer.id);
                if !transfer.incremental {
                    notify(wire, &transfer.request, 0)?;
                }
            } else {
                index += 1;
            }
        }
        Ok(())
    }
    pub(super) fn pump(&mut self, wire: &mut Transport, atoms: &Atoms, now: Instant) -> io::Result<()> {
        let mut index = 0;
        while index < self.transfers.len() {
            let Some(transfer) = self.transfers.get_mut(index) else { break };
            let failed = now >= transfer.deadline
                || match transfer.read(now) {
                    Ok(()) => false,
                    Err(error) => {
                        log::warn!("compositor: clipboard source: {error}");
                        true
                    }
                };
            if failed {
                log::warn!("compositor: X11 clipboard transfer {} failed or timed out: not pasted", transfer.id);
                if !transfer.incremental {
                    notify(wire, &transfer.request, 0)?;
                }
                self.transfers.remove(index);
                continue;
            }
            let done;
            if !transfer.incremental && transfer.eof {
                property8(wire, transfer, atoms)?;
                notify(wire, &transfer.request, transfer.property)?;
                done = true;
            } else if !transfer.incremental && transfer.buffered() >= CHUNK {
                // The marker and notify must precede any data; the requestor's deletion starts INCR.
                property32(wire, transfer.request.requestor, transfer.property, atoms.get("INCR"), &[transfer.data.len() as u32])?;
                notify(wire, &transfer.request, transfer.property)?;
                transfer.incremental = true;
                done = false;
            } else if transfer.incremental && transfer.permission && (!transfer.data.is_empty() || transfer.eof) {
                done = transfer.eof && transfer.data.is_empty();
                property8(wire, transfer, atoms)?;
                transfer.permission = false;
            } else {
                done = false;
            }
            if done {
                self.transfers.remove(index);
            } else {
                index += 1;
            }
        }
        Ok(())
    }
}
fn property8(wire: &mut Transport, transfer: &mut Transfer, atoms: &Atoms) -> io::Result<()> {
    let type_ = if transfer.format.latin1 {
        AtomEnum::STRING.into()
    } else if transfer.request.target == atoms.get("TEXT") {
        atoms.get("UTF8_STRING")
    } else {
        transfer.request.target
    };
    wire.request(
        ChangePropertyRequest {
            mode: PropMode::REPLACE,
            window: transfer.request.requestor,
            property: transfer.property,
            type_,
            format: 8,
            data_len: transfer.data.len() as u32,
            data: transfer.data.as_slice().into(),
        },
        0,
        false,
    )?;
    transfer.data.clear();
    Ok(())
}
fn property32(wire: &mut Transport, window: u32, property: u32, type_: u32, values: &[u32]) -> io::Result<()> {
    wire.request(
        ChangePropertyRequest {
            mode: PropMode::REPLACE,
            window,
            property,
            type_,
            format: 32,
            data_len: values.len() as u32,
            data: values.iter().flat_map(|value| value.to_ne_bytes()).collect::<Vec<_>>().into(),
        },
        0,
        false,
    )?;
    Ok(())
}
fn notify(wire: &mut Transport, request: &SelectionRequestEvent, property: u32) -> io::Result<()> {
    use x11rb_protocol::x11_utils::Serialize;
    let event = SelectionNotifyEvent {
        response_type: SELECTION_NOTIFY_EVENT,
        sequence: 0,
        time: request.time,
        requestor: request.requestor,
        selection: request.selection,
        target: request.target,
        property,
    };
    let mut bytes = [0; 32];
    if let Some(destination) = bytes.get_mut(..24) {
        destination.copy_from_slice(&event.serialize());
    }
    wire.request(
        SendEventRequest { propagate: false, destination: request.requestor, event_mask: EventMask::NO_EVENT, event: std::borrow::Cow::Owned(bytes) },
        0,
        false,
    )?;
    Ok(())
}

/// X11 STRING is Latin-1. Preserve split UTF-8 scalars and replace unrepresentable/invalid text.
#[derive(Default)]
struct Latin1 {
    pending: Vec<u8>,
}
impl Latin1 {
    fn append(&mut self, bytes: &[u8], eof: bool, out: &mut Vec<u8>) {
        self.pending.extend_from_slice(bytes);
        let mut consumed = 0;
        while let Some(rest) = self.pending.get(consumed..) {
            if rest.is_empty() {
                break;
            }
            match std::str::from_utf8(rest) {
                Ok(text) => {
                    out.extend(text.chars().map(|character| u8::try_from(u32::from(character)).unwrap_or(b'?')));
                    consumed = self.pending.len();
                }
                Err(error) => {
                    if let Some(valid) = rest.get(..error.valid_up_to()).and_then(|valid| std::str::from_utf8(valid).ok()) {
                        out.extend(valid.chars().map(|character| u8::try_from(u32::from(character)).unwrap_or(b'?')));
                    }
                    consumed += error.valid_up_to();
                    if let Some(length) = error.error_len() {
                        out.push(b'?');
                        consumed += length;
                    } else if eof {
                        out.push(b'?');
                        consumed = self.pending.len();
                    } else {
                        break;
                    }
                }
            }
        }
        self.pending.drain(..consumed);
    }
}

#[cfg(test)]
mod tests;
