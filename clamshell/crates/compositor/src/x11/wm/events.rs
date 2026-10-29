//! What Xwayland tells the window manager: its events and the replies to its queries.

use super::*;

impl Wm {
    pub(super) fn reply(&mut self, query: Query, bytes: &[u8], wire: &mut Transport, server: &mut Server, now: Instant) -> io::Result<()> {
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
                    window.pid = reply
                        .ids
                        .iter()
                        .find(|v| v.spec.mask.contains(res::ClientIdMask::LOCAL_CLIENT_PID))
                        .and_then(|v| v.value.first())
                        .copied()
                        .filter(|pid| *pid > 1);
                    // The window waits on for its process (see `take_identified`).
                    match window.pid {
                        Some(pid) => server.identities.ask(pid, crate::clients::Waiting::X11Window(id)),
                        None => window.outstanding = window.outstanding.saturating_sub(1),
                    }
                }
            }
        }
        Ok(())
    }
    pub(super) fn discover(
        &mut self,
        xid: u32,
        geometry: Rectangle<i32, Logical>,
        override_redirect: bool,
        wire: &mut Transport,
        server: &mut Server,
    ) -> io::Result<()> {
        if wire.setup.as_ref().is_some_and(|setup| (xid & !setup.resource_id_mask) == setup.resource_id_base) {
            return Ok(());
        }
        if self.windows.iter().any(|w| w.xid == xid) || xid == self.owner {
            return Ok(());
        }
        if self.windows.len() >= WINDOWS_MAX {
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
            process: Process::default(),
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
            parent: None,
            dirty: true,
            refresh: 0,
            shown: None,
        });
        wire.request(
            ChangeWindowAttributesRequest { window: xid, value_list: Cow::Owned(ChangeWindowAttributesAux::new().event_mask(EventMask::PROPERTY_CHANGE)) },
            0,
            false,
        )?;

        Ok(())
    }
    pub(super) fn event(&mut self, bytes: &[u8], wire: &mut Transport, server: &mut Server) -> io::Result<()> {
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
            // A window made top-level from inside another is found as a created one is; one put inside
            // another is no longer a window of ours.
            Some(REPARENT_NOTIFY_EVENT) => {
                let event = ReparentNotifyEvent::try_parse(bytes).map_err(io::Error::other)?.0;
                if event.parent == self.root {
                    let at = Rectangle::new((i32::from(event.x), i32::from(event.y)).into(), (1, 1).into());
                    self.discover(event.window, at, event.override_redirect, wire, server)?;
                } else if let Some(id) = self.windows.iter().find(|w| w.xid == event.window).map(|w| w.id) {
                    self.forget(id, server);
                }
            }
            Some(MAP_REQUEST_EVENT) => {
                let event = MapRequestEvent::try_parse(bytes).map_err(io::Error::other)?.0;
                // Only a managed window's map is redirected to us: one created override-redirect has
                // since cleared it, and is managed now.
                if let Some(window) = self.windows.iter_mut().find(|w| w.xid == event.window) {
                    window.wants_map = true;
                    window.override_redirect = false;
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
                        // Unmapped by its client, a managed window is withdrawn (ICCCM 4.1.4): it may be
                        // mapped again as new.
                        let withdrawn = w.configured.is_some() && !w.override_redirect && !w.input_only;
                        let opened = w.opened;
                        w.wants_map = false;
                        w.opened = false;
                        w.serial = None;
                        w.surface = None;
                        w.configured = None;
                        for client in &mut server.windows {
                            let parent = client.id == id;
                            client.window.dismiss_x11_popups(&mut server.x11.popup_dismiss, |popup| parent || popup.id == id);
                        }
                        server.windows.retain(|v| v.id != id);
                        server.x11.changes.retain(|v| v.id != id);
                        if opened {
                            server.core.handle(WindowEvent::Closed(id).into());
                        }
                        if withdrawn {
                            Self::property(&self.atoms, wire, xid, "WM_STATE", self.atoms.get("WM_STATE"), &[0, 0])?;
                            // EWMH: a withdrawn window's state is the client's to set again.
                            wire.request(DeletePropertyRequest { window: xid, property: self.atoms.get("_NET_WM_STATE") }, 0, false)?;
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
                if event.format == 32 && event.type_ == self.atoms.get("_NET_ACTIVE_WINDOW") {
                    self.activate(event.window, event.data.as_data32(), server);
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
    /// A client asks for `xid` to be in front (`_NET_ACTIVE_WINDOW`; `data` is its source, time and the
    /// asker's active window). As for a Wayland client's activation, only the program that has the focus
    /// may move it: the asker's active window is the focused one, or the window is the focused one's
    /// process's.
    pub(super) fn activate(&self, xid: u32, data: [u32; 5], server: &mut Server) {
        let Some(window) = self.windows.iter().find(|w| w.xid == xid && w.opened) else {
            return log::debug!("X11 activation of {xid}: no such window open");
        };
        let [_, _, asker, ..] = data;
        if self.may_activate(window, asker) {
            server.core.handle(WindowEvent::Activate(window.id).into());
        } else {
            log::info!("X11 activation of window {} refused: its program doesn't have the focus", window.id);
        }
    }
    /// Whether `window` may be put in front at the ask of a client whose active window is `asker`.
    pub(super) fn may_activate(&self, window: &Window, asker: u32) -> bool {
        let focused = self.focus.and_then(|focus| self.windows.iter().find(|w| w.xid == focus));
        focused.is_some_and(|f| f.xid == asker || (f.pid.is_some() && f.pid == window.pid))
    }
    pub(super) fn forget(&mut self, id: i64, server: &mut Server) {
        self.windows.retain(|w| w.id != id);
        for client in &mut server.windows {
            let parent = client.id == id;
            client.window.dismiss_x11_popups(&mut server.x11.popup_dismiss, |popup| parent || popup.id == id);
        }
        server.x11.changes.retain(|facts| facts.id != id);
        server.windows.retain(|w| w.id != id);
        if server.core.state().session().window(id).is_some() {
            server.core.handle(WindowEvent::Closed(id).into());
        }
    }
}
