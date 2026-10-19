//! The dock: the entries pinned there, in the user's order (saved), and those running,
//! added at the end as they start and gone again when they stop (unless pinned). A pinned entry not
//! running shows dimmed; a tap starts it. A tap on a running one shows its workspace (all its windows on
//! the bottom screen: shows them there). A long press opens its menu: Open another (another launch,
//! another tab; an app's only, see [`crate::Game::another`]), Close window (the one in view), Pin or Unpin, Move (the D-pad moves it, A or B ends),
//! Quit and Force kill; a stopped one's: Unpin and Move. An entry with a tray icon (see [`crate::tray`])
//! runs, and has Menu: its program's own menu, as a page of the same kind (a submenu another).

use std::sync::Arc;

use crate::State;
use crate::effect::{Effect, Save, WindowCommand};
use crate::id::DockId;
use crate::keymap::Dir;
use crate::windows::{SHELF_APP_ID, Window};

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
    pub(crate) menu: Option<Arc<str>>,
    /// The entry being moved (the D-pad moves it).
    pub(crate) moving: Option<Arc<str>>,
    /// The menu's entry's tray menu open instead of its own: the submenus open in it (none: the top).
    pub(crate) tray: Option<Vec<i32>>,
}

impl Dock {
    /// The pinned entries, in order, as saved.
    pub fn pinned(ids: Vec<Arc<str>>) -> Dock {
        Dock { items: ids.into_iter().map(|id| DockItem { id, pinned: true }).collect(), ..Dock::default() }
    }

    /// The pinned entries' ids, in order (what is saved).
    pub fn pinned_ids(&self) -> Vec<Arc<str>> {
        self.items.iter().filter(|i| i.pinned).map(|i| i.id.clone()).collect()
    }

    pub fn items(&self) -> &[DockItem] {
        &self.items
    }

    /// The entry whose menu is open.
    pub fn menu(&self) -> Option<&Arc<str>> {
        self.menu.as_ref()
    }

    /// Whether the menu shows its entry's tray menu (see [`crate::tray`]).
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
}

impl State {
    pub fn dock(&self) -> &Dock {
        &self.dock
    }

    /// Whether a dock entry runs: as ClamShell's, or by its tray icon.
    pub fn runs(&self, id: &str) -> bool {
        self.session.running(id).is_some() || self.tray_of(id).is_some()
    }

    /// After anything that may start or stop an entry: those running (tray icons' own tiles too) and not
    /// on the dock added at its end; those stopped and not pinned (nor being moved) gone. Whether it
    /// changed.
    pub(crate) fn follow_dock(&mut self) -> bool {
        let before = self.dock.items.len();
        let moving = self.dock.moving.clone();
        let keep: Vec<bool> = self.dock.items.iter().map(|i| i.pinned || self.runs(&i.id) || moving.as_ref() == Some(&i.id)).collect();
        let mut keep = keep.into_iter();
        self.dock.items.retain(|_| keep.next().unwrap_or(true));
        let removed = before != self.dock.items.len();
        let session = &self.session;
        let mut new: Vec<Arc<str>> = session.running.iter().filter(|r| self.dock.position(&r.id).is_none()).map(|r| r.id.clone()).collect();
        new.extend(self.tray_tile_ids().into_iter().filter(|id| self.dock.position(id).is_none()));
        let added = !new.is_empty();
        self.dock.items.extend(new.into_iter().map(|id| DockItem { id, pinned: false }));
        removed || added
    }

    /// An entry's window in view: in front on the top screen, else shown there, else (nothing of it on
    /// top) shown on the bottom screen. For the menu's Close window.
    pub fn entry_window(&self, id: &str) -> Option<i64> {
        let session = &self.session;
        let mine = |w: &&Window| session.running_of(w).is_some_and(|r| &*r.id == id);
        let windows = &session.windows;
        let top = |w: &&Window| !w.down && mine(w);
        let below = |w: &&Window| w.down && mine(w);
        let on_top = windows.iter().filter(top).find(|w| w.focused).or_else(|| windows.iter().filter(top).find(|w| w.visible));
        let shown = on_top.or_else(|| windows.iter().find(top).is_none().then(|| windows.iter().filter(below).find(|w| w.visible)).flatten());
        shown.map(|w| w.id)
    }

