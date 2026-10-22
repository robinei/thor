//! The program's state and its rules. No I/O: every change comes in as an [`Event`], and what the outside
//! world must do comes back as [`Effect`]s. The UI and the system threads only read the state and send
//! events, so all of this is unit-testable.
//!
//! The state is grouped by what it is about: shelf ([`Shelf`]), the second screen ([`Bottom`]), what runs
//! and is in front ([`Session`]), the dock ([`Dock`]), volume and brightness ([`Levels`]), the radios
//! ([`Radios`]), the controller ([`Pad`]), focus and presses ([`Pointer`]), and the system's readings
//! ([`Sys`]). The rules are a module per feature, each an `impl State` block, since most features read
//! more than one group. A few pure models live here too because the system threads share them with the
//! state: the fan's curve ([`fan`]) and the sticks' calibration ([`sticks`]).
//!
//! Panics are ruled out by the workspace's lints (no `unwrap`, `expect` or indexing). A lookup whose
//! index is valid by construction (an enum's place in an array sized for it, an index the library's own
//! tree holds) falls back quietly; where a bug could reach a fallback, a `debug_assert!` says so in tests.

pub mod account;
pub mod actions;
pub mod autostart;
pub mod background;
pub mod battery;
pub mod bottom_screen;
pub mod device;
pub mod dock;
pub mod editor;
mod effect;
pub mod enqueue;
mod event;
pub mod fan;
pub mod files;
mod found;
pub mod hardware;
mod id;
pub mod idle;
pub mod instance;
pub mod keyboard;
pub mod keymap;
pub mod keymap_screens;
pub mod kit;
pub mod levels;
pub mod metrics;
pub mod nav;
pub mod notice;
mod panel;
pub mod program;
pub mod radios;
mod resume;
mod running;
mod session;
pub mod settings;
mod shelf;
pub mod slider;
pub mod sticks;
mod sys;
mod tab;
pub mod time;
pub mod touch;
pub mod tray;
pub mod windows;

use keyboard::{Key, Keyboard};
use kit::replace;
pub use resume::Resume;
use resume::{SAVE_AFTER, Saves};
use settings::{MouseAtLaunch, Settings};
use slider::Control;

pub use crate::kit::list::Scroll;
pub use crate::nav::lists::{ListEvent, ListId, Lists, RowPart};
pub use crate::nav::pad::{Claimant, Pad, PadInput, PadMouse, PadRoute};
pub use crate::nav::pointer::Pointer;
pub use dock::{Dock, DockItem};
pub use editor::{Editor, count_env, parse_env};
pub use effect::{Dirty, Effect, Save, Watch, WindowCommand};
pub use event::{Event, LaunchEvent, LevelEvent, PointerEvent, Reading, ShelfEvent, Slide};
pub use id::{AudioId, BtId, DockId, EditorId, Hold, Id, KeymapId, SettingsId, ShelfId, Target, WifiId};
pub use instance::Instance;
pub use levels::{Brightness, Level, Levels, Sound};
pub use panel::{Scope, TileOrigin};
pub use program::Program;
pub use radios::{RadioStatus, Radios};
pub use running::{Front, Launching, Origin, Running};
pub use session::Session;
pub use shelf::{Cursor, Shelf};
pub use sys::{Status, Sys};
pub use tab::{Bottom, Tab};
pub use time::{ClockField, TimeId, TimePage, TimeRow, TimeZones};

