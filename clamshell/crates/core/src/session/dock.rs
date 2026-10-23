//! The dock: the entries pinned there, in the user's saved order, and the running entries, added at the end
//! as they start and removed when they stop unless pinned. A pinned entry that isn't running shows dimmed,
//! and a tap starts it. A tap on a running entry shows its workspace, or, if all its windows are on the
//! bottom screen, shows them there.
//!
//! A long press opens the entry's menu:
//!
//! - Open another starts another launch in another tab, for apps only (see [`crate::Entry::another`]).
//! - Close window closes the one in view.
//! - Pin or Unpin.
//! - Move lets the D-pad move the entry; A or B ends it.
//! - Quit and Force kill.
//!
//! A stopped entry's menu has Unpin and Move. A running entry with a tray icon (see
//! [`crate::session::tray`]) also has Menu, which shows its program's own menu as a page of the same kind,
//! with each submenu as another page.

use std::sync::Arc;

use crate::effect::{Effect, Save, WindowCommand};
use crate::id::DockId;
use crate::keymap::Dir;
use crate::session::tray::{TrayCommand, TrayEntry, TrayItem};
use crate::session::windows::Window;
use crate::state::out::Out;
use crate::{EntryAct, Event, Session};

/// The dock's tiles in a row.
pub const DOCK_COLUMNS: usize = 4;

/// An entry on the dock.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DockItem {
    pub id: Arc<str>,
    pub pinned: bool,
}

/// The dock's entries, and what its menu or move mode is about.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Dock {
    items: Vec<DockItem>,
    /// The entry whose menu is open.
    menu: Option<Arc<str>>,
    /// The entry being moved with the D-pad.
    moving: Option<Arc<str>>,
    /// Set when the menu shows its entry's tray menu instead of its own. It holds the submenus open in it;
    /// empty is the top.
    tray: Option<Vec<i32>>,
    /// Pins of entries that no longer exist were dropped at start. The dock is saved once the UI is up.
    unsaved: bool,
}

impl Dock {
    /// The pinned entries, in order, as saved.
    pub fn pinned(ids: Vec<Arc<str>>) -> Dock {
        Dock { items: ids.into_iter().map(|id| DockItem { id, pinned: true }).collect(), ..Dock::default() }
    }

    /// The pinned entries' ids, in order, which is what is saved.
    pub fn pinned_ids(&self) -> impl Iterator<Item = &Arc<str>> {
        self.items.iter().filter(|i| i.pinned).map(|i| &i.id)
    }

    /// Saves the pinned entries.
    pub(crate) fn save_effect(&self) -> Effect {
        Effect::Save(Save::Dock(self.pinned_ids().cloned().collect()))
    }

    pub fn items(&self) -> &[DockItem] {
        &self.items
    }

    /// The entry whose menu is open.
    pub fn menu(&self) -> Option<&Arc<str>> {
        self.menu.as_ref()
    }

    /// Whether the menu shows its entry's tray menu (see [`crate::session::tray`]).
    pub fn tray_open(&self) -> bool {
        self.tray.is_some()
    }

    /// The entry being moved.
    pub fn moving(&self) -> Option<&Arc<str>> {
        self.moving.as_ref()
    }

    /// Whether `id` is pinned.
    pub fn is_pinned(&self, id: &str) -> bool {
        self.items.iter().any(|i| i.pinned && &*i.id == id)
    }

    fn position(&self, id: &str) -> Option<usize> {
        self.items.iter().position(|i| &*i.id == id)
    }

    /// Drops the pins of entries the library doesn't have, and whose source isn't away, unless they run.
    /// Returns whether any were dropped.
    pub(crate) fn drop_gone(&mut self, library: &crate::Library, session: &crate::Session) -> bool {
        let before = self.items.len();
        self.items.retain(|i| library.has(&i.id) || session.running(&i.id).is_some());
        self.items.len() != before
    }

