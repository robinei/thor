//! Programs' tray icons (StatusNotifierItem, over D-Bus) and their menus, as the system reads them, on
//! the dock. Each is someone's ([`State::tray_owner`]): the running entry whose scope its process is in;
//! else the dock entry of the desktop file named as its id (`desktop:steam` for Steam's `steam`, started
//! outside ClamShell); else a dock tile of its own (`tray:<key>`, its picture the library's desktop entry
//! of that name, if there is one, else its own icon). A tile with one runs: a tap shows its windows, or
//! (none of ours) asks the program to show itself; its corner mark opens the program's menu, and so
//! does Menu in its long-press menu. Quit and Force kill of a program not started by ClamShell end the
//! icon's process (asked, then killed).

use std::path::PathBuf;
use std::sync::Arc;

use crate::effect::Effect;
use crate::instance::Instance;
use crate::{Game, State};

/// A tray icon.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TrayItem {
    /// Which, to the system: its D-Bus name and object path.
    pub key: Arc<str>,
    /// Its own id (`steam`).
    pub id: String,
    pub title: String,
    /// Its process, and that process's systemd unit (`clamshell-run-…scope` if ours).
    pub pid: Option<u32>,
    pub unit: Option<String>,
    /// Its icon's image file, if one was found.
    pub icon: Option<PathBuf>,
    /// Its menu's entries (separators and hidden ones left out).
    pub menu: Vec<TrayEntry>,
}

/// An entry of a tray icon's menu.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TrayEntry {
    /// Its id in the menu, which a click names.
    pub id: i32,
    pub label: String,
    pub enabled: bool,
    /// A checkmark's or radio button's state; none for a plain entry.
    pub checked: Option<bool>,
    /// A submenu's entries.
    pub children: Vec<TrayEntry>,
}

/// What the tray thread is asked.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TrayCommand {
    /// The program shows itself (the icon clicked).
    Activate(Arc<str>),
    /// An entry of its menu chosen.
    Click { item: Arc<str>, id: i32 },
    /// Its process asked to end (SIGTERM), or killed.
    End { pid: u32, force: bool },
}

/// The id of a tray icon's own dock tile.
const TILE_PREFIX: &str = "tray:";

impl State {
    /// The tray icons, after any change.
    pub(crate) fn set_tray(&mut self, items: Vec<TrayItem>) -> bool {
        if self.tray == items {
            return false;
        }
        self.tray = items;
        self.follow_tray_tiles();
        self.follow_dock();
        true
    }

    /// Whose tray icon `t` is (see the module's doc).
    pub fn tray_owner(&self, t: &TrayItem) -> Arc<str> {
        let key = t.unit.as_deref().and_then(Instance::from_unit).and_then(|i| i.key().map(str::to_owned));
        if let Some(r) = key.and_then(|k| self.session.running.iter().find(|r| r.key == k)) {
            return r.id.clone();
        }
        let named = format!("desktop:{}", t.id).to_lowercase();
        if let Some(item) = self.dock.items().iter().find(|i| i.id.to_lowercase() == named) {
            return item.id.clone();
        }
        format!("{TILE_PREFIX}{}", t.key).into()
    }

    /// The tray icon of a dock entry, if it has one.
    pub fn tray_of(&self, entry: &str) -> Option<&TrayItem> {
        self.tray.iter().find(|t| &*self.tray_owner(t) == entry)
    }

    /// The tiles of tray icons that are no entry's, as entries (see the module's doc): made again
    /// whenever the icons or the library change.
    pub(crate) fn follow_tray_tiles(&mut self) {
        let games = &self.shelf.library().games;
        let tiles = self
            .tray
            .iter()
            .filter_map(|t| {
                let owner = self.tray_owner(t);
                owner.starts_with(TILE_PREFIX).then(|| {
                    let named = format!("desktop:{}", t.id).to_lowercase();
                    let app = games.iter().find(|g| g.id.to_lowercase() == named);
                    let title = app.map_or_else(|| t.title.clone(), |g| g.title.clone());
                    Game {
                        icon: app.and_then(|g| g.icon.clone()).or_else(|| t.icon.clone()),
                        tool: true,
                        ..Game::new(&owner, if title.is_empty() { &t.id } else { &title })
                    }
                })
            })
            .collect();
        self.tray_tiles = tiles;
    }

    /// The tiles of tray icons that are no entry's (see [`State::follow_tray_tiles`]).
    pub(crate) fn tray_tile(&self, id: &str) -> Option<&Game> {
        self.tray_tiles.iter().find(|g| &*g.id == id)
    }

    /// The ids of tray icons' own tiles, for the dock.
    pub(crate) fn tray_tile_ids(&self) -> Vec<Arc<str>> {
        self.tray_tiles.iter().map(|g| g.id.clone()).collect()
    }

    /// The menu page open (the entry's tray icon, and the entries of the submenu open), if one is.
    pub fn tray_page(&self) -> Option<(&TrayItem, &[TrayEntry])> {
        let path = self.dock.tray.as_ref()?;
        let item = self.tray_of(self.dock.menu()?)?;
        let mut entries = item.menu.as_slice();
        for id in path {
            entries = entries.iter().find(|e| e.id == *id)?.children.as_slice();
        }
        Some((item, entries))
    }

