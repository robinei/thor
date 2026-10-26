//! The private XWM pipelines requests and translates replies without waiting for an X11 peer.
use super::{
    properties::{Atoms, NAMES, PROPERTIES, Properties},
    transport::Transport,
};
use crate::{
    Process,
    server::{ClientWindow, Server},
    surface::Surface,
};
use clamshell_state::{WindowEvent, session::presentation::WindowLayout};
use smithay::{
    backend::renderer::utils::with_renderer_surface_state,
    reexports::wayland_server::protocol::wl_surface::WlSurface,
    utils::{Logical, Point, Rectangle},
};
use std::{
    borrow::Cow,
    io,
    time::{Duration, Instant},
};
use x11rb_protocol::{
    id_allocator::IdAllocator,
    protocol::{composite, randr, res, xproto::*},
    x11_utils::{Request, Serialize, TryParse},
};

struct Pending {
    sequence: u64,
    kind: Query,
    deadline: Instant,
}
enum Query {
    Atom(&'static str),
    Extension,
    CompositeExtension,
    CompositeVersion,
    Version,
    Property(i64, &'static str),
    Pid(i64),
    Attributes(i64),
    OwnershipBarrier,
    RandrExtension,
    RandrVersion,
    RandrModes,
}
/// The server time for a focus change. The X server ignores a focus request older than the last focus
/// change, which an active client may have made itself, so each change waits for a fresh time: an empty
/// append to a property of our own window, whose PropertyNotify carries the current time (ICCCM 2.1).
#[derive(Default)]
enum Stamp {
    #[default]
    None,
    Requested(Instant),
    Fresh(u32),
}
struct Window {
    xid: u32,
    id: i64,
    override_redirect: bool,
    position: Point<i32, Logical>,
    popup_parent: Option<i64>,
    properties: Properties,
    pid: Option<u32>,
    outstanding: usize,
    wants_map: bool,
    queried: bool,
    input_only: bool,
    layout: Option<WindowLayout>,
    opened: bool,
    /// The serial Xwayland gave its surface (see `super::shell`), and the surface once it is committed.
    serial: Option<u64>,
    surface: Option<WlSurface>,
    size: (u16, u16),
    configured: Option<Rectangle<i32, smithay::utils::Logical>>,
    sent: Option<clamshell_state::session::windows::WindowFacts>,
    dirty: bool,
    refresh: u16,
    revision: Option<u64>,
    policy: Option<clamshell_state::session::presentation::WindowPresentation>,
}
#[derive(Default)]
pub(super) struct Wm {
    atoms: Atoms,
    pending: Vec<Pending>,
    windows: Vec<Window>,
    started: bool,
    owner: u32,
    root: u32,
    res: Option<u8>,
    extension_done: bool,
    composite: Option<u8>,
    composite_done: bool,
    /// RandR's opcode, for the modes.
    randr: Option<u8>,
    pub ready: bool,
    focus: Option<u32>,
    stamp: Stamp,
    selection: super::selection::Bridge,
}
impl Wm {
    pub fn deadline(&self) -> Option<Instant> {
        let stamp = match self.stamp {
            Stamp::Requested(deadline) => Some(deadline),
            _ => None,
        };
        self.pending.iter().map(|p| p.deadline).chain(self.selection.deadline()).chain(stamp).min()
    }
    fn query(&mut self, wire: &mut Transport, request: impl Request, opcode: u8, kind: Query, now: Instant) -> io::Result<()> {
        if self.pending.len() >= 256 {
            return Err(io::Error::other("too many pending XWM replies"));
        }
        let sequence = wire.request(request, opcode, true)?;
        self.pending.push(Pending { sequence, kind, deadline: now + Duration::from_secs(3) });
        Ok(())
    }
    pub fn turn(&mut self, wire: &mut Transport, server: &mut Server, now: Instant) -> io::Result<()> {
        self.start_setup(wire, now)?;
        self.dispatch_replies(wire, server, now)?;
        self.finish_setup(wire)?;
        self.anchor_sequence(wire, now)?;
        self.dispatch_events(wire, server)?;
        self.query_window_properties(wire, now)?;
        self.sync_selection(wire, server, now)?;
        self.settle(wire, server)?;
        self.settle_menus(wire, server)?;
        server.sync_windows();
        Ok(())
    }
    fn start_setup(&mut self, wire: &mut Transport, now: Instant) -> io::Result<()> {
        if !self.started && wire.setup.is_some() {
            self.started = true;
            for name in NAMES {
                self.query(wire, InternAtomRequest { only_if_exists: false, name: name.as_bytes().into() }, 0, Query::Atom(name), now)?;
            }
            self.query(wire, QueryExtensionRequest { name: b"X-Resource".as_slice().into() }, 0, Query::Extension, now)?;
            self.query(wire, QueryExtensionRequest { name: b"RANDR".as_slice().into() }, 0, Query::RandrExtension, now)?;
        }
        if self.started && self.composite.is_none() && !self.pending.iter().any(|p| matches!(p.kind, Query::CompositeExtension)) {
            self.query(wire, QueryExtensionRequest { name: b"Composite".as_slice().into() }, 0, Query::CompositeExtension, now)?;
        }
        Ok(())
    }
    fn dispatch_replies(&mut self, wire: &mut Transport, server: &mut Server, now: Instant) -> io::Result<()> {
        let mut index = 0;
        while index < self.pending.len() {
            let Some(pending) = self.pending.get(index) else { break };
            if let Some(bytes) = wire.reply(pending.sequence) {
                let pending = self.pending.remove(index);
                self.reply(pending.kind, &bytes, wire, server, now)?;
            } else {
                if now >= pending.deadline {
                    return Err(io::Error::new(io::ErrorKind::TimedOut, "XWM reply timed out"));
                }
                index += 1;
            }
        }
        if let Stamp::Requested(deadline) = self.stamp
            && now >= deadline
        {
            return Err(io::Error::new(io::ErrorKind::TimedOut, "X11 server time for focus timed out"));
        }
        Ok(())
    }
    fn finish_setup(&mut self, wire: &mut Transport) -> io::Result<()> {
        if self.started && !self.ready && self.atoms.0.len() == NAMES.len() && self.extension_done && self.composite_done {
            self.initialise(wire)?;
        }
        Ok(())
    }
    fn anchor_sequence(&mut self, wire: &mut Transport, now: Instant) -> io::Result<()> {
        // X11's 16-bit wire sequence needs a reply before wrapping, only after actual traffic.
        if self.ready && self.pending.len() < 256 && wire.needs_sequence_anchor() && !self.pending.iter().any(|p| matches!(p.kind, Query::OwnershipBarrier)) {
            self.query(wire, GetSelectionOwnerRequest { selection: self.atoms.get("WM_S0") }, 0, Query::OwnershipBarrier, now)?;
        }
        Ok(())
    }
    fn dispatch_events(&mut self, wire: &mut Transport, server: &mut Server) -> io::Result<()> {
        for _ in 0..64 {
            let Some(bytes) = wire.event() else { break };
            self.event(&bytes, wire, server)?;
        }
        Ok(())
    }
    fn sync_selection(&mut self, wire: &mut Transport, server: &mut Server, now: Instant) -> io::Result<()> {
        if self.ready {
            self.selection.init_imports(wire, self.owner, now)?;
            self.selection.turn(wire, &self.atoms, self.owner, server, now)?;
        }
        Ok(())
    }
    fn query_window_properties(&mut self, wire: &mut Transport, now: Instant) -> io::Result<()> {
        for i in 0..self.windows.len() {
            self.query_initial_properties(i, wire, now)?;
            self.query_changed_properties(i, wire, now)?;
        }
        Ok(())
    }
    fn query_initial_properties(&mut self, i: usize, wire: &mut Transport, now: Instant) -> io::Result<()> {
        let Some(window) = self.windows.get(i) else { return Ok(()) };
        if window.wants_map && !window.queried && self.pending.len() + PROPERTIES.len() + 2 <= 256 {
            let (id, xid) = (window.id, window.xid);
            self.query(wire, GetWindowAttributesRequest { window: xid }, 0, Query::Attributes(id), now)?;
            for name in PROPERTIES {
                self.query(
                    wire,
                    GetPropertyRequest { delete: false, window: xid, property: self.atoms.get(name), type_: 0, long_offset: 0, long_length: 1024 },
                    0,
                    Query::Property(id, name),
                    now,
                )?;
            }
            self.query(
                wire,
                res::QueryClientIdsRequest { specs: vec![res::ClientIdSpec { client: xid, mask: res::ClientIdMask::LOCAL_CLIENT_PID }].into() },
                self.res.unwrap_or_default(),
                Query::Pid(id),
                now,
            )?;
            if let Some(window) = self.windows.get_mut(i) {
                window.queried = true;
                window.refresh = 0;
            }
        }
        Ok(())
    }
    fn query_changed_properties(&mut self, i: usize, wire: &mut Transport, now: Instant) -> io::Result<()> {
        for (bit, name) in PROPERTIES.iter().enumerate() {
            let Some(window) = self.windows.get_mut(i) else { continue };
            if !window.queried
                || self.pending.len() >= 256
                || window.refresh & (1 << bit) == 0
                || self.pending.iter().any(|p| matches!(p.kind, Query::Property(id, n) if id == window.id && n == *name))
            {
                continue;
            }
            window.refresh &= !(1 << bit);
            let (id, xid) = (window.id, window.xid);
            self.query(
                wire,
                GetPropertyRequest { delete: false, window: xid, property: self.atoms.get(name), type_: 0, long_offset: 0, long_length: 1024 },
                0,
                Query::Property(id, name),
                now,
            )?;
        }
        Ok(())
    }
    fn initialise(&mut self, wire: &mut Transport) -> io::Result<()> {
        let setup = wire.setup.as_ref().ok_or_else(|| io::Error::other("missing X11 setup"))?;
        let root = setup.roots.first().ok_or_else(|| io::Error::other("X11 server has no screen"))?;
        self.root = root.root;
        self.owner = IdAllocator::new(setup.resource_id_base, setup.resource_id_mask)
            .map_err(io::Error::other)?
            .generate_id()
            .ok_or_else(|| io::Error::other("no X11 resource ids"))?;
        wire.request(
            CreateWindowRequest {
                depth: 0,
                wid: self.owner,
                parent: self.root,
                x: 0,
                y: 0,
                width: 1,
                height: 1,
                border_width: 0,
                class: WindowClass::INPUT_OUTPUT,
                visual: 0,
                value_list: Cow::Owned(CreateWindowAux::new().event_mask(EventMask::PROPERTY_CHANGE)),
            },
            0,
            false,
        )?;
        wire.request(
            ChangeWindowAttributesRequest {
                window: self.root,
                value_list: Cow::Owned(ChangeWindowAttributesAux::new().event_mask(EventMask::SUBSTRUCTURE_REDIRECT | EventMask::SUBSTRUCTURE_NOTIFY)),
            },
            0,
            false,
        )?;
        wire.request(
            composite::RedirectSubwindowsRequest { window: self.root, update: composite::Redirect::MANUAL },
            self.composite.unwrap_or_default(),
            false,
        )?;
        for name in ["WM_S0", "_NET_WM_CM_S0"] {
            wire.request(SetSelectionOwnerRequest { owner: self.owner, selection: self.atoms.get(name), time: 0 }, 0, false)?;
        }
        Self::property(&self.atoms, wire, self.root, "_NET_SUPPORTING_WM_CHECK", AtomEnum::WINDOW.into(), &[self.owner])?;
        Self::property(&self.atoms, wire, self.owner, "_NET_SUPPORTING_WM_CHECK", AtomEnum::WINDOW.into(), &[self.owner])?;
        wire.request(
            ChangePropertyRequest {
                mode: PropMode::REPLACE,
                window: self.owner,
                property: self.atoms.get("_NET_WM_NAME"),
                type_: self.atoms.get("UTF8_STRING"),
                format: 8,
                data_len: 9,
                data: b"ClamShell".as_slice().into(),
            },
            0,
            false,
        )?;
        let supported: Vec<u32> =
            ["_NET_WM_STATE", "_NET_WM_STATE_FULLSCREEN", "_NET_ACTIVE_WINDOW", "_NET_WM_WINDOW_TYPE"].iter().map(|name| self.atoms.get(name)).collect();
        Self::property(&self.atoms, wire, self.root, "_NET_SUPPORTED", AtomEnum::ATOM.into(), &supported)?;
        self.ready = true;
        log::info!("compositor: asynchronous X11 window manager ready");
        Ok(())
    }
    fn property(atoms: &Atoms, wire: &mut Transport, window: u32, name: &str, type_: u32, values: &[u32]) -> io::Result<()> {
        let bytes: Vec<u8> = values.iter().flat_map(|v| v.to_ne_bytes()).collect();
        wire.request(
            ChangePropertyRequest {
                mode: PropMode::REPLACE,
                window,
                property: atoms.get(name),
                type_,
                format: 32,
                data_len: values.len() as u32,
                data: bytes.into(),
            },
            0,
            false,
        )?;
        Ok(())
    }
    fn reply(&mut self, query: Query, bytes: &[u8], wire: &mut Transport, server: &mut Server, now: Instant) -> io::Result<()> {
        if bytes.first() == Some(&0) {
            match query {
                Query::Property(id, _) | Query::Pid(id) | Query::Attributes(id) => {
                    self.forget(id, server);
                    return Ok(());
                }
                Query::RandrExtension | Query::RandrVersion | Query::RandrModes => {
                    log::warn!("compositor: Xwayland's modes can't be read: a game's window at one is shown at its own size");
                    return Ok(());
                }
                _ => return Err(io::Error::other("XWM initialisation request rejected")),
            }
        }
        match query {
            Query::Atom(name) => self.atoms.0.push((name, InternAtomReply::try_parse(bytes).map_err(io::Error::other)?.0.atom)),
            Query::Extension => {
                let reply = QueryExtensionReply::try_parse(bytes).map_err(io::Error::other)?.0;
                if !reply.present {
                    return Err(io::Error::other("Xwayland lacks X-Resource"));
                }
                self.res = Some(reply.major_opcode);
                self.query(wire, res::QueryVersionRequest { client_major: 1, client_minor: 2 }, reply.major_opcode, Query::Version, now)?;
            }
            Query::CompositeExtension => {
                let reply = QueryExtensionReply::try_parse(bytes).map_err(io::Error::other)?.0;
                if !reply.present {
                    return Err(io::Error::other("Xwayland lacks Composite"));
                }
                self.composite = Some(reply.major_opcode);
                self.query(
                    wire,
                    composite::QueryVersionRequest { client_major_version: 0, client_minor_version: 4 },
                    reply.major_opcode,
                    Query::CompositeVersion,
                    now,
                )?;
            }
            Query::RandrExtension => {
                let reply = QueryExtensionReply::try_parse(bytes).map_err(io::Error::other)?.0;
                if !reply.present {
                    log::warn!("compositor: Xwayland lacks RandR: a game's window at one of its modes is shown at its own size");
                    return Ok(());
                }
                self.randr = Some(reply.major_opcode);
                self.query(wire, randr::QueryVersionRequest { major_version: 1, minor_version: 3 }, reply.major_opcode, Query::RandrVersion, now)?;
            }
            Query::RandrVersion => {
                randr::QueryVersionReply::try_parse(bytes).map_err(io::Error::other)?;
                let root = wire.setup.as_ref().and_then(|setup| setup.roots.first()).map(|root| root.root);
                if let (Some(opcode), Some(window)) = (self.randr, root) {
                    self.query(wire, randr::GetScreenResourcesCurrentRequest { window }, opcode, Query::RandrModes, now)?;
                }
            }
            Query::RandrModes => {
                let reply = randr::GetScreenResourcesCurrentReply::try_parse(bytes).map_err(io::Error::other)?.0;
                // Read once, at the start: they are the common sizes no larger than the screen.
                let mut modes: Vec<(u32, u32)> = reply.modes.iter().map(|mode| (u32::from(mode.width), u32::from(mode.height))).collect();
                modes.sort_unstable();
                modes.dedup();
                log::info!("compositor: Xwayland's modes: {modes:?}");
                server.core.handle(clamshell_state::Event::Reading(clamshell_state::Reading::XwaylandModes(modes)));
            }
            Query::CompositeVersion => {
                composite::QueryVersionReply::try_parse(bytes).map_err(io::Error::other)?;
                self.composite_done = true;
            }
            Query::Version => {
                let version = res::QueryVersionReply::try_parse(bytes).map_err(io::Error::other)?.0;
                if (version.server_major, version.server_minor) < (1, 2) {
                    return Err(io::Error::other("X-Resource 1.2 required"));
                }
                self.extension_done = true;
            }
            Query::Property(id, name) => {
                let reply = GetPropertyReply::try_parse(bytes).map_err(io::Error::other)?.0;
                if let Some(window) = self.windows.iter_mut().find(|w| w.id == id) {
                    window.outstanding = window.outstanding.saturating_sub(1);
                    window.properties.update(name, &reply, &self.atoms);
                    window.dirty = true;
                    if matches!(name, "WM_HINTS" | "WM_PROTOCOLS") && self.focus == Some(window.xid) {
                        self.focus = None;
                    }
                }
            }
            Query::OwnershipBarrier => {
                let reply = GetSelectionOwnerReply::try_parse(bytes).map_err(io::Error::other)?.0;
                if reply.owner != self.owner {
                    return Err(io::Error::other("X11 WM selection lost"));
                }
            }
            Query::Attributes(id) => {
                let reply = GetWindowAttributesReply::try_parse(bytes).map_err(io::Error::other)?.0;
                if let Some(window) = self.windows.iter_mut().find(|w| w.id == id) {
                    window.input_only = reply.class == WindowClass::INPUT_ONLY;
                    window.outstanding = window.outstanding.saturating_sub(1);
                }
            }
            Query::Pid(id) => {
                let reply = res::QueryClientIdsReply::try_parse(bytes).map_err(io::Error::other)?.0;
                if let Some(window) = self.windows.iter_mut().find(|w| w.id == id) {
                    window.outstanding = window.outstanding.saturating_sub(1);
                    window.pid = reply
                        .ids
                        .iter()
                        .find(|v| v.spec.mask.contains(res::ClientIdMask::LOCAL_CLIENT_PID))
                        .and_then(|v| v.value.first())
                        .copied()
                        .filter(|pid| *pid > 1);
                }
            }
        }
        Ok(())
    }
    fn discover(&mut self, xid: u32, geometry: Rectangle<i32, Logical>, override_redirect: bool, wire: &mut Transport, server: &mut Server) -> io::Result<()> {
        if wire.setup.as_ref().is_some_and(|setup| (xid & !setup.resource_id_mask) == setup.resource_id_base) {
            return Ok(());
        }
        if self.windows.iter().any(|w| w.xid == xid) || xid == self.owner {
            return Ok(());
        }
        if self.windows.len() >= 128 {
            log::warn!("compositor: X11 window limit exceeded by resource {xid}; ending its client");
            wire.request(KillClientRequest { resource: xid }, 0, false)?;
            return Ok(());
        }
        let id = server.next_id;
        server.next_id = id.checked_add(1).ok_or_else(|| io::Error::other("window ids exhausted"))?;
        self.windows.push(Window {
            xid,
            id,
            override_redirect,
            position: geometry.loc,
            popup_parent: None,
            properties: Properties::default(),
            pid: None,
            outstanding: PROPERTIES.len() + 2,
            wants_map: false,
            queried: false,
            input_only: false,
            layout: None,
            opened: false,
            serial: None,
            surface: None,
            size: (u16::try_from(geometry.size.w).unwrap_or(1), u16::try_from(geometry.size.h).unwrap_or(1)),
            configured: None,
            sent: None,
            dirty: true,
            refresh: 0,
            revision: None,
            policy: None,
        });
        wire.request(
            ChangeWindowAttributesRequest { window: xid, value_list: Cow::Owned(ChangeWindowAttributesAux::new().event_mask(EventMask::PROPERTY_CHANGE)) },
            0,
            false,
        )?;

        Ok(())
    }
    fn event(&mut self, bytes: &[u8], wire: &mut Transport, server: &mut Server) -> io::Result<()> {
        self.selection.event(bytes, wire, &self.atoms, self.owner, server, Instant::now())?;
        match bytes.first().map(|v| v & 0x7f) {
            Some(CREATE_NOTIFY_EVENT) => {
                let event = CreateNotifyEvent::try_parse(bytes).map_err(io::Error::other)?.0;
                if event.parent == self.root {
                    self.discover(
                        event.window,
                        Rectangle::new((i32::from(event.x), i32::from(event.y)).into(), (i32::from(event.width), i32::from(event.height)).into()),
                        event.override_redirect,
                        wire,
                        server,
                    )?;
                }
            }
            Some(MAP_REQUEST_EVENT) => {
                let event = MapRequestEvent::try_parse(bytes).map_err(io::Error::other)?.0;
                if let Some(window) = self.windows.iter_mut().find(|w| w.xid == event.window) {
                    window.wants_map = true;
                }
            }
            Some(MAP_NOTIFY_EVENT) => {
                let event = MapNotifyEvent::try_parse(bytes).map_err(io::Error::other)?.0;
                if let Some(window) = self.windows.iter_mut().find(|w| w.xid == event.window) {
                    if !window.wants_map {
                        window.popup_parent = None;
                    }
                    window.wants_map = true;
                    window.override_redirect = event.override_redirect;
                }
            }
            Some(CONFIGURE_NOTIFY_EVENT) => {
                let event = ConfigureNotifyEvent::try_parse(bytes).map_err(io::Error::other)?.0;
                if let Some(window) = self.windows.iter_mut().find(|w| w.xid == event.window && w.override_redirect) {
                    window.position = (i32::from(event.x), i32::from(event.y)).into();
                    window.size = (event.width, event.height);
                }
            }
            Some(UNMAP_NOTIFY_EVENT | DESTROY_NOTIFY_EVENT) => {
                let xid = if bytes.first().map(|v| v & 0x7f) == Some(UNMAP_NOTIFY_EVENT) {
                    UnmapNotifyEvent::try_parse(bytes).map_err(io::Error::other)?.0.window
                } else {
                    DestroyNotifyEvent::try_parse(bytes).map_err(io::Error::other)?.0.window
                };
                if let Some(id) = self.windows.iter().find(|w| w.xid == xid).map(|w| w.id) {
                    if bytes.first().map(|v| v & 0x7f) == Some(DESTROY_NOTIFY_EVENT) {
                        self.forget(id, server);
                    } else if let Some(w) = self.windows.iter_mut().find(|w| w.id == id) {
                        let opened = w.opened;
                        w.wants_map = false;
                        w.opened = false;
                        w.sent = None;
                        w.serial = None;
                        w.surface = None;
                        w.configured = None;
                        for client in &mut server.windows {
                            let parent = client.id == id;
                            client.window.dismiss_x11_popups(&mut server.x11_popup_dismiss, |popup| parent || popup.id == id);
                        }
                        server.windows.retain(|v| v.id != id);
                        server.x11_changes.retain(|v| v.id != id);
                        if opened {
                            server.core.handle(WindowEvent::Closed(id).into());
                        }
                    }
                }
            }
            Some(PROPERTY_NOTIFY_EVENT) => {
                let event = PropertyNotifyEvent::try_parse(bytes).map_err(io::Error::other)?.0;
                if event.window == self.owner && event.atom == self.atoms.get("_THOR_TIMESTAMP") {
                    self.stamp = Stamp::Fresh(event.time);
                }
                if let Some(window) = self.windows.iter_mut().find(|w| w.xid == event.window)
                    && let Some(index) = PROPERTIES.iter().position(|name| self.atoms.get(name) == event.atom)
                {
                    window.refresh |= 1 << index;
                }
            }
            Some(CLIENT_MESSAGE_EVENT) => {
                let event = ClientMessageEvent::try_parse(bytes).map_err(io::Error::other)?.0;
                log::debug!("X11 client message window={} type={} data={:?}", event.window, event.type_, event.data.as_data32());
                if event.format == 32
                    && event.type_ == self.atoms.get("WL_SURFACE_SERIAL")
                    && let Some(window) = self.windows.iter_mut().find(|w| w.xid == event.window)
                    && let [low, high, ..] = event.data.as_data32()
                {
                    window.serial = Some(u64::from(low) | (u64::from(high) << 32));
                    log::debug!("X11 surface serial id={}", window.id);
                }
            }
            Some(CONFIGURE_REQUEST_EVENT) => {
                let event = ConfigureRequestEvent::try_parse(bytes).map_err(io::Error::other)?.0;
                if let Some(window) = self.windows.iter_mut().find(|w| w.xid == event.window) {
                    if event.value_mask.contains(ConfigWindow::WIDTH) {
                        window.size.0 = event.width.max(1);
                    }
                    if event.value_mask.contains(ConfigWindow::HEIGHT) {
                        window.size.1 = event.height.max(1);
                    }
                    window.configured = None;
                }
            }
            Some(0) if bytes.get(1) == Some(&10) => return Err(io::Error::other("X11 window manager lost root ownership")),
            Some(0) => {
                log::debug!("compositor: X11 request rejected for a vanished or unmapped window");
            }
            _ => {}
        }
        Ok(())
    }
    fn forget(&mut self, id: i64, server: &mut Server) {
        self.windows.retain(|w| w.id != id);
        for client in &mut server.windows {
            let parent = client.id == id;
            client.window.dismiss_x11_popups(&mut server.x11_popup_dismiss, |popup| parent || popup.id == id);
        }
        server.x11_changes.retain(|facts| facts.id != id);
        server.windows.retain(|w| w.id != id);
        if server.core.state().session().window(id).is_some() {
            server.core.handle(WindowEvent::Closed(id).into());
        }
    }
    pub fn sync_selection_sources(
        &self,
        handle: &smithay::reexports::calloop::LoopHandle<'static, Server>,
        sources: &mut Vec<super::selection::IoSource>,
    ) -> io::Result<()> {
        self.selection.sync_sources(handle, sources)
    }
    pub fn clear(&mut self, server: &mut Server) {
        self.selection.clear(server);
        while let Some(window) = self.windows.last() {
            let id = window.id;
            self.forget(id, server);
        }
    }
    fn settle(&mut self, wire: &mut Transport, server: &mut Server) -> io::Result<()> {
        self.pair_surfaces(server);
        self.settle_windows(wire, server)?;
        self.settle_focus(wire, server)?;
        self.settle_close(wire, server)?;
        Ok(())
    }
    /// Each window given a serial, paired with the surface Xwayland committed with it.
    fn pair_surfaces(&mut self, server: &mut Server) {
        for window in &mut self.windows {
            if window.surface.is_none()
                && let Some(serial) = window.serial
                && let Some(surface) = super::shell::take(server, serial)
            {
                log::debug!("X11 association id={} map={} pending={}", window.id, window.wants_map, window.outstanding);
                window.surface = Some(surface);
            }
        }
    }
    fn settle_windows(&mut self, wire: &mut Transport, server: &mut Server) -> io::Result<()> {
        for i in 0..self.windows.len() {
            let Some(w) = self.windows.get(i) else { continue };
            if w.override_redirect || !w.wants_map || w.outstanding != 0 {
                continue;
            }
            if w.input_only {
                if w.configured.is_none() {
                    wire.request(MapWindowRequest { window: w.xid }, 0, false)?;
                    if let Some(w) = self.windows.get_mut(i) {
                        w.configured = Some(Rectangle::from_size((1, 1).into()));
                    }
                }
                continue;
            }
            let parent = w.properties.transient.and_then(|xid| self.windows.iter().find(|p| p.xid == xid && p.opened).map(|p| p.id));
            let Some(w) = self.windows.get_mut(i) else { continue };
            w.sync_facts(parent, server);
            w.bind_surface(server);
            w.configure(&self.atoms, wire, server)?;
        }
        Ok(())
    }
    fn settle_focus(&mut self, wire: &mut Transport, server: &Server) -> io::Result<()> {
        let focus = server.core.state().session().keyboard_focus().and_then(|id| self.windows.iter().find(|w| w.id == id && w.opened).map(|w| w.xid));
        if self.focus == focus {
            // A time that arrives after the change it was for was undone would be stale by the next one.
            if matches!(self.stamp, Stamp::Fresh(_)) {
                self.stamp = Stamp::None;
            }
            return Ok(());
        }
        let time = match self.stamp {
            Stamp::Fresh(time) => time,
            Stamp::Requested(_) => return Ok(()),
            Stamp::None => {
                wire.request(
                    ChangePropertyRequest {
                        mode: PropMode::APPEND,
                        window: self.owner,
                        property: self.atoms.get("_THOR_TIMESTAMP"),
                        type_: AtomEnum::INTEGER.into(),
                        format: 32,
                        data_len: 0,
                        data: Cow::Borrowed(&[]),
                    },
                    0,
                    false,
                )?;
                self.stamp = Stamp::Requested(Instant::now() + Duration::from_secs(3));
                return Ok(());
            }
        };
        self.stamp = Stamp::None;
        {
            let target = focus.and_then(|xid| self.windows.iter().find(|w| w.xid == xid));
            let direct = target.filter(|w| w.properties.input.unwrap_or(true)).map_or(0, |w| w.xid);
            wire.request(SetInputFocusRequest { revert_to: InputFocus::POINTER_ROOT, focus: direct, time }, 0, false)?;
            if let Some(window) = target.filter(|w| w.properties.take_focus) {
                let event = ClientMessageEvent::new(32, window.xid, self.atoms.get("WM_PROTOCOLS"), [self.atoms.get("WM_TAKE_FOCUS"), time, 0, 0, 0]);
                wire.request(
                    SendEventRequest {
                        propagate: false,
                        destination: window.xid,
                        event_mask: EventMask::NO_EVENT,
                        event: Cow::Owned(Serialize::serialize(&event)),
                    },
                    0,
                    false,
                )?;
            }
            Self::property(&self.atoms, wire, self.root, "_NET_ACTIVE_WINDOW", AtomEnum::WINDOW.into(), &[focus.unwrap_or(0)])?;
            self.focus = focus;
        }
        Ok(())
    }
    fn settle_close(&mut self, wire: &mut Transport, server: &mut Server) -> io::Result<()> {
        for id in server.x11_close.drain(..) {
            if let Some(w) = self.windows.iter().find(|w| w.id == id) {
                if w.properties.delete {
                    let event = ClientMessageEvent::new(32, w.xid, self.atoms.get("WM_PROTOCOLS"), [self.atoms.get("WM_DELETE_WINDOW"), 0, 0, 0, 0]);
                    wire.request(
                        SendEventRequest {
                            propagate: false,
                            destination: w.xid,
                            event_mask: EventMask::NO_EVENT,
                            event: Cow::Owned(Serialize::serialize(&event)),
                        },
                        0,
                        false,
                    )?;
                } else {
                    wire.request(KillClientRequest { resource: w.xid }, 0, false)?;
                }
            }
        }
        Ok(())
    }
}

impl Window {
    fn sync_facts(&mut self, parent: Option<i64>, server: &mut Server) {
        if !self.opened || self.dirty || self.sent.as_ref().is_some_and(|old| old.parent != parent) {
            let process = server
                .core
                .state()
                .session()
                .window(self.id)
                .map(|v| Process { unit: v.unit.clone(), pids: v.pids.clone(), steam_app: None })
                .unwrap_or_else(|| self.pid.map(|pid| (server.identify)(pid)).unwrap_or_default());
            let facts = self.properties.facts(self.id, parent, process);
            if !self.opened {
                server.core.handle(WindowEvent::Opened(facts.clone()).into());
                let focused = server.core.state().session().top_window().is_some_and(|w| w.id == self.id);
                log::info!("compositor: X11 window {} opens: {:?} {:?}, focused {focused}, limits {:?}", self.id, facts.app_id, facts.title, facts.limits);
                self.opened = true;
                self.sent = Some(facts);
            } else if self.sent.as_ref().is_none_or(|old| {
                old.app_id != facts.app_id || old.title != facts.title || old.parent != facts.parent || old.limits != facts.limits || old.role != facts.role
            }) {
                if let Some(pending) = server.x11_changes.iter_mut().find(|old| old.id == facts.id) {
                    *pending = facts.clone();
                } else {
                    server.x11_changes.push(facts.clone());
                }
                self.sent = Some(facts);
            }
            self.dirty = false;
        }
    }
    fn bind_surface(&self, server: &mut Server) {
        if let Some(surface) = self.surface.clone()
            && server.windows.iter().all(|v| v.id != self.id)
        {
            let mapped = with_renderer_surface_state(&surface, |s| s.buffer().is_some()).unwrap_or(false);
            log::debug!("X11 surface bound {} mapped={mapped}", self.id);
            server.windows.push(ClientWindow {
                id: self.id,
                window: Surface::new_x11(surface),
                opened: true,
                mapped,
                title_dirty: false,
                shown: crate::server::Shown::of(server.core.state(), self.id),
                frames: Default::default(),
            });
        }
    }
    fn configure(&mut self, atoms: &Atoms, wire: &mut Transport, server: &Server) -> io::Result<()> {
        let Some(model) = server.core.state().session().window(self.id) else { return Ok(()) };
        if self.revision != Some(server.core.state_revision()) {
            self.policy = server.core.state().window_presentation(self.id);
            self.revision = Some(server.core.state_revision());
        }
        let Some(policy) = self.policy else { return Ok(()) };
        let area = server.area(model.down);
        // (Its clarity as read for its window this revision, else from the state while it has none yet.)
        let clarity = server.windows.iter().find(|w| w.id == self.id).map_or_else(|| server.core.state().window_clarity(self.id), |w| w.shown.clarity);
        let fill = crate::server::fill_size(clarity, area.size);
        let size = crate::geometry::configure_size(model, policy, area.size, fill, (i32::from(self.size.0), i32::from(self.size.1)).into())
            .unwrap_or((i32::from(self.size.0), i32::from(self.size.1)).into());
        // A fullscreen game that switched mode is configured to it, so that Xwayland emulates it: the window
        // at the mode's size is scaled to the screen, and comes to us as a smaller buffer scaled by a viewport.
        let size = match self.properties.emulated {
            Some((w, h)) if policy.layout == WindowLayout::Fullscreen => (i32::try_from(w).unwrap_or(size.w), i32::try_from(h).unwrap_or(size.h)).into(),
            _ => size,
        };
        let offset: smithay::utils::Point<i32, smithay::utils::Logical> =
            if policy.layout == WindowLayout::Centred { ((area.size.w - size.w).max(0) / 2, (area.size.h - size.h).max(0) / 2).into() } else { (0, 0).into() };
        let geometry = Rectangle::new(area.loc + offset, size);
        if self.configured != Some(geometry) || self.layout != Some(policy.layout) {
            log::debug!(
                "X11 configure {} to {geometry:?}: {:?}, limits {:?}, emulated {:?}",
                self.id,
                policy.layout,
                self.properties.limits,
                self.properties.emulated
            );
            wire.request(
                ConfigureWindowRequest {
                    window: self.xid,
                    value_list: Cow::Owned(
                        ConfigureWindowAux::new()
                            .x(geometry.loc.x)
                            .y(geometry.loc.y)
                            .width(geometry.size.w.max(1) as u32)
                            .height(geometry.size.h.max(1) as u32)
                            .border_width(0),
                    ),
                },
                0,
                false,
            )?;
            let fullscreen = [atoms.get("_NET_WM_STATE_FULLSCREEN")];
            Wm::property(
                atoms,
                wire,
                self.xid,
                "_NET_WM_STATE",
                AtomEnum::ATOM.into(),
                if policy.layout == WindowLayout::Fullscreen { &fullscreen } else { &[] },
            )?;
            Wm::property(atoms, wire, self.xid, "WM_STATE", atoms.get("WM_STATE"), &[1, 0])?;
            wire.request(MapWindowRequest { window: self.xid }, 0, false)?;
            self.configured = Some(geometry);
            self.layout = Some(policy.layout);
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::indexing_slicing)]
    use super::*;
    use std::{os::unix::net::UnixStream, sync::Arc};
    #[test]
    fn outstanding_deadlines_stale_replies_and_menu_ownership_follow_window_lifetimes() {
        let display = smithay::reexports::wayland_server::Display::<Server>::new().unwrap();
        let mut server = Server::new(
            clamshell_state::State::new(clamshell_state::Init::default()),
            clamshell_ui::Config { top_size: (960, 540), bottom_size: (620, 540), touch_layout: Arc::default() },
            display.handle(),
            false,
            || {},
            |_| Process::default(),
            |_| {},
        )
        .unwrap();
        let (socket, _peer) = UnixStream::pair().unwrap();
        let now = Instant::now();
        let mut wire = Transport::new(socket, now).unwrap();
        let mut wm = Wm::default();
        assert_eq!(wm.deadline(), None);
        wm.query(&mut wire, GetWindowAttributesRequest { window: 10 }, 0, Query::Attributes(7), now).unwrap();
        assert_eq!(wm.deadline(), Some(now + Duration::from_secs(3)));
        assert_eq!(wm.turn(&mut wire, &mut server, now + Duration::from_secs(3)).unwrap_err().kind(), io::ErrorKind::TimedOut);
        wm.pending.clear();
        assert_eq!(wm.deadline(), None);
        wm.discover(10, Rectangle::from_size((640, 360).into()), false, &mut wire, &mut server).unwrap();
        let old_id = wm.windows[0].id;
        wm.forget(old_id, &mut server);
        wm.discover(10, Rectangle::from_size((640, 360).into()), false, &mut wire, &mut server).unwrap();
        assert_ne!(wm.windows[0].id, old_id);
        wm.res = Some(140);
        wm.turn(&mut wire, &mut server, now).unwrap();
        assert!(wm.pending.is_empty(), "creation alone does not snapshot properties before the app sets them");
        let event = MapRequestEvent { response_type: MAP_REQUEST_EVENT, parent: 1, window: 10, ..MapRequestEvent::default() };
        let mut event = Serialize::serialize(&event).to_vec();
        event.resize(32, 0);
        wm.event(&event, &mut wire, &mut server).unwrap();
        wm.turn(&mut wire, &mut server, now).unwrap();
        assert_eq!(wm.pending.len(), PROPERTIES.len() + 2, "MapRequest starts the attributes, properties and PID snapshot");
        assert!(server.core.state().session().windows().is_empty(), "the initial facts precede placement");

        // An old window's reply cannot change another window, even if X11 reuses its resource id.
        let mut bytes = Serialize::serialize(&GetPropertyReply {
            format: 8,
            type_: AtomEnum::STRING.into(),
            value: b"stale".to_vec(),
            value_len: 5,
            length: 2,
            ..GetPropertyReply::default()
        });
        bytes.resize(40, 0);
        wm.reply(Query::Property(old_id, "WM_NAME"), &bytes, &mut wire, &mut server, now).unwrap();
        assert_eq!(wm.windows.len(), 1);
        assert!(wm.windows[0].properties.name.is_empty());
        assert_eq!(server.core.state().session().windows().len(), 0);

        // Override-redirect ownership follows live transient ancestors; cached ownership never
        // transfers a mapped menu to a different focused client.
        for (xid, redirect) in [(20, false), (30, true), (40, true)] {
            wm.discover(xid, Rectangle::from_size((100, 100).into()), redirect, &mut wire, &mut server).unwrap();
        }
        for window in &mut wm.windows {
            window.pid = Some(123);
            window.wants_map = true;
        }
        let parent = wm.windows[0].id;
        let other = wm.windows[1].id;
        assert_eq!(wm.popup_owner(&wm.windows[2], Some(parent), None), Some(parent));
        wm.windows[2].pid = Some(456);
        assert_eq!(wm.popup_owner(&wm.windows[2], Some(parent), None), None);
        wm.windows[2].pid = Some(123);
        wm.windows[2].properties.tooltip = true;
        assert_eq!(wm.popup_owner(&wm.windows[2], Some(parent), Some(other)), Some(other));
        wm.windows[2].properties.transient = Some(10);
        wm.windows[3].properties.transient = Some(30);
        wm.windows[3].popup_parent = Some(parent);
        assert_eq!(wm.popup_owner(&wm.windows[3], Some(other), None), Some(parent));
        wm.windows[2].wants_map = false;
        assert_eq!(wm.popup_owner(&wm.windows[3], Some(parent), None), None);
        wm.windows[2].wants_map = true;
        wm.windows[2].properties.transient = Some(40);
        assert_eq!(wm.popup_owner(&wm.windows[3], Some(parent), None), None, "transient cycles are bounded");
        wm.windows[2].properties.transient = Some(20);
        assert_eq!(wm.popup_owner(&wm.windows[3], Some(other), None), None, "a mapped menu cannot change owner");
    }
    #[test]
    fn placement_is_committed_and_configured_before_mapping_and_focus() {
        use std::io::Read;
        let display = smithay::reexports::wayland_server::Display::<Server>::new().unwrap();
        let mut server = Server::new(
            clamshell_state::State::new(clamshell_state::Init::default()),
            clamshell_ui::Config { top_size: (960, 540), bottom_size: (620, 540), touch_layout: Arc::default() },
            display.handle(),
            false,
            || {},
            |_| Process::default(),
            |_| {},
        )
        .unwrap();
        let (socket, mut peer) = UnixStream::pair().unwrap();
        let now = Instant::now();
        let mut wire = Transport::new(socket, now).unwrap();
        wire.turn(now).unwrap();
        peer.read_exact(&mut [0; 12]).unwrap();
        let mut wm = Wm { root: 1, owner: 2, atoms: Atoms(NAMES.iter().enumerate().map(|(i, name)| (*name, 100 + i as u32)).collect()), ..Wm::default() };
        wm.discover(10, Rectangle::from_size((400, 300).into()), false, &mut wire, &mut server).unwrap();
        wm.windows[0].outstanding = 0;
        wm.windows[0].wants_map = true;
        wm.windows[0].properties.class = "fixture".into();
        let id = wm.windows[0].id;
        wm.settle(&mut wire, &mut server).unwrap();
        assert!(server.core.state().session().window(id).is_some());
        assert_eq!(server.core.state().session().top_window().map(|w| w.id), Some(id));
        wire.turn(now).unwrap();
        let mut bytes = [0; 4096];
        let length = peer.read(&mut bytes).unwrap();
        let mut packets = &bytes[..length];
        let mut requests = Vec::new();
        while !packets.is_empty() {
            requests.push(packets[0]);
            let size = usize::from(u16::from_ne_bytes([packets[2], packets[3]])) * 4;
            packets = &packets[size..];
        }
        // The X server must receive final geometry before it can map or focus the new window.
        assert_eq!(
            requests,
            vec![
                CHANGE_WINDOW_ATTRIBUTES_REQUEST,
                CONFIGURE_WINDOW_REQUEST,
                CHANGE_PROPERTY_REQUEST,
                CHANGE_PROPERTY_REQUEST,
                MAP_WINDOW_REQUEST,
                CHANGE_PROPERTY_REQUEST
            ]
        );
        // Focus waits for the server's time: the last request is the empty append on our own window.
        let stamp = &bytes[length - 24..length];
        assert_eq!((stamp[0], stamp[1]), (CHANGE_PROPERTY_REQUEST, u8::from(PropMode::APPEND)));
        assert_eq!(u32::from_ne_bytes(stamp[4..8].try_into().unwrap()), 2);
        assert_eq!(u32::from_ne_bytes(stamp[20..24].try_into().unwrap()), 0);
        let notify = |wm: &mut Wm, server: &mut Server, wire: &mut Transport, time: u32| {
            let event = PropertyNotifyEvent {
                response_type: PROPERTY_NOTIFY_EVENT,
                sequence: 0,
                window: 2,
                atom: wm.atoms.get("_THOR_TIMESTAMP"),
                time,
                state: Property::NEW_VALUE,
            };
            wm.event(&<[u8; 32]>::from(&event), wire, server).unwrap();
        };
        for (n, (input, take)) in [(true, false), (true, true), (false, true), (false, false), (true, false)].into_iter().enumerate() {
            wm.focus = None;
            wm.windows[0].properties.input = Some(input);
            wm.windows[0].properties.take_focus = take;
            wm.settle_focus(&mut wire, &server).unwrap();
            if n > 0 {
                // Each change asks for a new time; the last one is never reused.
                wire.turn(now).unwrap();
                let count = peer.read(&mut bytes).unwrap();
                assert_eq!((count, bytes[0], bytes[1]), (24, CHANGE_PROPERTY_REQUEST, u8::from(PropMode::APPEND)));
            }
            let time = 1234 + n as u32;
            notify(&mut wm, &mut server, &mut wire, time);
            wm.settle_focus(&mut wire, &server).unwrap();
            wire.turn(now).unwrap();
            let count = peer.read(&mut bytes).unwrap();
            let focus = &bytes[..12];
            assert_eq!(focus[0], SET_INPUT_FOCUS_REQUEST);
            assert_eq!(u32::from_ne_bytes(focus[4..8].try_into().unwrap()), if input { 10 } else { 0 });
            assert_eq!(u32::from_ne_bytes(focus[8..12].try_into().unwrap()), time);
            let rest = &bytes[12..count];
            if take {
                // The client sees the same nonzero time as the preceding SetInputFocus request.
                assert_eq!(rest[0], SEND_EVENT_REQUEST);
                let message = ClientMessageEvent::try_parse(&rest[12..]).unwrap().0;
                assert_eq!(message.data.as_data32()[0], wm.atoms.get("WM_TAKE_FOCUS"));
                assert_eq!(message.data.as_data32()[1], time);
                assert_eq!(rest[44], CHANGE_PROPERTY_REQUEST);
            } else {
                assert_eq!(rest[0], CHANGE_PROPERTY_REQUEST);
            }
        }
        wm.settle(&mut wire, &mut server).unwrap();
        wire.turn(now).unwrap();
        peer.set_nonblocking(true).unwrap();
        assert_eq!(peer.read(&mut bytes).unwrap_err().kind(), io::ErrorKind::WouldBlock, "unchanged state sends no X11 commands");
        wm.windows[0].properties.name = "first title".into();
        wm.windows[0].dirty = true;
        wm.settle(&mut wire, &mut server).unwrap();
        wm.windows[0].properties.name = "last title".into();
        wm.windows[0].dirty = true;
        wm.settle(&mut wire, &mut server).unwrap();
        assert_eq!(server.x11_changes.len(), 1, "X11 title changes coalesce before recording a frame");
        assert_eq!(server.core.state().session().window(id).unwrap().title, "");
        server.frame();
        assert_eq!(server.core.state().session().window(id).unwrap().title, "last title");
        assert!(server.x11_changes.is_empty());
        wm.windows[0].properties.name = "dialog title".into();
        wm.windows[0].properties.role = clamshell_state::session::presentation::WindowRole::Dialog;
        wm.windows[0].dirty = true;
        wm.settle(&mut wire, &mut server).unwrap();
        let mut adopted = server.core.state().session().window(id).unwrap().facts.clone();
        adopted.unit = Some("adopted.scope".into());
        adopted.pids = vec![456];
        server.core.handle(WindowEvent::Changed(adopted).into());
        server.frame();
        let window = server.core.state().session().window(id).unwrap();
        assert_eq!(window.title, "dialog title");
        assert_eq!(window.role, clamshell_state::session::presentation::WindowRole::Dialog);
        assert_eq!(window.unit.as_deref(), Some("adopted.scope"));
        assert_eq!(window.pids, vec![456], "deferred metadata cannot undo scope adoption");
        let recording = server.trace.borrow().snapshot(server.core.state());
        // Identity and title settle atomically: the UI must never record half of this update.
        assert!(
            matches!(recording.records.back().map(|r| &r.input), Some(crate::trace::Input::Batch(events)) if matches!(events.as_slice(), [WindowEvent::Changed(_), WindowEvent::Title { window, title }] if *window == id && title == "dialog title"))
        );
    }
}

#[path = "menus.rs"]
mod menus;
