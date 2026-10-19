//! The program's state and its rules. No I/O: every change comes in as an [`Event`], and what the outside
//! world must do comes back as [`Effect`]s. The UI and the system threads only read the state and send
//! events, so all of this is unit-testable.
//!
//! The state is grouped by what it is about: shelf ([`Shelf`]), the second screen ([`Bottom`]), what runs
//! and is in front ([`Session`]), the dock ([`Dock`]), volume and brightness ([`Levels`]), the
//! radios ([`Radios`]), the controller ([`Pad`]), focus and presses ([`Pointer`]), and the system's
//! readings ([`Sys`]). Each owns its own rules; what involves two of them is [`State`]'s.

pub mod actions;
pub mod autostart;
pub mod background;
pub mod battery;
pub mod bluetooth;
pub mod bottom_screen;
pub mod dock;
pub mod editor;
mod effect;
pub mod enqueue;
mod event;
pub mod files;
pub mod hardware;
mod id;
pub mod idle;
pub mod instance;
mod items;
pub mod keyboard;
pub mod keymap;
pub mod keymap_screens;
pub mod levels;
pub mod metrics;
mod names;
mod pad;
mod panel;
mod pointer;
pub mod program;
mod radios;
mod resume;
mod running;
pub mod scroll;
mod session;
pub mod settings;
mod shelf;
pub mod slider;
mod sys;
mod tab;
pub mod time;
pub mod touch;
pub mod tray;
pub mod wifi;
pub mod windows;

use keyboard::{Key, Keyboard};
pub use resume::Resume;
use resume::{SAVE_AFTER, Saves};
use settings::{Setting, Settings};
use slider::Control;

pub use dock::{Dock, DockItem};
pub use editor::{Editor, count_env, parse_env};
pub use effect::{Dirty, Effect, Save, Watch, WindowCommand};
pub use event::{Event, GameEvent, LevelEvent, PointerEvent, Reading, ShelfEvent, Slide};
pub use id::{AudioId, BtId, DockId, EditorId, Hold, Id, KeymapId, SettingsId, ShelfId, Target, WifiId};
pub use instance::Instance;
pub use levels::{Brightness, Level, Levels, Sound};
pub use pad::{Claimant, Pad, PadInput, PadMouse, PadRoute};
pub use panel::{Scope, TileOrigin};
pub use pointer::Pointer;
pub use program::Program;
pub use radios::{RadioStatus, Radios};
pub use running::{Front, Launching, Origin, Running};
pub use scroll::{ListEvent, ListId, Lists, RowPart, Scroll};
pub use session::Session;
pub use shelf::{Cursor, Shelf};
pub use sys::{Status, Sys};
pub use tab::{Bottom, Tab};
pub use time::{ClockField, TimeId, TimePage, TimeRow, TimeZones};

pub use clamshell_library::{BrowseKind, Browsed, Collection, Game, Launch, Library, Meta, RunnerOf, runner_key};

/// A game's power profile: how fast CPU and GPU may run (frequency caps; the Thor has no TDP setting).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum PowerProfile {
    Battery,
    #[default]
    Balanced,
    Performance,
}

impl PowerProfile {
    pub const ALL: [PowerProfile; 3] = [PowerProfile::Battery, PowerProfile::Balanced, PowerProfile::Performance];

    pub fn label(self) -> &'static str {
        match self {
            PowerProfile::Battery => "Battery",
            PowerProfile::Balanced => "Balanced",
            PowerProfile::Performance => "Performance",
        }
    }

    /// The power profile a [`Setting::Power`] value stands for.
    pub fn from_value(v: u16) -> PowerProfile {
        match v {
            0 => PowerProfile::Battery,
            1 => PowerProfile::Balanced,
            _ => PowerProfile::Performance,
        }
    }
}

/// What the state starts from: what was read from disk and the environment.
#[derive(Debug, Default)]
pub struct Init {
    pub library: Library,
    /// Every game's settings and environment.
    pub settings: Settings,
    /// The keymaps.
    pub keymaps: keymap::Keymaps,
    /// The screens' brightness as it was left.
    pub brightness: Brightness,
    /// The dock's pinned entries, in order.
    pub dock: Dock,
    /// The entries started with ClamShell (see [`autostart`]).
    pub autostart: Vec<std::sync::Arc<str>>,
    /// Where shelf and the second screen were.
    pub resume: Option<Resume>,
    /// The tab to start in instead (a development option).
    pub start_tab: Option<Tab>,
    /// Start with the pad mouse on.
    pub pad_mouse: bool,
    /// ClamShell is the device's own session: it may put the device to sleep when idle.
    pub device_session: bool,
    /// Which programs' windows go to the bottom screen.
    pub window_rules: bottom_screen::WindowRules,
    /// Clocks on a 12-hour dial (else 24-hour).
    pub twelve_hour: bool,
}

#[derive(Debug, Default)]
pub struct State {
    shelf: Shelf,
    bottom: Bottom,
    session: Session,
    dock: Dock,
    /// Programs' tray icons (see [`tray`]).
    tray: Vec<tray::TrayItem>,
    /// The UI is up (its Start), and the watch in force (see [`State::follow_watch`]).
    ui_up: bool,
    watching: Option<Watch>,
    /// The battery tab's row whose menu is open (see [`battery`]).
    spender_menu: Option<battery::Group>,
    /// Programs paused or ended while hidden (see [`background`]).
    background: background::Background,
    /// The entries started with ClamShell, and whether they have been (see [`autostart`]).
    autostart: Vec<std::sync::Arc<str>>,
    autostarted: bool,
    levels: Levels,
    radios: Radios,
    pad: Pad,
    pointer: Pointer,
    sys: Sys,
    settings: Settings,
    keyboard: Keyboard,
    keymaps: keymap::Keymaps,
    /// The keymap in force as last sent to the pad.
    keymap_sent: Option<keymap::Keymap>,
    lists: Lists,
    unix_secs: u64,
    saves: Saves,
    idle: idle::Idle,
    /// ClamShell is the device's own session (see [`Init::device_session`]).
    device_session: bool,
    /// Which programs' windows go to the bottom screen.
    window_rules: bottom_screen::WindowRules,
    /// Clocks on a 12-hour dial (see [`crate::time`]).
    pub(crate) twelve_hour: bool,
    /// The clock was just stepped by hand: the next time read is a jump, not time gone by (idle starts
    /// from it).
    pub(crate) clock_stepped: bool,
    /// Metadata is being fetched; what shelf showed when it was last asked for first (see
    /// [`State::follow_shown`]).
    fetching: bool,
    shown: Option<(usize, Option<usize>, usize, usize, usize)>,
    /// Pins of entries gone were dropped at start: saved once the UI is up.
    pins_dropped: bool,
    /// The file actions' clipboard and confirmation, the folder browsed (see [`files`]).
    files: files::Files,
}

