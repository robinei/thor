//! The program's state and its rules. There is no I/O here. Every change comes in as an [`Event`], and
//! what the outside world must do comes back as [`Effect`]s. The UI and the system threads only read the
//! state and send events, so all of this can be unit-tested.
//!
//! The state is made of areas, one per subject, and so are the modules. Each area is a type that owns its
//! fields privately, with its rules as methods and the getters the UI reads. A rule takes what it reads
//! of other areas as parameters, and an area it changes, where that area alone owns the change, as a
//! `&mut` parameter (a notice, the pointer's focus). The areas:
//!
//! - [`library_view`]: the library view on the main screen ([`LibraryView`]), with its actions and files;
//! - [`bottom`]: the second screen's tabs ([`Bottom`]), the editor, the on-screen keyboard, and the
//!   screen's own rules;
//! - [`session`]: what is running and in front ([`Session`]), covering launches, windows, programs
//!   found, the tray, the dock and Home;
//! - [`levels`]: volume and brightness, and the slider that sets them;
//! - [`radios`]: Wi-Fi and Bluetooth;
//! - [`settings`]: the settings table and panel, the environment variables, and the fan mode;
//! - [`builtin_apps`]: what each of ClamShell's own apps on the main screen does (see
//!   [`builtin_apps::BuiltinAppDefinition`]);
//! - [`device_view`]: the Device view: stick, gyro and refresh calibration, and remote login's answer;
//! - [`setup`]: Setup, the first start's steps, and the account;
//! - [`idle`]: the screens dimmed and off, and sleep, with no input ([`Idle`]);
//! - [`status`]: the system readings ([`Sys`]), covering status, metrics, the battery and the time;
//! - [`notifications`]: the desktop notifications shown over the bottom screen ([`Notifications`]);
//! - [`presentation`]: what the compositor said of the screens and Xwayland, and the window presentation
//!   policy;
//! - [`keymap`]: keymaps and their screens;
//! - [`curve`]: tone curves and their screens;
//! - [`lights`]: the stick lights and their screens;
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
pub mod builtin_apps;
mod clock;
pub mod colour;
pub mod curve;
pub mod device_view;
pub mod dock;
mod effect;
mod event;
mod id;
pub mod idle;
pub mod keymap;
pub mod kit;
pub mod levels;
pub mod library_view;
pub mod lights;
pub mod nav;
pub mod notifications;
pub(crate) mod out;
pub use out::LogLevel;
pub mod portal;
pub mod presentation;
pub mod profiles;
pub mod radios;
#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod random;
pub(crate) mod resume;
pub mod session;
pub mod settings;
pub mod setup;
pub mod shown;
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
pub use apply::keys::HardwareKey;
pub use clock::Clock;
pub use effect::{Compositor, Effect, Save, ScreenshotPixels, Watch};
pub use event::{EntryAct, Event, KeymapEvent, LaunchEvent, LevelEvent, LibraryViewEvent, PointerEvent, Reading, Slide, Thread};
pub use id::{AudioId, BatteryId, BtId, DeviceId, DockId, EditorId, Hold, Id, KeymapId, LibraryViewId, SettingsId, SetupId, Target, TimeId, WifiId};
pub use resume::Resume;
pub use session::windows::{WindowEvent, WindowPlace};

pub use bottom::editor::Editor;
pub use bottom::{Bottom, Tab};
pub use dock::{Dock, DockItem};
pub use kit::list::{ItemKey, Scroll};
pub use levels::slider::{Control, Slider};
pub use levels::{Brightness, Level, Levels, Sound};
pub use library_view::{Cursor, LibraryView};
pub use nav::lists::{ListEvent, ListId, Lists, RowPart};
pub use nav::pad::{Claimant, Pad, PadInput, PadMouse, PadRoute};
pub use nav::pointer::Pointer;
pub use notifications::{CloseReason, Notification, NotificationEvent, NotificationId, Notifications, Urgency};
pub use portal::{Asks, FileRequest, PortalEvent};
pub use radios::{RadioStatus, Radios};
pub use session::Session;
pub use session::launch_unit::LaunchUnit;
pub use session::running::{Launching, Origin, Running};
pub use settings::panel::{GRID_COLUMNS, Panel, Prefs, Program, SettingsLevel, Source, TileOrigin};
pub use shown::Top;
pub use status::time::{ClockField, TimePage, TimeRow, TimeZones};
pub use status::{Status, Sys};