    /// Drops the pins of entries gone at start (see [`Dock::drop_gone`]). The dock is then saved once the
    /// UI is up.
    pub(crate) fn drop_gone_at_start(&mut self, library: &crate::Library, session: &crate::Session) {
        self.unsaved = self.drop_gone(library, session);
    }

    /// Whether pins were dropped at start and not saved yet. Clears it.
    pub(crate) fn take_unsaved(&mut self) -> bool {
        std::mem::take(&mut self.unsaved)
    }
}

impl Dock {
    /// Closes the menu and ends move mode, as when the tab is left.
    pub(crate) fn close(&mut self) {
        self.menu = None;
        self.moving = None;
    }

    /// Runs after anything that may start or stop an entry. Running entries not on the dock are added at
    /// its end, and stopped ones that are neither pinned nor being moved are removed. Returns whether it
    /// changed.
    pub(crate) fn follow(&mut self, session: &Session) -> bool {
        let before = self.items.len();
        let moving = &self.moving;
        self.items.retain(|i| i.pinned || session.running(&i.id).is_some() || moving.as_ref() == Some(&i.id));
        let removed = before != self.items.len();
        let mut added = false;
        for r in session.running_entries() {
            if self.position(&r.id).is_none() {
                self.items.push(DockItem { id: r.id.clone(), pinned: false });
                added = true;
            }
        }
        removed || added
    }

    /// A tap or long press on a dock tile, a choice in its menu, which then closes, or a step in move mode.
    /// Returns the window to show on the bottom screen when a tap shows an entry whose windows are all
    /// there, since the bottom screen shows it.
    pub(crate) fn on(&mut self, id: DockId, long: bool, session: &Session, out: &mut Out) -> Option<i64> {
        match id {
            DockId::Entry(i) => {
                let entry = self.items.get(usize::from(i)).map(|item| item.id.clone())?;
                if long {
                    self.menu = Some(entry);
                    self.tray = None;
                    return None;
                }
                return tap(entry, session, out);
            }
            DockId::Tray(i) => {
                if let Some(entry) = self.items.get(usize::from(i)).map(|item| item.id.clone()) {
                    self.menu = Some(entry);
                    self.tray = Some(vec![]);
                }
            }
            DockId::TrayMenu => {
                if self.menu.is_some() {
                    self.tray = Some(vec![]);
                }
            }
            DockId::TrayEntry(id) => self.tray_choice(id, session, out),
            // Back in a submenu goes to the menu above it.
            DockId::Back if self.tray.as_ref().is_some_and(|path| !path.is_empty()) => {
                if let Some(path) = &mut self.tray {
                    path.pop();
                }
            }
            DockId::Step(dir) => self.step(dir, out),
            DockId::Done => {
                self.moving = None;
                self.follow(session);
            }
            choice => {
                self.tray = None;
                if let Some(entry) = self.menu.take() {
                    self.menu_choice(choice, entry, session, out);
                }
            }
        }
        None
    }

    fn menu_choice(&mut self, choice: DockId, entry: Arc<str>, session: &Session, out: &mut Out) {
        let act = match choice {
            DockId::OpenAnother => EntryAct::OpenAnother,
            DockId::CloseWindow => {
                if let Some(w) = session.entry_window(&entry) {
                    out.effects.push(Effect::Window(WindowCommand::Close(vec![w])));
                }
                return;
            }
            DockId::Pin => return self.toggle_pin(entry, session, out),
            DockId::Autostart => EntryAct::ToggleAutostart,
            // Resume shows it, which thaws it.
            DockId::Suspend if session.suspended(&entry) => EntryAct::Show,
            DockId::Suspend => EntryAct::Suspend,
            DockId::Move => {
                self.moving = Some(entry);
                return;
            }
            DockId::Quit => EntryAct::Quit,
            DockId::ForceKill => EntryAct::ForceKill,
            _ => return,
        };
        out.events.push(Event::Entry(entry, act));
    }