/// Replaces `slot` with `value`; whether that changed it.
pub(crate) fn replace<T: PartialEq>(slot: &mut T, value: T) -> bool {
    let changed = *slot != value;
    *slot = value;
    changed
}

impl State {
    pub fn new(init: Init) -> State {
        let mut s = State {
            shelf: Shelf::new(init.library),
            settings: init.settings,
            keymaps: init.keymaps,
            window_rules: init.window_rules,
            twelve_hour: init.twelve_hour,
            levels: Levels::new(init.brightness),
            dock: init.dock,
            autostart: init.autostart,
            device_session: init.device_session,
            ..State::default()
        };
        if let Some(resume) = &init.resume {
            s.restore(resume);
        }
        // (Saved once the UI is up.)
        s.pins_dropped = s.drop_gone_pins();
        s.follow_pausable_runners();
        s.pointer.focus = s.shelf.selected().cloned().map(|g| Id::Shelf(ShelfId::Game(g)));
        if let Some(tab) = init.start_tab {
            s.bottom.tab = tab;
        }
        s.reconcile_pad();
        s.keymap_sent = Some(s.keymap_in_force());
        if init.pad_mouse {
            s.pad.toggle_mouse();
            s.reconcile_pad();
        }
        s
    }

    pub fn shelf(&self) -> &Shelf {
        &self.shelf
    }

    pub fn bottom(&self) -> &Bottom {
        &self.bottom
    }

    pub fn session(&self) -> &Session {
        &self.session
    }

    pub fn levels(&self) -> &Levels {
        &self.levels
    }

    pub fn radios(&self) -> &Radios {
        &self.radios
    }

    pub fn pad(&self) -> &Pad {
        &self.pad
    }

    pub fn pointer(&self) -> &Pointer {
        &self.pointer
    }

    pub fn sys(&self) -> &Sys {
        &self.sys
    }

    pub fn settings(&self) -> &Settings {
        &self.settings
    }

    pub fn keyboard(&self) -> &Keyboard {
        &self.keyboard
    }

    pub fn idle(&self) -> &idle::Idle {
        &self.idle
    }

    /// Wall clock (epoch seconds), as last ticked.
    pub fn unix_secs(&self) -> u64 {
        self.unix_secs
    }

    /// A game of the library, by its id; one that runs and left it (its file deleted), as it was.
    /// Whether the dock menu offers Open another for `id`: it runs, and another launch of it makes sense
    /// (see [`Game::another`]).
    pub fn opens_another(&self, id: &str) -> bool {
        // (Not one handed over to another program, Android's: a launch shows the one it runs.)
        self.session.running(id).is_some() && self.game(id).is_some_and(|g| g.another && !g.runner.as_ref().is_some_and(|r| r.handover))
    }

    pub fn game(&self, id: &str) -> Option<&Game> {
        self.shelf.game(id).or_else(|| self.session.running(id).and_then(|r| r.entry.as_deref()))
    }

    /// Whether the pad, on ClamShell's screens, may go to shelf's (shelf is in front and nothing else
    /// has the pad), or only to the second screen.
    pub fn pad_on_top(&self) -> bool {
        self.pad.on_top()
    }

    /// Applies one event; returns which screens changed and what must happen outside. After the event
    /// itself, what follows from it everywhere: the pad mouse follows the game in front, the pad goes
    /// where it is wanted, and idle moves on (or back).
    pub fn apply(&mut self, event: Event) -> (Dirty, Vec<Effect>) {
        let (mut dirty, mut effects) = self.apply_event(event);
        effects.extend(self.follow_shown());
        effects.extend(self.follow_browse());
        effects.extend(self.follow_idle());
        effects.extend(self.follow_watch());
        let (changed, applied) = self.follow_program();
        effects.extend(applied);
        if changed && self.follow_pad_mouse() {
            dirty = Dirty::BOTH;
        }
        effects.extend(self.follow_keymap(false));
        if let Some(route) = self.reconcile_pad() {
            effects.push(Effect::PadRoute(route));
            // The focus ring shows where the pad is.
            if route != PadRoute::Ui && self.pointer.hide_focus() {
                dirty = Dirty::BOTH;
            }
        }
        (dirty, effects)
    }

    /// While metadata is fetched, what shelf shows changed: its entries' metadata first.
    fn follow_shown(&mut self) -> Option<Effect> {
        if !self.fetching {
            return None;
        }
        let view = self.shelf.view();
        if self.shown == Some(view) {
            return None;
        }
        self.shown = Some(view);
        Some(Effect::FetchFirst(self.shelf.shown()))
    }

    /// The program in force changed: the pad mouse on if it wants it (Setting::PadMouse), off again if it
    /// was the program's. Whether it changed.
    fn follow_pad_mouse(&mut self) -> bool {
        let wanted = self.session.program.is_some() && self.in_force().get(Setting::PadMouse) == 1;
        self.pad.follow_game(wanted)
    }

    /// Where the pad goes, as things are now; the new route, if it changed.
    fn reconcile_pad(&mut self) -> Option<PadRoute> {
        let listening = self.listening();
        let moving = self.dock.moving.is_some() && self.bottom.tab == Tab::Dock;
        self.pad.reconcile(self.session.front(), self.bottom.tab, self.pointer.show_focus, listening, moving)
    }

