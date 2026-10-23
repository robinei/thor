//! Programs' tray icons (StatusNotifierItem, over D-Bus) and their menus, as the system reads them.
//!
//! An icon belongs to the running entry whose scope its process is in, if ClamShell started it. Otherwise,
//! in the device's own session, the program runs as an entry found by its icon ([`Origin::Found`]). That is
//! the library's desktop entry named like the icon's id, such as `desktop:steam` for Steam's `steam`, or else
//! an entry of its own (`tray:<id>`) with the icon as its picture. In a desktop session the program is left
//! to the desktop.
//!
//! The found program's process, and those it started, are moved into a scope of ours as a launch of the
//! entry. Once systemd lists that scope, the entry runs as a launch ([`Origin::Ours`]) and is paused and
//! ended through its slice. A process that a systemd service runs is not moved; it stays the service's
//! and is signalled directly.
//!
//! Like any running entry it is on the dock with a workspace of its own, and its windows (those of the
//! icon's process and the processes it started) go there wherever they open. A tap shows them, or, with
//! none open, asks the program to show itself. The tile's corner mark opens the program's menu, as does
//! Menu in its long-press menu. Quit closes its windows and then asks the icon's process to end; Force
//! kill kills it.

use std::path::PathBuf;
use std::sync::Arc;

use clamshell_library::ids;

use crate::effect::Effect;
use crate::out::Out;
use crate::session::found::found_entry;
use crate::session::instance::Instance;
use crate::session::running::{Origin, Running};
use crate::{Clock, Entry, Library, Session};

/// A tray icon.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TrayItem {
    /// What identifies it to the system: its D-Bus name and object path.
    pub key: Arc<str>,
    /// Its own id, such as `steam`.
    pub id: String,
    pub title: String,
    /// Its process, and that process's systemd unit, which is `clamshell-run-…scope` if it is ours.
    pub pid: Option<u32>,
    pub unit: Option<String>,
    /// Its icon's image file, if one was found.
    pub icon: Option<PathBuf>,
    /// Its menu's entries, leaving out separators and hidden ones.
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
    /// Asks the program to show itself, as when the icon is clicked.
    Activate(Arc<str>),
    /// Chooses an entry of its menu.
    Click { item: Arc<str>, id: i32 },
}

impl Session {
    /// Takes the tray icons after any change. Programs not started by ClamShell run as entries found by
    /// their icons, and end with them, or with their last window if one is still open.
    pub(crate) fn set_tray(&mut self, items: Vec<TrayItem>, clock: &Clock, library: &Library, out: &mut Out) {
        if self.tray != items {
            self.tray = items;
            self.find_by_tray(clock, library, out);
        }
    }

    /// Makes entries found by tray icons: one for each icon of a program not started by ClamShell, whose
    /// process is in no scope of ours, unless the icon already has one. This happens only in the device's
    /// session. In a desktop's session the programs are the desktop's, and ClamShell is a guest there that
    /// manages only what it started.
    fn find_by_tray(&mut self, clock: &Clock, library: &Library, out: &mut Out) {
        if !self.device_session {
            return;
        }
        let now = clock.mono();
        let mut new = Vec::new();
        for t in &self.tray {
            let ours = t.unit.as_deref().and_then(Instance::from_unit).is_some();
            let Some(pid) = t.pid.filter(|_| !ours) else {
                continue;
            };
            if self.running.iter().any(|r| r.tray().is_some_and(|k| *k == t.key)) {
                continue;
            }
            let entry = found_entry(library, &t.id, || {
                let title = if t.title.is_empty() { &t.id } else { &t.title };
                // It is named by the icon's own id, which is the same each time the program runs, so that
                // a pin or a setting stays with it. Without an id, the icon's D-Bus name is used.
                let name = if t.id.is_empty() { &*t.key } else { t.id.as_str() };
                Entry { icon: t.icon.clone(), app: true, ..Entry::new(&format!("{}{name}", ids::TRAY), title) }
            });
            // Its entry runs already. If it was found another way, by a window, the icon becomes that
            // entry's. If it is a launch of ours, the icon is already its own.
            if let Some(r) = self.running.iter_mut().find(|r| r.id == entry.id) {
                if let Origin::Found { pid: found_pid, tray } = &mut r.origin {
                    *tray = Some(t.key.clone());
                    *found_pid = found_pid.or(Some(pid));
                }
                continue;
            }
            new.push(Running::of(&entry, Origin::Found { pid: Some(pid), tray: Some(t.key.clone()) }, now));
        }
        self.start_found(new, &[], clock, out);
    }

