//! The program's state and its rules. There is no I/O here. Every change comes in as an [`Event`], and
//! what the outside world must do comes back as [`Effect`]s. The UI and the system threads only read the
//! state and send events, so all of this can be unit-tested.
//!
//! The state is made of areas, one per subject, and so are the modules. Each area is a type that owns its
//! fields privately, with its rules as methods and the getters the UI reads. A rule takes what it reads
//! of other areas as parameters, and an area it changes, where that area alone owns the change, as a
//! `&mut` parameter (a notice, the pointer's focus). The areas:
//!
//! - [`shelf`]: the library view on the main screen ([`Shelf`]), with its actions and files;
//! - [`bottom`]: the second screen's tabs ([`Bottom`]), the editor, the on-screen keyboard, and the
//!   screen's own rules;
//! - [`session`]: what is running and in front ([`Session`]), covering launches, windows, programs
//!   found, the tray, the dock and Home;
//! - [`levels`]: volume and brightness, and the slider that sets them;
//! - [`radios`]: Wi-Fi and Bluetooth;
//! - [`settings`]: the settings table and panel, the environment variables, and the fan mode;
//! - [`device`]: the Device view, the account, stick calibration, and the device buttons;
//! - [`idle`]: the screens dimmed and off, and sleep, with no input ([`Idle`]);
//! - [`status`]: the system readings ([`Sys`]), covering status, metrics, the battery and the time;
//! - [`notifications`]: the desktop notifications shown over the bottom screen ([`Notifications`]);
//! - [`presentation`]: what the compositor said of the screens and Xwayland, and the window presentation
//!   policy;
//! - [`keymap`]: keymaps and their screens;
//! - [`nav`]: focus and presses ([`Pointer`]), the lists, and the pad's routing ([`Pad`]);
//! - [`kit`]: generic parts the areas build on, which know nothing about them.
//!
//! [`State`] holds the areas and coordinates them. It is defined in `lib.rs`, after [`Init`], with what
//! builds the areas, gives each to outside code to read, and runs [`State::apply`]'s phases. What needs
//! more than one area is carried out in an `impl State` of its own file, by subject, under [`apply`].
//! What only State uses is in modules beside the areas: the record of what the outside was last sent
//! ([`sync`]), the output of one event ([`out`]), and the resume record ([`resume`]). Outside code reads
//! an area through State's accessor for it (`s.radios().wifi()`), and changes state only by sending an
//! event to [`State::apply`], which runs in three phases:
//!
//! 1. Handle: the event goes to the rules of its area. They push effects into one [`out::Out`] in the
//!    order things happen. What needs several areas, such as a tab change, playing an entry or choosing
//!    a keymap, is queued as an event, which State handles after the rule returns, so its effects follow
//!    those pushed before it. Outside code may send the same events. Rules don't say which screens
//!    they changed: after an event the UI records each screen again and draws one that looks different.
//! 2. Settle: the steps that bring every area up to date with what happened, in an order where each
//!    finds the ones it reads settled (see [`State::settle`]).
//! 3. Sync: what the outside should have as things are now is compared with what it was last sent, and
//!    the difference is sent ([`sync`]).
//!
//! The workspace lints rule out panics: no `unwrap`, `expect` or indexing. A lookup whose index is valid
//! by construction, such as an enum's position in an array sized for it, or an index held by the
//! library's own tree, falls back quietly. Where a bug could reach a fallback, a `debug_assert!` catches
//! it in tests.

mod apply;
pub mod bottom;
mod clock;
pub mod colour;
pub mod curve;
pub mod device;
mod effect;
mod event;
mod id;
pub mod idle;
pub mod keymap;
pub mod kit;
pub mod levels;
pub mod nav;
pub mod notifications;
pub(crate) mod out;
pub mod presentation;
pub mod profiles;
pub mod radios;
#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod random;
pub(crate) mod resume;
pub mod session;
pub mod settings;
pub mod shelf;
pub mod status;
pub(crate) mod sync;
#[cfg(test)]
#[allow(clippy::unwrap_used)]
pub(crate) mod testkit;
#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests;