    fn apply_event(&mut self, event: Event) -> (Dirty, Vec<Effect>) {
        match event {
            Event::Tick { unix_secs } => self.tick(unix_secs),
            Event::Reading(reading) => {
                let status = matches!(reading, Reading::Status(_));
                let dirty = self.on_reading(reading);
                let effects = if status { self.sys.battery_warning().into_iter().collect() } else { vec![] };
                (if self.follow_lists() { Dirty::BOTTOM } else { dirty }, effects)
            }
            Event::List(e) => (if self.on_list(e) { e.list().screens() } else { Dirty::NONE }, vec![]),
            Event::Pointer(e) => self.on_pointer(e),
            Event::Slide(slide) => self.on_slide(slide),
            Event::Level(e) => self.on_level(e),
            Event::Shelf(e) => self.on_shelf_event(e),
            Event::Game(e) => self.on_game(e),
            Event::Windows(windows) => {
                let was = self.session.windows.iter().find(|w| w.focused && !w.down).map(|w| w.workspace.clone());
                let known: Vec<i64> = self.session.windows.iter().map(|w| w.id).collect();
                self.session.set_windows(windows);
                let handed = self.follow_windows(&known);
                let mut effects = self.follow_window_rules();
                effects.extend(self.follow_background());
                effects.extend(self.session.show_pending());
                effects.extend(self.session.show_reopened(self.unix_secs));
                effects.extend(self.follow_quitting());
                effects.extend(self.follow_closed(was));
                effects.extend(handed);
                self.follow_dock();
                (Dirty::BOTH, effects)
            }
            Event::Meta { id, meta } => (if self.shelf.apply_meta(&id, &meta) { Dirty::BOTH } else { Dirty::NONE }, vec![]),
            Event::Library(library) => {
                if self.shelf.reload(*library) {
                    self.moved_collection();
                }
                self.follow_pausable_runners();
                let mut effects = self.follow_runner_keymaps();
                effects.push(self.land_rules());
                if self.drop_gone_pins() {
                    effects.push(Effect::Save(Save::Dock(self.dock.pinned_ids())));
                }
                self.follow_dock();
                self.shown = None;
                (Dirty::BOTH, effects)
            }
            Event::Files(e) => (Dirty::BOTH, self.on_files(e)),
            Event::Fetching(on) => {
                self.fetching = on;
                self.shown = None;
                (Dirty::NONE, vec![])
            }
            Event::Tray(items) => {
                let Some(mut effects) = self.set_tray(items) else {
                    return (Dirty::NONE, vec![]);
                };
                let known: Vec<i64> = self.session.windows.iter().map(|w| w.id).collect();
                effects.extend(self.follow_windows(&known));
                effects.extend(self.session.show_pending());
                self.follow_dock();
                (Dirty::BOTH, effects)
            }
            Event::Units(units) => {
                self.session.set_units(units);
                let mut effects = self.follow_units();
                effects.extend(self.start_autostart());
                self.follow_dock();
                effects.extend(self.follow_quitting());
                (Dirty::BOTTOM, effects)
            }
            Event::Start => {
                // (The open tab's watch from now: see follow_watch.)
                self.ui_up = true;
                let mut effects: Vec<Effect> = Vec::new();
                if std::mem::take(&mut self.pins_dropped) {
                    effects.push(Effect::Save(Save::Dock(self.dock.pinned_ids())));
                }
                effects.extend(self.levels.brightness_effect());
                effects.push(Effect::PadRoute(self.pad.route()));
                effects.extend(self.follow_runner_keymaps());
                effects.extend(self.follow_keymap(true));
                effects.push(self.down_rules_effect());
                (Dirty::BOTH, effects)
            }
            // On a screen that listens, Home leaves it (it is never an input).
            Event::Home if self.listening() => {
                self.keymap_back();
                (Dirty::BOTTOM, vec![])
            }
            Event::Home => (Dirty::NONE, self.home()),
            Event::Listened(input) => (Dirty::BOTTOM, self.listened(input)),
            Event::PadSwitch => {
                self.pad.switch();
                (Dirty::NONE, vec![])
            }
            Event::Pad(_) => (Dirty::NONE, vec![]),
            Event::TextInput(active) => {
                let (before, pointing) = (self.bottom.tab, self.bottom.pointing);
                // (A field taking focus: typing is back.)
                self.bottom.pointing &= !active;
                let effects = self.text_input(active);
                let dirty = if self.bottom.tab != before {
                    Dirty::BOTH
                } else if self.bottom.pointing != pointing {
                    Dirty::BOTTOM
                } else {
                    Dirty::NONE
                };
                (dirty, effects)
            }
            Event::Pointing(pointing) => {
                let before = self.bottom.pointing;
                let effects = match pointing {
                    crate::touch::Pointing::Contacts(contacts) => {
                        self.bottom.pointing = true;
                        vec![Effect::Touchpad(contacts)]
                    }
                    crate::touch::Pointing::Idle => {
                        self.bottom.pointing = false;
                        vec![]
                    }
                };
                (if self.bottom.pointing == before { Dirty::NONE } else { Dirty::BOTTOM }, effects)
            }
            // Sleep may have reset what the settings set (frequency caps, cores): again.
            // Sleep may have reset what the settings set (frequency caps, cores): again.
            Event::Resumed => {
                self.idle.resumed(self.unix_secs);
                (Dirty::NONE, vec![Effect::Apply { game: self.session.program.clone(), settings: self.in_force() }])
            }
            Event::Idle(event) => {
                self.idle.heard(event, self.unix_secs);
                (Dirty::NONE, vec![])
            }
            Event::Device(key) => (Dirty::BOTTOM, self.on_device_key(key)),
            Event::Screenshot(result) => {
                self.on_screenshot(result);
                (Dirty::BOTTOM, vec![])
            }
            Event::GyroCalibrated(ok) => {
                self.notify(crate::hardware::NoticeKind::Gyro(Some(ok)));
                (Dirty::BOTTOM, vec![])
            }
            Event::Quit => (Dirty::NONE, vec![Effect::Save(Save::Resume(self.resume())), Effect::Quit]),
        }
    }

    /// The clock: once a minute the bar's clock and both screens' burn-in orbit step; starts systemd
    /// never listed are forgotten; what changed is saved once changes have stopped for a moment.
    fn tick(&mut self, unix_secs: u64) -> (Dirty, Vec<Effect>) {
        // (A clock stepped by hand moved: no input missed meanwhile.)
        if std::mem::take(&mut self.clock_stepped) {
            self.idle.own_input(unix_secs);
        }
        let minute_changed = unix_secs / 60 != self.unix_secs / 60;
        // The clock's first word: what idle heard before it was dated 0, as if decades ago. Count from now.
        if self.unix_secs == 0 {
            self.idle.resumed(unix_secs);
        }
        self.unix_secs = unix_secs;
        let mut effects = vec![];
        if self.saves.resume.due(unix_secs, SAVE_AFTER) {
            effects.push(Effect::Save(Save::Resume(self.resume())));
        }
        if self.saves.brightness.due(unix_secs, SAVE_AFTER) {
            effects.push(Effect::Save(Save::Brightness(self.levels.brightness().clone())));
        }
        let notice_gone = self.expire_notice(unix_secs);
        // (A file's launch that waited for its window in vain: off the dock.)
        let known: Vec<i64> = self.session.windows.iter().map(|w| w.id).collect();
        let before = self.session.running.len();
        effects.extend(self.follow_windows(&known));
        effects.extend(self.follow_background());
        let ended = self.session.running.len() != before;
        if ended {
            self.follow_dock();
        }
        let dirty = if minute_changed {
            Dirty::BOTH
        } else if notice_gone || ended {
            Dirty::BOTTOM
        } else {
            Dirty::NONE
        };
        (dirty, effects)
    }