pub use clamshell_device::DeviceProfile;
pub use clamshell_device::layout::{ControlLayout, Group as ControlGroup, Labels as ControlLabels};
pub use clamshell_library::{BrowseKind, Browsed, BuiltinApp, Collection, Entry, Launch, Library, Meta, RunnerOf, config, ids, runner_key};

/// What the state starts from, as read from disk and the environment.
#[derive(Debug, Default)]
pub struct Init {
    pub profile: DeviceProfile,
    pub display_modes: presentation::modes::DisplayModes,
    /// The library, or `None` if it is read after the start and comes as [`Event::Library`]: until then
    /// the Library is empty, and what needs the library waits for it (the Library's place, the dock's pins, autostart).
    pub library: Option<Library>,
    /// Every game's settings and environment.
    pub settings: Settings,
    /// The keymaps.
    pub keymaps: keymap::Keymaps,
    /// The tone curves.
    pub curves: curve::Curves,
    /// The sets of stick lights.
    pub lights: lights::LightSets,
    /// The screens' brightness as it was left.
    pub brightness: Brightness,
    /// The dock's pinned entries, in order.
    pub dock: Dock,
    /// The entries started with ClamShell (see [`crate::session::autostart`]).
    pub autostart: Vec<Arc<str>>,
    /// Where the Library and the second screen were.
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
    /// The account as known before the state starts: ClamShell's starts unread (tests and the snapshots
    /// give one, see [`crate::setup::account::Account`]).
    pub account: crate::setup::account::Account,
    /// This is ClamShell's first start: Setup hasn't been finished with Done yet, so it shows (see
    /// [`setup`]).
    pub first_start: bool,
    /// The sticks' calibrations, by controller (see [`crate::device_view::sticks`]).
    pub sticks: crate::device_view::sticks::Calibrations,
    /// How many players there are, and every controller seen with its player (see
    /// [`crate::device_view::controllers`]).
    pub controllers: crate::device_view::controllers::Saved,
}

/// Everything ClamShell knows, as its areas (see the [crate docs](crate)).
#[derive(Debug, Default)]
pub struct State {
    profile: DeviceProfile,
    library_view: LibraryView,
    bottom: Bottom,
    session: Session,
    dock: Dock,
    /// Setup, the first start's steps, and the account (see [`crate::setup`]).
    setup: setup::Setup,
    /// The Device view on the main screen (see [`crate::device_view`]).
    device_view: device_view::DeviceView,
    levels: Levels,
    radios: Radios,
    pad: Pad,
    pointer: Pointer,
    sys: Sys,
    notifications: Notifications,
    /// What went wrong and was worked around, until taken (see [`State::take_log`]).
    log: Vec<(out::LogLevel, String)>,
    /// Apps' file choosers waiting (see [`portal`]).
    portal: portal::Portal,
    /// Every entry's settings, and the level the settings panel edits.
    prefs: Prefs,
    /// The slider held, while a finger is on it: a level's in the bar, a keymap's or a curve's parameter's,
    /// or a setting's (see [`apply::sliders`]). Its control says whose.
    slider: Option<Slider>,
    keyboard: Keyboard,
    /// The keymaps and their screens (see [`crate::keymap::screens`]).
    keymapping: Keymapping,
    curving: crate::curve::screens::Curving,
    /// The sets of stick lights and their screens (see [`crate::lights::screens`]).
    lighting: crate::lights::screens::Lighting,
    presentation: crate::presentation::Presentation,
    /// Where the lists but the Library are scrolled.
    scrolls: Scrolls,
    clock: Clock,
    /// Where the Library and the second screen are ([`State::resume`]), saved once they stop changing for a
    /// moment. The Library and the bottom screen say when they changed; `apply` touches this at its end.
    resume: Debounce,
    idle: Idle,
    /// The UI is up (Start came). Before that, no tab or account is watched.
    ui_up: bool,
    /// What the outside was last sent (see [`crate::sync`]).
    sent: Sent,
    /// The second screen's tab as the last settle saw it, to tell the tab left (see [`State::follow_tab`]).
    settled_tab: Tab,
    /// The Library's place as the last settle saw it, to tell when it moved (see [`State::follow_library_view`]).
    settled_library_view: (usize, Option<usize>, Option<Arc<str>>),
    /// A hash of the place as a resume saves it, as the last event left it, to tell when it changed (see
    /// [`State::place_hash`]).
    saved_place: u64,
    /// The events the rules queue for State, lent to `apply`'s [`Out`] and returned empty, so that its
    /// room is kept.
    queue: Vec<Event>,
    /// The plays the session's steps ended, until they are recorded in the Library. Kept empty, so that its
    /// room is reused.
    plays: Vec<crate::session::running::Play>,
    /// The entries settle starts (autostart's, then those Steam started), and the launcher games due after
    /// their launcher's launch; kept empty between settles, so that their room is reused as `plays`' is.
    starting: Vec<Arc<str>>,
    after_launchers: Vec<(Arc<str>, Start)>,
}

