//! What the proxy reads of the protocol and changes: the registry, `xdg_wm_base`, its surfaces and their
//! toplevels (app id, size, activated); toplevel configures of an unchanged size dropped, Android's
//! caption put out of sight, the active window's commits counted as its frame rate.

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::time::{Duration, Instant};

use super::wire::{ints, message};

/// The display object's id, and the opcodes read.
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

/// xdg_toplevel: `set_app_id` (request), `configure` (event; its `activated` state).
pub(super) const SET_APP_ID: u16 = 3;

pub(super) const CONFIGURE: u16 = 0;

pub(super) const ACTIVATED: u32 = 4;

/// wl_surface: `commit`.
pub(super) const COMMIT: u16 = 6;

/// How often a frame rate row is written, and how often the settings' files are read again.
pub(super) const ROW_EVERY: Duration = Duration::from_millis(500);
pub(super) const SETTINGS_EVERY: Duration = Duration::from_secs(1);

/// What to do with a message.
#[derive(Debug, PartialEq, Eq)]
pub(super) enum Pass {
    Keep,
    Drop,
    /// Passed on changed (whole: header and arguments).
    Replace(Vec<u8>),
}

/// The settings ClamShell gives the proxy (see the module's doc), read again at most every second.
#[derive(Debug, Default)]
pub struct Settings {
    dir: Option<PathBuf>,
    read: Option<Instant>,
    /// The caption's height (pixels; 0: shown).
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
        self.caption = std::fs::read_to_string(dir.join("android-caption")).unwrap_or_default().trim().parse().unwrap_or(0);
    }
}

/// A toplevel the proxy follows.
#[derive(Debug)]
pub(super) struct Toplevel {
    xdg_surface: u32,
    surface: u32,
    app_id: String,
    /// The size of the last configure passed on (none yet: the first goes), as the compositor gave it.
    size: Option<(i32, i32)>,
    active: bool,
    /// Commits since `since`.
    commits: u32,
    since: Option<Instant>,
}

/// What the proxy knows of a connection's objects.
#[derive(Debug, Default)]
pub(super) struct Objects {
    registries: HashSet<u32>,
    wm_bases: HashSet<u32>,
    /// Each xdg_surface's wl_surface.
    xdg_surfaces: HashMap<u32, u32>,
    pub(super) toplevels: HashMap<u32, Toplevel>,
    pub(super) settings: Settings,
}

impl Objects {
    pub fn new(settings_dir: Option<PathBuf>) -> Objects {
        Objects { settings: Settings { dir: settings_dir, ..Settings::default() }, ..Objects::default() }
    }

    pub(super) fn toplevel_of_xdg_surface(&self, xdg_surface: u32) -> Option<u32> {
        self.toplevels.iter().find(|(_, t)| t.xdg_surface == xdg_surface).map(|(id, _)| *id)
    }

    pub(super) fn toplevel_of_surface(&self, surface: u32) -> Option<u32> {
        self.toplevels.iter().find(|(_, t)| t.surface == surface).map(|(id, _)| *id)
    }

    /// A request from the client: the objects it makes that the proxy follows; a window geometry moved
    /// below the caption.
    pub(super) fn request(&mut self, object: u32, opcode: u16, args: &[u8], now: Instant) -> Pass {
        let word = |i: usize| args.get(i * 4..i * 4 + 4).and_then(|b| b.try_into().ok()).map(u32::from_ne_bytes);
        if self.registries.contains(&object) && opcode == BIND {
            // name, interface (a string: its length, then its bytes padded to 4), version, id.
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
                if let (Some(id), Some(surface)) = (word(0), word(1)) {
                    self.xdg_surfaces.insert(id, surface);
                }
            }
            GET_TOPLEVEL if self.xdg_surfaces.contains_key(&object) => {
                let surface = self.xdg_surfaces.get(&object).copied().unwrap_or(0);
                if let Some(id) = word(0) {
                    let t = Toplevel { xdg_surface: object, surface, app_id: String::new(), size: None, active: false, commits: 0, since: None };
                    self.toplevels.insert(id, t);
                }
            }
            SET_WINDOW_GEOMETRY if self.xdg_surfaces.contains_key(&object) && self.toplevel_of_xdg_surface(object).is_some() => {
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
            COMMIT => {
                if let Some(t) = self.toplevel_of_surface(object).and_then(|id| self.toplevels.get_mut(&id)) {
                    t.commits += 1;
                    t.since.get_or_insert(now);
                }
            }
            _ => {}
        }
        Pass::Keep
    }

    /// An event from the compositor: a configure of the size the window had dropped, another made taller
    /// by the caption; ids deleted forgotten.
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
        // The states: an array (its length in bytes, then u32s).
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

    /// The active toplevel's frame rate, once half a second has passed since it was last told: its app id
    /// and its commits a second.
    pub(super) fn frame_rate(&mut self, now: Instant) -> Option<(String, f32)> {
        let t = self.toplevels.values_mut().find(|t| t.active && !t.app_id.is_empty())?;
        let since = t.since?;
        let elapsed = now.duration_since(since);
        if elapsed < ROW_EVERY {
            return None;
        }
        let fps = t.commits as f32 / elapsed.as_secs_f32();
        t.commits = 0;
        t.since = Some(now);
        Some((t.app_id.clone(), fps))
    }
}