    /// A reading of the system: kept, and redrawn if it changed what the second screen shows.
    fn on_reading(&mut self, reading: Reading) -> Dirty {
        let shown = self.bottom.tab.shows(&reading);
        let changed = match reading {
            Reading::Status(status) => self.read_status(status),
            // (The graphs move on with every reading.)
            Reading::Metrics(metrics) => {
                self.sys.read_metrics(metrics);
                true
            }
            Reading::Frames { fps, frame_ms } => {
                self.sys.frames(fps, frame_ms);
                true
            }
            Reading::Wifi(wifi) => replace(&mut self.radios.wifi, wifi),
            Reading::Bluetooth(bluetooth) => replace(&mut self.radios.bluetooth, bluetooth),
            Reading::Battery(battery) => replace(&mut self.sys.battery, battery),
            Reading::Audio(audio) => self.levels.read_audio(audio),
            Reading::Backlights(backlights) => self.levels.read_backlights(backlights),
            Reading::Time(time) => replace(&mut self.sys.time, time),
        };
        if changed && shown { Dirty::BOTTOM } else { Dirty::NONE }
    }

    /// A status reading: the volume, mute and brightness go to the levels (where the finger's value
    /// wins while it is on one). Whether what the bar shows changed.
    fn read_status(&mut self, status: Status) -> bool {
        let shown = |s: &State| Status { volume: s.levels.volume(), muted: s.levels.muted(), ..s.sys.status };
        let before = shown(self);
        self.sys.status = status;
        self.levels.read(status.volume, status.muted, status.brightness);
        shown(self) != before
    }

    fn on_pointer(&mut self, event: PointerEvent) -> (Dirty, Vec<Effect>) {
        // Input that reached ClamShell itself (the pad driving the screens).
        self.idle.own_input(self.unix_secs);
        match event {
            PointerEvent::Focus(id) => {
                self.show_focused(&id);
                self.pointer.focus = Some(id);
                self.pointer.show_focus = true;
                (Dirty::BOTH, vec![])
            }
            PointerEvent::HideFocus => (if self.pointer.hide_focus() { Dirty::BOTH } else { Dirty::NONE }, vec![]),
            // (A press that woke the screens only woke them.)
            PointerEvent::Activate(_) | PointerEvent::LongPress(_) if self.idle.swallows_press(self.unix_secs) => (Dirty::NONE, vec![]),
            PointerEvent::Activate(id) => self.activate(&id, false),
            PointerEvent::LongPress(id) => self.activate(&id, true),
            PointerEvent::Down(id) => {
                let dirty = id.screen();
                if let Id::Key(pos) = id {
                    self.keyboard.down(pos);
                }
                self.pointer.press = self.item_key(&id).map(|item| pointer::Press { id: id.clone(), item });
                self.pointer.pressed = Some(id);
                (dirty, vec![])
            }
            PointerEvent::Up(id) => {
                if self.pointer.pressed.as_ref() == Some(&id) {
                    self.pointer.pressed = None;
                }
                if Level::of(&id).is_some() {
                    self.levels.release();
                }
                if self.pointer.press.as_ref().is_some_and(|p| p.id == id) {
                    self.pointer.press = None;
                }
                if let Id::Key(pos) = id {
                    self.keyboard.up(pos);
                }
                (id.screen(), vec![])
            }
        }
    }

    /// A tap (or long press) on `id`, as the press that came down on it meant it: it gets the focus,
    /// and its subsystem acts on it.
    fn activate(&mut self, id: &Id, long: bool) -> (Dirty, Vec<Effect>) {
        // The list item the press came down on is gone.
        let Some(id) = self.resolve(id) else {
            return (Dirty::BOTTOM, vec![]);
        };
        self.pointer.focus = Some(id.clone());
        match id {
            // Held on the selected entry with the pad (its focus showing): the pad to its actions, the
            // first focused (up and down go through them, A does one, B goes back to the list).
            Id::Shelf(ShelfId::Game(game)) if long && self.pointer.show_focus && self.shelf.selected() == Some(&game) => {
                if let Some(first) = self.actions().as_slice().first() {
                    self.pointer.focus = Some(Id::Shelf(first.id()));
                }
                (Dirty::BOTH, vec![])
            }
            Id::Shelf(id) => (Dirty::BOTH, self.on_shelf(id)),
            Id::Dock(id) => (Dirty::BOTTOM, self.on_dock(id, long)),
            Id::Settings(id @ (SettingsId::Tile(_) | SettingsId::Reset)) => (Dirty::BOTH, self.on_settings(id, long)),
            Id::Settings(id) => (Dirty::BOTTOM, self.on_settings(id, long)),
            Id::Editor(id) => (Dirty::BOTTOM, self.on_editor(id)),
            Id::Wifi(id) => (Dirty::BOTTOM, self.on_wifi(id)),
            Id::Bluetooth(id) => (Dirty::BOTTOM, self.on_bluetooth(id)),
            Id::Audio(id) => (Dirty::BOTTOM, self.on_audio(id)),
            Id::Row { list, i, part } => (list.screens(), self.on_row(list, usize::from(i), part, long)),
            Id::Keymap(id) => (Dirty::BOTTOM, self.on_keymap(id, long)),
            Id::Tab(tab) => (Dirty::BOTH, self.choose_tab(tab)),
            // A tap opens the tab (a slide is the UI's: it doesn't end in a tap).
            Id::Control(control) if !long => {
                let tab = match control {
                    Control::Volume => Tab::Volume,
                    Control::Brightness => Tab::Brightness,
                    // (A keymap's parameter is held or slid, not tapped.)
                    Control::Param(_) => return (Dirty::NONE, vec![]),
                };
                (Dirty::BOTH, self.choose_tab(tab))
            }
            Id::Control(_) | Id::Level(_) | Id::List(_) | Id::Blank => (Dirty::NONE, vec![]),
            Id::Key(pos) => {
                let stroke = self.keyboard.press(pos, long);
                // Into the editor while it is open; with shelf in front, to finding an entry; else to the
                // focused app.
                let effects = match stroke {
                    Some(stroke) if self.bottom.editor().is_some() => self.edit(stroke),
                    Some(stroke) if self.capturing() => self.captured(State::chord(stroke)),
                    Some(stroke) if self.session.front() == running::Front::Shelf => return (Dirty::BOTH, self.find(stroke)),
                    Some(stroke) => vec![Effect::Key(stroke)],
                    None => vec![],
                };
                (Dirty::BOTTOM, effects)
            }
            Id::BottomWindows => match self.hidden_down() {
                Some(window) => (Dirty::BOTH, self.show_program_down(window)),
                None => (Dirty::NONE, vec![]),
            },
            Id::Time(id) => self.on_time(id),
            Id::Battery(id) => (Dirty::BOTTOM, self.on_battery(id)),
            Id::PadMouse => {
                self.pad.toggle_mouse();
                (Dirty::BOTTOM, vec![])
            }
            Id::Wake => {
                self.bottom.woken = true;
                (Dirty::BOTTOM, vec![])
            }
        }
    }

