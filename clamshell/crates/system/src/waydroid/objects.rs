//! What the proxy reads of the protocol and what it changes. It reads the registry, `xdg_wm_base`, its
//! surfaces and their toplevels (app id, size, activated). It drops toplevel configures of an unchanged
//! size and puts Android's caption out of sight.

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::time::{Duration, Instant};

use super::wire::{ints, message};

/// The display object's id. The opcodes read follow.
pub(super) const DISPLAY: u32 = 1;

/// wl_display: `get_registry` (request), `delete_id` (event).
pub(super) const GET_REGISTRY: u16 = 1;

pub(super) const DELETE_ID: u16 = 1;

/// wl_registry: `bind`.
pub(super) const BIND: u16 = 0;

/// xdg_wm_base: `get_xdg_surface`.
pub(super) const GET_XDG_SURFACE: u16 = 2;

/// xdg_surface: `get_toplevel`, `set_window_geometry`.
pub(super) const GET_TOPLEVEL: u16 = 1;

pub(super) const SET_WINDOW_GEOMETRY: u16 = 3;

/// xdg_toplevel: `set_app_id` (request), `configure` (event) and its `activated` state.
pub(super) const SET_APP_ID: u16 = 3;

pub(super) const CONFIGURE: u16 = 0;

pub(super) const ACTIVATED: u32 = 4;

/// How often the settings' files are read again.
pub(super) const SETTINGS_EVERY: Duration = Duration::from_secs(1);

/// What to do with a message.
#[derive(Debug, PartialEq, Eq)]
pub(super) enum Pass {
    Keep,
    Drop,
    /// Pass on this changed message instead, whole with header and arguments.
    Replace(Vec<u8>),
}

/// The settings ClamShell gives the proxy (see the module docs), read again at most every second.
#[derive(Debug, Default)]
pub(crate) struct Settings {
    dir: Option<PathBuf>,
    read: Option<Instant>,
    /// The caption's height in pixels. 0 means the caption is shown.
    pub(super) caption: i32,
}

impl Settings {
    pub(super) fn refresh(&mut self, now: Instant) {
        let Some(dir) = &self.dir else {
            return;
        };
        if self.read.is_some_and(|t| now.duration_since(t) < SETTINGS_EVERY) {
            return;
        }
        self.read = Some(now);
        let path = dir.join("android-caption");
        self.caption = match std::fs::read_to_string(&path) {
            Ok(text) => text.trim().parse().inspect_err(|e| log::debug!("wayland filter: {}: {e}", path.display())).unwrap_or(0),
            Err(e) => {
                if e.kind() == std::io::ErrorKind::NotFound {
                    log::trace!("wayland filter: {}: {e}", path.display());
                } else {
                    log::debug!("wayland filter: {}: {e}", path.display());
                }
                0
            }
        };
    }
}

/// A toplevel the proxy follows.
#[derive(Debug)]
pub(super) struct Toplevel {
    xdg_surface: u32,
    app_id: String,
    /// The size of the last configure passed on, as the compositor gave it. With none yet, the first
    /// configure goes through.
    size: Option<(i32, i32)>,
    active: bool,
}

/// What the proxy knows of a connection's objects.
#[derive(Debug, Default)]
pub(super) struct Objects {
    registries: HashSet<u32>,
    wm_bases: HashSet<u32>,
    xdg_surfaces: HashSet<u32>,
    pub(super) toplevels: HashMap<u32, Toplevel>,
    pub(super) settings: Settings,
}

impl Objects {
    pub(crate) fn new(settings_dir: Option<PathBuf>) -> Objects {
        Objects { settings: Settings { dir: settings_dir, ..Settings::default() }, ..Objects::default() }
    }

    pub(super) fn toplevel_of_xdg_surface(&self, xdg_surface: u32) -> Option<u32> {
        self.toplevels.iter().find(|(_, t)| t.xdg_surface == xdg_surface).map(|(id, _)| *id)
    }