    /// A tap or long press on a dock tile, a choice in its menu (which then closes), or a step in move
    /// mode.
    pub(crate) fn on_dock(&mut self, id: DockId, long: bool) -> Vec<Effect> {
        match id {
            DockId::Entry(i) => {
                let Some(entry) = self.dock.items.get(usize::from(i)).map(|item| item.id.clone()) else {
                    return vec![];
                };
                if long {
                    self.dock.menu = Some(entry);
                    self.dock.tray = None;
                    return vec![];
                }
                self.tap_entry(entry)
            }
            DockId::Tray(i) => {
                if let Some(entry) = self.dock.items.get(usize::from(i)).map(|item| item.id.clone()) {
                    self.dock.menu = Some(entry);
                    self.dock.tray = Some(vec![]);
                }
                vec![]
            }
            DockId::TrayMenu => {
                if self.dock.menu.is_some() {
                    self.dock.tray = Some(vec![]);
                }
                vec![]
            }
            DockId::TrayEntry(id) => self.tray_choice(id),
            // Back in a submenu: the menu above it.
            DockId::Back if self.dock.tray.as_ref().is_some_and(|path| !path.is_empty()) => {
                if let Some(path) = &mut self.dock.tray {
                    path.pop();
                }
                vec![]
            }
            DockId::Step(dir) => self.step(dir),
            DockId::Done => {
                self.dock.moving = None;
                self.follow_dock();
                vec![]
            }
            choice => {
                self.dock.tray = None;
                let Some(entry) = self.dock.menu.take() else {
                    return vec![];
                };
                self.menu_choice(choice, entry)
            }
        }
    }

    /// A tap on a dock entry: a stopped one starts; a running one is shown (all its windows on the bottom
    /// screen: shown there).
    fn tap_entry(&mut self, entry: Arc<str>) -> Vec<Effect> {
        let session = &self.session;
        let Some(r) = session.running(&entry) else {
            // (Its program runs, not as ours: shown by itself.)
            if let Some(show) = self.activate_tray(&entry) {
                return vec![show];
            }
            return self.play(entry);
        };
        let mine = |w: &&Window| session.running_of(w).is_some_and(|o| o.id == r.id);
        if !session.windows.iter().filter(mine).any(|w| !w.down)
            && let Some(down) = session.windows.iter().filter(mine).find(|w| w.down)
        {
            return self.show_program_down(down.id);
        }
        self.show_running(&entry)
    }

    fn menu_choice(&mut self, choice: DockId, entry: Arc<str>) -> Vec<Effect> {
        match choice {
            DockId::OpenAnother if self.opens_another(&entry) => self.launch(entry),
            DockId::OpenAnother => vec![],
            DockId::CloseWindow => self.entry_window(&entry).map(|w| vec![Effect::Window(WindowCommand::Close(vec![w]))]).unwrap_or_default(),
            DockId::Pin => self.toggle_pin(entry),
            DockId::Move => {
                self.dock.moving = Some(entry);
                vec![]
            }
            DockId::Quit if self.session.running(&entry).is_none() => self.end_tray(&entry, false),
            DockId::ForceKill if self.session.running(&entry).is_none() => self.end_tray(&entry, true),
            DockId::Quit => self.quit(&entry),
            DockId::ForceKill => self.force_kill(&entry),
            _ => vec![],
        }
    }

    /// The pins of entries the library doesn't have (and whose source isn't away) dropped, unless they
    /// run. Whether any were.
    pub(crate) fn drop_gone_pins(&mut self) -> bool {
        let before = self.dock.items.len();
        let (library, session) = (self.shelf.library(), &self.session);
        self.dock.items.retain(|i| library.has(&i.id) || session.running(&i.id).is_some());
        self.dock.items.len() != before
    }

    /// Pins `entry` to the dock (at the end, if it isn't there), or unpins it (gone, unless it runs); the
    /// pins saved.
    pub(crate) fn toggle_pin(&mut self, entry: Arc<str>) -> Vec<Effect> {
        match self.dock.position(&entry).and_then(|i| self.dock.items.get_mut(i)) {
            Some(item) => item.pinned = !item.pinned,
            None => self.dock.items.push(DockItem { id: entry, pinned: true }),
        }
        self.follow_dock();
        vec![Effect::Save(Save::Dock(self.dock.pinned_ids()))]
    }

    /// Move mode: the entry one place along (left, right) or a row (up, down); the order saved.
    fn step(&mut self, dir: Dir) -> Vec<Effect> {
        let Some(at) = self.dock.moving.as_ref().and_then(|id| self.dock.position(id)) else {
            return vec![];
        };
        let last = self.dock.items.len().saturating_sub(1);
        let to = match dir {
            Dir::Left => at.saturating_sub(1),
            Dir::Right => (at + 1).min(last),
            Dir::Up => at.saturating_sub(DOCK_COLUMNS),
            Dir::Down => (at + DOCK_COLUMNS).min(last),
        };
        if to == at {
            return vec![];
        }
        let item = self.dock.items.remove(at);
        self.dock.items.insert(to, item);
        vec![Effect::Save(Save::Dock(self.dock.pinned_ids()))]
    }