impl State {
    // Construction and the areas.

    pub fn new(init: Init) -> State {
        let mut s = State {
            profile: init.profile,
            library_view: init.library.map_or_else(|| LibraryView::unread(init.resume.clone()), LibraryView::new),
            prefs: Prefs::new(init.settings),
            keymapping: Keymapping::new(init.keymaps),
            curving: crate::curve::screens::Curving::new(init.curves),
            lighting: crate::lights::screens::Lighting::new(init.lights),
            bottom: Bottom::default(),
            sys: Sys::new(init.twelve_hour),
            levels: Levels::new(init.brightness),
            session: Session::new(init.autostart, init.device_session, init.window_rules),
            dock: init.dock,
            setup: setup::Setup::new(init.first_start, init.account),
            device_view: device_view::DeviceView::new(init.sticks, init.controllers),
            ..State::default()
        };
        if let Some(resume) = &init.resume {
            resume.restore_tab(&mut s.bottom);
            if s.library_view.read() {
                resume.restore_library_view(&mut s.library_view);
            }
        }
        s.presentation.read_display_modes(init.display_modes);
        // Unread, the pins are checked as the library comes (`library_read`).
        if s.library_view.read() {
            s.dock.drop_gone_at_start(s.library_view.library(), &s.session);
        }
        s.pointer.focus_library_view(s.library_view.selected());
        if init.first_start {
            s.pointer.focus_on(Id::Setup(crate::SetupId::Password));
        }
        if let Some(tab) = init.start_tab {
            s.bottom.restore_tab(tab);
        }
        // What was restored is where the last settle and save left things.
        s.settled_tab = s.bottom.tab();
        let (current, open, selected) = s.library_view.place_key();
        s.settled_library_view = (current, open, selected.cloned());
        s.saved_place = s.place_hash();
        s.reconcile_pad();
        if init.pad_mouse {
            s.pad.toggle_mouse();
            s.reconcile_pad();
        }
        s.sent = Sent::new(&s.wanted());
        s
    }

    pub fn profile(&self) -> &DeviceProfile {
        &self.profile
    }

    pub fn library_view(&self) -> &LibraryView {
        &self.library_view
    }

    pub fn bottom(&self) -> &Bottom {
        &self.bottom
    }

    /// Whether the bottom screen is drawn dimmed now (see [`Bottom::dimmed_at`]).
    pub fn bottom_dimmed(&self) -> bool {
        self.bottom.dimmed_at(&self.session, crate::idle::dim_after(&self.prefs.settings().global), self.clock.mono())
    }

    /// Whether the bottom screen is off now, by hand or by itself (see [`Bottom::off_at`]).
    pub fn bottom_off(&self) -> bool {
        self.bottom.off_at(self.power(), self.clock.mono())
    }

    pub fn session(&self) -> &Session {
        &self.session
    }

    pub fn dock(&self) -> &Dock {
        &self.dock
    }

    pub fn setup(&self) -> &setup::Setup {
        &self.setup
    }