    /// Handles a request from the client. It notes the objects the client makes that the proxy follows,
    /// and moves a window geometry below the caption.
    pub(super) fn request(&mut self, object: u32, opcode: u16, args: &[u8], now: Instant) -> Pass {
        let word = |i: usize| args.get(i * 4..i * 4 + 4).and_then(|b| b.try_into().ok()).map(u32::from_ne_bytes);
        if self.registries.contains(&object) && opcode == BIND {
            // The arguments are name, interface, version and id. The interface is a string: its length,
            // then its bytes padded to 4.
            let len = word(1).unwrap_or(0) as usize;
            let after = 8 + len.div_ceil(4) * 4;
            let id = args.get(after + 4..after + 8).and_then(|b| b.try_into().ok()).map(u32::from_ne_bytes);
            if args.get(8..8 + len.saturating_sub(1)) == Some(b"xdg_wm_base") {
                self.wm_bases.extend(id);
            }
            return Pass::Keep;
        }
        match opcode {
            GET_REGISTRY if object == DISPLAY => self.registries.extend(word(0)),
            GET_XDG_SURFACE if self.wm_bases.contains(&object) => {
                self.xdg_surfaces.extend(word(0));
            }
            GET_TOPLEVEL if self.xdg_surfaces.contains(&object) => {
                if let Some(id) = word(0) {
                    let t = Toplevel { xdg_surface: object, app_id: String::new(), size: None, active: false };
                    self.toplevels.insert(id, t);
                }
            }
            SET_WINDOW_GEOMETRY if self.xdg_surfaces.contains(&object) && self.toplevel_of_xdg_surface(object).is_some() => {
                self.settings.refresh(now);
                let caption = self.settings.caption;
                let int = |i: usize| word(i).map(|w| w as i32);
                if let (Some(x), Some(y), Some(w), Some(h)) = (int(0), int(1), int(2), int(3))
                    && caption > 0
                    && h > caption
                {
                    return Pass::Replace(message(object, opcode, &ints(&[x, y + caption, w, h - caption])));
                }
            }
            SET_APP_ID if self.toplevels.contains_key(&object) => {
                let len = word(0).unwrap_or(0) as usize;
                let app_id = args.get(4..4 + len.saturating_sub(1)).map(|b| String::from_utf8_lossy(b).into_owned()).unwrap_or_default();
                if let Some(t) = self.toplevels.get_mut(&object) {
                    t.app_id = app_id;
                }
            }
            _ => {}
        }
        Pass::Keep
    }

    /// Handles an event from the compositor. A configure of the size the window already had is dropped,
    /// and any other is made taller by the caption. Deleted ids are forgotten.
    pub(super) fn event(&mut self, object: u32, opcode: u16, args: &[u8], now: Instant) -> Pass {
        let int = |i: usize| args.get(i * 4..i * 4 + 4).and_then(|b| b.try_into().ok()).map(i32::from_ne_bytes);
        if object == DISPLAY && opcode == DELETE_ID {
            let Some(id) = int(0).map(|i| i as u32) else {
                return Pass::Keep;
            };
            self.registries.remove(&id);
            self.wm_bases.remove(&id);
            self.xdg_surfaces.remove(&id);
            self.toplevels.remove(&id);
            return Pass::Keep;
        }
        if opcode != CONFIGURE {
            return Pass::Keep;
        }
        let Some(t) = self.toplevels.get_mut(&object) else {
            return Pass::Keep;
        };
        let (Some(w), Some(h)) = (int(0), int(1)) else {
            return Pass::Keep;
        };
        // The states are an array: its length in bytes, then u32s.
        let states_len = int(2).unwrap_or(0) as usize;
        let states = args.get(12..12 + states_len).unwrap_or_default();
        t.active = states.chunks_exact(4).any(|s| s.try_into().ok().map(u32::from_ne_bytes) == Some(ACTIVATED));
        if t.size == Some((w, h)) {
            return Pass::Drop;
        }
        t.size = Some((w, h));
        self.settings.refresh(now);
        let caption = self.settings.caption;
        if caption > 0 && h > 0 {
            let mut changed = args.to_vec();
            changed.splice(4..8, (h + caption).to_ne_bytes());
            return Pass::Replace(message(object, opcode, &changed));
        }
        Pass::Keep
    }
}
