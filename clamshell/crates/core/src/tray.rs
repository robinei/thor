//! Programs' tray icons (StatusNotifierItem, over D-Bus) and their menus, as the system reads them. An
//! icon is a running entry's: the one whose scope its process is in (started by ClamShell); else, in the
//! device's session (in a desktop's the program is the desktop's), the program runs as an entry found by
//! its icon ([`Origin::Tray`]): the library's desktop entry named as
//! the icon's id (`desktop:steam` for Steam's `steam`), else one of its own (`tray:<key>`, its picture
//! the icon). Like any running entry it is on the dock with a workspace of its own: its windows (those of
//! the icon's process and of the processes it started) go there, wherever they open. A tap shows them,
//! or (none open) asks the program to show itself, its window then shown there; the tile's corner mark
//! opens the program's menu, and so does Menu in its long-press menu. Quit closes its windows, then asks
//! the icon's process to end; Force kill kills it.

use std::path::PathBuf;
use std::sync::Arc;

use crate::effect::Effect;
use crate::instance::Instance;
use crate::running::{Origin, Running};
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
}

/// The id of an entry found by a tray icon the library has no desktop file for.
const PREFIX: &str = "tray:";

impl State {
    /// The tray icons, after any change: programs not started by ClamShell run as entries found by
    /// them, and end with them (or with their last window, if one is still open).
    pub(crate) fn set_tray(&mut self, items: Vec<TrayItem>) -> bool {
        if self.tray == items {
            return false;
        }
        self.tray = items;
        self.follow_tray();
        true
    }

    /// Entries found by tray icons: one for each icon of a program not started by ClamShell (its process
    /// in no scope of ours) that isn't one's already. Only in the device's session: in a desktop's, its
    /// programs are the desktop's (ClamShell a guest there, managing what it started).
    pub(crate) fn follow_tray(&mut self) {
        if !self.device_session {
            return;
        }
        let now = self.unix_secs;
        let games = &self.shelf.library().games;
        let mut new = Vec::new();
        for t in &self.tray {
            let ours = t.unit.as_deref().and_then(Instance::from_unit).is_some();
            let Some(pid) = t.pid.filter(|_| !ours) else {
                continue;
            };
            if self.session.running.iter().any(|r| r.tray.as_deref() == Some(&*t.key)) {
                continue;
            }
            let named = format!("desktop:{}", t.id).to_lowercase();
            let game = match games.iter().find(|g| g.id.to_lowercase() == named) {
                Some(g) => g.clone(),
                None => {
                    let title = if t.title.is_empty() { &t.id } else { &t.title };
                    Game { icon: t.icon.clone(), tool: true, ..Game::new(&format!("{PREFIX}{}", t.key), title) }
                }
            };
            // (Its entry runs already, found otherwise: its icon is that one's.)
            if let Some(r) = self.session.running.iter_mut().find(|r| r.id == game.id) {
                r.tray = Some(t.key.clone());
                r.pid = r.pid.or(Some(pid));
                continue;
            }
            new.push(Running {
                id: game.id.clone(),
                key: game.key.clone(),
                origin: Origin::Tray,
                app_id: None,
                pid: Some(pid),
                tray: Some(t.key.clone()),
                start: now,
                launches: 0,
                adopted: false,
                entry: Some(Box::new(game)),
            });
        }
        self.session.running.extend(new);
    }