    /// What holding `id` does: keyboard keys repeat, modifiers and everything else get one long press.
    pub fn hold(&self, id: &Id) -> Hold {
        match id {
            Id::Key(pos) => match self.keyboard.effective(*pos) {
                Some(Key::Mod(_) | Key::Caps) | None => Hold::Long,
                Some(_) => Hold::Repeat,
            },
            _ => Hold::Long,
        }
    }

    /// Shelf's targets.
    fn on_shelf(&mut self, id: ShelfId) -> Vec<Effect> {
        match id {
            ShelfId::Game(game) => {
                // Activating the selected entry again opens it (a group) or plays it (starts it, or
                // switches to it).
                if self.shelf.selected() == Some(&game) {
                    return self.activate_selected();
                }
                if self.shelf.select(&game) {
                    self.saves.resume.touch(self.unix_secs);
                }
                vec![]
            }
            ShelfId::Collection(i) => {
                if self.shelf.set_collection(usize::from(i)) {
                    self.moved_collection();
                }
                vec![]
            }
            ShelfId::Launch => self.activate_selected(),
            ShelfId::Favourite => self.toggle_favourite(),
            ShelfId::Pin => match self.shelf.selected().cloned() {
                Some(id) => self.toggle_pin(id),
                None => vec![],
            },
            ShelfId::Continue => self.continue_selected(),
            ShelfId::Enqueue => self.enqueue_selected(),
            ShelfId::File(action) => self.on_file(action),
            ShelfId::Up => {
                if self.shelf.up() {
                    self.moved_collection();
                }
                vec![]
            }
        }
    }

    /// The selected group's Continue (see [`Library::to_continue`]): that entry played.
    fn continue_selected(&mut self) -> Vec<Effect> {
        let next = self.shelf.selected_index().and_then(|g| self.shelf.library().to_continue(g));
        match next.and_then(|i| self.shelf.library().games.get(i)).map(|g| g.id.clone()) {
            Some(id) => self.play(id),
            None => vec![],
        }
    }

    /// Shelf's action on its selected entry: a group opens, anything else plays.
    fn activate_selected(&mut self) -> Vec<Effect> {
        if self.shelf.open() {
            self.moved_collection();
            return vec![];
        }
        match self.shelf.selected().cloned() {
            Some(game) => self.play(game),
            None => vec![],
        }
    }

    /// Shelf went to another collection or list: the place is saved, and a focus in shelf's list follows.
    fn moved_collection(&mut self) {
        self.saves.resume.touch(self.unix_secs);
        if matches!(self.pointer.focus, Some(Id::Shelf(ShelfId::Game(_) | ShelfId::Collection(_)))) {
            self.pointer.focus = self.shelf.selected().cloned().map(|g| Id::Shelf(ShelfId::Game(g)));
        }
    }

    fn on_shelf_event(&mut self, event: ShelfEvent) -> (Dirty, Vec<Effect>) {
        match event {
            ShelfEvent::ListStep(step) => {
                if self.shelf.step(step) {
                    self.saves.resume.touch(self.unix_secs);
                }
                self.pointer.focus = self.shelf.selected().cloned().map(|g| Id::Shelf(ShelfId::Game(g)));
                self.pointer.show_focus = true;
                (Dirty::BOTH, vec![])
            }
            ShelfEvent::CollectionStep(step) => {
                if self.shelf.collection_step(step) {
                    self.moved_collection();
                }
                self.pointer.show_focus = true;
                (Dirty::BOTH, vec![])
            }
            ShelfEvent::Rows(rows) => (if self.shelf.set_rows(rows) { Dirty::BOTH } else { Dirty::NONE }, vec![]),
            // The details' actions on the pad's buttons (see [`actions`]).
            ShelfEvent::Continue => (Dirty::BOTH, self.act(actions::Shortcut::Start)),
            ShelfEvent::ToggleFavourite => (Dirty::BOTH, self.act(actions::Shortcut::Y)),
            ShelfEvent::TogglePin => (Dirty::BOTH, self.act(actions::Shortcut::X)),
            ShelfEvent::ClearFilter => {
                if self.shelf.set_filter(String::new()) {
                    self.moved_collection();
                }
                (Dirty::BOTH, vec![])
            }
        }
    }

    /// A key typed with shelf in front: a character narrows the current list to the titles starting with
    /// what is typed, Backspace takes the last one back, Esc clears it, Enter opens or plays the selected
    /// entry.
    fn find(&mut self, stroke: keyboard::Stroke) -> Vec<Effect> {
        let mut filter = self.shelf.filter().to_owned();
        match stroke.key {
            Key::Char(c) if !stroke.ctrl && !stroke.alt && !stroke.logo => filter.push(if stroke.shift { keyboard::shifted(c) } else { c }),
            Key::Named(keyboard::Named::Backspace) => {
                filter.pop();
            }
            Key::Named(keyboard::Named::Esc) => filter.clear(),
            Key::Named(keyboard::Named::Enter) => return self.activate_selected(),
            _ => return vec![],
        }
        if self.shelf.set_filter(filter) {
            self.moved_collection();
        }
        vec![]
    }