use std::sync::Arc;

use crate::bottom::keyboard::Keyboard;
use crate::bottom::notice::NoticeKind;
use crate::idle::Idle;
use crate::keymap::screens::Keymapping;
use crate::kit::debounce::Debounce;
use crate::nav::lists::Scrolls;
use crate::session::running::Start;
use crate::settings::{MouseAtLaunch, Setting, Settings, Values};
use out::Out;
use sync::{Sent, Wanted};

// What other crates use to talk to the state: the events and effects and what they carry, the ids, and
// the groups the getters return.
pub use clock::Clock;
pub use effect::{Compositor, Effect, Save, ScreenshotPixels, Watch};
pub use event::{EntryAct, Event, KeymapEvent, LaunchEvent, LevelEvent, PointerEvent, Reading, ShelfEvent, Slide, Thread};
pub use id::{AudioId, BatteryId, BtId, DeviceId, DockId, EditorId, Hold, Id, KeymapId, SettingsId, ShelfId, Target, TimeId, WifiId};
pub use resume::Resume;
pub use session::windows::{WindowEvent, WindowPlace};

pub use bottom::editor::Editor;
pub use bottom::{Bottom, Tab};
pub use kit::list::{ItemKey, Scroll};
pub use levels::slider::{Control, Slider};
pub use levels::{Brightness, Level, Levels, Sound};
pub use nav::lists::{ListEvent, ListId, Lists, RowPart};
pub use nav::pad::{Claimant, Pad, PadInput, PadMouse, PadRoute};
pub use nav::pointer::Pointer;
pub use notifications::{CloseReason, Notification, NotificationEvent, NotificationId, Notifications, Urgency};
pub use radios::{RadioStatus, Radios};
pub use session::Session;
pub use session::dock::{Dock, DockItem};
pub use session::instance::Instance;
pub use session::running::{Front, Launching, Origin, Running};
pub use settings::panel::{GRID_COLUMNS, Panel, Prefs, Program, Scope, Source, TileOrigin};
pub use shelf::{Cursor, Shelf};
pub use status::time::{ClockField, TimePage, TimeRow, TimeZones};
pub use status::{Status, Sys};

pub use clamshell_device::Profile;
pub use clamshell_device::layout::{ControlLayout, Group as ControlGroup, Labels as ControlLabels};
pub use clamshell_library::{BrowseKind, Browsed, Collection, Entry, Launch, Library, Meta, RunnerOf, runner_key};

/// What the state starts from, as read from disk and the environment.
#[derive(Debug, Default)]
pub struct Init {
    pub profile: Profile,
    pub display_modes: presentation::modes::DisplayModes,
    /// The library, or `None` if it is read after the start and comes as [`Event::Library`]: until then
    /// shelf is empty, and what needs the library waits for it (shelf's place, the dock's pins, autostart).
    pub library: Option<Library>,
    /// Every game's settings and environment.
    pub settings: Settings,
    /// The keymaps.
    pub keymaps: keymap::Keymaps,
    /// The tone curves.
    pub curves: curve::Curves,
    /// The screens' brightness as it was left.
    pub brightness: Brightness,
    /// The dock's pinned entries, in order.
    pub dock: Dock,
    /// The entries started with ClamShell (see [`crate::session::autostart`]).
    pub autostart: Vec<Arc<str>>,
    /// Where shelf and the second screen were.
    pub resume: Option<Resume>,
    /// The tab to start in instead (a development option).
    pub start_tab: Option<Tab>,
    /// Start with the pad mouse on.
    pub pad_mouse: bool,
    /// ClamShell is the device's own session, so it may put the device to sleep when idle.
    pub device_session: bool,
    /// Which programs' windows go to the bottom screen.
    pub window_rules: crate::session::below::WindowRules,
    /// Show clocks in 12-hour format rather than 24-hour.
    pub twelve_hour: bool,
    /// This is ClamShell's first start, meaning the Device view hasn't been closed with Done yet. The
    /// Device view opens (see [`device`]).
    pub first_start: bool,
    /// The sticks' calibrations, by controller (see [`crate::device::sticks`]).
    pub sticks: crate::device::sticks::Calibrations,
}

