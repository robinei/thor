//! Seat selection changes are translated here; neither State nor drawing knows about X11 transfers.
mod receive;
mod send;

use super::{properties::Atoms, transport::Transport};
use crate::server::Server;
use smithay::{
    reexports::calloop::{LoopHandle, RegistrationToken},
    wayland::selection::{SelectionSource, SelectionTarget},
};
use std::{
    io,
    os::fd::OwnedFd,
    time::{Duration, Instant},
};
use x11rb_protocol::{protocol::xproto::*, x11_utils::TryParse};

const TIMEOUT: Duration = Duration::from_secs(5);
const MAX_TRANSFERS: usize = 8;
const CHUNK: usize = 16 * 1024;

#[derive(Default)]
struct Pair<T> {
    clipboard: T,
    primary: T,
}
impl<T> Pair<T> {
    fn get(&self, index: usize) -> &T {
        if index == 0 { &self.clipboard } else { &self.primary }
    }
    fn get_mut(&mut self, index: usize) -> &mut T {
        if index == 0 { &mut self.clipboard } else { &mut self.primary }
    }
}
#[derive(Default)]
pub(crate) struct SeatSelections {
    entries: Pair<SeatSelection>,
    requests: Vec<ReadRequest>,
    next_origin: u64,
}
#[derive(Default)]
struct SeatSelection {
    revision: u64,
    mime_types: Vec<String>,
    origin: Option<u64>,
}
impl SeatSelections {
    pub fn changed(&mut self, target: SelectionTarget, source: Option<SelectionSource>) {
        let entry = self.entries.get_mut(slot(target));
        entry.revision = entry.revision.wrapping_add(1);
        entry.origin = None;
        entry.mime_types = source.map(|source| source.mime_types()).unwrap_or_default();
        entry.mime_types.retain(|mime| mime.len() <= 255);
        entry.mime_types.truncate(64);
    }
}
struct ReadRequest {
    selection: usize,
    origin: u64,
    mime: String,
    fd: OwnedFd,
}
impl SeatSelections {
    pub fn read(&mut self, target: SelectionTarget, mime: String, fd: OwnedFd, origin: u64) {
        if self.entries.get(slot(target)).origin != Some(origin) || self.requests.len() >= MAX_TRANSFERS {
            return;
        }
        if let Err(error) = rustix::fs::fcntl_setfl(&fd, rustix::fs::OFlags::NONBLOCK) {
            log::warn!("compositor: selection reader flags: {error}");
            return;
        }
        self.requests.push(ReadRequest { selection: slot(target), origin, mime, fd });
    }
    fn origin(&mut self) -> u64 {
        self.next_origin = self.next_origin.wrapping_add(1);
        self.next_origin
    }
}
fn slot(target: SelectionTarget) -> usize {
    match target {
        SelectionTarget::Clipboard => 0,
        SelectionTarget::Primary => 1,
    }
}
fn target(index: usize) -> SelectionTarget {
    if index == 0 { SelectionTarget::Clipboard } else { SelectionTarget::Primary }
}
#[derive(Clone)]
struct Format {
    atom: u32,
    mime: String,
    latin1: bool,
}
#[derive(Default)]
struct Export {
    revision: Option<u64>,
    owned: bool,
    claim: bool,
    timestamp: u32,
    formats: Vec<Format>,
    pending: usize,
}
struct PendingAtom {
    sequence: u64,
    selection: usize,
    revision: u64,
    mime: String,
    deadline: Instant,
}
#[derive(Default)]
pub(super) struct Bridge {
    exports: Pair<Export>,
    pending: Vec<PendingAtom>,
    transfers: Vec<send::Transfer>,
    next_id: u64,
    clock_pending: Option<Clock>,
    imports: receive::Imports,
}
struct Clock {
    deadline: Instant,
    revisions: Pair<Option<u64>>,
}
pub(super) struct IoSource {
    id: u64,
    token: RegistrationToken,
    incoming: bool,
}
impl Bridge {
    pub fn deadline(&self) -> Option<Instant> {
        self.pending
            .iter()
            .map(|pending| pending.deadline)
            .chain(self.transfers.iter().map(|transfer| transfer.deadline))
            .chain(self.clock_pending.as_ref().map(|clock| clock.deadline))
            .chain(self.imports.deadline())
            .min()
    }
    pub fn clear(&mut self, server: &mut Server) {
        self.imports.clear(server);
        self.transfers.clear();
        self.pending.clear();
        self.exports = Default::default();
        self.clock_pending = None;
    }
    pub fn init_imports(&mut self, wire: &mut Transport, owner: u32, now: Instant) -> io::Result<()> {
        self.imports.start_setup(wire, owner, now)
    }
    pub fn turn(&mut self, wire: &mut Transport, atoms: &Atoms, owner: u32, server: &mut Server, now: Instant) -> io::Result<()> {
        if self.clock_pending.as_ref().is_some_and(|clock| now >= clock.deadline) {
            return Err(io::Error::new(io::ErrorKind::TimedOut, "clipboard timestamp event timed out"));
        }
        let mut index = 0;
        while index < self.pending.len() {
            let pending = self.pending.get(index).ok_or_else(|| io::Error::other("clipboard pending index"))?;
            if let Some(bytes) = wire.reply(pending.sequence) {
                let pending = self.pending.remove(index);
                let export = self.exports.get_mut(pending.selection);
                if export.revision != Some(pending.revision) {
                    continue;
                }
                export.pending = export.pending.saturating_sub(1);
                if bytes.first() != Some(&0) {
                    let reply = InternAtomReply::try_parse(&bytes).map_err(io::Error::other)?.0;
                    export.formats.push(Format { atom: reply.atom, mime: pending.mime, latin1: false });
                }
            } else if now >= pending.deadline {
                return Err(io::Error::new(io::ErrorKind::TimedOut, "clipboard atom reply timed out"));
            } else {
                index += 1;
            }
        }
        self.imports.turn(wire, atoms, server, now)?;
        for selection in 0..2 {
            if let Some(revision) = self.imports.take_change(selection) {
                let entry = server.x11_selections.entries.get(selection);
                if entry.revision == revision || entry.origin.is_some() {
                    self.cancel(selection, wire, now)?;
                    *self.exports.get_mut(selection) = Export { revision: Some(entry.revision), ..Default::default() };
                }
            }
            let seat = server.x11_selections.entries.get(selection);
            if self.exports.get(selection).revision != Some(seat.revision) && self.pending.len() + seat.mime_types.len() <= 128 {
                self.cancel(selection, wire, now)?;
                if seat.origin.is_some() {
                    *self.exports.get_mut(selection) = Export { revision: Some(seat.revision), ..Default::default() };
                    continue;
                }
                if seat.mime_types.is_empty() && self.exports.get(selection).owned {
                    wire.request(
                        SetSelectionOwnerRequest { owner: 0, selection: selection_atom(atoms, selection), time: self.exports.get(selection).timestamp },
                        0,
                        false,
                    )?;
                }
                let export = self.exports.get_mut(selection);
                *export = Export { revision: Some(seat.revision), claim: true, ..Export::default() };
                if let Some(mime) = seat
                    .mime_types
                    .iter()
                    .find(|mime| mime.as_str() == "text/plain;charset=utf-8")
                    .or_else(|| seat.mime_types.iter().find(|mime| mime.as_str() == "text/plain"))
                {
                    for name in ["UTF8_STRING", "TEXT", "STRING"] {
                        export.formats.push(Format {
                            atom: if name == "STRING" { AtomEnum::STRING.into() } else { atoms.get(name) },
                            mime: mime.clone(),
                            latin1: name == "STRING",
                        });
                    }
                }
                for mime in &seat.mime_types {
                    let sequence = wire.request(InternAtomRequest { only_if_exists: false, name: mime.as_bytes().into() }, 0, true)?;
                    self.pending.push(PendingAtom { sequence, selection, revision: seat.revision, mime: mime.clone(), deadline: now + Duration::from_secs(3) });
                    export.pending += 1;
                }
            }
            let export = self.exports.get_mut(selection);
            if export.claim && export.pending == 0 && !export.formats.is_empty() && export.timestamp != 0 {
                wire.request(SetSelectionOwnerRequest { owner, selection: selection_atom(atoms, selection), time: export.timestamp }, 0, false)?;
                export.owned = true;
                export.claim = false;
            }
        }
        let revisions = Pair { clipboard: self.awaiting_clock(0), primary: self.awaiting_clock(1) };
        if self.clock_pending.is_none() && (revisions.clipboard.is_some() || revisions.primary.is_some()) {
            wire.request(
                ChangePropertyRequest {
                    mode: PropMode::REPLACE,
                    window: owner,
                    property: atoms.get("TIMESTAMP"),
                    type_: AtomEnum::INTEGER.into(),
                    format: 32,
                    data_len: 1,
                    data: 0u32.to_ne_bytes().as_slice().into(),
                },
                0,
                false,
            )?;
            self.clock_pending = Some(Clock { deadline: now + Duration::from_secs(3), revisions });
        }
        self.pump(wire, atoms, now)
    }
    fn awaiting_clock(&self, selection: usize) -> Option<u64> {
        let export = self.exports.get(selection);
        if export.claim && export.pending == 0 && !export.formats.is_empty() && export.timestamp == 0 { export.revision } else { None }
    }
    pub fn event(&mut self, bytes: &[u8], wire: &mut Transport, atoms: &Atoms, owner: u32, server: &mut Server, now: Instant) -> io::Result<()> {
        self.imports.event(bytes, wire, atoms, server, now)?;
        match bytes.first().map(|byte| byte & 0x7f) {
            Some(SELECTION_REQUEST_EVENT) => {
                let request = SelectionRequestEvent::try_parse(bytes).map_err(io::Error::other)?.0;
                if request.owner == owner {
                    self.request(request, wire, atoms, server, now)?;
                }
            }
            Some(SELECTION_CLEAR_EVENT) => {
                let event = SelectionClearEvent::try_parse(bytes).map_err(io::Error::other)?.0;
                if event.owner == owner
                    && let Some(selection) = (0..2).find(|index| {
                        selection_atom(atoms, *index) == event.selection && (event.time.wrapping_sub(self.exports.get(*index).timestamp) as i32) >= 0
                    })
                {
                    self.exports.get_mut(selection).owned = false;
                    self.exports.get_mut(selection).claim = false;
                    self.exports.get_mut(selection).formats.clear();
                    self.cancel(selection, wire, now)?;
                }
            }
            Some(PROPERTY_NOTIFY_EVENT) => {
                let event = PropertyNotifyEvent::try_parse(bytes).map_err(io::Error::other)?.0;
                if event.window == owner
                    && event.atom == atoms.get("TIMESTAMP")
                    && event.state == Property::NEW_VALUE
                    && let Some(clock) = self.clock_pending.take()
                {
                    for selection in 0..2 {
                        let export = self.exports.get_mut(selection);
                        if export.claim && export.pending == 0 && export.timestamp == 0 && export.revision == *clock.revisions.get(selection) {
                            export.timestamp = event.time;
                        }
                    }
                }
                if event.state == Property::DELETE {
                    for transfer in &mut self.transfers {
                        if transfer.request.requestor == event.window && transfer.property == event.atom && transfer.incremental {
                            transfer.permission = true;
                            transfer.deadline = now + TIMEOUT;
                        }
                    }
                }
            }
            Some(DESTROY_NOTIFY_EVENT) => {
                let event = DestroyNotifyEvent::try_parse(bytes).map_err(io::Error::other)?.0;
                self.transfers.retain(|transfer| transfer.request.requestor != event.window);
            }
            _ => {}
        }
        Ok(())
    }
    pub fn sync_sources(&self, handle: &LoopHandle<'static, Server>, sources: &mut Vec<IoSource>) -> io::Result<()> {
        sources.retain(|source| {
            if source.incoming || self.transfers.iter().any(|transfer| transfer.id == source.id && transfer.reading()) {
                true
            } else {
                handle.remove(source.token);
                false
            }
        });
        for transfer in self.transfers.iter().filter(|transfer| transfer.reading()) {
            if !sources.iter().any(|source| !source.incoming && source.id == transfer.id) {
                sources.push(IoSource {
                    id: transfer.id,
                    incoming: false,
                    token: super::watch(handle, &transfer.pipe, smithay::reexports::calloop::Interest::READ)?,
                });
            }
        }
        self.imports.sync_sources(handle, sources)?;
        Ok(())
    }
    pub fn remove_sources(handle: &LoopHandle<'static, Server>, sources: &mut Vec<IoSource>) {
        for source in sources.drain(..) {
            handle.remove(source.token);
        }
    }
}
fn selection_atom(atoms: &Atoms, selection: usize) -> u32 {
    if selection == 0 { atoms.get("CLIPBOARD") } else { AtomEnum::PRIMARY.into() }
}
