//! Each turn's settling: windows paired with their surfaces, placed and configured as the state says,
//! the focus set and closes carried out; and each window's facts, sent to the state.

use super::*;

impl Wm {
    pub(super) fn settle(&mut self, wire: &mut Transport, server: &mut Server) -> io::Result<()> {
        self.pair_surfaces(server);
        self.settle_windows(wire, server)?;
        self.settle_focus(wire, server)?;
        self.settle_close(wire, server)?;
        Ok(())
    }
    /// Each window given a serial, paired with the surface Xwayland committed with it.
    pub(super) fn pair_surfaces(&mut self, server: &mut Server) {
        for window in &mut self.windows {
            if window.surface.is_none()
                && let Some(serial) = window.serial
                && let Some(surface) = crate::x11::shell::take(server, serial)
            {
                log::debug!("X11 association id={} map={} pending={}", window.id, window.wants_map, window.outstanding);
                window.surface = Some(surface);
            }
        }
    }
    pub(super) fn settle_windows(&mut self, wire: &mut Transport, server: &mut Server) -> io::Result<()> {
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
    pub(super) fn settle_focus(&mut self, wire: &mut Transport, server: &Server) -> io::Result<()> {
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
                self.stamp = Stamp::Requested(Instant::now() + crate::x11::HUNG);
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
    pub(super) fn settle_close(&mut self, wire: &mut Transport, server: &mut Server) -> io::Result<()> {
        for id in server.x11.close.drain(..) {
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
    pub(super) fn sync_facts(&mut self, parent: Option<i64>, server: &mut Server) {
        if !self.opened || self.dirty || self.sent.as_ref().is_some_and(|old| old.parent != parent) {
            let process = server
                .core
                .state()
                .session()
                .window(self.id)
                .map(|v| Process { unit: v.unit.clone(), pids: v.pids.clone(), game: v.game.clone() })
                .unwrap_or_else(|| self.process.clone());
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
                if let Some(pending) = server.x11.changes.iter_mut().find(|old| old.id == facts.id) {
                    *pending = facts.clone();
                } else {
                    server.x11.changes.push(facts.clone());
                }
                self.sent = Some(facts);
            }
            self.dirty = false;
        }
    }
    pub(super) fn bind_surface(&self, server: &mut Server) {
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
                shown: crate::windows::Shown::of(server.core.state(), self.id),
                frames: Default::default(),
            });
        }
    }
    pub(super) fn configure(&mut self, atoms: &Atoms, wire: &mut Transport, server: &Server) -> io::Result<()> {
        let Some(model) = server.core.state().session().window(self.id) else { return Ok(()) };
        if self.revision != Some(server.core.state_revision()) {
            self.policy = server.core.state().window_presentation(self.id);
            self.revision = Some(server.core.state_revision());
        }
        let area = server.area(server.core.state().session().below(self.id));
        // (Its clarity as read for its window this revision, else from the state while it has none yet.)
        let clarity = server.windows.iter().find(|w| w.id == self.id).map_or_else(|| server.core.state().window_clarity(self.id), |w| w.shown.clarity);
        let shown = crate::windows::Shown { presentation: self.policy, clarity, ..Default::default() };
        let own = (i32::from(self.size.0), i32::from(self.size.1)).into();
        // A fullscreen game that switched mode is configured to it, so that Xwayland emulates it: the window
        // at the mode's size is scaled to the screen, and comes to us as a smaller buffer scaled by a viewport.
        let Some(configure) = crate::geometry::configure(model, &shown, area, Some(own), self.properties.emulated) else { return Ok(()) };
        // An X11 window has a size: one the client chooses is its own.
        let geometry = Rectangle::new(area.loc + configure.offset, configure.size.unwrap_or(own));
        let layout = if configure.fullscreen { WindowLayout::Fullscreen } else { WindowLayout::Centred };
        if self.configured != Some(geometry) || self.layout != Some(layout) {
            log::debug!("X11 configure {} to {geometry:?}: {layout:?}, limits {:?}, emulated {:?}", self.id, self.properties.limits, self.properties.emulated);
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
            Wm::property(atoms, wire, self.xid, "_NET_WM_STATE", AtomEnum::ATOM.into(), if configure.fullscreen { &fullscreen } else { &[] })?;
            Wm::property(atoms, wire, self.xid, "WM_STATE", atoms.get("WM_STATE"), &[1, 0])?;
            wire.request(MapWindowRequest { window: self.xid }, 0, false)?;
            self.configured = Some(geometry);
            self.layout = Some(layout);
        }
        Ok(())
    }
}