/// Everything ClamShell knows, as its areas (see the [crate docs](crate)).
#[derive(Debug, Default)]
pub struct State {
    profile: Profile,
    shelf: Shelf,
    bottom: Bottom,
    session: Session,
    dock: Dock,
    /// The Device view on the main screen (see [`crate::device`]).
    device: device::Device,
    levels: Levels,
    radios: Radios,
    pad: Pad,
    pointer: Pointer,
    sys: Sys,
    notifications: Notifications,
    /// Every entry's settings, and the scope the settings panel edits.
    prefs: Prefs,
    keyboard: Keyboard,
    /// The keymaps and their screens (see [`crate::keymap::screens`]).
    keymapping: Keymapping,
    curving: crate::curve::screens::Curving,
    presentation: crate::presentation::Presentation,
    /// Where the lists but shelf are scrolled.
    scrolls: Scrolls,
    clock: Clock,
    /// Where shelf and the second screen are ([`State::resume`]), saved once they stop changing for a
    /// moment. Shelf and the bottom screen say when they changed; `apply` touches this at its end.
    resume: Debounce,
    idle: Idle,
    /// The UI is up (Start came). Before that, no tab or account is watched.
    ui_up: bool,
    /// What the outside was last sent (see [`crate::sync`]).
    sent: Sent,
    /// The events the rules queue for State, lent to `apply`'s [`Out`] and returned empty, so that its
    /// room is kept.
    queue: Vec<Event>,
    /// The plays the session's steps ended, until they are recorded in shelf. Kept empty, so that its
    /// room is reused.
    plays: Vec<crate::session::running::Play>,
}

impl State {
    // Construction and the areas.

    pub fn new(init: Init) -> State {
        let mut s = State {
            profile: init.profile,
            shelf: init.library.map_or_else(|| Shelf::unread(init.resume.clone()), Shelf::new),
            prefs: Prefs::new(init.settings),
            keymapping: Keymapping::new(init.keymaps),
            curving: crate::curve::screens::Curving::new(init.curves),
            bottom: Bottom::default(),
            sys: Sys::new(init.twelve_hour),
            levels: Levels::new(init.brightness),
            session: Session::new(init.autostart, init.device_session, init.window_rules),
            dock: init.dock,
            device: device::Device::new(init.sticks, init.first_start),
            ..State::default()
        };
        if let Some(resume) = &init.resume {
            resume.restore_tab(&mut s.bottom);
            if s.shelf.read() {
                resume.restore_shelf(&mut s.shelf);
            }
        }
        s.presentation.read_display_modes(init.display_modes);
        // Unread, the pins are checked as the library comes (`library_read`).
        if s.shelf.read() {
            s.dock.drop_gone_at_start(s.shelf.library(), &s.session);
        }
        s.session.set_pausable_runners(s.shelf.library());
        s.pointer.focus_shelf(s.shelf.selected());
        if init.first_start {
            s.pointer.focus_on(Id::Device(crate::DeviceId::Wifi));
        }
        if let Some(tab) = init.start_tab {
            s.bottom.restore_tab(tab);
        }
        s.reconcile_pad();
        if init.pad_mouse {
            s.pad.toggle_mouse();
            s.reconcile_pad();
        }
        s.sent = Sent::new(&s.wanted());
        s
    }