    /// The selected entry in or out of the favourites, saved.
    pub(crate) fn toggle_favourite(&mut self) -> Vec<Effect> {
        let Some(moved) = self.shelf.toggle_favourite() else {
            return vec![];
        };
        if moved {
            self.saves.resume.touch(self.unix_secs);
        }
        let ids = self.shelf.library().favourite_ids().map(str::to_owned).collect();
        vec![Effect::Save(Save::Favourites(ids))]
    }

    fn on_game(&mut self, event: GameEvent) -> (Dirty, Vec<Effect>) {
        let mut effects = match event {
            GameEvent::Started(game) => {
                self.started(game);
                vec![]
            }
            GameEvent::Ended(game) => self.ended(&game),
            GameEvent::Position { game, position } => {
                self.shelf.set_position(&game, position, self.unix_secs);
                vec![]
            }
            // (A launch that never started: nothing runs one less, but it is no longer starting.)
            GameEvent::LaunchFailed(game) => {
                self.launch_failed(game);
                vec![]
            }
        };
        self.follow_dock();
        effects.extend(self.follow_quitting());
        (Dirty::BOTH, effects)
    }

    /// The volume tab's buttons.
    fn on_audio(&mut self, id: AudioId) -> Vec<Effect> {
        match id {
            AudioId::MuteOutput => self.levels.toggle_mute(),
            AudioId::Output(i) => self.levels.choose_output(usize::from(i)),
        }
    }

    /// Changes the levels with `change`; a brightness changed is saved a moment later.
    fn change_levels(&mut self, change: impl FnOnce(&mut Levels) -> Vec<Effect>) -> Vec<Effect> {
        let before = self.levels.brightness().clone();
        let effects = change(&mut self.levels);
        if *self.levels.brightness() != before {
            self.saves.brightness.touch(self.unix_secs);
        }
        effects
    }

    fn on_slide(&mut self, slide: Slide) -> (Dirty, Vec<Effect>) {
        match slide {
            Slide::Start { control: Control::Param(param), x, width } => {
                self.param_slide_start(param, x, width);
                (Dirty::BOTTOM, vec![])
            }
            Slide::Start { control, x, width } => {
                self.levels.slide_start(control, x, width);
                (Dirty::BOTTOM, vec![])
            }
            Slide::To { x } if self.levels.slider().is_none() => (if self.param_slide_to(x) { Dirty::BOTTOM } else { Dirty::NONE }, vec![]),
            Slide::To { x } => {
                let effects = self.change_levels(|l| l.slide_to(x));
                (if effects.is_empty() { Dirty::NONE } else { Dirty::BOTTOM }, effects)
            }
            Slide::End => {
                self.levels.slide_end();
                (Dirty::BOTTOM, self.param_slide_end())
            }
        }
    }

    fn on_level(&mut self, event: LevelEvent) -> (Dirty, Vec<Effect>) {
        match event {
            LevelEvent::Set(level, value) => {
                let Some(level) = self.resolve_level(level) else {
                    return (Dirty::NONE, vec![]);
                };
                let before = self.levels.level(level);
                let effects = self.change_levels(|l| l.set(level, value));
                (if self.levels.level(level) == before { Dirty::NONE } else { Dirty::BOTTOM }, effects)
            }
            LevelEvent::Step(level, direction) => (Dirty::BOTTOM, self.change_levels(|l| l.step(level, direction))),
        }
    }

    /// The user chose a tab (its icon, a control's tap, the shoulder buttons): it stays, also over a
    /// keyboard a text field brought up.
    fn choose_tab(&mut self, tab: Tab) -> Vec<Effect> {
        if self.bottom.raised_from.take().is_some() {
            self.saves.resume.touch(self.unix_secs);
        }
        self.set_tab(tab)
    }

    /// Changes the second screen's tab (its watch follows: see [`State::follow_watch`]); a level being
    /// set ends. What the tab left had
    /// open (an editor, the keymaps' screens) stays for coming back to.
    fn set_tab(&mut self, tab: Tab) -> Vec<Effect> {
        if tab == self.bottom.tab {
            return vec![];
        }
        self.saves.resume.touch(self.unix_secs);
        self.bottom.tab = tab;
        self.dock.menu = None;
        self.dock.moving = None;
        self.levels.leave();
        // (The time tab opens on its main page again.)
        self.bottom.time_page = TimePage::Main;
        vec![]
    }

    /// What the second screen watches: the open tab's (the dock's, with an entry's menu open: what the
    /// entry costs, the battery tab's readings), while it can be seen (ClamShell's view on the bottom
    /// screen, that on, the screens on) and the UI is up. Its watch started and stopped as that changes.
    fn follow_watch(&mut self) -> Vec<Effect> {
        let seen = self.ui_up && !self.bottom_shows_windows() && !self.bottom_off() && self.idle.stage() < idle::Stage::Off;
        let tab = match self.bottom.tab {
            Tab::Dock if self.dock.menu.is_some() => Some(Watch::Battery),
            tab => tab.watch(),
        };
        let wanted = tab.filter(|_| seen);
        if wanted == self.watching {
            return vec![];
        }
        let was = std::mem::replace(&mut self.watching, wanted);
        was.map(|w| Effect::Watch(w, false)).into_iter().chain(wanted.map(|w| Effect::Watch(w, true))).collect()
    }

    /// A text field wants typing (or is done): the keyboard comes up, and goes back to the tab it came up
    /// from when the field is done, unless the user chose a tab meanwhile.
    fn text_input(&mut self, active: bool) -> Vec<Effect> {
        if active {
            // (Not over an open editor: what is typed there would be lost.)
            if self.bottom.tab == Tab::Keyboard || self.bottom.editor().is_some() {
                return vec![];
            }
            self.bottom.raised_from = Some(self.bottom.tab);
            return self.set_tab(Tab::Keyboard);
        }
        match self.bottom.raised_from.take() {
            Some(tab) if self.bottom.tab == Tab::Keyboard => self.set_tab(tab),
            _ => vec![],
        }
    }

