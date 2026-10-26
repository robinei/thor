//! From X11 to Wayland: XFixes tells when an X11 client takes a selection; its formats are read and offered
//! to the seat, tagged with a generation so a stale offer is never published, and a selection we own for a
//! Wayland client is not offered back. A Wayland client's paste reads the data from the owner (`transfer`).

mod transfer;
use super::*;
use smithay::wayland::selection::{data_device, primary_selection};
use std::{borrow::Cow, fs::File};
use x11rb_protocol::{id_allocator::IdAllocator, protocol::xfixes, x11_utils::Request};

struct Pending {
    sequence: u64,
    query: Query,
    deadline: Instant,
}
enum Query {
    Extension,
    Version,
    Owner(usize, u64),
    Property(u64),
    Name(usize, u64, u32),
}
#[derive(Default)]
struct Offer {
    serial: u64,
    owner: u32,
    origin: u64,
    revision: u64,
    timestamp: u32,
    formats: Vec<Format>,
    pending_names: usize,
    published: bool,
    targets_done: bool,
    previous_origin: Option<u64>,
}
#[derive(Default)]
pub(super) struct Imports {
    started: bool,
    wm_owner: u32,
    extension: Option<(u8, u8)>,
    ready: bool,
    allocator: Option<IdAllocator>,
    offers: Pair<Offer>,
    pending: Vec<Pending>,
    transfers: Vec<transfer::Transfer>,
    next_id: u64,
    changes: Pair<Option<u64>>,
}
impl Imports {
    pub fn deadline(&self) -> Option<Instant> {
        self.pending.iter().map(|pending| pending.deadline).chain(self.transfers.iter().map(|transfer| transfer.deadline)).min()
    }
    pub fn clear(&mut self, server: &mut Server) {
        for selection in 0..2 {
            self.clear_seat(selection, server);
        }
        server.x11.selections.requests.clear();
        *self = Self::default();
    }
    fn clear_seat(&self, selection: usize, server: &mut Server) {
        let entry = server.x11.selections.entries.get_mut(selection);
        if entry.origin == Some(self.offers.get(selection).origin)
            || (entry.origin.is_some() && entry.origin == self.offers.get(selection).previous_origin && entry.revision == self.offers.get(selection).revision)
        {
            entry.origin = None;
            entry.revision = entry.revision.wrapping_add(1);
            entry.mime_types.clear();
            match target(selection) {
                SelectionTarget::Clipboard => data_device::clear_data_device_selection(&server.dh, &server.seat),
                SelectionTarget::Primary => primary_selection::clear_primary_selection(&server.dh, &server.seat),
            }
        }
    }
    fn query(&mut self, wire: &mut Transport, request: impl Request, opcode: u8, query: Query, now: Instant) -> io::Result<()> {
        if self.pending.len() >= 256 {
            return Err(io::Error::other("selection request queue exceeded"));
        }
        let sequence = wire.request(request, opcode, true)?;
        self.pending.push(Pending { sequence, query, deadline: now + Duration::from_secs(3) });
        Ok(())
    }
    pub fn start_setup(&mut self, wire: &mut Transport, owner: u32, now: Instant) -> io::Result<()> {
        if !self.started
            && let Some(setup) = &wire.setup
        {
            let mut allocator = IdAllocator::new(setup.resource_id_base, setup.resource_id_mask).map_err(io::Error::other)?;
            // Wm allocated its hidden owner first. All further private resources are owned here.
            allocator.generate_id();
            self.allocator = Some(allocator);
            self.wm_owner = owner;
            self.started = true;
            self.query(wire, QueryExtensionRequest { name: b"XFIXES".as_slice().into() }, 0, Query::Extension, now)?;
        }
        Ok(())
    }
    pub fn turn(&mut self, wire: &mut Transport, atoms: &Atoms, server: &mut Server, now: Instant) -> io::Result<()> {
        let mut index = 0;
        while index < self.pending.len() {
            let Some(pending) = self.pending.get(index) else { break };
            if let Some(bytes) = wire.reply(pending.sequence) {
                let pending = self.pending.remove(index);
                self.reply(pending.query, &bytes, wire, atoms, server, now)?;
            } else if now >= pending.deadline {
                return Err(io::Error::new(io::ErrorKind::TimedOut, "X11 selection reply timed out"));
            } else {
                index += 1;
            }
        }
        for selection in 0..2 {
            let offer = self.offers.get(selection);
            let entry = server.x11.selections.entries.get(selection);
            if offer.owner != 0 && (if offer.published { entry.origin != Some(offer.origin) } else { entry.revision != offer.revision }) {
                log::debug!("compositor: X11 selection generation {} superseded by a Wayland selection", offer.origin);
                self.cancel(selection, wire)?;
                self.offers.get_mut(selection).owner = 0;
            }
            self.publish(selection, server);
            let offer = self.offers.get(selection);
            if offer.owner != 0 && !offer.published && offer.targets_done && offer.pending_names == 0 && offer.formats.is_empty() {
                self.clear_seat(selection, server);
                self.offers.get_mut(selection).owner = 0;
            }
        }
        for request in server.x11.selections.requests.drain(..) {
            let offer = self.offers.get(request.selection);
            if !offer.published || offer.owner == 0 || offer.origin != request.origin {
                continue;
            }
            let Some(format) = offer.formats.iter().find(|format| format.mime == request.mime).cloned() else { continue };
            self.start(request.selection, format, Some(File::from(request.fd)), wire, atoms, now)?;
        }
        self.pump(wire, atoms, now)
    }
    fn reply(&mut self, query: Query, bytes: &[u8], wire: &mut Transport, atoms: &Atoms, server: &mut Server, now: Instant) -> io::Result<()> {
        if bytes.first() == Some(&0) {
            match query {
                Query::Property(id) => self.finish(id, wire)?,
                Query::Name(selection, origin, _) => {
                    let offer = self.offers.get_mut(selection);
                    if offer.origin == origin {
                        offer.pending_names = offer.pending_names.saturating_sub(1);
                    }
                }
                _ => log::warn!("compositor: X11 selection initialisation rejected"),
            }
            return Ok(());
        }
        match query {
            Query::Extension => {
                let reply = QueryExtensionReply::try_parse(bytes).map_err(io::Error::other)?.0;
                if !reply.present {
                    log::warn!("compositor: Xwayland lacks XFixes selection notifications");
                    return Ok(());
                }
                self.extension = Some((reply.major_opcode, reply.first_event));
                self.query(wire, xfixes::QueryVersionRequest { client_major_version: 5, client_minor_version: 0 }, reply.major_opcode, Query::Version, now)?;
            }
            Query::Version => {
                let reply = xfixes::QueryVersionReply::try_parse(bytes).map_err(io::Error::other)?.0;
                if reply.major_version < 2 {
                    log::warn!("compositor: XFixes 2 required for selection notifications");
                    return Ok(());
                }
                for selection in 0..2 {
                    wire.request(
                        xfixes::SelectSelectionInputRequest {
                            window: self.wm_owner,
                            selection: selection_atom(atoms, selection),
                            event_mask: xfixes::SelectionEventMask::SET_SELECTION_OWNER
                                | xfixes::SelectionEventMask::SELECTION_WINDOW_DESTROY
                                | xfixes::SelectionEventMask::SELECTION_CLIENT_CLOSE,
                        },
                        self.extension.map_or(0, |(opcode, _)| opcode),
                        false,
                    )?;
                    self.query(
                        wire,
                        GetSelectionOwnerRequest { selection: selection_atom(atoms, selection) },
                        0,
                        Query::Owner(selection, self.offers.get(selection).serial),
                        now,
                    )?;
                }
                self.ready = true;
            }
            Query::Owner(selection, serial) => {
                if self.offers.get(selection).serial == serial {
                    let reply = GetSelectionOwnerReply::try_parse(bytes).map_err(io::Error::other)?.0;
                    self.owner(selection, (reply.owner, 0), wire, atoms, server, now)?;
                }
            }
            Query::Property(id) => {
                let reply = GetPropertyReply::try_parse(bytes).map_err(io::Error::other)?.0;
                self.property(id, reply, wire, atoms, now)?;
            }
            Query::Name(selection, origin, atom) => {
                let reply = GetAtomNameReply::try_parse(bytes).map_err(io::Error::other)?.0;
                let offer = self.offers.get_mut(selection);
                if offer.origin == origin {
                    offer.pending_names = offer.pending_names.saturating_sub(1);
                    if reply.name.len() <= 255
                        && let Ok(mime) = std::str::from_utf8(&reply.name)
                        && (mime.contains('/') || mime == "x-kde-passwordManagerHint")
                        && !offer.formats.iter().any(|format| format.mime == mime)
                    {
                        offer.formats.push(Format { atom, mime: mime.into(), latin1: false });
                    }
                }
            }
        }
        Ok(())
    }
    fn owner(&mut self, selection: usize, ownership: (u32, u32), wire: &mut Transport, atoms: &Atoms, server: &mut Server, now: Instant) -> io::Result<()> {
        let (xid, timestamp) = ownership;
        let owner = self.wm_owner;
        self.cancel(selection, wire)?;
        if xid == 0 || xid == owner {
            self.clear_seat(selection, server);
        }
        if xid != 0 && xid != owner {
            *self.changes.get_mut(selection) = Some(server.x11.selections.entries.get(selection).revision);
        }
        let serial = self.offers.get(selection).serial.wrapping_add(1);
        let origin = server.x11.selections.origin();
        *self.offers.get_mut(selection) = Offer {
            serial,
            owner: if xid == owner { 0 } else { xid },
            origin,
            revision: server.x11.selections.entries.get(selection).revision,
            previous_origin: server.x11.selections.entries.get(selection).origin,
            timestamp,
            ..Default::default()
        };
        if xid != 0 && xid != owner {
            self.start(selection, Format { atom: atoms.get("TARGETS"), mime: String::new(), latin1: false }, None, wire, atoms, now)?;
        }
        Ok(())
    }
    pub fn take_change(&mut self, selection: usize) -> Option<u64> {
        self.changes.get_mut(selection).take()
    }
    fn publish(&mut self, selection: usize, server: &mut Server) {
        let offer = self.offers.get_mut(selection);
        if offer.owner == 0 || offer.published || !offer.targets_done || offer.pending_names != 0 || offer.formats.is_empty() {
            return;
        }
        let entry = server.x11.selections.entries.get_mut(selection);
        if entry.revision != offer.revision {
            return;
        }
        entry.revision = entry.revision.wrapping_add(1);
        entry.origin = Some(offer.origin);
        entry.mime_types = offer.formats.iter().map(|format| format.mime.clone()).collect();
        offer.published = true;
        log::debug!("compositor: imported X11 selection generation {} with {} formats", offer.origin, offer.formats.len());
        match target(selection) {
            SelectionTarget::Clipboard => data_device::set_data_device_selection(&server.dh, &server.seat, entry.mime_types.clone(), offer.origin),
            SelectionTarget::Primary => primary_selection::set_primary_selection(&server.dh, &server.seat, entry.mime_types.clone(), offer.origin),
        }
    }
    pub fn event(&mut self, bytes: &[u8], wire: &mut Transport, atoms: &Atoms, server: &mut Server, now: Instant) -> io::Result<()> {
        if self.ready && bytes.first().map(|byte| byte & 0x7f) == self.extension.map(|(_, event)| event) {
            let event = xfixes::SelectionNotifyEvent::try_parse(bytes).map_err(io::Error::other)?.0;
            if let Some(selection) = (0..2).find(|selection| selection_atom(atoms, *selection) == event.selection) {
                self.owner(selection, (event.owner, event.selection_timestamp), wire, atoms, server, now)?;
            }
            return Ok(());
        }
        self.transfer_event(bytes, wire, atoms, now)
    }
    pub fn sync_sources(&self, handle: &LoopHandle<'static, Server>, sources: &mut Vec<IoSource>) -> io::Result<()> {
        sources.retain(|source| {
            if !source.incoming || self.transfers.iter().any(|transfer| transfer.id == source.id && transfer.writing()) {
                true
            } else {
                handle.remove(source.token);
                false
            }
        });
        for transfer in self.transfers.iter().filter(|transfer| transfer.writing()) {
            if !sources.iter().any(|source| source.incoming && source.id == transfer.id)
                && let Some(pipe) = &transfer.pipe
            {
                sources.push(IoSource {
                    id: transfer.id,
                    incoming: true,
                    token: super::super::watch(handle, pipe, smithay::reexports::calloop::Interest::WRITE)?,
                });
            }
        }
        Ok(())
    }
}