    pub fn profile(&self) -> &Profile {
        &self.profile
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

    pub fn dock(&self) -> &Dock {
        &self.dock
    }

    pub fn device(&self) -> &device::Device {
        &self.device
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

    pub fn notifications(&self) -> &Notifications {
        &self.notifications
    }

    pub fn prefs(&self) -> &Prefs {
        &self.prefs
    }

    pub fn keyboard(&self) -> &Keyboard {
        &self.keyboard
    }

    pub fn keymapping(&self) -> &Keymapping {
        &self.keymapping
    }

    pub fn presentation(&self) -> &crate::presentation::Presentation {
        &self.presentation
    }

    pub fn curving(&self) -> &crate::curve::screens::Curving {
        &self.curving
    }

    /// The lists, their rows and where each is scrolled (see [`Lists`]).
    pub fn lists(&self) -> Lists<'_> {
        Lists {
            scrolls: &self.scrolls,
            shelf: &self.shelf,
            session: &self.session,
            prefs: &self.prefs,
            keymapping: &self.keymapping,
            curving: &self.curving,
            radios: &self.radios,
            levels: &self.levels,
            sys: &self.sys,
            dock: &self.dock,
            pointer: &self.pointer,
            menu_rows: self.menu_rows_len(),
        }
    }

    pub fn idle(&self) -> &crate::idle::Idle {
        &self.idle
    }

    pub fn clock(&self) -> &Clock {
        &self.clock
    }

    // Applying an event: handle, settle, sync.

    /// Applies one event and appends what must happen outside to `effects`. After the event itself, the
    /// state is settled ([`State::settle`]), and then the outside is told what changed for it
    /// ([`State::sync`]).
    pub fn apply(&mut self, event: Event, effects: &mut Vec<Effect>) {
        let mut out = Out { effects: std::mem::take(effects), events: std::mem::take(&mut self.queue) };
        self.apply_event(event, &mut out);
        self.settle(&mut out);
        self.sync(&mut out);
        // Both are taken, so neither is left marked.
        if self.shelf.take_unsaved() | self.bottom.take_unsaved() {
            self.resume.touch(self.clock.mono());
        }
        // The follow-up steps don't queue events: one queued here would never be handled.
        debug_assert!(out.events.is_empty(), "an event queued after the event was handled: {:?}", out.events);
        *effects = out.effects;
        out.events.clear();
        self.queue = out.events;
    }

    /// Hands `event` to the rule that handles it, then handles what that queued.
    fn apply_event(&mut self, event: Event, out: &mut Out) {
        // Where the events this one queues start.
        let mark = out.events.len();
        let now = self.clock.mono();
        match event {
            Event::Tick { unix_secs, mono_secs } => self.tick(unix_secs, mono_secs, out),
            Event::Reading(reading) => self.on_reading(reading, out),
            Event::Notification(e) => self.on_notification(e, out),
            Event::List(e) => self.on_list(e),
            Event::Pointer(e) => self.on_pointer(e, out),
            Event::Slide(slide) => self.on_slide(slide, out),
            Event::Level(e) => self.on_level(e, out),
            Event::Shelf(e) => self.shelf.on_event(e, &mut self.pointer, out),
            Event::Launch(e) => self.on_launch_event(e),
            Event::Window(event) => {
                self.window_event(event, out);
                self.session.remember_front();
            }
            Event::WindowBatch(events) => {
                for event in events {
                    self.window_event(event, out);
                }
                self.session.remember_front();
            }
            Event::Meta { id, meta } => self.shelf.apply_meta(&id, &meta),
            Event::Library(library) => self.library_read(*library, out),
            Event::Files(e) => self.shelf.on_files(e, &mut self.pointer, self.bottom.notice_mut(), now, out),
            Event::Fetching(on) => self.fetching(on),
            Event::Tray(items) => self.session.set_tray(items, &self.clock, self.shelf.library(), out),
            Event::Units(units) => self.session.set_units(units),
            Event::Start => self.ui_started(out),
            Event::Home => self.home(),
            Event::HomeHeld(held) => self.session.hold_home(held),
            Event::HomeTap => {
                if self.session.home_tap() {
                    self.home();
                }
            }
            Event::PadActive => self.session.pad_active(),
            Event::Listened(input) => self.keymapping.listened(input, self.pad.route() == PadRoute::Listen, out),
            Event::PadSwitch => self.pad.switch(),
            // Never applied here: the UI handles pad navigation itself.
            Event::Pad(_) => {}
            Event::TextInput(active) => self.text_input(active),
            Event::Pointing(pointing) => self.bottom.point(pointing, out),
            Event::Resumed => self.resumed(),
            Event::Idle(event) => self.idle.heard(event, now),
            Event::Device(key) => self.on_device_key(key, out),
            Event::BottomTouched => self.bottom.used(&self.session, &self.clock),
            Event::Frozen(keys) => self.session.found_frozen(keys),
            Event::Paste => self.bottom.paste(),
            Event::DumpWindowEvents(path) => out.effects.push(Effect::Compositor(Compositor::DumpWindowEvents(path))),
            Event::Screenshot(result) => self.bottom.notice_mut().show(NoticeKind::Screenshot(result), now),
            Event::Sticks(event) => self.device.sticks_event(event, now, out),
            Event::RefreshCalibrated(result) => self.refresh_calibrated(result),
            Event::Restarted(thread) => self.restarted(thread, now, out),
            Event::CalibrationRate(hz) => self.device.calibration_asks(hz),
            Event::CalibrateGyro => self.calibrate_gyro(out),
            Event::GyroCalibrated(ok) => self.bottom.notice_mut().show(NoticeKind::Gyro(Some(ok)), now),
            // What the helper did, or why it failed, shown for a moment.
            Event::AccountChanged(result) => {
                self.device.account_changed();
                self.bottom.notice_mut().show(NoticeKind::Account(result), now);
            }
            Event::ChooseTab(tab) => self.choose_tab(tab),
            Event::Entry(id, act) => self.on_entry(id, act, out),
            Event::Enqueue => self.enqueue(out),
            Event::Keymap(e) => self.keymap_event(e, out),
            Event::Profile(kind, e) => self.profile_event(kind, e, out),
            Event::EditOnKeyboard(editor) => self.edit_on_keyboard(editor),
            Event::Quit => self.quit(out),
        }
        self.handle_queued(mark, out);
    }

    fn window_event(&mut self, event: WindowEvent, out: &mut Out) {
        if self.session.on_window(event, self.shelf.library(), out) {
            self.bottom.show_below();
        }
    }

    /// Handles the events queued from `mark` on: what the event's rules asked of State, in the order they
    /// asked, after the effects they pushed. Each is handled with what it queues in turn, before the next.
    fn handle_queued(&mut self, mark: usize, out: &mut Out) {
        while out.events.len() > mark {
            let queued = out.events.remove(mark);
            self.apply_event(queued, out);
        }
    }

    /// Brings the state up to date with what happened. The steps run in this order, since each reads
    /// what the earlier ones settled. They run after every event; a step does nothing when nothing
    /// changed for it. Where an order is pinned by a test, the test is named.
    ///
    /// 1. systemd's units: entries found running, found ones promoted to launches, and ours ended
    ///    ([`crate::Session::follow_units`]). The first time, once what runs is known, the entries to
    ///    start with ClamShell are started
    ///    (`chosen_entries_start_once_what_runs_is_known_unless_they_run`).
    /// 2. Entries started by their windows' app ids ([`crate::Session::start_by_app_id`]), and Steam's
    ///    games among them launched before the windows are followed, since a starting launch's window is
    ///    left where it is (`a_steam_game_found_by_its_window_is_launched_not_found_as_a_program`).
    /// 3. The windows: moved to their entries, given to a browsed file waiting for one, programs found by
    ///    them, the rest given groups of their own, and entries ended by them
    ///    ([`crate::Session::follow_windows`], which compares with the windows the last pass saw).
    /// 4. The window rules send windows down, which reads the owners 3 found
    ///    (`a_found_programs_window_goes_down_by_its_rule_as_it_is_found`), and the tab a window moved
    ///    down left is settled as any tab left (`the_tab_a_rule_leaves_is_settled_in_the_same_event`).
    /// 5. What is shown: a launch's first window, then a group whose window came back, so the one that
    ///    came back ends in front (`a_launchs_first_window_is_shown_before_a_window_that_came_back`);
    ///    then going back to what was before when the window in front closed.
    /// 6. In the background: entries paused, thawed or ended by their setting, once the front is settled
    ///    (`a_paused_game_shown_when_the_front_closes_is_thawed_at_once`).
    /// 7. Quitting entries are ended once their windows are gone, after 6, which a quitting entry isn't
    ///    paused for (`the_background_is_followed_before_the_quitting`).
    /// 8. The dock: running entries are added to it and ended ones removed.
    /// 9. The windows and the group in front are remembered for the next pass's 3 and 5
    ///    ([`crate::Session::followed`]; the tests of 5).
    ///
    /// Then the rest:
    ///
    /// 10. The second screen's dimming.
    /// 11. The browsed folder's selection and armed action.
    /// 12. The idle stage.
    /// 13. The program in force.
    /// 14. The second screen turning off or on by itself, which reads the program in force's setting and
    ///     darkness (`dark_for_a_game_on_battery_it_goes_off_and_a_tap_wakes_it`).
    /// 15. The notifications' times, which run while the second screen is lit, as 12 and 14 left it
    ///     (`a_notification_waits_for_the_bottom_screen_and_then_shows_its_time`).
    /// 16. The pad mouse, when the program in force changed.
    /// 17. What claims the pad, and where it goes. The focus ring shows where the pad is, so it is hidden
    ///     when the pad leaves ClamShell's screens.
    /// 18. The scrolled lists keep their top items where they are ([`State::follow_lists`]).
    fn settle(&mut self, out: &mut Out) {
        if self.session.follow_units(&self.clock, self.shelf.library(), self.bottom.notice_mut(), &mut self.plays) {
            self.bottom.unwake();
        }
        self.record_plays(out);
        let mut due = Vec::new();
        // The entries to start are looked for in the library, so not before it is read.
        if self.shelf.read() {
            self.session.autostart_due(self.shelf.library(), &mut due);
        }
        for id in due {
            self.launch(id, None, Start::Background, out);
        }
        let mut steam = Vec::new();
        if self.session.start_by_app_id(&self.clock, self.shelf.library(), &mut steam) {
            self.bottom.unwake();
        }
        // Before following the windows, which leaves a starting launch's windows where they are.
        for id in steam {
            self.launch(id, None, Start::Front, out);
        }
        self.session.follow_windows(&self.clock, self.shelf.library(), &mut self.plays, out);
        self.record_plays(out);
        // After the windows are followed, which finds their owners.
        if self.session.follow_window_rules(self.shelf.library()) {
            self.bottom.show_below();
        }
        // A window moved down opened the windows' tab; the tab it left settles as any tab left.
        if let Some(left) = self.bottom.take_left() {
            self.left_tab(left);
        }
        self.session.follow_front_launch(self.clock.mono());
        self.session.follow_replacement(self.clock.mono());
        self.session.follow_front(self.clock.mono());
        // Once the front is settled, so a program shown again when the front window closed is thawed at
        // once; before the quitting, which a quitting entry isn't paused for.
        self.session.follow_background(self.clock.mono(), self.shelf.library(), self.prefs.settings(), out);
        self.session.follow_quitting(self.clock.mono(), self.shelf.library(), out);
        self.dock.follow(&self.session);
        self.session.followed();
        self.bottom.follow_dim(&self.session, &self.clock);
        self.shelf.follow_folder();
        self.radios.follow(self.clock.mono());
        // Sleep only in the device's own session. A program in front doesn't hold it off; one playing a
        // video asks the compositor to keep the screen on.
        let policy = crate::idle::Policy::of(&self.prefs.settings().global, self.session.device_session());
        let refreshing = self.device.calibrating();
        self.idle.follow(self.clock.mono(), policy, refreshing, self.session.awake_command(), out);
        let changed = self.session.follow_program(self.clock.mono());
        self.bottom.follow_power(self.power(), self.clock.mono());
        let lit = self.bottom_lit();
        self.notifications.follow(self.clock.mono(), lit, out);
        if changed {
            self.follow_pad_mouse();
        }
        if self.reconcile_pad().is_some_and(|route| route != PadRoute::Ui) {
            self.pointer.hide_focus();
        }
        self.follow_lists();
    }

    /// Records in shelf the plays the session's steps ended.
    fn record_plays(&mut self, out: &mut Out) {
        for (entry, start) in self.plays.drain(..) {
            self.shelf.record_play(entry, start, &self.clock, &mut self.pointer, out);
        }
    }

    /// Called when the program in force changed. Turns the pad mouse on if the program wants it
    /// (Setting::PadMouse), and off again if the program had turned it on.
    fn follow_pad_mouse(&mut self) {
        let wanted = self.session.program().is_some() && self.in_force().choice::<MouseAtLaunch>() == MouseAtLaunch::On;
        self.pad.follow_game(wanted);
    }

    /// Works out where the pad goes as things are now. Returns the new route if it changed.
    fn reconcile_pad(&mut self) -> Option<PadRoute> {
        let listening = self.listening();
        let moving = self.dock.moving().is_some() && self.bottom.tab() == Tab::Dock;
        self.pad.reconcile(self.session.front(), self.bottom.tab(), listening, moving)
    }

    /// What the outside should have as things are now. The second screen watches what the open tab
    /// watches; the dock with an entry's menu open watches the battery tab's readings, to show what the
    /// entry costs. It can be seen when ClamShell's view is on the bottom screen and that screen is on.
    fn wanted(&self) -> Wanted<'_> {
        let tab = match self.bottom.tab() {
            Tab::Dock if self.dock.menu().is_some() => Some(Watch::Battery),
            tab => tab.watch(),
        };
        let settings = self.in_force();
        Wanted {
            shelf: &self.shelf,
            fetch_view: self.shelf.fetch_first(),
            browsed: self.shelf.browsed(),
            hidden: self.shelf.files().hidden(),
            running: self.session.running_entries(),
            stage: self.idle.stage(),
            levels: &self.levels,
            program: self.session.program(),
            settings,
            refresh: self.refresh_wanted(&settings),
            // Off while the refresh rate calibration runs, so only the main screen's vblanks come.
            bottom_on: !self.bottom.off() && !self.device.calibrating(),
            bottom_seen: !self.bottom.shows_windows(&self.session) && !self.bottom.off(),
            tab,
            device_open: self.device.is_open(),
            ui_up: self.ui_up,
            keymap: self.keymapping.version_of(settings.get(Setting::Keymap)),
            keymapping: &self.keymapping,
            route: self.pad.route(),
            pad_activity: self.session.top_window().is_some_and(|w| Some(w.id) != self.session.keyboard_focus()),
        }
    }

    /// The main screen's refresh rate (see [`crate::sync::Wanted::refresh`]), with `in_force` the
    /// settings in force.
    fn refresh_wanted(&self, in_force: &Values) -> u16 {
        if let Some(hz) = self.device.calibration_rate() {
            return hz;
        }
        let settings = self.session.refresh_launch().map_or(*in_force, |id| self.settings_of(id));
        self.presentation.display_modes().refresh_for(settings.get(Setting::Refresh), settings.get(Setting::FpsCap))
    }

    /// Sends the outside what differs from what it was last sent.
    fn sync(&mut self, out: &mut Out) {
        let mut sent = std::mem::take(&mut self.sent);
        sent.sync(&self.wanted(), out);
        self.sent = sent;
    }
}