pub use clamshell_library::{BrowseKind, Browsed, Collection, Entry, Launch, Library, Meta, RunnerOf, runner_key};

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
    /// ClamShell's first start (the Device view not closed with Done yet): the Device view opens (see
    /// [`device`]).
    pub first_start: bool,
    /// The sticks' calibrations, by controller (see [`sticks`]).
    pub sticks: sticks::Calibrations,
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
    /// The account is watched (the Device view open: see [`State::follow_watch`]).
    account_watched: bool,
    /// The Device view on the main screen (see [`device`]).
    device: device::Device,
    /// The screens are off: the bar's status isn't watched (see [`Watch::Status`]).
    status_unwatched: bool,
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
    /// The keymaps' screens over the settings tab (see [`keymap_screens`]).
    keymap_screens: keymap_screens::Screens,
    /// The keymap in force as last sent to the pad.
    keymap_sent: Option<keymap::Keymap>,
    lists: Lists,
    unix_secs: u64,
    /// A monotonic clock, as last ticked (see [`Event::Tick`]): every duration is timed by it (idle, the
    /// bottom screen, notices, saves, plays, background grace, handoffs), so that a step of the wall clock
    /// (by hand, by NTP) is no time gone by. The wall clock dates things only.
    mono_secs: u64,
    saves: Saves,
    idle: idle::Idle,
    /// ClamShell is the device's own session (see [`Init::device_session`]).
    device_session: bool,
    /// Which programs' windows go to the bottom screen.
    window_rules: bottom_screen::WindowRules,
    /// Clocks on a 12-hour dial (see [`crate::time`]).
    pub(crate) twelve_hour: bool,
    /// Metadata is being fetched; what shelf showed when it was last asked for first (see
    /// [`State::follow_shown`]).
    fetching: bool,
    shown: Option<shelf::ShelfView>,
    /// Pins of entries gone were dropped at start: saved once the UI is up.
    pins_dropped: bool,
    /// ClamShell's first start (see [`Init::first_start`]).
    first_start: bool,
    /// The file actions' clipboard and confirmation, the folder browsed (see [`files`]).
    files: files::Files,
    /// The text on the system's clipboard, for the editor's Ctrl+V (files copied in the Files tab are its
    /// own, above).
    clipboard: Option<std::sync::Arc<str>>,
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
            first_start: init.first_start,
            ..State::default()
        };
        if let Some(resume) = &init.resume {
            s.restore(resume);
        }
        // (Saved once the UI is up.)
        s.pins_dropped = s.drop_gone_pins();
        s.follow_pausable_runners();
        s.pointer.focus = s.shelf.selected().cloned().map(|g| Id::Shelf(ShelfId::Entry(g)));
        s.device.calibrations = init.sticks;
        if init.first_start {
            s.device.open = true;
            s.pointer.focus = Some(Id::Device(device::DeviceId::Wifi));
        }
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

    /// Whether the clocks have been read (the first [`Event::Tick`] came).
    pub(crate) fn clock_read(&self) -> bool {
        self.unix_secs != 0
    }

    /// Epoch seconds at monotonic time `mono` (as the wall clock reads now: a step of it since moves it).
    pub(crate) fn unix_at(&self, mono: u64) -> u64 {
        self.unix_secs.saturating_sub(self.mono_secs.saturating_sub(mono))
    }

    /// Whether the dock menu offers Open another for `id`: it runs, and another launch of it makes sense
    /// (see [`crate::Entry::another`]).
    pub fn opens_another(&self, id: &str) -> bool {
        // (Not one handed over to another program, Android's: a launch shows the one it runs.)
        self.session.running(id).is_some() && self.entry(id).is_some_and(|g| g.another && !g.runner.as_ref().is_some_and(|r| r.handover))
    }

    /// A library entry by its id, or one that runs after leaving the library (its file deleted), as it was.
    pub fn entry(&self, id: &str) -> Option<&Entry> {
        self.shelf.entry(id).or_else(|| self.session.running(id).and_then(|r| r.entry.as_deref()))
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
        let session_input = matches!(event, Event::Tick { .. } | Event::Windows(_) | Event::Units(_) | Event::Tray(_) | Event::Launch(_) | Event::Library(_));
        let (mut dirty, mut effects) = self.apply_event(event);
        if session_input {
            let (changed, reconciled) = self.reconcile();
            effects.extend(reconciled);
            if changed {
                dirty = dirty.or(Dirty::BOTTOM);
            }
        }
        if self.follow_bottom_dim() {
            dirty = dirty.or(Dirty::BOTTOM);
        }
        effects.extend(self.follow_shown());
        effects.extend(self.follow_browse());
        effects.extend(self.follow_idle());
        effects.extend(self.follow_watch());
        let (changed, applied) = self.follow_program();
        effects.extend(applied);
        // (After the program in force: its setting, its darkness; the watch again if it changed.)
        let power = self.follow_bottom_power();
        if !power.is_empty() {
            effects.extend(power);
            effects.extend(self.follow_watch());
        }
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

    /// After any change of what the session follows (the windows, systemd's scopes, the tray, the
    /// launcher's word, the library) and every second: everything that follows from it, in this order
    /// (each step reads what the ones before settled):
    ///
    /// 1. systemd's scopes: entries found running, found ones promoted to launches, ours ended
    ///    ([`State::follow_units`]); the first time, the entries to start with ClamShell started;
    /// 2. the windows: entries started or ended by them, windows moved to their entries, programs found
    ///    by them ([`State::follow_windows`]; `known`: the windows the last pass saw); rules sending
    ///    windows down;
    /// 3. In the background: entries paused, thawed or ended by their setting (reads 1 and 2);
    /// 4. what is shown: a launch's first window, a workspace whose window came back, the one in front
    ///    closed (back to what was before);
    /// 5. entries quitting ended once their windows are gone (reads 1–3);
    /// 6. the dock: running entries on it, ended ones off.
    ///
    /// Each step does nothing when nothing changed for it. Whether the running entries changed.
    fn reconcile(&mut self) -> (bool, Vec<Effect>) {
        let before: Vec<std::sync::Arc<str>> = self.session.running.iter().map(|r| r.id.clone()).collect();
        let known = std::mem::take(&mut self.session.followed_windows);
        let was = self.session.followed_front.take();
        let mut effects = self.follow_units();
        if self.session.units_known {
            effects.extend(self.start_autostart());
        }
        let by_windows = self.follow_windows(&known);
        effects.extend(self.follow_window_rules());
        effects.extend(self.follow_background());
        effects.extend(self.session.show_pending());
        effects.extend(self.session.show_reopened(self.mono_secs));
        effects.extend(self.follow_quitting());
        effects.extend(self.follow_closed(was));
        // (After the one in front closing: a window just taken by a browsed file is shown over it.)
        effects.extend(by_windows);
        let docked = self.follow_dock();
        self.session.followed_windows = self.session.windows.iter().map(|w| w.id).collect();
        self.session.followed_front = self.session.windows.iter().find(|w| w.focused && !w.down).map(|w| w.workspace.clone());
        let after = self.session.running.iter().map(|r| &r.id);
        (docked || !after.eq(before.iter()), effects)
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
        let wanted = self.session.program.is_some() && self.in_force().choice::<MouseAtLaunch>() == MouseAtLaunch::On;
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
            Event::Tick { unix_secs, mono_secs } => self.tick(unix_secs, mono_secs),
            Event::Reading(reading) => self.reading_came(reading),
            Event::List(e) => (if self.on_list(e) { e.list().screens() } else { Dirty::NONE }, vec![]),
            Event::Pointer(e) => self.pointer_event(e),
            Event::Slide(slide) => self.on_slide(slide),
            Event::Level(e) => self.on_level(e),
            Event::Shelf(e) => self.on_shelf_event(e),
            Event::Launch(e) => self.on_launch_event(e),
            Event::Windows(windows) => {
                self.session.set_windows(windows);
                (Dirty::BOTH, vec![])
            }
            Event::Meta { id, meta } => (if self.shelf.apply_meta(&id, &meta) { Dirty::BOTH } else { Dirty::NONE }, vec![]),
            Event::Library(library) => (Dirty::BOTH, self.library_read(*library)),
            Event::Files(e) => (Dirty::BOTH, self.on_files(e)),
            Event::Fetching(on) => {
                self.fetching = on;
                self.shown = None;
                (Dirty::NONE, vec![])
            }
            Event::Tray(items) => match self.set_tray(items) {
                Some(effects) => (Dirty::BOTH, effects),
                None => (Dirty::NONE, vec![]),
            },
            Event::Units(units) => {
                self.session.set_units(units);
                (Dirty::BOTTOM, vec![])
            }
            Event::Start => (Dirty::BOTH, self.ui_started()),
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
            // (Never applied: the UI handles the pad's navigation itself.)
            Event::Pad(_) => (Dirty::NONE, vec![]),
            Event::TextInput(active) => self.text_input_changed(active),
            Event::Pointing(pointing) => self.pointing(pointing),
            // Sleep may have reset what the settings set (frequency caps, cores): again.
            Event::Resumed => {
                self.idle.resumed(self.mono_secs);
                (Dirty::NONE, vec![Effect::Apply { entry: self.session.program.clone(), settings: self.in_force() }])
            }
            Event::Idle(event) => {
                self.idle.heard(event, self.mono_secs);
                (Dirty::NONE, vec![])
            }
            Event::Device(key) => (Dirty::BOTTOM, self.on_device_key(key)),
            Event::BottomTouched => (self.bottom_used(), vec![]),
            Event::Frozen(keys) => {
                self.found_frozen(keys);
                (Dirty::BOTTOM, vec![])
            }
            Event::Paste => {
                self.paste();
                (Dirty::BOTTOM, vec![])
            }
            Event::Screenshot(result) => {
                self.on_screenshot(result);
                (Dirty::BOTTOM, vec![])
            }
            Event::Sticks(event) => (Dirty::TOP, self.sticks_event(event)),
            Event::RefreshCalibrated(result) => {
                self.on_refresh(result);
                // (The tool turned the bottom screen off meanwhile and back as it was: as ClamShell has it.)
                (Dirty::TOP, vec![Effect::Window(WindowCommand::BottomPower(!self.bottom_off()))])
            }
            Event::GyroCalibrated(ok) => {
                self.notify(crate::notice::NoticeKind::Gyro(Some(ok)));
                (Dirty::BOTTOM, vec![])
            }
            Event::AccountChanged(result) => {
                self.on_account_changed(result);
                (Dirty::BOTTOM, vec![])
            }
            Event::Quit => (Dirty::NONE, vec![Effect::Save(Save::Resume(self.resume())), Effect::Quit]),
        }
    }

    /// A reading from a system thread: the battery warned about after a status reading.
    fn reading_came(&mut self, reading: Reading) -> (Dirty, Vec<Effect>) {
        let status = matches!(reading, Reading::Status(_));
        let dirty = self.on_reading(reading);
        let effects = if status { self.sys.battery_warning().into_iter().collect() } else { vec![] };
        (if self.follow_lists() { Dirty::BOTTOM } else { dirty }, effects)
    }

    /// A touch or the pad's focus: the pad's focus, or a press, on the second screen's targets means it
    /// is in use.
    fn pointer_event(&mut self, e: PointerEvent) -> (Dirty, Vec<Effect>) {
        let used = match &e {
            PointerEvent::Focus(id) | PointerEvent::Activate(id) | PointerEvent::LongPress(id) | PointerEvent::Down(id) => id.screen() == Dirty::BOTTOM,
            _ => false,
        };
        let lit = if used { self.bottom_used() } else { Dirty::NONE };
        let (dirty, effects) = self.on_pointer(e);
        (dirty.or(lit), effects)
    }

    /// The library read anew: problems it didn't have before said once, shelf reloaded, and what
    /// follows from its entries (runners' pauses and keymaps, landing rules, pins of entries gone).
    fn library_read(&mut self, library: Library) -> Vec<Effect> {
        let problems = &library.problems;
        if !problems.is_empty() && *problems != self.shelf.library().problems {
            let first = problems.first().cloned().unwrap_or_default();
            self.notify(notice::NoticeKind::Problems(first, problems.len()));
        }
        if self.shelf.reload(library) {
            self.moved_collection();
        }
        self.follow_pausable_runners();
        let mut effects = self.follow_runner_keymaps();
        effects.push(self.land_rules());
        if self.drop_gone_pins() {
            effects.push(Effect::Save(Save::Dock(self.dock.pinned_ids())));
        }
        self.shown = None;
        effects
    }

    /// The UI is up: what was waiting for it (pins dropped at start saved, the brightness, the pad's
    /// route, the keymaps, the down rules). The open tab's watch starts from now (see
    /// [`State::follow_watch`]).
    fn ui_started(&mut self) -> Vec<Effect> {
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
        effects
    }

    /// A text field of the app in front wants typing, or no longer does. A field taking focus brings
    /// typing back from the touchpad.
    fn text_input_changed(&mut self, active: bool) -> (Dirty, Vec<Effect>) {
        let (before, pointing) = (self.bottom.tab, self.bottom.pointing);
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

    /// The touchpad tab's contacts: passed on as the pointer's, or none.
    fn pointing(&mut self, pointing: crate::touch::Pointing) -> (Dirty, Vec<Effect>) {
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

    /// A stick calibration's progress or end; a calibration made is saved.
    fn sticks_event(&mut self, event: device::SticksEvent) -> Vec<Effect> {
        let done = matches!(event, device::SticksEvent::Done(_, Some(_)));
        self.on_sticks(event);
        if done { vec![self.sticks_saved()] } else { vec![] }
    }

    /// The clock: once a minute the bar's clock and both screens' burn-in orbit step; starts systemd
    /// never listed are forgotten; what changed is saved once changes have stopped for a moment.
    fn tick(&mut self, unix_secs: u64, mono_secs: u64) -> (Dirty, Vec<Effect>) {
        let minute_changed = unix_secs / 60 != self.unix_secs / 60;
        // The clock's first word: what idle heard before it was dated 0. Count from now.
        if self.unix_secs == 0 {
            self.idle.resumed(mono_secs);
        }
        self.unix_secs = unix_secs;
        self.mono_secs = mono_secs;
        let mut effects = vec![];
        if self.saves.resume.due(mono_secs, SAVE_AFTER) {
            effects.push(Effect::Save(Save::Resume(self.resume())));
        }
        if self.saves.brightness.due(mono_secs, SAVE_AFTER) {
            effects.push(Effect::Save(Save::Brightness(self.levels.brightness().clone())));
        }
        let notice_gone = self.expire_notice(mono_secs);
        let dirty = if minute_changed {
            Dirty::BOTH
        } else if notice_gone {
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
            Reading::Clipboard(text) => replace(&mut self.clipboard, text),
            Reading::Icon { entry, path } => {
                let running = self.session.running.iter_mut().find(|r| r.id == entry);
                running.and_then(|r| r.entry.as_mut()).is_some_and(|g| replace(&mut g.icon, Some(path)))
            }
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
            Reading::Account(account) => {
                // (The Device view shows it, on the main screen.)
                let changed = replace(&mut self.sys.account, account);
                return if changed && self.device.open { Dirty::TOP } else { Dirty::NONE };
            }
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
        self.idle.own_input(self.mono_secs);
        match event {
            PointerEvent::Focus(id) => {
                self.show_focused(&id);
                self.pointer.focus = Some(id);
                self.pointer.show_focus = true;
                (Dirty::BOTH, vec![])
            }
            PointerEvent::HideFocus => (if self.pointer.hide_focus() { Dirty::BOTH } else { Dirty::NONE }, vec![]),
            // (A press that woke the screens only woke them.)
            PointerEvent::Activate(_) | PointerEvent::LongPress(_) if self.idle.swallows_press(self.mono_secs) => (Dirty::NONE, vec![]),
            PointerEvent::Activate(id) => self.activate(&id, false),
            PointerEvent::LongPress(id) => self.activate(&id, true),
            PointerEvent::Down(id) => {
                let dirty = id.screen();
                if let Id::Key(pos) = id {
                    self.keyboard.down(pos);
                }
                self.pointer.press = self.item_key(&id).map(|item| crate::nav::pointer::Press { id: id.clone(), item });
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
            Id::Shelf(ShelfId::Entry(game)) if long && self.pointer.show_focus && self.shelf.selected() == Some(&game) => {
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
            Id::Device(id) => (Dirty::BOTH, self.on_device(id)),
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
            ShelfId::Entry(game) => {
                // Activating the selected entry again opens it (a group) or plays it (starts it, or
                // switches to it).
                if self.shelf.selected() == Some(&game) {
                    return self.activate_selected();
                }
                if self.shelf.select(&game) {
                    self.saves.resume.touch(self.mono_secs);
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
        match next.and_then(|i| self.shelf.library().entries.get(i)).map(|g| g.id.clone()) {
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
        self.saves.resume.touch(self.mono_secs);
        if matches!(self.pointer.focus, Some(Id::Shelf(ShelfId::Entry(_) | ShelfId::Collection(_)))) {
            self.pointer.focus = self.shelf.selected().cloned().map(|g| Id::Shelf(ShelfId::Entry(g)));
        }
    }

    fn on_shelf_event(&mut self, event: ShelfEvent) -> (Dirty, Vec<Effect>) {
        match event {
            ShelfEvent::ListStep(step) => {
                if self.shelf.step(step) {
                    self.saves.resume.touch(self.mono_secs);
                }
                self.pointer.focus = self.shelf.selected().cloned().map(|g| Id::Shelf(ShelfId::Entry(g)));
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
            self.saves.resume.touch(self.mono_secs);
        }
        let ids = self.shelf.library().favourite_ids().map(str::to_owned).collect();
        vec![Effect::Save(Save::Favourites(ids))]
    }

    fn on_launch_event(&mut self, event: LaunchEvent) -> (Dirty, Vec<Effect>) {
        let effects = match event {
            LaunchEvent::Position { entry: game, position } => {
                self.shelf.set_position(&game, position, self.unix_secs);
                vec![]
            }
            // (A launch that never started: nothing runs one less, but it is no longer starting.)
            LaunchEvent::LaunchFailed(game) => {
                self.launch_failed(game);
                vec![]
            }
        };
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
            self.saves.brightness.touch(self.mono_secs);
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
            self.saves.resume.touch(self.mono_secs);
        }
        self.set_tab(tab)
    }

    /// Changes the second screen's tab (its watch follows: see [`State::follow_watch`]); a level being set
    /// ends. What the tab left had open (an editor, the keymaps' screens) stays for coming back to.
    fn set_tab(&mut self, tab: Tab) -> Vec<Effect> {
        if tab == self.bottom.tab {
            return vec![];
        }
        self.saves.resume.touch(self.mono_secs);
        self.bottom.tab = tab;
        self.dock.menu = None;
        self.dock.moving = None;
        // No level row of the tab left is being set any more.
        self.levels.release();
        // (The time tab opens on its main page again.)
        self.bottom.time_page = TimePage::Main;
        vec![]
    }

    /// What the second screen watches: the open tab's (the dock's, with an entry's menu open: what the
    /// entry costs, the battery tab's readings), while it can be seen (ClamShell's view on the bottom
    /// screen, that on, the screens on) and the UI is up. Its watch started and stopped as that changes.
    fn follow_watch(&mut self) -> Vec<Effect> {
        let mut effects = Vec::new();
        // (The bar's status: while the screens are on.)
        let screens_off = self.idle.stage() >= idle::Stage::Off;
        if screens_off != std::mem::replace(&mut self.status_unwatched, screens_off) {
            effects.push(Effect::Watch(Watch::Status, !screens_off));
        }
        let seen = self.ui_up && !self.bottom_shows_windows() && !self.bottom_off() && self.idle.stage() < idle::Stage::Off;
        let tab = match self.bottom.tab {
            Tab::Dock if self.dock.menu.is_some() => Some(Watch::Battery),
            tab => tab.watch(),
        };
        // (The account: while the Device view is open and the UI up.)
        let account = self.ui_up && self.device.open;
        if account != std::mem::replace(&mut self.account_watched, account) {
            effects.push(Effect::Watch(Watch::Account, account));
        }
        let wanted = tab.filter(|_| seen);
        if wanted == self.watching {
            return effects;
        }
        let was = std::mem::replace(&mut self.watching, wanted);
        effects.extend(was.map(|w| Effect::Watch(w, false)).into_iter().chain(wanted.map(|w| Effect::Watch(w, true))));
        effects
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
        self.session.closed_front = Some((was, self.mono_secs));
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
        // (The Device view in shelf's place: back to shelf.)
        if self.device.open {
            return self.close_device();
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

    /// Entry `id`'s launch starts, as systemd tells it: its scope listed with the others (the launch's
    /// asked for, else one of the entry's, as a ClamShell before this one started).
    pub fn start(s: &mut State, id: &str) -> (Dirty, Vec<Effect>) {
        let key = s.entry(id).map_or_else(|| clamshell_library::key_of(id), |g| g.key.clone());
        let launch = s.session.launching.iter().find(|l| &*l.entry == id).map(|l| l.instance.clone());
        let mut units = s.session.units.clone();
        units.push(launch.unwrap_or(Instance::Run { key, secs: s.unix_secs, n: 99 }));
        s.apply(Event::Units(units))
    }

    /// Entry `id`'s launches end, as systemd tells it: its scopes no longer listed.
    pub fn end(s: &mut State, id: &str) -> (Dirty, Vec<Effect>) {
        let key = s.entry(id).map_or_else(|| clamshell_library::key_of(id), |g| g.key.clone());
        let units = s.session.units.iter().filter(|u| u.key() != Some(key.as_str())).cloned().collect();
        s.apply(Event::Units(units))
    }

    /// What a test state starts from: the demo library, nothing else.
    pub fn init() -> Init {
        Init { library: clamshell_library::demo::library(), ..Init::default() }
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
        Id::Shelf(ShelfId::Entry(id.into()))
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

    /// Shelf's window, in front or not.
    pub fn shelf_window(focused: bool) -> Window {
        win(1, crate::windows::SHELF_APP_ID, &Instance::Shelf.name(), focused)
    }

    /// The notice shown, if one is.
    pub fn notice(s: &State) -> Option<crate::notice::NoticeKind> {
        s.bottom().notice().map(|n| n.what.clone())
    }

    /// The window commands among `fx`.
    pub fn window_effects(fx: Vec<Effect>) -> Vec<Effect> {
        fx.into_iter().filter(|e| matches!(e, Effect::Window(_))).collect()
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
        crate::testkit::start(s, id);
        instance.unwrap()
    }
}

#[cfg(test)]
mod tests {
    use crate::testkit::*;
    use crate::*;

    /// Every order of `events`.
    fn orders(events: &[Event]) -> Vec<Vec<Event>> {
        if events.len() <= 1 {
            return vec![events.to_vec()];
        }
        let mut all = Vec::new();
        for i in 0..events.len() {
            let mut rest = events.to_vec();
            let first = rest.remove(i);
            for mut order in orders(&rest) {
                order.insert(0, first.clone());
                all.push(order);
            }
        }
        all
    }

    /// The session's inputs coming in any order (systemd's list, the compositor's windows: separate
    /// threads) end in the same state: the reconcile after each reads what is there,
    /// not which came last.
    #[test]
    fn a_launchs_start_and_end_come_to_the_same_in_any_order() {
        let shape = |s: &State| {
            let running: Vec<(String, bool)> = s.session().running_entries().iter().map(|r| (r.id.to_string(), r.origin == running::Origin::Ours)).collect();
            let dock: Vec<String> = s.dock().items().iter().map(|i| i.id.to_string()).collect();
            (running, dock)
        };
        let mut shapes = Vec::new();
        let mut plays = Vec::new();
        let start_end = |s: &mut State| {
            s.apply(Event::tick(1000));
            let fx = tap(s, game("zelda-lttp"));
            let fx: Vec<Effect> = fx.into_iter().chain(tap(s, game("zelda-lttp"))).collect();
            fx.iter().find_map(|e| match e {
                Effect::Launch { instance, .. } => Some(instance.clone()),
                _ => None,
            })
        };
        // (No launch: the shapes below say so.)
        let probe = start_end(&mut state()).unwrap_or(Instance::Shelf);
        let window = Window { unit: probe.unit(), visible: true, ..win(5, "zelda3", &probe.workspace(), true) };
        let starts = [Event::Units(vec![probe.clone()]), Event::Windows(vec![window])];
        let ends = [Event::Units(vec![]), Event::Windows(vec![])];
        for start in orders(&starts) {
            for end in orders(&ends) {
                let mut s = state();
                start_end(&mut s);
                for e in start.clone() {
                    s.apply(e);
                }
                let started = shape(&s);
                let mut recorded = 0;
                for e in end.clone() {
                    recorded += s.apply(e).1.iter().filter(|e| matches!(e, Effect::RecordPlay { .. })).count();
                }
                s.apply(Event::tick(1001));
                shapes.push((started, shape(&s)));
                plays.push(recorded);
            }
        }
        assert!(shapes.windows(2).all(|w| w.first() == w.last()), "{shapes:#?}");
        let first = shapes.first().cloned().unwrap_or_default();
        assert_eq!(first.0.0, vec![("zelda-lttp".to_owned(), true)], "running as a launch of ours");
        assert!(first.1.0.is_empty(), "ended");
        assert!(plays.iter().all(|&n| n == 1), "one play recorded each time: {plays:?}");
    }

    #[test]
    fn new_library_problems_are_said_once() {
        let mut s = state();
        let with = |problems: &[&str]| {
            let mut library = clamshell_library::demo::library();
            library.problems = problems.iter().map(|p| (*p).to_owned()).collect();
            Event::Library(Box::new(library))
        };
        s.apply(with(&["grim isn't installed: no screenshots", "x: no launch command"]));
        assert_eq!(notice(&s), Some(notice::NoticeKind::Problems("grim isn't installed: no screenshots".into(), 2)));
        s.apply(Event::tick(10_000));
        s.apply(with(&["grim isn't installed: no screenshots", "x: no launch command"]));
        assert_eq!(notice(&s), None, "the same again: not said");
    }

    #[test]
    fn a_launch_says_it_is_starting() {
        let mut s = state();
        launch(&mut s, "zelda-lttp");
        assert!(matches!(s.bottom().notice().map(|n| &n.what), Some(notice::NoticeKind::Starting(t)) if t == "A Link to the Past"));
    }

    #[test]
    fn a_launch_that_ends_without_a_window_didnt_start() {
        let mut s = state();
        launch(&mut s, "zelda-lttp");
        crate::testkit::end(&mut s, "zelda-lttp");
        assert!(matches!(notice(&s), Some(notice::NoticeKind::NotStarted(_))), "{:?}", notice(&s));
        // One that showed a window and ended: nothing said.
        let mut s = state();
        let zelda = launch(&mut s, "zelda-lttp");
        s.apply(Event::Windows(vec![Window { unit: zelda.unit(), visible: true, ..win(5, "zelda3", &zelda.workspace(), true) }]));
        s.apply(Event::Windows(vec![]));
        crate::testkit::end(&mut s, "zelda-lttp");
        assert!(!matches!(notice(&s), Some(notice::NoticeKind::NotStarted(_))), "{:?}", notice(&s));
    }

    #[test]
    fn a_launch_that_didnt_start_says_so() {
        let mut s = state();
        tap(&mut s, game("zelda-lttp"));
        tap(&mut s, game("zelda-lttp"));
        s.apply(Event::Launch(LaunchEvent::LaunchFailed("zelda-lttp".into())));
        let notice = s.bottom().notice().map(|n| n.what.clone());
        assert!(matches!(&notice, Some(notice::NoticeKind::NotStarted(t)) if t == "A Link to the Past"), "{notice:?}");
        assert!(s.session().running("zelda-lttp").is_none());
    }

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
        assert!(!s.apply(Event::tick(5)).1.iter().any(|e| matches!(e, Effect::FetchFirst(_))), "the same view: not again");
        let (_, fx) = s.apply(ShelfEvent::ListStep(1).into());
        assert!(fx.contains(&Effect::FetchFirst(vec!["mk-ds".into(), "ff6".into()])), "{fx:?}");
        s.apply(Event::Fetching(false));
        assert!(!s.apply(ShelfEvent::ListStep(1).into()).1.iter().any(|e| matches!(e, Effect::FetchFirst(_))), "done");
    }

    #[test]
    fn home_goes_to_shelf_and_back_to_what_was_in_front() {
        let mut s = state();
        let zelda = Instance::workspace_of(&clamshell_library::key_of("zelda-lttp"));
        let home = |s: &mut State| s.apply(Event::Home).1;
        assert_eq!(home(&mut s), vec![], "nothing before shelf: it stays");
        crate::testkit::start(&mut s, "zelda-lttp");
        assert_eq!(home(&mut s), vec![], "the program in force, no window of it yet: shelf stays");
        s.apply(Event::Windows(vec![shelf_window(true), win(3, "retroarch", &zelda, false)]));
        assert_eq!(home(&mut s), vec![Effect::Window(WindowCommand::Focus(zelda.clone()))], "no history: the program in force");
        // A terminal in front, then shelf: home goes back to the terminal.
        let terminal = "clamshell-run-foot_1";
        s.apply(Event::Windows(vec![shelf_window(false), win(2, "foot", terminal, true)]));
        assert_eq!(home(&mut s), vec![Effect::Window(WindowCommand::ShowShelf)]);
        s.apply(Event::Windows(vec![shelf_window(true), win(2, "foot", terminal, false)]));
        assert_eq!(home(&mut s), vec![Effect::Window(WindowCommand::Focus(terminal.into()))]);
        // The terminal closed: back to the game, which was in front before.
        s.apply(Event::Windows(vec![shelf_window(false), win(3, "retroarch", &zelda, true)]));
        s.apply(Event::Windows(vec![shelf_window(true), win(3, "retroarch", &zelda, false)]));
        assert_eq!(home(&mut s), vec![Effect::Window(WindowCommand::Focus(zelda))]);
    }

    #[test]
    fn a_closed_window_goes_back_to_what_was_in_front_before_it_shelf_last() {
        let mut s = state();
        let (zelda, terminal) = ("clamshell-run-zelda-1".to_owned(), "clamshell-run-foot-2".to_owned());
        // Shelf, then the terminal, then the game in front.
        s.apply(Event::Windows(vec![shelf_window(true)]));
        s.apply(Event::Windows(vec![shelf_window(false), win(2, "foot", &terminal, true)]));
        s.apply(Event::Windows(vec![shelf_window(false), win(2, "foot", &terminal, false), win(3, "retroarch", &zelda, true)]));
        // The game quits: the terminal, in front before it.
        let (_, fx) = s.apply(Event::Windows(vec![shelf_window(false), win(2, "foot", &terminal, false)]));
        assert_eq!(window_effects(fx), vec![Effect::Window(WindowCommand::Focus(terminal.clone()))]);
        s.apply(Event::Windows(vec![shelf_window(false), win(2, "foot", &terminal, true)]));
        // The terminal quits: shelf.
        let (_, fx) = s.apply(Event::Windows(vec![shelf_window(false)]));
        assert_eq!(window_effects(fx), vec![Effect::Window(WindowCommand::ShowShelf)]);
        // A program's own second window closing, or a new workspace before its window: nothing.
        s.apply(Event::Windows(vec![shelf_window(false), win(3, "retroarch", &zelda, true), win(4, "retroarch", &zelda, false)]));
        assert_eq!(window_effects(s.apply(Event::Windows(vec![shelf_window(false), win(4, "retroarch", &zelda, true)])).1), vec![]);
        s.apply(Event::Windows(vec![shelf_window(true), win(4, "retroarch", &zelda, false)]));
        assert_eq!(
            window_effects(s.apply(Event::Windows(vec![shelf_window(false), win(4, "retroarch", &zelda, false)])).1),
            vec![],
            "shelf's window is still there"
        );
    }

    #[test]
    fn a_window_in_front_replaced_at_once_is_shown_again() {
        let mut s = state();
        let app = "clamshell-run-fdroid-1".to_owned();
        s.apply(Event::tick(100));
        s.apply(Event::Windows(vec![shelf_window(false), win(2, "waydroid.org.fdroid.fdroid", &app, true)]));
        // Its window closes (shelf), another comes on its workspace (moved there, not in front): shown.
        assert_eq!(window_effects(s.apply(Event::Windows(vec![shelf_window(false)])).1), vec![Effect::Window(WindowCommand::ShowShelf)]);
        let (_, fx) = s.apply(Event::Windows(vec![shelf_window(true), win(3, "waydroid.org.fdroid.fdroid", &app, false)]));
        assert_eq!(window_effects(fx), vec![Effect::Window(WindowCommand::Focus(app.clone()))]);
        // Later: not.
        s.apply(Event::Windows(vec![shelf_window(false), win(3, "waydroid.org.fdroid.fdroid", &app, true)]));
        s.apply(Event::Windows(vec![shelf_window(false)]));
        s.apply(Event::tick(103));
        let (_, fx) = s.apply(Event::Windows(vec![shelf_window(true), win(4, "waydroid.org.fdroid.fdroid", &app, false)]));
        assert_eq!(window_effects(fx), vec![], "too late: it stays behind");
    }

    #[test]
    fn home_doesnt_go_back_to_a_workspace_whose_window_moved_down() {
        let mut s = state();
        let terminal = "clamshell-run-foot_1";
        s.apply(Event::Windows(vec![shelf_window(false), win(2, "foot", terminal, true)]));
        // Its window moved to the bottom screen, shelf in front: nothing to go back to on the main screen.
        s.apply(Event::Windows(vec![shelf_window(true), Window { down: true, ..win(2, "foot", terminal, false) }]));
        assert_eq!(s.apply(Event::Home).1, vec![]);
    }

    #[test]
    fn home_goes_back_to_a_workspace_that_isnt_ours() {
        let mut s = state();
        s.apply(Event::Windows(vec![shelf_window(false), foot()]));
        s.apply(Event::Windows(vec![shelf_window(true), Window { focused: false, ..foot() }]));
        assert_eq!(s.apply(Event::Home).1, vec![Effect::Window(WindowCommand::Focus("1".into()))]);
    }

    #[test]
    fn tick_redraws_once_a_minute() {
        let mut s = state();
        assert_eq!(s.apply(Event::tick(60)).0, Dirty::BOTH);
        assert_eq!(s.apply(Event::tick(119)).0, Dirty::NONE);
    }
}