    /// Pins `entry` to the dock, at the end if it isn't there, or unpins it, which removes it unless it
    /// runs. The pins are saved.
    pub(crate) fn toggle_pin(&mut self, entry: Arc<str>, session: &Session, out: &mut Out) {
        match self.position(&entry).and_then(|i| self.items.get_mut(i)) {
            Some(item) => item.pinned = !item.pinned,
            None => self.items.push(DockItem { id: entry, pinned: true }),
        }
        self.follow(session);
        out.effects.push(self.save_effect());
    }

    /// In move mode, moves the entry one place left or right, or one row up or down, and saves the order.
    fn step(&mut self, dir: Dir, out: &mut Out) {
        let Some(at) = self.moving.as_ref().and_then(|id| self.position(id)) else {
            return;
        };
        let last = self.items.len().saturating_sub(1);
        let to = match dir {
            Dir::Left => at.saturating_sub(1),
            Dir::Right => (at + 1).min(last),
            Dir::Up => at.saturating_sub(DOCK_COLUMNS),
            Dir::Down => (at + DOCK_COLUMNS).min(last),
        };
        if to == at {
            return;
        }
        let item = self.items.remove(at);
        self.items.insert(to, item);
        out.effects.push(self.save_effect());
    }
}

impl Dock {
    /// The open menu page, if there is one: the entry's tray icon and the entries of the open submenu.
    pub fn tray_page<'a>(&self, session: &'a Session) -> Option<(&'a TrayItem, &'a [TrayEntry])> {
        let path = self.tray.as_ref()?;
        let item = session.tray_of(self.menu()?)?;
        let mut entries = item.menu.as_slice();
        for id in path {
            entries = entries.iter().find(|e| e.id == *id)?.children.as_slice();
        }
        Some((item, entries))
    }

    /// An entry of the open menu page was chosen. A submenu opens; anything else is clicked, and the menu
    /// closes. A disabled entry does nothing.
    pub(crate) fn tray_choice(&mut self, id: i32, session: &Session, out: &mut Out) {
        let Some((item, entries)) = self.tray_page(session) else {
            return;
        };
        let Some(entry) = entries.iter().find(|e| e.id == id).filter(|e| e.enabled) else {
            return;
        };
        if !entry.children.is_empty() {
            if let Some(path) = &mut self.tray {
                path.push(id);
            }
            return;
        }
        let click = Effect::Tray(TrayCommand::Click { item: item.key.clone(), id });
        self.menu = None;
        self.tray = None;
        out.effects.push(click);
    }
}

/// A tap on a dock entry. A stopped entry starts. A running one is shown, on the bottom screen if all its
/// windows are there: that window is returned.
fn tap(entry: Arc<str>, session: &Session, out: &mut Out) -> Option<i64> {
    if session.running(&entry).is_none() {
        out.events.push(Event::Entry(entry, EntryAct::Play));
        return None;
    }
    let below = |w: &&Window| w.down;
    let down = session.windows_of(&entry).all(|w| w.down).then(|| session.windows_of(&entry).find(below).map(|w| w.id)).flatten();
    if down.is_none() {
        out.events.push(Event::Entry(entry, EntryAct::Show));
    }
    down
}

