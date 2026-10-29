//! Becoming Xwayland's window manager: the atoms, the root's attributes, the owner window, and a time
//! anchor for the replies.

use super::*;

impl Wm {
    pub(super) fn start_setup(&mut self, wire: &mut Transport, now: Instant) -> io::Result<()> {
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
    pub(super) fn finish_setup(&mut self, wire: &mut Transport) -> io::Result<()> {
        if self.started && !self.ready && self.atoms.0.len() == NAMES.len() && self.extension_done && self.composite_done {
            self.initialise(wire)?;
        }
        Ok(())
    }
    pub(super) fn anchor_sequence(&mut self, wire: &mut Transport, now: Instant) -> io::Result<()> {
        // X11's 16-bit wire sequence needs a reply before wrapping, only after actual traffic.
        if self.ready && self.pending.len() < 256 && wire.needs_sequence_anchor() && !self.pending.iter().any(|p| matches!(p.kind, Query::OwnershipBarrier)) {
            self.query(wire, GetSelectionOwnerRequest { selection: self.atoms.get("WM_S0") }, 0, Query::OwnershipBarrier, now)?;
        }
        Ok(())
    }
    pub(super) fn initialise(&mut self, wire: &mut Transport) -> io::Result<()> {
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
    pub(super) fn property(atoms: &Atoms, wire: &mut Transport, window: u32, name: &str, type_: u32, values: &[u32]) -> io::Result<()> {
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
}