    /// An entry of the open menu page chosen: a submenu opens; anything else is clicked, and the menu
    /// closes. A disabled one does nothing.
    pub(crate) fn tray_choice(&mut self, id: i32) -> Vec<Effect> {
        let Some((item, entries)) = self.tray_page() else {
            return vec![];
        };
        let Some(entry) = entries.iter().find(|e| e.id == id).filter(|e| e.enabled) else {
            return vec![];
        };
        if !entry.children.is_empty() {
            if let Some(path) = &mut self.dock.tray {
                path.push(id);
            }
            return vec![];
        }
        let click = Effect::Tray(TrayCommand::Click { item: item.key.clone(), id });
        self.dock.menu = None;
        self.dock.tray = None;
        vec![click]
    }

    /// A tap on a dock entry whose program shows no window of ours, with a tray icon: the program asked
    /// to show itself.
    pub(crate) fn activate_tray(&self, entry: &str) -> Option<Effect> {
        self.tray_of(entry).map(|t| Effect::Tray(TrayCommand::Activate(t.key.clone())))
    }

    /// Quit or Force kill of an entry that doesn't run as ClamShell's but has a tray icon: its process
    /// asked to end, or killed.
    pub(crate) fn end_tray(&self, entry: &str, force: bool) -> Vec<Effect> {
        self.tray_of(entry).and_then(|t| t.pid).map(|pid| Effect::Tray(TrayCommand::End { pid, force })).into_iter().collect()
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::indexing_slicing)]
mod tests {
    use super::*;
    use crate::id::{DockId, Id};
    use crate::testkit::*;
    use crate::{Event, PointerEvent};

    fn entry(id: i32, label: &str, children: Vec<TrayEntry>) -> TrayEntry {
        TrayEntry { id, label: label.into(), enabled: true, checked: None, children }
    }

    fn item(key: &str, id: &str, unit: Option<&str>) -> TrayItem {
        let menu = vec![entry(1, "Library", vec![]), entry(2, "Games", vec![entry(3, "Zelda", vec![])]), entry(4, "Exit", vec![])];
        TrayItem { key: key.into(), id: id.into(), title: id.into(), pid: Some(77), unit: unit.map(Into::into), icon: None, menu }
    }

    fn tap_dock(s: &mut State, id: DockId) -> Vec<Effect> {
        tap(s, Id::Dock(id))
    }

    #[test]
    fn a_tray_icon_is_its_programs_entry_or_a_tile_of_its_own() {
        let mut s = state();
        // A running entry's (its process in the entry's scope): its menu, no tile of its own.
        let zelda = launch(&mut s, "zelda-lttp");
        s.apply(Event::Tray(vec![item(":1.5/a", "zelda3", zelda.unit().as_deref())]));
        assert_eq!(s.tray_of("zelda-lttp").map(|t| &*t.key), Some(":1.5/a"));
        assert_eq!(s.dock().items().len(), 1);
        // A program not started by ClamShell (Firefox's desktop file named as its id): a tile of its own,
        // the library's picture and title.
        s.apply(Event::Tray(vec![item(":1.5/a", "zelda3", zelda.unit().as_deref()), item(":1.9/b", "firefox", None)]));
        let tile: Arc<str> = "tray::1.9/b".into();
        assert_eq!(s.dock().items().last().map(|i| i.id.clone()), Some(tile.clone()));
        assert_eq!(s.game(&tile).map(|g| g.title.as_str()), Some("Firefox"));
        assert!(s.runs(&tile));
        // A tap: the program shows itself. Quit and Force kill: its process.
        let at = s.dock().items().iter().position(|i| i.id == tile).unwrap() as u16;
        assert_eq!(tap_dock(&mut s, DockId::Entry(at)), vec![Effect::Tray(TrayCommand::Activate(":1.9/b".into()))]);
        s.apply(PointerEvent::LongPress(Id::Dock(DockId::Entry(at))).into());
        assert_eq!(tap_dock(&mut s, DockId::Quit), vec![Effect::Tray(TrayCommand::End { pid: 77, force: false })]);
        // Its corner mark: its menu; a submenu opens, Back goes up, an entry is clicked and it closes.
        tap_dock(&mut s, DockId::Tray(at));
        assert_eq!(s.tray_page().map(|(_, e)| e.len()), Some(3));
        tap_dock(&mut s, DockId::TrayEntry(2));
        assert_eq!(s.tray_page().map(|(_, e)| e[0].label.as_str()), Some("Zelda"));
        tap_dock(&mut s, DockId::Back);
        assert_eq!(s.tray_page().map(|(_, e)| e.len()), Some(3));
        assert_eq!(tap_dock(&mut s, DockId::TrayEntry(4)), vec![Effect::Tray(TrayCommand::Click { item: ":1.9/b".into(), id: 4 })]);
        assert!(s.dock().menu().is_none());
        // Gone: its tile too.
        s.apply(Event::Tray(vec![]));
        assert!(!s.dock().items().iter().any(|i| i.id == tile));
        // On the dock as itself (pinned): the icon is that entry's, no tile of its own.
        s.toggle_pin("desktop:firefox".into());
        s.apply(Event::Tray(vec![item(":1.9/b", "firefox", None)]));
        assert!(s.tray_of("desktop:firefox").is_some() && s.runs("desktop:firefox"));
        assert!(!s.dock().items().iter().any(|i| i.id == tile));
    }
}