impl Session {
    /// An entry's window in view, for the menu's Close window. It is the one in front on the top screen,
    /// or else one shown there, or else, with nothing of the entry on top, one shown on the bottom screen.
    pub fn entry_window(&self, id: &str) -> Option<i64> {
        let mine = || self.windows_of(id);
        let on_top = mine().filter(|w| !w.down).find(|w| w.focused).or_else(|| mine().filter(|w| !w.down).find(|w| w.visible));
        let shown = on_top.or_else(|| mine().all(|w| w.down).then(|| mine().filter(|w| w.down).find(|w| w.visible)).flatten());
        shown.map(|w| w.id)
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use crate::testkit::*;
    use crate::*;

    /// The effect of a menu closing, which stops reading what its entry costs.
    fn closed() -> Effect {
        Effect::Watch(crate::effect::Watch::Battery, false)
    }

    fn entry(s: &State, i: usize) -> Option<&str> {
        s.dock().items().get(i).map(|item| &*item.id)
    }

    fn dock(i: u16) -> Id {
        Id::Dock(DockId::Entry(i))
    }

    #[test]
    fn running_entries_join_the_dock_pinned_ones_stay() {
        let mut s = state();
        tap(&mut s, Id::Tab(crate::Tab::Dock));
        let zelda = launch(&mut s, "zelda-lttp");
        assert_eq!(entry(&s, 0), Some("zelda-lttp"), "running: on the dock");
        // A tap with no window of it to show leaves what is in front, and a notice says so.
        assert_eq!(tap(&mut s, dock(0)), vec![]);
        assert!(s.bottom().notice().is_some());
        // With a window, a tap shows it, and its menu pins it.
        s.applied(Event::Windows(vec![win(3, "zelda3", &zelda.workspace(), false)]));
        assert_eq!(tap(&mut s, dock(0)), vec![focus(&zelda)]);
        long(&mut s, dock(0));
        assert_eq!(s.dock().menu().map(|m| &**m), Some("zelda-lttp"));
        let fx = tap(&mut s, Id::Dock(DockId::Pin));
        assert!(matches!(fx.as_slice(), [Effect::Save(Save::Dock(ids)), _] if ids.last().is_some_and(|i| &**i == "zelda-lttp")), "{fx:?}");
        assert_eq!(fx.last(), Some(&closed()));
        // When it stops it stays, being pinned, and a tap starts it.
        end(&mut s, "zelda-lttp");
        assert_eq!(entry(&s, 0), Some("zelda-lttp"));
        assert!(tap(&mut s, dock(0)).iter().any(|e| matches!(e, Effect::Launch { .. })));
        // Unpinned while stopped, it is removed.
        s.applied(LaunchEvent::LaunchFailed("zelda-lttp".into()).into());
        long(&mut s, dock(0));
        tap(&mut s, Id::Dock(DockId::Pin));
        assert_eq!(entry(&s, 0), None);
    }

    #[test]
    fn shelf_pins_and_unpins_any_entry_a_series_too() {
        let mut s = state();
        tap(&mut s, Id::Shelf(ShelfId::Collection(2)));
        let fx = s.applied(ShelfEvent::TogglePin.into());
        assert!(matches!(fx.as_slice(), [Effect::Save(Save::Dock(ids))] if ids.iter().map(|i| &**i).eq(["tv:Cosmos"])), "{fx:?}");
        assert_eq!(entry(&s, 0), Some("tv:Cosmos"));
        assert!(s.dock().is_pinned("tv:Cosmos"));
        // A tap on it opens it in shelf.
        tap(&mut s, Id::Shelf(ShelfId::Collection(0)));
        assert_eq!(tap(&mut s, dock(0)), vec![Effect::Window(WindowCommand::ShowShelf)]);
        assert_eq!(s.shelf().group().map(|g| &*g.id), Some("tv:Cosmos"));
        // Unpinned from shelf, it is removed, since it doesn't run.
        tap(&mut s, Id::Shelf(ShelfId::Up));
        s.applied(ShelfEvent::TogglePin.into());
        assert_eq!(entry(&s, 0), None);
    }

    #[test]
    fn pins_of_entries_gone_are_dropped_but_not_those_away() {
        let mut library = clamshell_library::demo::library();
        library.away = vec!["sd:".into()];
        let dock = crate::session::dock::Dock::pinned(vec!["ff6".into(), "gone:x".into(), "sd:Zelda.sfc".into()]);
        let mut s = State::new(Init { library, dock, ..init() });
        assert!(s.dock().pinned_ids().map(|i| &**i).eq(["ff6", "sd:Zelda.sfc"]), "the gone one dropped; the away one kept");
        let fx = s.applied(Event::Start);
        assert!(fx.iter().any(|e| matches!(e, Effect::Save(Save::Dock(ids)) if ids.len() == 2)), "saved: {fx:?}");
        // A rescan without ff6 drops its pin too.
        let mut library = clamshell_library::demo::library();
        library.entries.retain(|g| &*g.id != "ff6");
        library.collections.iter_mut().for_each(|c| c.entries.clear());
        let fx = s.applied(Event::Library(Box::new(library)));
        assert!(fx.iter().any(|e| matches!(e, Effect::Save(Save::Dock(ids)) if ids.is_empty())), "{fx:?}");
    }

    #[test]
    fn move_mode_reorders_by_the_dpad_and_saves_the_pins() {
        let mut s = State::new(Init { dock: crate::session::dock::Dock::pinned(vec!["zelda-lttp".into(), "ff6".into()]), ..init() });
        assert_eq!((entry(&s, 0), entry(&s, 1)), (Some("zelda-lttp"), Some("ff6")));
        long(&mut s, dock(0));
        tap(&mut s, Id::Dock(DockId::Move));
        let fx = tap(&mut s, Id::Dock(DockId::Step(keymap::Dir::Right)));
        assert!(matches!(fx.as_slice(), [Effect::Save(Save::Dock(ids))] if ids.iter().map(|i| &**i).eq(["ff6", "zelda-lttp"])), "{fx:?}");
        assert_eq!(tap(&mut s, Id::Dock(DockId::Step(keymap::Dir::Right))), vec![], "at the end");
        tap(&mut s, Id::Dock(DockId::Done));
        assert_eq!(s.dock().moving(), None);
    }

    #[test]
    fn the_menu_opens_another_closes_a_window_quits_and_force_kills() {
        let mut s = state();
        tap(&mut s, Id::Tab(crate::Tab::Dock));
        let zelda = launch(&mut s, "zelda-lttp");
        let w = Window { unit: zelda.unit(), visible: true, ..win(5, "zelda3", &zelda.workspace(), true) };
        s.applied(Event::Windows(vec![w]));
        let key = clamshell_library::key_of("zelda-lttp");
        long(&mut s, dock(0));
        assert!(!s.opens_another("zelda-lttp") && tap(&mut s, Id::Dock(DockId::OpenAnother)) == vec![closed()], "a game: not another");
        long(&mut s, dock(0));
        assert_eq!(tap(&mut s, Id::Dock(DockId::CloseWindow)), vec![Effect::Window(WindowCommand::Close(vec![5])), closed()]);
        long(&mut s, dock(0));
        assert_eq!(tap(&mut s, Id::Dock(DockId::Quit)), vec![Effect::Window(WindowCommand::Close(vec![5])), closed()]);
        long(&mut s, dock(0));
        assert_eq!(tap(&mut s, Id::Dock(DockId::ForceKill)), vec![Effect::EndRun { key, force: true }, closed()]);
        // Home with B closes the window in front; held, it quits the window's entry.
        assert_eq!(s.applied(Event::Device(crate::device::buttons::DeviceKey::CloseWindow)), vec![Effect::Window(WindowCommand::Close(vec![5]))]);
        assert_eq!(s.applied(Event::Device(crate::device::buttons::DeviceKey::Quit)), vec![Effect::Window(WindowCommand::Close(vec![5]))]);
    }

    #[test]
    fn home_b_closes_the_bottom_screens_window_with_none_in_front_above() {
        use crate::device::buttons::DeviceKey;
        let mut s = state();
        s.applied(Event::Windows(vec![shelf_window(false), foot()]));
        s.applied(Event::Device(DeviceKey::WindowDown));
        s.applied(Event::Windows(vec![shelf_window(true), Window { down: true, visible: true, focused: false, ..foot() }]));
        assert!(s.bottom().shows_windows(s.session()));
        assert_eq!(s.applied(Event::Device(DeviceKey::CloseWindow)), vec![Effect::Window(WindowCommand::Close(vec![foot().id]))]);
        // With ClamShell's view on the bottom screen instead, there is nothing to close.
        s.applied(Event::Device(DeviceKey::BottomView));
        assert_eq!(s.applied(Event::Device(DeviceKey::CloseWindow)), vec![]);
    }
}