    /// The tray icon of a running entry, if it has one: its scope's process's, or the one it was found by.
    pub fn tray_of(&self, entry: &str) -> Option<&TrayItem> {
        let r = self.session.running(entry)?;
        let in_scope = |t: &&TrayItem| t.unit.as_deref().and_then(Instance::from_unit).is_some_and(|i| i.key() == Some(r.key.as_str()));
        self.tray.iter().find(|t| r.tray.as_deref() == Some(&*t.key) || in_scope(t))
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

    /// Showing a running entry with no window open, with a tray icon: the program asked to show itself,
    /// and its window shown when it comes (on the entry's workspace, moved there if it opens elsewhere).
    pub(crate) fn activate_tray(&mut self, entry: &str) -> Option<Effect> {
        let key = self.tray_of(entry)?.key.clone();
        let r = self.session.running(entry)?;
        self.session.pending_show = Some(Instance::Run { key: r.key.clone(), secs: self.unix_secs, n: 0 });
        Some(Effect::Tray(TrayCommand::Activate(key)))
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::indexing_slicing)]
mod tests {
    use super::*;
    use crate::id::{DockId, Id};
    use crate::testkit::*;
    use crate::windows::{self, Window};
    use crate::{Event, WindowCommand};

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
    fn a_program_with_a_tray_icon_runs_with_its_windows_on_its_workspace() {
        let mut s = State::new(crate::Init { device_session: true, ..init() });
        let shelf = win(1, windows::SHELF_APP_ID, &Instance::Shelf.name(), true);
        s.apply(Event::Windows(vec![shelf.clone()]));
        // A running entry's (its process in the entry's scope): its menu.
        let zelda = launch(&mut s, "zelda-lttp");
        let zelda_icon = item(":1.5/a", "zelda3", zelda.unit().as_deref());
        s.apply(Event::Tray(vec![zelda_icon.clone()]));
        assert_eq!(s.tray_of("zelda-lttp").map(|t| &*t.key), Some(":1.5/a"));
        assert_eq!(s.session().running_entries().len(), 1);
        // A program not started by ClamShell (Firefox's desktop file named as its id): it runs as that entry.
        let firefox_icon = item(":1.9/b", "firefox", None);
        s.apply(Event::Tray(vec![zelda_icon.clone(), firefox_icon.clone()]));
        let r = s.session().running("desktop:firefox").unwrap();
        assert_eq!((r.origin, r.pid), (Origin::Tray, Some(77)));
        let ws = r.workspace();
        assert!(s.dock().items().iter().any(|i| &*i.id == "desktop:firefox"), "on the dock");
        // No window: a tap asks it to show itself; its window, opening on shelf's workspace (its process a
        // child of the icon's), goes to its own, and is shown there.
        let at = s.dock().items().iter().position(|i| &*i.id == "desktop:firefox").unwrap() as u16;
        assert_eq!(tap_dock(&mut s, DockId::Entry(at)), vec![Effect::Tray(TrayCommand::Activate(":1.9/b".into()))]);
        let window = Window { pids: vec![900, 77], ..win(6, "firefox", &Instance::Shelf.name(), true) };
        let (_, fx) = s.apply(Event::Windows(vec![shelf.clone(), window.clone()]));
        assert!(fx.contains(&Effect::Window(WindowCommand::Move { windows: vec![6], to: ws.clone() })), "{fx:?}");
        let moved = Window { workspace: ws.clone(), ..window };
        let (_, fx) = s.apply(Event::Windows(vec![shelf.clone(), moved.clone()]));
        assert!(fx.contains(&Effect::Window(WindowCommand::Focus(ws.clone()))), "{fx:?}");
        // With its window open, a tap shows it.
        assert_eq!(tap_dock(&mut s, DockId::Entry(at)), vec![Effect::Window(WindowCommand::Focus(ws.clone()))]);
        // Its corner mark: its menu; a submenu opens, Back goes up, an entry is clicked and it closes.
        tap_dock(&mut s, DockId::Tray(at));
        assert_eq!(s.tray_page().map(|(_, e)| e.len()), Some(3));
        tap_dock(&mut s, DockId::TrayEntry(2));
        assert_eq!(s.tray_page().map(|(_, e)| e[0].label.as_str()), Some("Zelda"));
        tap_dock(&mut s, DockId::Back);
        assert_eq!(s.tray_page().map(|(_, e)| e.len()), Some(3));
        let closed = Effect::Watch(crate::effect::Watch::Battery, false);
        assert_eq!(tap_dock(&mut s, DockId::TrayEntry(4)), vec![Effect::Tray(TrayCommand::Click { item: ":1.9/b".into(), id: 4 }), closed]);
        assert!(s.dock().menu().is_none());
        // Quit: its windows closed, then its process asked to end. Force kill: killed.
        let id: Arc<str> = "desktop:firefox".into();
        assert_eq!(s.quit(&id), vec![Effect::Window(WindowCommand::Close(vec![6]))]);
        let (_, fx) = s.apply(Event::Windows(vec![shelf.clone()]));
        assert!(fx.contains(&Effect::EndProcesses { pids: vec![77], force: false }), "{fx:?}");
        assert_eq!(s.force_kill("desktop:firefox"), vec![Effect::EndProcesses { pids: vec![77], force: true }]);
        // Its icon gone (and no window): ended.
        s.apply(Event::Tray(vec![zelda_icon.clone()]));
        assert!(s.session().running("desktop:firefox").is_none());
        // One the library has no desktop file for: an entry of its own, named as the icon.
        s.apply(Event::Tray(vec![zelda_icon, item(":1.7/c", "thing", None)]));
        assert_eq!(s.game("tray::1.7/c").map(|g| g.title.as_str()), Some("thing"));
        assert!(s.dock().items().iter().any(|i| &*i.id == "tray::1.7/c"));
    }

    #[test]
    fn a_guest_in_a_desktops_session_adopts_none_of_its_programs() {
        let mut s = state();
        let zelda = launch(&mut s, "zelda-lttp");
        s.apply(Event::Tray(vec![item(":1.5/a", "zelda3", zelda.unit().as_deref()), item(":1.9/b", "firefox", None)]));
        assert!(s.session().running("desktop:firefox").is_none(), "the desktop's");
        assert!(s.tray_of("zelda-lttp").is_some(), "its own launch's icon: its menu");
    }
}