    /// After the windows changed: the window in front (`was`, its workspace) closed, nothing of its
    /// workspace left (sway leaves the empty workspace in front): back to what was in front before it,
    /// shelf at the bottom of that; the workspace remembered for a window soon after (see
    /// [`session::Session::show_reopened`]).
    fn follow_closed(&mut self, was: Option<String>) -> Vec<Effect> {
        let windows = &self.session.windows;
        let Some(was) = was else { return vec![] };
        if windows.iter().any(|w| w.focused || w.workspace == was) {
            return vec![];
        }
        let shown = |ws: &String| windows.iter().any(|w| w.workspace == *ws && !w.down);
        let effects = match self.session.recent.iter().find(|ws| shown(ws)) {
            Some(ws) if !Instance::Shelf.is(ws) => vec![Effect::Window(WindowCommand::Focus(ws.clone()))],
            _ => vec![Effect::Window(WindowCommand::ShowShelf)],
        };
        self.session.closed_front = Some((was, self.unix_secs));
        effects
    }

    /// The home button: from anything to shelf; from shelf back to what was in front before it, or
    /// without that history (no compositor's word) to the program in force.
    fn home(&mut self) -> Vec<Effect> {
        let session = &self.session;
        let front = session.windows.iter().find(|w| w.focused);
        let shelf_in_front = front.is_none_or(|w| w.app_id == windows::SHELF_APP_ID);
        if !shelf_in_front {
            return vec![Effect::Window(WindowCommand::ShowShelf)];
        }
        // (Only where a window shows on the main screen: one moved down leaves its workspace empty.)
        let shown = |ws: &String| session.windows.iter().any(|w| w.workspace == *ws && !w.down);
        let before = session.recent.iter().find(|ws| !Instance::Shelf.is(ws) && shown(ws)).cloned();
        let game = session.program.as_deref().and_then(|id| session.running(id)).map(Running::workspace).filter(shown);
        match before.or(game) {
            Some(workspace) => vec![Effect::Window(WindowCommand::Focus(workspace))],
            None => vec![],
        }
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
pub(crate) mod testkit {
    //! Shorthands for the tests.

    pub use crate::settings::Values;
    pub use crate::windows::Window;

    use crate::*;

    /// What a test state starts from: the demo library, nothing else.
    pub fn init() -> Init {
        Init { library: clamshell_library::demo_library(), ..Init::default() }
    }

    /// (Up, as after the UI's Start: its watches in force.)
    pub fn state() -> State {
        state_with(init().library)
    }

    /// A test state with this library.
    pub fn state_with(library: Library) -> State {
        let mut s = State::new(Init { library, ..Init::default() });
        s.ui_up = true;
        let _ = s.follow_watch();
        s
    }

    pub fn win(id: i64, app_id: &str, workspace: &str, focused: bool) -> Window {
        Window { id, app_id: app_id.into(), title: String::new(), focused, unit: None, workspace: workspace.into(), ..Window::default() }
    }

    /// A terminal of the user's in front (not one of ours).
    pub fn foot() -> Window {
        win(1, "foot", "1", true)
    }

    /// A game in shelf's list.
    pub fn game(id: &str) -> Id {
        Id::Shelf(ShelfId::Game(id.into()))
    }

    /// A tap on `id`: what it asks for.
    pub fn tap(s: &mut State, id: Id) -> Vec<Effect> {
        s.apply(PointerEvent::Activate(id).into()).1
    }

    pub fn long(s: &mut State, id: Id) -> Vec<Effect> {
        s.apply(PointerEvent::LongPress(id).into()).1
    }

    pub fn read(s: &mut State, reading: Reading) -> Dirty {
        s.apply(reading.into()).0
    }

    /// The launcher done with `game`'s launch (it ended).
    pub fn played(s: &mut State, game: &str) -> Vec<Effect> {
        s.apply(GameEvent::Ended(game.into()).into()).1
    }

    /// Showing `instance`'s workspace.
    pub fn focus(instance: &Instance) -> Effect {
        Effect::Window(WindowCommand::Focus(instance.workspace()))
    }

    /// Launches `game` from shelf and has it start; returns its instance.
    pub fn launch(s: &mut State, id: &str) -> Instance {
        // Select it (if it isn't), then play it: the launch is in one of the two.
        let mut fx = tap(s, game(id));
        fx.extend(tap(s, game(id)));
        let instance = fx.iter().find_map(|e| match e {
            Effect::Launch { instance, .. } => Some(instance.clone()),
            _ => None,
        });
        s.apply(GameEvent::Started(id.into()).into());
        instance.unwrap()
    }
}

#[cfg(test)]
mod tests {
    use crate::testkit::*;
    use crate::*;

    #[test]
    fn starts_with_first_game_selected_and_focused() {
        let s = state();
        assert_eq!(s.shelf().selected().map(|g| &**g), Some("zelda-lttp"));
        assert_eq!(s.pointer().focus(), Some(&game("zelda-lttp")));
    }

    #[test]
    fn starts_where_it_was_told() {
        let s = State::new(Init { start_tab: Some(Tab::Dock), pad_mouse: true, ..init() });
        assert_eq!((s.bottom().tab(), s.pad().mouse(), s.pad().route()), (Tab::Dock, PadMouse::ByUser, PadRoute::Mouse));
    }

    #[test]
    fn while_metadata_is_fetched_shelfs_view_goes_first() {
        let mut s = state();
        assert!(!s.apply(ShelfEvent::ListStep(1).into()).1.iter().any(|e| matches!(e, Effect::FetchFirst(_))), "not fetching");
        let (_, fx) = s.apply(Event::Fetching(true));
        assert_eq!(fx, vec![Effect::FetchFirst(vec!["metroid-zm".into(), "mk-ds".into()])], "the selected first, then the rows shown");
        assert!(!s.apply(Event::Tick { unix_secs: 5 }).1.iter().any(|e| matches!(e, Effect::FetchFirst(_))), "the same view: not again");
        let (_, fx) = s.apply(ShelfEvent::ListStep(1).into());
        assert!(fx.contains(&Effect::FetchFirst(vec!["mk-ds".into(), "ff6".into()])), "{fx:?}");
        s.apply(Event::Fetching(false));
        assert!(!s.apply(ShelfEvent::ListStep(1).into()).1.iter().any(|e| matches!(e, Effect::FetchFirst(_))), "done");
    }

    #[test]
    fn home_goes_to_shelf_and_back_to_what_was_in_front() {
        let mut s = state();
        let shelf = |focused| win(1, windows::SHELF_APP_ID, &Instance::Shelf.name(), focused);
        let zelda = Instance::workspace_of(&clamshell_library::key_of("zelda-lttp"));
        let home = |s: &mut State| s.apply(Event::Home).1;
        assert_eq!(home(&mut s), vec![], "nothing before shelf: it stays");
        s.apply(GameEvent::Started("zelda-lttp".into()).into());
        assert_eq!(home(&mut s), vec![], "the program in force, no window of it yet: shelf stays");
        s.apply(Event::Windows(vec![shelf(true), win(3, "retroarch", &zelda, false)]));
        assert_eq!(home(&mut s), vec![Effect::Window(WindowCommand::Focus(zelda.clone()))], "no history: the program in force");
        // A terminal in front, then shelf: home goes back to the terminal.
        let terminal = "clamshell-run-foot_1";
        s.apply(Event::Windows(vec![shelf(false), win(2, "foot", terminal, true)]));
        assert_eq!(home(&mut s), vec![Effect::Window(WindowCommand::ShowShelf)]);
        s.apply(Event::Windows(vec![shelf(true), win(2, "foot", terminal, false)]));
        assert_eq!(home(&mut s), vec![Effect::Window(WindowCommand::Focus(terminal.into()))]);
        // The terminal closed: back to the game, which was in front before.
        s.apply(Event::Windows(vec![shelf(false), win(3, "retroarch", &zelda, true)]));
        s.apply(Event::Windows(vec![shelf(true), win(3, "retroarch", &zelda, false)]));
        assert_eq!(home(&mut s), vec![Effect::Window(WindowCommand::Focus(zelda))]);
    }

    #[test]
    fn a_closed_window_goes_back_to_what_was_in_front_before_it_shelf_last() {
        let mut s = state();
        let shelf = |focused| win(1, windows::SHELF_APP_ID, &Instance::Shelf.name(), focused);
        let (zelda, terminal) = ("clamshell-run-zelda-1".to_owned(), "clamshell-run-foot-2".to_owned());
        let window = |fx: Vec<Effect>| fx.into_iter().filter(|e| matches!(e, Effect::Window(_))).collect::<Vec<_>>();
        // Shelf, then the terminal, then the game in front.
        s.apply(Event::Windows(vec![shelf(true)]));
        s.apply(Event::Windows(vec![shelf(false), win(2, "foot", &terminal, true)]));
        s.apply(Event::Windows(vec![shelf(false), win(2, "foot", &terminal, false), win(3, "retroarch", &zelda, true)]));
        // The game quits: the terminal, in front before it.
        let (_, fx) = s.apply(Event::Windows(vec![shelf(false), win(2, "foot", &terminal, false)]));
        assert_eq!(window(fx), vec![Effect::Window(WindowCommand::Focus(terminal.clone()))]);
        s.apply(Event::Windows(vec![shelf(false), win(2, "foot", &terminal, true)]));
        // The terminal quits: shelf.
        let (_, fx) = s.apply(Event::Windows(vec![shelf(false)]));
        assert_eq!(window(fx), vec![Effect::Window(WindowCommand::ShowShelf)]);
        // A program's own second window closing, or a new workspace before its window: nothing.
        s.apply(Event::Windows(vec![shelf(false), win(3, "retroarch", &zelda, true), win(4, "retroarch", &zelda, false)]));
        assert_eq!(window(s.apply(Event::Windows(vec![shelf(false), win(4, "retroarch", &zelda, true)])).1), vec![]);
        s.apply(Event::Windows(vec![shelf(true), win(4, "retroarch", &zelda, false)]));
        assert_eq!(window(s.apply(Event::Windows(vec![shelf(false), win(4, "retroarch", &zelda, false)])).1), vec![], "shelf's window is still there");
    }

    #[test]
    fn a_window_in_front_replaced_at_once_is_shown_again() {
        let mut s = state();
        let shelf = |focused| win(1, windows::SHELF_APP_ID, &Instance::Shelf.name(), focused);
        let app = "clamshell-run-fdroid-1".to_owned();
        let window = |fx: Vec<Effect>| fx.into_iter().filter(|e| matches!(e, Effect::Window(_))).collect::<Vec<_>>();
        s.apply(Event::Tick { unix_secs: 100 });
        s.apply(Event::Windows(vec![shelf(false), win(2, "waydroid.org.fdroid.fdroid", &app, true)]));
        // Its window closes (shelf), another comes on its workspace (moved there, not in front): shown.
        assert_eq!(window(s.apply(Event::Windows(vec![shelf(false)])).1), vec![Effect::Window(WindowCommand::ShowShelf)]);
        let (_, fx) = s.apply(Event::Windows(vec![shelf(true), win(3, "waydroid.org.fdroid.fdroid", &app, false)]));
        assert_eq!(window(fx), vec![Effect::Window(WindowCommand::Focus(app.clone()))]);
        // Later: not.
        s.apply(Event::Windows(vec![shelf(false), win(3, "waydroid.org.fdroid.fdroid", &app, true)]));
        s.apply(Event::Windows(vec![shelf(false)]));
        s.apply(Event::Tick { unix_secs: 103 });
        let (_, fx) = s.apply(Event::Windows(vec![shelf(true), win(4, "waydroid.org.fdroid.fdroid", &app, false)]));
        assert_eq!(window(fx), vec![], "too late: it stays behind");
    }

    #[test]
    fn home_doesnt_go_back_to_a_workspace_whose_window_moved_down() {
        let mut s = state();
        let shelf = |focused| win(1, windows::SHELF_APP_ID, &Instance::Shelf.name(), focused);
        let terminal = "clamshell-run-foot_1";
        s.apply(Event::Windows(vec![shelf(false), win(2, "foot", terminal, true)]));
        // Its window moved to the bottom screen, shelf in front: nothing to go back to on the main screen.
        s.apply(Event::Windows(vec![shelf(true), Window { down: true, ..win(2, "foot", terminal, false) }]));
        assert_eq!(s.apply(Event::Home).1, vec![]);
    }

    #[test]
    fn home_goes_back_to_a_workspace_that_isnt_ours() {
        let mut s = state();
        let shelf = |focused| win(1, windows::SHELF_APP_ID, &Instance::Shelf.name(), focused);
        s.apply(Event::Windows(vec![shelf(false), foot()]));
        s.apply(Event::Windows(vec![shelf(true), Window { focused: false, ..foot() }]));
        assert_eq!(s.apply(Event::Home).1, vec![Effect::Window(WindowCommand::Focus("1".into()))]);
    }

    #[test]
    fn tick_redraws_once_a_minute() {
        let mut s = state();
        assert_eq!(s.apply(Event::Tick { unix_secs: 60 }).0, Dirty::BOTH);
        assert_eq!(s.apply(Event::Tick { unix_secs: 119 }).0, Dirty::NONE);
    }
}