    pub fn device_view(&self) -> &device_view::DeviceView {
        &self.device_view
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

    pub fn lighting(&self) -> &crate::lights::screens::Lighting {
        &self.lighting
    }

    /// The lists, their rows and where each is scrolled (see [`Lists`]).
    pub fn lists(&self) -> Lists<'_> {
        Lists {
            scrolls: &self.scrolls,
            library_view: &self.library_view,
            session: &self.session,
            prefs: &self.prefs,
            keymapping: &self.keymapping,
            curving: &self.curving,
            lighting: &self.lighting,
            players: self.device_view.controllers().count(),
            radios: &self.radios,
            levels: &self.levels,
            sys: &self.sys,
            dock: &self.dock,
            pointer: &self.pointer,
            subject: self.subject(),
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
        let mut out = Out { effects: std::mem::take(effects), events: std::mem::take(&mut self.queue), log: std::mem::take(&mut self.log) };
        self.apply_event(event, &mut out);
        self.settle(&mut out);
        self.sync(&mut out);
        // The place is saved a while after it last changed (see [`State::resume`]).
        let place = self.place_hash();
        if place != self.saved_place {
            self.saved_place = place;
            self.resume.touch(self.clock.mono());
        }
        // The follow-up steps don't queue events: one queued here would never be handled, so it is a bug,
        // and in a release build at least logged.
        debug_assert!(out.events.is_empty(), "an event queued after the event was handled: {:?}", out.events);
        for dropped in out.events.drain(..) {
            out.log.push((LogLevel::Warn, format!("an event queued after the event was handled, dropped: {dropped:?}")));
        }
        *effects = out.effects;
        self.queue = out.events;
        self.log = out.log;
    }

    /// What went wrong and was worked around since the last look, a line each with its level, for the
    /// journal: the state's fallbacks, which it can't log itself.
    pub fn take_log(&mut self) -> std::vec::Drain<'_, (out::LogLevel, String)> {
        self.log.drain(..)
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
            Event::Portal(e) => self.on_portal(e, out),
            Event::List(e) => self.on_list(e),
            Event::Pointer(e) => self.on_pointer(e, out),
            Event::Slide(slide) => self.on_slide(slide, out),
            Event::Level(e) => self.on_level(e, out),
            Event::LibraryView(e) => self.library_view.on_event(e, &mut self.pointer, out),
            Event::Launch(e) => self.on_launch_event(e, out),
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
            Event::Meta { id, meta } => self.library_view.apply_meta(&id, &meta),
            Event::Library(library) => self.library_read(*library, out),
            Event::LibraryWritten(library) => {
                self.library_view.written_read();
                self.library_read(*library, out);
            }
            Event::Files(e) => self.library_view.on_files(e, self.bottom.notice_mut(), now, out),
            Event::NotSaved(file) => self.bottom.notice_mut().show(NoticeKind::NotSaved(file), now),
            Event::Fetching(on) => self.fetching(on),
            Event::Tray(items) => self.session.set_tray(items, &self.clock, self.library_view.library(), out),
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
            Event::HardwareKey(key) => self.on_hardware_key(key, out),
            // Input, timed here rather than by the input thread's reports, which come at most every few
            // seconds, so that both screens count from the same touch.
            Event::BottomTouched => {
                self.idle.own_input(now);
                self.bottom.used(&self.clock);
            }
            Event::Frozen(keys) => self.session.found_frozen(keys),
            Event::Paste => self.bottom.paste(),
            Event::DumpWindowEvents(path) => out.effects.push(Effect::Compositor(Compositor::DumpWindowEvents(path))),
            Event::StartedWithoutSettings(entry) => {
                let title = self.library_view.library().entry(&entry).map_or_else(|| entry.to_string(), |g| g.title.clone());
                out.log(LogLevel::Warn, format!("{entry}: started by Steam without ClamShell's settings (see the journal, clamshell-steam)"));
                self.bottom.notice_mut().show(NoticeKind::WithoutSettings(title), now);
            }
            Event::Screenshot(result) => self.bottom.notice_mut().show(NoticeKind::Screenshot(result), now),
            Event::Sticks(event) => self.device_view.sticks_event(event, now, out),
            Event::Controller(event) => self.device_view.controller_event(event, self.bottom.notice_mut(), now, out),
            Event::RefreshCalibrated(result) => self.refresh_calibrated(result),
            Event::Restarted(thread) => self.restarted(thread, now, out),
            Event::CalibrationRate(hz) => self.device_view.calibration_asks(hz),
            Event::CalibrateGyro => self.calibrate_gyro(out),
            Event::GyroCalibrated(ok) => self.bottom.notice_mut().show(NoticeKind::Gyro(Some(ok)), now),
            // What the helper did, or why it failed, shown for a moment.
            Event::AccountChanged(result) => {
                self.setup.account_changed(&result);
                self.device_view.account_answered();
                self.bottom.notice_mut().show(NoticeKind::Account(result), now);
            }
            Event::ChooseTab(tab) => self.choose_tab(tab),
            Event::Entry(id, act) => self.on_entry(id, act, out),
            Event::Overview(event) => self.session.on_overview(event),
            Event::Enqueue => self.enqueue(out),
            Event::Keymap(e) => self.keymap_event(e, out),
            Event::Profile(kind, e) => self.profile_event(kind, e, out),
            Event::EditOnKeyboard(editor) => self.edit_on_keyboard(editor),
            Event::Quit => self.quit(out),
        }
        self.handle_queued(mark, out);
    }

    fn window_event(&mut self, event: WindowEvent, out: &mut Out) {
        if self.session.on_window(event, self.library_view.library(), self.clock.mono(), out) {
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
    ///    ([`crate::Session::follow_units`]). The first time, once what runs is known, the library is read
    ///    and the account lets programs run, the entries to start with ClamShell are started
    ///    (`chosen_entries_start_once_what_runs_is_known_unless_they_run`); then a launcher's games that
    ///    waited for its launch (`a_launchers_second_game_waits_for_the_launchers_launch`).
    /// 2. Entries started by their windows' app ids ([`crate::Session::start_by_app_id`]), and Steam's
    ///    games among them launched before the windows are followed, since a starting launch's window is
    ///    left where it is (`a_steam_game_found_by_its_window_is_launched_not_found_as_a_program`).
    /// 3. The windows: claimed by a browsed file waiting for one, entries ended by them, programs found by
    ///    them, new ones of no program claimed by the program in front, and each placed in its group
    ///    ([`crate::Session::follow_windows`], which compares with the windows the last pass saw). Owners
    ///    are worked out from facts and claims, so each of these sees the same.
    /// 4. The window rules send windows down, which reads the owners 3 found
    ///    (`a_found_programs_window_goes_down_by_its_rule_as_it_is_found`); while Setup shows, the second
    ///    screen takes the tab its step needs; then the tab left since the last settle, by a choice, a
    ///    window moved down or Setup, is settled ([`State::follow_tab`],
    ///    `the_tab_a_rule_leaves_is_settled_in_the_same_event`).
    /// 5. What is shown: a launch's first window, then a group whose window came back, so the one that
    ///    came back ends in front (`a_launchs_first_window_is_shown_before_a_window_that_came_back`);
    ///    then going back to what was before when the window in front closed; then the launcher of a
    ///    launcher's game still without its window after a while ([`crate::Session::follow_launcher`]).
    ///    Once that front is settled,
    ///    an app needed now (Setup) is put in front and one no longer needed closed (`State::follow_needed`),
    ///    and apps' file choosers are picked over their program or set
    ///    aside ([`State::follow_portal`]); then the overview follows what is in front.
    /// 6. In the background: entries paused, thawed or ended by their setting, once the front is settled
    ///    (`a_paused_game_shown_when_the_front_closes_is_thawed_at_once`).
    /// 7. Quitting entries are ended once their windows are gone, after 6, which a quitting entry isn't
    ///    paused for (`the_background_is_followed_before_the_quitting`). Whether an entry is quitting is
    ///    worked out from when its quit was asked (`Quitting::going_on`), so no step gives quits up first.
    /// 8. The dock: running entries are added to it and ended ones removed.
    /// 9. The windows and the group in front are remembered for the next pass's 3 and 5
    ///    ([`crate::Session::followed`]; the tests of 5).
    ///
    /// Then the rest:
    ///
    /// 10. (Whether the second screen is dimmed, or off by itself, is worked out when asked
    ///     ([`State::bottom_dimmed`], [`State::bottom_off`]), from when it was last used: the first tick
    ///     starts that.)
    /// 11. The browsed folder's selection and armed action; the radios' scans and joins, by the clock.
    /// 12. The idle stage.
    /// 13. The program in force.
    /// 14. (The second screen off by itself reads the program in force's setting and darkness as 13 left
    ///     them: `dark_for_a_game_on_battery_it_goes_off_and_a_tap_wakes_it`.)
    /// 15. The notifications' times, which run while the second screen is lit, as 12 and 13 leave it
    ///     (`a_notification_waits_for_the_bottom_screen_and_then_shows_its_time`).
    /// 16. The pad mouse, when the program in force changed.
    /// 17. What claims the pad, and where it goes. The focus ring shows where the pad is, so it is hidden
    ///     when the pad leaves ClamShell's screens.
    /// 18. The Library's place, after the steps that change it (plays recorded), so that its moves are
    ///     found in this pass ([`State::follow_library_view`]); then the scrolled lists keep their top items
    ///     where they are ([`State::follow_lists`]).
    fn settle(&mut self, out: &mut Out) {
        if self.session.follow_units(&self.clock, self.library_view.library(), self.bottom.notice_mut(), &mut self.plays, out) {
            self.bottom.unwake();
        }
        self.record_plays(out);
        // (Each list taken while `launch` borrows the state, and put back empty with its room.)
        let mut starting = std::mem::take(&mut self.starting);
        // The entries to start are looked for in the library, so not before it is read, and not while the
        // account holds programs.
        if self.library_view.read() && !self.setup.holds_programs() {
            self.session.autostart_due(self.library_view.library(), &mut starting);
        }
        for id in starting.drain(..) {
            self.launch(id, None, Start::Background, out);
        }
        // Launcher games asked for while their launcher's launch was starting, now that it has.
        let mut after = std::mem::take(&mut self.after_launchers);
        self.session.due_after_launchers(&mut after);
        for (id, start) in after.drain(..) {
            self.launch(id, None, start, out);
        }
        self.after_launchers = after;
        if self.session.start_by_app_id(&self.clock, self.library_view.library(), &mut starting) {
            self.bottom.unwake();
        }
        // Before following the windows, which leaves a starting launch's windows where they are.
        for id in starting.drain(..) {
            self.launch(id, None, Start::Front, out);
        }
        self.starting = starting;
        self.session.follow_windows(&self.clock, self.library_view.library(), &mut self.plays, out);
        self.record_plays(out);
        // After the windows are followed, which finds their owners.
        if self.session.follow_window_rules(self.library_view.library()) {
            self.bottom.show_below();
        }
        // While Setup shows, the second screen has the tab its step needs (see `crate::setup::wizard`).
        if let Some(step) = self.setup.wizard() {
            self.bottom.choose_tab(step.tab());
        }
        // The tab left, by a choice or by a window moved down opening the windows' tab.
        self.follow_tab();
        // A launch's first window is shown before a window that came back to the group whose front closed,
        // so that the one that came back ends in front.
        self.session.follow_front_launch(self.clock.mono());
        self.session.follow_replacement(self.clock.mono());
        self.session.follow_front(self.clock.mono());
        // After the launch's window and the going back, so a launcher's game whose window came in this
        // event takes the front, not its launcher.
        self.session.follow_launcher(self.clock.mono(), self.library_view.library());
        // An app needed now (Setup) is in front, over any window; one no longer needed closes.
        self.follow_needed();
        // Order: once the front is settled (a launch's window, a closed window's replacement), which an
        // app's pick is shown over or set aside for; before the overview, which follows what is in front, so
        // a pick shown or a program back after it is what the overview and the next steps see.
        self.follow_portal(out);
        self.session.follow_overview();
        // Once the front is settled, so a program shown again when the front window closed is thawed at
        // once; before the quitting, which a quitting entry isn't paused for.
        self.session.follow_background(self.clock.mono(), self.library_view.library(), self.prefs.settings(), out);
        self.session.follow_quitting(self.clock.mono(), self.library_view.library(), out);
        self.dock.follow(&self.session);
        // After every step that reads what the last pass saw: the new windows and the group in front.
        self.session.followed();
        self.library_view.follow_folder();
        self.radios.follow(self.clock.mono());
        // Sleep only in the device's own session. A program in front doesn't hold it off; one playing a
        // video asks the compositor to keep the screen on.
        let policy = crate::idle::Policy::of(&self.prefs.settings().global, self.session.device_session());
        let refreshing = self.device_view.calibrating();
        self.idle.follow(self.clock.mono(), policy, refreshing, self.session.awake_command(), out);
        // Once the front is settled, which the program in force follows; the pad's route then follows the
        // program.
        let changed = self.session.follow_program();
        let lit = self.bottom_lit();
        self.notifications.follow(self.clock.read().then(|| self.clock.mono()), lit, out);
        if changed {
            self.follow_pad_mouse();
        }
        if self.reconcile_pad().is_some_and(|route| route != PadRoute::Ui) {
            self.pointer.hide_focus();
        }
        // After the steps that change the Library (the plays recorded), so that its moves are found in this pass.
        self.follow_library_view();
        self.follow_lists();
    }

    /// Records in the Library the plays the session's steps ended.
    fn record_plays(&mut self, out: &mut Out) {
        for (entry, start) in self.plays.drain(..) {
            self.library_view.record_play(entry, start, &self.clock, out);
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
        let stepping = (self.bottom.tab().definition().steps_pad)(self) || self.session.overview().is_some();
        self.pad.reconcile(self.top(), self.bottom.tab(), listening, stepping)
    }

    /// What the outside should have as things are now. The second screen watches what the open tab
    /// watches. It can be seen when ClamShell's view is on the bottom screen and that screen is on.
    fn wanted(&self) -> Wanted<'_> {
        let tab = (self.bottom.tab().definition().watch)(self);
        let settings = self.in_force();
        let (program, applied) = self.hardware_settings();
        let bottom_off = self.bottom_off();
        Wanted {
            library_view: &self.library_view,
            fetch_view: self.library_view.fetch_first(),
            browsed: self.library_view.browsed(),
            hidden: self.library_view.files().hidden(),
            running: self.session.running_entries(),
            stage: self.idle.stage(),
            levels: &self.levels,
            program,
            settings: applied,
            refresh: self.refresh_wanted(&applied),
            // Off while the refresh rate calibration runs, so only the main screen's vblanks come.
            bottom_on: !bottom_off && !self.device_view.calibrating(),
            bottom_seen: !self.bottom.shows_windows(&self.session) && !bottom_off && !self.bottom_dark(),
            tab,
            account: self.setup.reads_account(),
            apps: clamshell_library::BuiltinApp::ALL.map(|app| matches!(self.top(), Top::App { app: front, .. } if front == app)),
            ui_up: self.ui_up,
            keymaps: {
                let count = self.device_view.controllers().count();
                crate::device_view::controllers::Player::ALL.map(|p| (p <= count).then(|| self.keymapping.version_of(settings.get(Setting::keymap(p)))))
            },
            keymapping: &self.keymapping,
            route: self.pad.route(),
            pad_activity: self.session.top_window().is_some_and(|w| Some(w.id) != self.session.keyboard_focus()),
            touchpad: self.presentation.touchpad(),
            lights: self.lights_shown(),
        }
    }

    /// The entry whose settings the hardware has, and those settings: the entry whose window is waited
    /// for to come in front ([`Session::awaited_entry`]), else the program in force. A launch in the
    /// background changes nothing here.
    pub(crate) fn hardware_settings(&self) -> (Option<&Arc<str>>, Values) {
        match self.session.awaited_entry() {
            Some(id) => (Some(id), self.settings_of(id)),
            None => (self.session.program(), self.in_force()),
        }
    }

    /// The main screen's refresh rate (see [`crate::sync::Wanted::refresh`]), with `applied` the
    /// settings applied.
    fn refresh_wanted(&self, applied: &Values) -> u16 {
        if let Some(hz) = self.device_view.calibration_rate() {
            return hz;
        }
        self.presentation.display_modes().refresh_for(applied.get(Setting::Refresh), applied.get(Setting::FpsCap))
    }

    /// Sends the outside what differs from what it was last sent.
    fn sync(&mut self, out: &mut Out) {
        let mut sent = std::mem::take(&mut self.sent);
        sent.sync(&self.wanted(), out);
        self.sent = sent;
    }
}