    /// The tray icon of a running entry, if it has one: the icon of its scope's process, or the one it was
    /// found by.
    pub fn tray_of(&self, entry: &str) -> Option<&TrayItem> {
        let r = self.running(entry)?;
        let in_scope = |t: &&TrayItem| t.unit.as_deref().and_then(Instance::from_unit).is_some_and(|i| i.key() == Some(r.key.as_str()));
        self.tray.iter().find(|t| r.tray().is_some_and(|k| *k == t.key) || in_scope(t))
    }

    /// Shows a running entry that has a tray icon but no open window. The program is asked to show itself,
    /// and its window is shown when it comes, on the entry's workspace; it is moved there if it opens
    /// elsewhere. Returns whether the entry has a tray icon to ask.
    pub(crate) fn activate_tray(&mut self, entry: &str, clock: &Clock, out: &mut Out) -> bool {
        let Some(key) = self.tray_of(entry).map(|t| t.key.clone()) else {
            return false;
        };
        let Some(r) = self.running(entry) else {
            return false;
        };
        self.pending_show = Some(Instance::Run { key: r.key.clone(), secs: clock.unix(), n: 0 });
        out.effects.push(Effect::Tray(TrayCommand::Activate(key)));
        true
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::indexing_slicing)]
mod tests {
    use super::*;
    use crate::id::{DockId, Id};
    use crate::session::windows::{self, Window};
    use crate::testkit::*;
    use crate::{Event, State, WindowCommand};

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
        let mut s = started(crate::Init { device_session: true, ..init() });
        tap(&mut s, Id::Tab(crate::Tab::Dock));
        let shelf = win(1, windows::SHELF_APP_ID, &Instance::Shelf.name(), true);
        s.applied(Event::Windows(vec![shelf.clone()]));
        // The icon of a running entry, whose process is in the entry's scope, gives that entry its menu.
        let zelda = launch(&mut s, "zelda-lttp");
        let zelda_icon = item(":1.5/a", "zelda3", zelda.unit().as_deref());
        s.applied(Event::Tray(vec![zelda_icon.clone()]));
        assert_eq!(s.session().tray_of("zelda-lttp").map(|t| &*t.key), Some(":1.5/a"));
        assert_eq!(s.session().running_entries().len(), 1);
        // A program not started by ClamShell runs as the entry whose desktop file is named as its id,
        // here Firefox's.
        let firefox_icon = item(":1.9/b", "firefox", None);
        let fx = s.applied(Event::Tray(vec![zelda_icon.clone(), firefox_icon.clone()]));
        assert!(
            fx.iter()
                .any(|e| matches!(e, Effect::Adopt { pid: 77, instance } if instance.key() == Some(clamshell_library::key_of("desktop:firefox").as_str()))),
            "{fx:?}"
        );
        let r = s.session().running("desktop:firefox").unwrap();
        assert_eq!((r.found(), r.pid()), (true, Some(77)));
        let ws = r.workspace();
        assert!(s.dock().items().iter().any(|i| &*i.id == "desktop:firefox"), "on the dock");
        // With no window, a tap asks it to show itself. Its window opens on shelf's workspace, from a child
        // of the icon's process, and is moved to its own workspace and shown there.
        let at = s.dock().items().iter().position(|i| &*i.id == "desktop:firefox").unwrap() as u16;
        assert_eq!(tap_dock(&mut s, DockId::Entry(at)), vec![Effect::Tray(TrayCommand::Activate(":1.9/b".into()))]);
        let window = Window { pids: vec![900, 77], ..win(6, "firefox", &Instance::Shelf.name(), true) };
        let fx = s.applied(Event::Windows(vec![shelf.clone(), window.clone()]));
        assert!(fx.contains(&Effect::Window(WindowCommand::Move { windows: vec![6], to: ws.clone() })), "{fx:?}");
        let moved = Window { workspace: ws.clone(), ..window };
        let fx = s.applied(Event::Windows(vec![shelf.clone(), moved.clone()]));
        assert!(fx.contains(&Effect::Window(WindowCommand::Focus(ws.clone()))), "{fx:?}");
        // With its window open, a tap shows it.
        assert_eq!(tap_dock(&mut s, DockId::Entry(at)), vec![Effect::Window(WindowCommand::Focus(ws.clone()))]);
        // Its corner mark opens its menu. A submenu opens, Back goes up, and clicking an entry closes the
        // menu.
        tap_dock(&mut s, DockId::Tray(at));
        assert_eq!(s.dock().tray_page(s.session()).map(|(_, e)| e.len()), Some(3));
        tap_dock(&mut s, DockId::TrayEntry(2));
        assert_eq!(s.dock().tray_page(s.session()).map(|(_, e)| e[0].label.as_str()), Some("Zelda"));
        tap_dock(&mut s, DockId::Back);
        assert_eq!(s.dock().tray_page(s.session()).map(|(_, e)| e.len()), Some(3));
        let closed = Effect::Watch(crate::effect::Watch::Battery, false);
        assert_eq!(tap_dock(&mut s, DockId::TrayEntry(4)), vec![Effect::Tray(TrayCommand::Click { item: ":1.9/b".into(), id: 4 }), closed]);
        assert!(s.dock().menu().is_none());
        // Quit closes its windows, then asks its process to end. Force kill kills it.
        let id: Arc<str> = "desktop:firefox".into();
        assert_eq!(quit(&mut s, &id), vec![Effect::Window(WindowCommand::Close(vec![6]))]);
        let fx = s.applied(Event::Windows(vec![shelf.clone()]));
        assert!(fx.contains(&Effect::EndProcesses { pids: vec![77], force: false }), "{fx:?}");
        assert_eq!(force_kill(&mut s, "desktop:firefox"), vec![Effect::EndProcesses { pids: vec![77], force: true }]);
        // With its icon gone and no window, it has ended.
        s.applied(Event::Tray(vec![zelda_icon.clone()]));
        assert!(s.session().running("desktop:firefox").is_none());
        // A program the library has no desktop file for gets an entry of its own, named as the icon.
        s.applied(Event::Tray(vec![zelda_icon.clone(), item(":1.7/c", "thing", None)]));
        assert_eq!(s.entry("tray:thing").map(|g| g.title.as_str()), Some("thing"), "named by its id, the same each run");
        assert!(s.dock().items().iter().any(|i| &*i.id == "tray:thing"));
        // Once moved into a scope of ours and listed by systemd, it is a launch. It is ended through its
        // slice, and has ended once the scope is gone.
        let fx = s.applied(Event::Tray(vec![zelda_icon.clone(), firefox_icon]));
        let instance = fx.iter().find_map(|e| if let Effect::Adopt { instance, .. } = e { Some(instance.clone()) } else { None }).unwrap();
        s.applied(Event::Units(vec![zelda.clone(), instance.clone()]));
        assert_eq!(s.session().running("desktop:firefox").map(|r| (r.origin.clone(), r.pid())), Some((Origin::Ours, None)));
        assert_eq!(force_kill(&mut s, "desktop:firefox"), vec![Effect::EndRun { key: clamshell_library::key_of("desktop:firefox"), force: true }]);
        s.applied(Event::Units(vec![zelda]));
        assert!(s.session().running("desktop:firefox").is_none(), "its scope gone");
    }

    #[test]
    fn a_guest_in_a_desktops_session_adopts_none_of_its_programs() {
        let mut s = state();
        let zelda = launch(&mut s, "zelda-lttp");
        s.applied(Event::Tray(vec![item(":1.5/a", "zelda3", zelda.unit().as_deref()), item(":1.9/b", "firefox", None)]));
        assert!(s.session().running("desktop:firefox").is_none(), "the desktop's");
        assert!(s.session().tray_of("zelda-lttp").is_some(), "its own launch's icon: its menu");
    }
}