    /// The window Home with B acts on: the one in front on the top screen (not shelf's); with none
    /// there, the one in view on the bottom screen while it shows the windows moved there.
    fn home_b_window(&self) -> Option<&Window> {
        let windows = &self.session.windows;
        windows
            .iter()
            .find(|w| w.focused && !w.down && w.app_id != SHELF_APP_ID)
            .or_else(|| self.bottom_shows_windows().then(|| windows.iter().filter(|w| w.down).max_by_key(|w| w.visible)).flatten())
    }

    /// Home with B, let go at once: that window (see [`State::home_b_window`]) closes, politely.
    pub(crate) fn close_front_window(&self) -> Vec<Effect> {
        match self.home_b_window() {
            Some(w) => vec![Effect::Window(WindowCommand::Close(vec![w.id]))],
            None => vec![],
        }
    }

    /// Home with B, held: that window's entry quits, as its menu's Quit would; any other window just
    /// closes.
    pub(crate) fn quit_front(&mut self) -> Vec<Effect> {
        let Some(w) = self.home_b_window() else {
            return vec![];
        };
        let (id, entry) = (w.id, self.session.running_of(w).map(|r| r.id.clone()));
        match entry {
            Some(entry) => self.quit(&entry),
            None => vec![Effect::Window(WindowCommand::Close(vec![id]))],
        }
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use crate::testkit::*;
    use crate::*;

    fn entry(s: &State, i: usize) -> Option<&str> {
        s.dock().items().get(i).map(|item| &*item.id)
    }

    fn dock(i: u16) -> Id {
        Id::Dock(DockId::Entry(i))
    }

    #[test]
    fn running_entries_join_the_dock_pinned_ones_stay() {
        let mut s = state();
        let zelda = launch(&mut s, "zelda-lttp");
        assert_eq!(entry(&s, 0), Some("zelda-lttp"), "running: on the dock");
        // A tap without a window of it to show: what is in front stays, a notice says so.
        assert_eq!(tap(&mut s, dock(0)), vec![]);
        assert!(s.bottom().notice().is_some());
        // With one, a tap shows it; its menu pins it.
        s.apply(Event::Windows(vec![win(3, "zelda3", &zelda.workspace(), false)]));
        assert_eq!(tap(&mut s, dock(0)), vec![focus(&zelda)]);
        long(&mut s, dock(0));
        assert_eq!(s.dock().menu.as_deref(), Some("zelda-lttp"));
        let fx = tap(&mut s, Id::Dock(DockId::Pin));
        assert!(matches!(fx.as_slice(), [Effect::Save(Save::Dock(ids))] if ids.last().is_some_and(|i| &**i == "zelda-lttp")), "{fx:?}");
        // It stops: still there (pinned), a tap starts it.
        played(&mut s, "zelda-lttp");
        assert_eq!(entry(&s, 0), Some("zelda-lttp"));
        assert!(tap(&mut s, dock(0)).iter().any(|e| matches!(e, Effect::Launch { .. })));
        // Unpinned while stopped: gone.
        s.apply(GameEvent::LaunchFailed("zelda-lttp".into()).into());
        long(&mut s, dock(0));
        tap(&mut s, Id::Dock(DockId::Pin));
        assert_eq!(entry(&s, 0), None);
    }

    #[test]
    fn shelf_pins_and_unpins_any_entry_a_series_too() {
        let mut s = state();
        tap(&mut s, Id::Shelf(ShelfId::Collection(2)));
        let fx = s.apply(ShelfEvent::TogglePin.into()).1;
        assert!(matches!(fx.as_slice(), [Effect::Save(Save::Dock(ids))] if ids.iter().map(|i| &**i).eq(["tv:Cosmos"])), "{fx:?}");
        assert_eq!(entry(&s, 0), Some("tv:Cosmos"));
        assert!(s.dock().is_pinned("tv:Cosmos"));
        // A tap on it opens it in shelf.
        tap(&mut s, Id::Shelf(ShelfId::Collection(0)));
        assert_eq!(tap(&mut s, dock(0)), vec![Effect::Window(WindowCommand::ShowShelf)]);
        assert_eq!(s.shelf().group().map(|g| &*g.id), Some("tv:Cosmos"));
        // Unpinned from shelf: gone (it doesn't run).
        tap(&mut s, Id::Shelf(ShelfId::Up));
        s.apply(ShelfEvent::TogglePin.into());
        assert_eq!(entry(&s, 0), None);
    }

    #[test]
    fn pins_of_entries_gone_are_dropped_but_not_those_away() {
        let mut library = clamshell_library::demo_library();
        library.away = vec!["sd:".into()];
        let dock = dock::Dock::pinned(vec!["ff6".into(), "gone:x".into(), "sd:Zelda.sfc".into()]);
        let mut s = State::new(Init { library, dock, ..init() });
        assert_eq!(s.dock().pinned_ids(), vec![std::sync::Arc::from("ff6"), "sd:Zelda.sfc".into()], "the gone one dropped; the away one kept");
        let fx = s.apply(Event::Start).1;
        assert!(fx.iter().any(|e| matches!(e, Effect::Save(Save::Dock(ids)) if ids.len() == 2)), "saved: {fx:?}");
        // A rescan without ff6: dropped then too.
        let mut library = clamshell_library::demo_library();
        library.games.retain(|g| &*g.id != "ff6");
        library.collections.iter_mut().for_each(|c| c.games.clear());
        let fx = s.apply(Event::Library(Box::new(library))).1;
        assert!(fx.iter().any(|e| matches!(e, Effect::Save(Save::Dock(ids)) if ids.is_empty())), "{fx:?}");
    }

    #[test]
    fn move_mode_reorders_by_the_dpad_and_saves_the_pins() {
        let mut s = State::new(Init { dock: dock::Dock::pinned(vec!["zelda-lttp".into(), "ff6".into()]), ..init() });
        assert_eq!((entry(&s, 0), entry(&s, 1)), (Some("zelda-lttp"), Some("ff6")));
        long(&mut s, dock(0));
        tap(&mut s, Id::Dock(DockId::Move));
        let fx = tap(&mut s, Id::Dock(DockId::Step(keymap::Dir::Right)));
        assert!(matches!(fx.as_slice(), [Effect::Save(Save::Dock(ids))] if ids.iter().map(|i| &**i).eq(["ff6", "zelda-lttp"])), "{fx:?}");
        assert_eq!(tap(&mut s, Id::Dock(DockId::Step(keymap::Dir::Right))), vec![], "at the end");
        tap(&mut s, Id::Dock(DockId::Done));
        assert_eq!(s.dock().moving, None);
    }

    #[test]
    fn the_menu_opens_another_closes_a_window_quits_and_force_kills() {
        let mut s = state();
        let zelda = launch(&mut s, "zelda-lttp");
        let w = Window { unit: zelda.unit(), visible: true, ..win(5, "zelda3", &zelda.workspace(), true) };
        s.apply(Event::Windows(vec![w]));
        let key = clamshell_library::key_of("zelda-lttp");
        long(&mut s, dock(0));
        assert!(!s.opens_another("zelda-lttp") && tap(&mut s, Id::Dock(DockId::OpenAnother)).is_empty(), "a game: not another");
        long(&mut s, dock(0));
        assert_eq!(tap(&mut s, Id::Dock(DockId::CloseWindow)), vec![Effect::Window(WindowCommand::Close(vec![5]))]);
        long(&mut s, dock(0));
        assert_eq!(tap(&mut s, Id::Dock(DockId::Quit)), vec![Effect::Window(WindowCommand::Close(vec![5]))]);
        long(&mut s, dock(0));
        assert_eq!(tap(&mut s, Id::Dock(DockId::ForceKill)), vec![Effect::EndRun { key, force: true }]);
        // Home with B: the window in front closes; held, its entry quits.
        assert_eq!(s.apply(Event::Device(hardware::DeviceKey::CloseWindow)).1, vec![Effect::Window(WindowCommand::Close(vec![5]))]);
        assert_eq!(s.apply(Event::Device(hardware::DeviceKey::Quit)).1, vec![Effect::Window(WindowCommand::Close(vec![5]))]);
    }

    #[test]
    fn home_b_closes_the_bottom_screens_window_with_none_in_front_above() {
        use crate::hardware::DeviceKey;
        let mut s = state();
        let shelf = |focused| win(1, windows::SHELF_APP_ID, &Instance::Shelf.name(), focused);
        s.apply(Event::Windows(vec![shelf(false), foot()]));
        s.apply(Event::Device(DeviceKey::WindowDown));
        s.apply(Event::Windows(vec![shelf(true), Window { down: true, visible: true, focused: false, ..foot() }]));
        assert!(s.bottom_shows_windows());
        assert_eq!(s.apply(Event::Device(DeviceKey::CloseWindow)).1, vec![Effect::Window(WindowCommand::Close(vec![foot().id]))]);
        // ClamShell's view down there instead: nothing to close.
        s.apply(Event::Device(DeviceKey::BottomView));
        assert_eq!(s.apply(Event::Device(DeviceKey::CloseWindow)).1, vec![]);
    }
}
