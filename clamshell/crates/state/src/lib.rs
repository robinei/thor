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
//! - [`device`]: the Device view, the account, stick calibration, the device buttons, and idle;
//! - [`status`]: the system readings ([`Sys`]), covering status, metrics, the battery and the time;
//! - [`keymap`]: keymaps and their screens;
//! - [`nav`]: focus and presses ([`Pointer`]), the lists, and the pad's routing ([`Pad`]);
//! - [`kit`]: generic parts the areas build on, which know nothing about them.
//!
//! [`State`] holds the areas and coordinates them. It is defined in `lib.rs`, after [`Init`]. Its impl
//! builds the areas, gives each to outside code to read, and carries out what needs more than one area.
//! What only it uses is in modules beside the areas: the record of what the outside was last sent
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

pub mod bottom;
mod clock;
pub mod curve;
pub mod device;
mod effect;
mod event;
mod id;
pub mod keymap;
pub mod kit;
pub mod levels;
pub mod nav;
pub(crate) mod out;
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

use crate::bottom::editor::{Done, Purpose};
use crate::bottom::keyboard::Keyboard;
use crate::bottom::notice::NoticeKind;
use crate::bottom::screen::Power;
use crate::device::buttons::DeviceKey;
use crate::device::idle::Idle;
use crate::keymap::screens::Keymapping;
use crate::kit::debounce::Debounce;
use crate::nav::lists::Scrolls;
use crate::profiles::picker::{Kind as ProfileKind, PickerCx, ProfileEvent, ProfileId};
use crate::session::running::{Instead, LaunchRequest, Start};
use crate::settings::{BottomWhenIdle, MouseAtLaunch, Setting, Settings, Values};
use out::Out;
use resume::SAVE_AFTER;
use sync::{Resend, Sent, Wanted};

// What other crates use to talk to the state: the events and effects and what they carry, the ids, and
// the groups the getters return.
pub use clock::Clock;
pub use effect::{Effect, Save, Watch, WindowCommand};
pub use event::{EntryAct, Event, KeymapEvent, LaunchEvent, LevelEvent, PointerEvent, Reading, ShelfEvent, Slide};
pub use id::{AudioId, BatteryId, BtId, DeviceId, DockId, EditorId, Hold, Id, KeymapId, SettingsId, ShelfId, Target, TimeId, WifiId};
pub use resume::Resume;
pub use session::windows::{WindowEvent, WindowPlace};

pub use bottom::editor::Editor;
pub use bottom::{Bottom, Tab};
pub use kit::list::{ItemKey, Scroll};
pub use levels::slider::Control;
pub use levels::{Brightness, Level, Levels, Sound};
pub use nav::lists::{ListEvent, ListId, Lists, RowPart};
pub use nav::pad::{Claimant, Pad, PadInput, PadMouse, PadRoute};
pub use nav::pointer::Pointer;
pub use radios::{RadioStatus, Radios};
pub use session::Session;
pub use session::dock::{Dock, DockItem};
pub use session::instance::Instance;
pub use session::running::{Front, Launching, Origin, Running};
pub use settings::panel::{Panel, Prefs, Program, Scope, TileOrigin};
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
    pub display_modes: device::display::DisplayModes,
    pub library: Library,
    /// Every game's settings and environment.
    pub settings: Settings,
    /// The keymaps.
    pub keymaps: keymap::Keymaps,
    /// The tone curves.
    pub curves: curve::Curves,
    /// Whether ClamShell's own compositor shows the windows, rather than sway. Only it applies tone curves.
    pub compositor: bool,
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
    pub window_rules: crate::bottom::screen::WindowRules,
    /// Show clocks in 12-hour format rather than 24-hour.
    pub twelve_hour: bool,
    /// This is ClamShell's first start, meaning the Device view hasn't been closed with Done yet. The
    /// Device view opens (see [`device`]).
    pub first_start: bool,
    /// The sticks' calibrations, by controller (see [`crate::device::sticks`]).
    pub sticks: crate::device::sticks::Calibrations,
}

/// How a target was pressed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Press {
    Tap,
    Long,
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
    /// Every entry's settings, and the scope the settings panel edits.
    prefs: Prefs,
    keyboard: Keyboard,
    /// The keymaps and their screens (see [`crate::keymap::screens`]).
    keymapping: Keymapping,
    curving: crate::curve::screens::Curving,
    /// The screens, as the compositor last said (see [`Reading::Screens`]).
    screens: crate::session::presentation::Screens,
    /// Whether ClamShell's own compositor shows the windows (see [`Init::compositor`]).
    compositor: bool,
    /// Where the lists but shelf are scrolled.
    scrolls: Scrolls,
    clock: Clock,
    /// Where shelf and the second screen are ([`State::resume`]), saved once they stop changing for a
    /// moment. Shelf and the bottom screen say when they changed; `apply` touches this at its end.
    resume: Debounce,
    idle: Idle,
    /// What the outside was last sent (see [`crate::sync`]).
    sent: Sent,
    /// The events the rules queue for State, lent to `apply`'s [`Out`] and returned empty, so that its
    /// room is kept.
    queue: Vec<Event>,
    /// The windows' effects while the session's steps run, put after the later steps' (see
    /// [`State::settle`]). Kept empty, so that its room is reused.
    window_effects: Vec<Effect>,
    /// Initial placements wait behind launches discovered by the same dispatch.
    opening_effects: Vec<Effect>,
    /// The plays the session's steps ended, until they are recorded in shelf. Kept empty, so that its
    /// room is reused.
    plays: Vec<crate::session::running::Play>,
}

impl State {
    // Construction and the areas.

    pub fn new(init: Init) -> State {
        let mut s = State {
            profile: init.profile,
            shelf: Shelf::new(init.library),
            prefs: Prefs::new(init.settings),
            keymapping: Keymapping::new(init.keymaps),
            curving: crate::curve::screens::Curving::new(init.curves),
            screens: Default::default(),
            compositor: init.compositor,
            bottom: Bottom::new(init.window_rules),
            sys: Sys::new(init.twelve_hour),
            levels: Levels::new(init.brightness),
            session: Session::new(init.autostart, init.device_session),
            dock: init.dock,
            device: device::Device::new(init.sticks, init.first_start),
            ..State::default()
        };
        if let Some(resume) = &init.resume {
            resume.restore(&mut s.shelf, &mut s.bottom);
        }
        s.device.set_display_modes(init.display_modes);
        s.dock.drop_gone_at_start(s.shelf.library(), &s.session);
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

    pub fn prefs(&self) -> &Prefs {
        &self.prefs
    }

    pub fn keyboard(&self) -> &Keyboard {
        &self.keyboard
    }

    pub fn keymapping(&self) -> &Keymapping {
        &self.keymapping
    }

    /// Screen `down`'s colour (the second's, or the top's), as the compositor describes it.
    pub fn screen_colour(&self, down: bool) -> crate::session::presentation::ColourDescription {
        let [top, bottom] = self.screens.colour;
        if down { bottom } else { top }
    }

    /// The top screen's size, once the compositor has said; under sway, none.
    pub fn top_size(&self) -> Option<(u32, u32)> {
        Some(self.screens.top_size).filter(|(w, h)| *w > 0 && *h > 0)
    }

    pub fn curving(&self) -> &crate::curve::screens::Curving {
        &self.curving
    }

    /// Whether ClamShell's own compositor shows the windows (see [`Init::compositor`]).
    pub fn compositor(&self) -> bool {
        self.compositor
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

    pub fn idle(&self) -> &crate::device::idle::Idle {
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
        let mark = out.effects.len();
        self.apply_event(event, &mut out);
        self.settle(&mut out);
        self.sync(&mut out, mark);
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
            Event::DisplayModes(modes) => self.device.set_display_modes(modes),
            Event::Tick { unix_secs, mono_secs } => self.tick(unix_secs, mono_secs, out),
            Event::Reading(reading) => self.on_reading(reading, out),
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
            Event::Home => self.home(out),
            Event::HomeHeld(held) => self.session.hold_home(held),
            Event::HomeTap => {
                if self.session.home_tap() {
                    self.home(out);
                }
            }
            Event::PadActive => self.session.pad_active(out),
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
            Event::DumpWindowEvents(path) => out.effects.push(Effect::DumpWindowEvents(path)),
            Event::Screenshot(result) => self.bottom.notice_mut().show(NoticeKind::Screenshot(result), now),
            Event::Sticks(event) => self.device.sticks_event(event, now, out),
            Event::RefreshCalibrated(result) => self.refresh_calibrated(result),
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
        match event {
            WindowEvent::Opened(mut window) => {
                if self.session.windows().iter().any(|w| w.id == window.id) {
                    return;
                }
                let parent = window.parent.and_then(|id| self.session.window(id)).map(|w| (w.workspace.clone(), w.down));
                let inherit_down = parent.as_ref().is_some_and(|(_, down)| *down) && !window.down;
                if inherit_down {
                    window.down = true;
                    window.focused = false;
                }
                {
                    let group = parent.map(|(group, _)| group).or_else(|| {
                        self.shelf
                            .library()
                            .entries
                            .iter()
                            .find(|g| crate::session::running::has_app_id(g, &window.app_id))
                            .map(|g| Instance::workspace_of(&g.key))
                            .or_else(|| self.session.running_of(&window).map(|r| r.workspace()))
                            .or_else(|| window.home().map(|i| i.workspace()))
                    });
                    if let Some(group) = group.filter(|g| *g != window.workspace) {
                        window.workspace.clone_from(&group);
                        if !window.down {
                            self.opening_effects.push(Effect::Window(WindowCommand::Move { windows: vec![window.id], to: group }));
                            if window.focused {
                                self.opening_effects.push(Effect::Window(WindowCommand::FocusWindow(window.id)));
                            }
                        }
                    }
                }
                let id = window.id;
                self.session.on_window(WindowEvent::Opened(window), out);
                if inherit_down {
                    self.bottom.show_down(id, &mut self.session, out);
                }
            }
            WindowEvent::Activate(id) => {
                if self.session.window(id).is_some() {
                    self.session.cancel_front_launch();
                }
                if let Some(owner) = self.session.window(id).and_then(|w| self.session.owner_of(w)) {
                    self.session.thaw(&owner, out);
                    self.session.resume_runner_of(&owner, self.shelf.library(), out);
                }
                if self.session.window(id).is_some_and(|w| w.down) {
                    self.bottom.show_program_down(id, &mut self.session, out);
                    self.session.command(WindowCommand::KeyboardFocus(id), out);
                } else {
                    self.session.on_window(WindowEvent::Activate(id), out);
                }
            }
            event => self.session.on_window(event, out),
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
    /// changed for it. The first eight follow the session's inputs (the windows, systemd's scopes, the
    /// tray, the launcher's messages, the library and the clock) and what the event did to the session:
    ///
    /// 1. systemd's scopes: entries found running, found ones promoted to launches, and ours ended
    ///    ([`crate::Session::follow_units`]). The first time, the entries to start with ClamShell are
    ///    started.
    /// 2. The windows: entries started by their app ids ([`crate::Session::start_by_app_id`]), with Steam's
    ///    games launched before the windows are followed, since a starting launch's window is left where
    ///    it is; then windows moved to their entries, programs found by them, and entries ended by them
    ///    ([`crate::Session::follow_windows`], which compares with the windows the last pass saw). The
    ///    effects of this step, the Steam games' launches among them, come after those of steps 3 to 7,
    ///    so that a window just taken by a browsed file is shown over the one in front closing.
    /// 3. The rules that send windows down.
    /// 4. In the background: entries paused, thawed or ended by their setting. This reads 1 and 2.
    /// 5. What is shown: a launch's first window, and a workspace whose window came back.
    /// 6. Quitting entries are ended once their windows are gone. This reads 1 to 4.
    /// 7. Going back to what was before when the one in front closed.
    /// 8. The dock: running entries are added to it and ended ones removed.
    ///
    /// Then the rest:
    ///
    /// 9. The second screen's dimming.
    /// 10. The browsed folder's selection and armed action.
    /// 11. The idle stage.
    /// 12. The program in force.
    /// 13. The second screen turning off or on by itself, which reads the program in force's setting and
    ///     darkness.
    /// 14. The pad mouse, when the program in force changed.
    /// 15. What claims the pad, and where it goes. The focus ring shows where the pad is, so it is hidden
    ///     when the pad leaves ClamShell's screens.
    /// 16. The scrolled lists keep their top items where they are ([`State::follow_lists`]).
    fn settle(&mut self, out: &mut Out) {
        if self.session.follow_units(&self.clock, self.shelf.library(), self.bottom.notice_mut(), &mut self.plays) {
            self.bottom.unwake();
        }
        self.record_plays(out);
        let mut due = Vec::new();
        self.session.autostart_due(self.shelf.library(), &mut due);
        for id in due {
            self.launch(id, None, Start::Background, out);
        }
        // The windows' effects go after those of the steps that follow them, below.
        let mut windows = Out { effects: std::mem::take(&mut self.window_effects), events: Vec::new() };
        let mut steam = Vec::new();
        if self.session.start_by_app_id(&self.clock, self.shelf.library(), &mut steam) {
            self.bottom.unwake();
        }
        // Before following the windows, which leaves a starting launch's windows where they are.
        for id in steam {
            self.launch(id, None, Start::Front, &mut windows);
        }
        windows.effects.append(&mut self.opening_effects);
        self.session.follow_windows(&self.clock, self.shelf.library(), &mut self.plays, &mut windows);
        self.record_plays(&mut windows);
        self.bottom.follow_window_rules(&mut self.session, self.shelf.library(), out);
        // A window moved down opened the windows' tab; the tab it left settles as any tab left.
        if let Some(left) = self.bottom.take_left() {
            self.left_tab(left);
        }
        self.session.follow_background(self.clock.mono(), self.shelf.library(), self.prefs.settings(), out);
        self.session.follow_front_launch(self.clock.mono(), &mut windows);
        self.session.follow_replacement(self.clock.mono(), &mut windows);
        self.session.follow_quitting(self.clock.mono(), self.shelf.library(), out);
        self.session.follow_front(self.clock.mono(), out);
        // After handling the front one closing, so that a window just taken by a browsed file is shown
        // over it.
        out.effects.append(&mut windows.effects);
        out.events.append(&mut windows.events);
        self.window_effects = windows.effects;
        self.dock.follow(&self.session);
        self.session.followed();
        self.bottom.follow_dim(&self.session, &self.clock);
        self.shelf.follow_folder();
        // Sleep only in the device's own session. A program in front doesn't hold it off; one playing a
        // video asks the compositor to keep the screen on.
        let policy = crate::device::idle::Policy::of(&self.prefs.settings().global, self.session.device_session());
        let refreshing = *self.device.refresh() == crate::device::Refresh::Running;
        self.idle.follow(self.clock.mono(), policy, refreshing, self.session.awake_command(), out);
        let changed = self.session.follow_program();
        self.bottom.follow_power(self.power(), self.clock.mono());
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
            launched: None,
            bottom_on: !self.bottom.off(),
            bottom_seen: !self.bottom.shows_windows(&self.session) && !self.bottom.off(),
            tab,
            device_open: self.device.is_open(),
            keymap: self.keymapping.version_of(settings.get(Setting::Keymap)),
            keymapping: &self.keymapping,
            route: self.pad.route(),
            pad_activity: self.session.top_window().is_some_and(|w| Some(w.id) != self.session.keyboard_focus()),
        }
    }

    /// Sends the outside what differs from what it was last sent. The event's effects start at `mark`.
    fn sync(&mut self, out: &mut Out, mark: usize) {
        let mut sent = std::mem::take(&mut self.sent);
        let launched = out.effects.get(mark..).unwrap_or_default().iter().rev().find_map(|e| match e {
            Effect::Launch { settings, .. } => Some(settings.get(Setting::Refresh)),
            _ => None,
        });
        sent.sync(&Wanted { launched, ..self.wanted() }, out);
        self.sent = sent;
    }

    // What comes in.

    /// The clock ticked. Once a minute, the bar's clock and both screens' burn-in orbit move on. Changes
    /// are saved once they have stopped for a moment.
    fn tick(&mut self, unix_secs: u64, mono_secs: u64, out: &mut Out) {
        // The first tick. Anything idle heard before it was timed at 0, so count from now.
        if !self.clock.read() {
            self.idle.resumed(mono_secs);
        }
        self.clock.tick(unix_secs, mono_secs);
        if self.resume.due(mono_secs, SAVE_AFTER) {
            out.effects.push(Effect::Save(Save::Resume(self.resume())));
        }
        self.levels.save_due(mono_secs, out);
        self.bottom.expire_notice(mono_secs);
    }

    /// A reading of the system from a system thread. It is stored, and after a status reading the
    /// battery warning is checked.
    fn on_reading(&mut self, reading: Reading, out: &mut Out) {
        let status = matches!(reading, Reading::Status(_));
        match reading {
            // The volume, mute and brightness go to the levels, where a finger on a slider wins over the
            // reading.
            Reading::Status(status) => {
                self.levels.read(status.volume, status.muted, status.brightness);
                self.sys.read_status(status);
            }
            Reading::Clipboard(text) => self.bottom.read_clipboard(text),
            Reading::Icon { entry, path } => self.session.set_icon(&entry, path),
            Reading::Metrics(metrics) => self.sys.read_metrics(metrics),
            Reading::Frames { fps, frame_ms } => self.sys.frames(fps, frame_ms),
            Reading::Screens(screens) => self.screens = *screens,
            Reading::Wifi(wifi) => self.radios.read_wifi(wifi),
            Reading::Bluetooth(bluetooth) => self.radios.read_bluetooth(bluetooth),
            Reading::Battery(battery) => self.sys.read_battery(battery),
            Reading::Audio(audio) => self.levels.read_audio(audio),
            Reading::Backlights(backlights) => self.levels.read_backlights(backlights),
            Reading::Time(time) => self.sys.read_time(time),
            Reading::Account(account) => self.device.read_account(account),
        }
        if status {
            self.sys.battery_warning(out);
        }
    }

    /// The library was read again. New problems are reported once and shelf is reloaded. Then what
    /// depends on its entries is updated: runners' pauses and keymaps, landing rules, and pins of entries
    /// that are gone. Shelf's entries shown are asked for first again.
    fn library_read(&mut self, library: Library, out: &mut Out) {
        let problems = &library.problems;
        if !problems.is_empty() && *problems != self.shelf.library().problems {
            let first = problems.first().cloned().unwrap_or_default();
            self.bottom.notice_mut().show(crate::bottom::notice::NoticeKind::Problems(first, problems.len()), self.clock.mono());
        }
        if self.shelf.reload(library) {
            self.shelf.moved(&mut self.pointer);
        }
        self.session.set_pausable_runners(self.shelf.library());
        self.keymapping.adopt_runner_keymaps(self.shelf.library(), &mut self.prefs, out);
        out.effects.push(crate::session::running::known_apps(self.shelf.library()));
        if self.dock.drop_gone(self.shelf.library(), &self.session) {
            out.effects.push(self.dock.save_effect());
        }
        self.sent.resend(Resend::FetchFirst);
    }

    /// Metadata fetching started or stopped. Shelf's entries shown are asked for first again.
    fn fetching(&mut self, on: bool) {
        self.shelf.set_fetching(on);
        self.sent.resend(Resend::FetchFirst);
    }

    /// Back from sleep: idle counts from now, and the settings in force are applied again.
    fn resumed(&mut self) {
        self.idle.resumed(self.clock.mono());
        self.sent.resend(Resend::Resumed);
    }

    /// The refresh rate calibration ended. It had turned the bottom screen off while it ran, so that
    /// screen's power is set again.
    fn refresh_calibrated(&mut self, result: Result<String, String>) {
        self.device.refresh_done(result);
        self.sent.resend(Resend::RefreshCalibrated);
    }

    /// The UI is up. Sends what was waiting for it: the dock if pins were dropped at start, the
    /// brightness, the runners' keymaps and the down rules, and after the event the pad's route and
    /// keymap. The open tab's watch starts from now (see [`crate::sync::Sent::ui_started`]).
    fn ui_started(&mut self, out: &mut Out) {
        self.sent.ui_started();
        self.sent.resend(Resend::Start);
        if self.dock.take_unsaved() {
            out.effects.push(self.dock.save_effect());
        }
        out.effects.extend(self.levels.brightness_effect());
        self.keymapping.adopt_runner_keymaps(self.shelf.library(), &mut self.prefs, out);
        out.effects.push(self.bottom.window_rules().effect());
    }

    /// A level event. A level row set is the one the finger landed on, and nothing once it is gone.
    fn on_level(&mut self, event: LevelEvent, out: &mut Out) {
        let event = match event {
            LevelEvent::Set(level, value) => match self.lists().resolve_level(level) {
                Some(level) => LevelEvent::Set(level, value),
                None => return,
            },
            e => e,
        };
        self.levels.on_event(event, self.clock.mono(), out);
    }

    /// Home. On a screen that listens for input, it leaves that screen; Home is never taken as an input.
    /// When the Device view is in shelf's place, it goes back to shelf. Otherwise the session handles it
    /// (see [`Session::home`]).
    fn home(&mut self, out: &mut Out) {
        if self.listening() {
            self.keymapping.back(&mut self.keyboard);
        } else if self.session.shelf_in_front() && self.device.is_open() {
            self.device.close(&mut self.pointer, self.shelf.selected());
        } else {
            self.session.home(out);
        }
    }

    /// Quitting ClamShell: where shelf and the second screen are is saved first.
    fn quit(&mut self, out: &mut Out) {
        out.effects.push(Effect::Save(Save::Resume(self.resume())));
        out.effects.push(Effect::Quit);
    }

    /// Something done to entry `id`.
    fn on_entry(&mut self, id: Arc<str>, act: EntryAct, out: &mut Out) {
        match act {
            EntryAct::Play => self.play(id, out),
            EntryAct::Show => self.show(&id, out),
            EntryAct::OpenAnother => self.open_another(id, out),
            EntryAct::Suspend => self.session.suspend(&id, out),
            EntryAct::ToggleAutostart => self.session.toggle_autostart(id, out),
            EntryAct::TogglePin => self.dock.toggle_pin(id, &self.session, out),
            EntryAct::Quit => self.session.quit(&id, self.clock.mono(), self.shelf.library(), out),
            EntryAct::ForceKill => self.session.force_kill(&id, self.shelf.library(), out),
        }
    }

    fn on_launch_event(&mut self, event: LaunchEvent) {
        match event {
            LaunchEvent::Position { entry, position } => self.shelf.set_position(&entry, position, self.clock.unix()),
            // A launch that never started doesn't change what runs, but it is no longer starting.
            LaunchEvent::LaunchFailed(entry) => self.session.launch_failed(entry, self.shelf.library(), &self.clock, self.bottom.notice_mut()),
        }
    }

    /// A touch or a pad focus change. Focusing or pressing a target on the second screen counts as using
    /// that screen.
    fn on_pointer(&mut self, event: PointerEvent, out: &mut Out) {
        let used = match &event {
            PointerEvent::Focus(id) | PointerEvent::Activate(id) | PointerEvent::LongPress(id) | PointerEvent::Down(id, _) => !id.on_top(),
            _ => false,
        };
        if used {
            self.bottom.used(&self.session, &self.clock);
        }
        // Input that reached ClamShell itself, such as the pad driving the screens.
        self.idle.own_input(self.clock.mono());
        match event {
            PointerEvent::Focus(id) => {
                self.show_focused(&id);
                self.pointer.focus_on(id);
                self.pointer.show();
            }
            PointerEvent::HideFocus => {
                self.pointer.hide_focus();
            }
            // A press that woke the screens does nothing else.
            PointerEvent::Activate(_) | PointerEvent::LongPress(_) if self.idle.swallows_press(self.clock.mono()) => {}
            PointerEvent::Activate(id) => self.activate(&id, Press::Tap, out),
            PointerEvent::LongPress(id) => self.activate(&id, Press::Long, out),
            PointerEvent::Down(id, item) => {
                if let Id::Key(pos) = id {
                    self.keyboard.down(pos);
                }
                let item = item.or_else(|| self.lists().item_key(&id));
                self.pointer.down(id, item);
            }
            PointerEvent::Up(id) => {
                self.pointer.up(&id);
                if Level::of(&id).is_some() {
                    self.levels.release();
                }
                if let Id::Key(pos) = id {
                    self.keyboard.up(pos);
                }
            }
        }
    }

    /// A tap or long press on `id`, resolved to what the press landed on. It gets the focus, and its
    /// subsystem acts on it.
    fn activate(&mut self, id: &Id, press: Press, out: &mut Out) {
        let long = press == Press::Long;
        // The list item the press came down on is gone.
        let Some(id) = self.lists().resolve(id) else {
            return;
        };
        // A shelf entry that isn't in the list shown takes nothing, not even the focus.
        if !self.shelf.has_target(&id) {
            return;
        }
        self.pointer.focus_on(id.clone());
        match id {
            Id::Shelf(id) => {
                self.shelf.on_target(id, long, &mut self.pointer, out);
            }
            Id::Dock(id) => {
                self.dock.on(id, long, &self.session, out);
            }
            Id::Settings(id) => {
                self.on_settings(id, long, out);
            }
            Id::Editor(id) => {
                let done = match id {
                    EditorId::Save => Done::Save,
                    EditorId::Cancel => Done::Cancel,
                };
                self.editor_done(done, out);
            }
            Id::Wifi(WifiId::Power) => {
                self.radios.power_wifi(out);
            }
            Id::Bluetooth(BtId::Power) => {
                self.radios.power_bluetooth(out);
            }
            Id::Audio(id) => {
                self.levels.on_audio(id, out);
            }
            Id::Row { list, i, part } => {
                self.on_row(list, usize::from(i), part, long, out);
            }
            Id::Keymap(id) => {
                self.on_keymap(id, long, out);
            }
            Id::Profile(kind, id) => {
                self.on_profile(kind, id, out);
            }
            Id::Tab(tab) => {
                self.choose_tab(tab);
            }
            // A tap opens the tab. Slides are handled by the UI and don't end in a tap.
            Id::Control(control) if !long => {
                let tab = match control {
                    Control::Volume => Tab::Volume,
                    Control::Brightness => Tab::Brightness,
                    // A keymap's or a curve's parameter is held or slid, not tapped.
                    Control::Param(_) | Control::Curve(_) => return,
                };
                self.choose_tab(tab);
            }
            Id::Control(_) | Id::Level(_) | Id::List(_) | Id::Blank => {}
            Id::Key(pos) => {
                let stroke = self.keyboard.press(pos, long);
                // The key goes to the editor while it is open, to finding an entry while shelf is in
                // front, and otherwise to the focused app.
                match stroke {
                    Some(stroke) if self.bottom.editor().is_some() => {
                        if let Some(done) = self.bottom.edit(stroke) {
                            self.editor_done(done, out);
                        }
                    }
                    Some(stroke) if self.keymapping.capturing(self.bottom.tab(), self.bottom.editor().is_some()) => {
                        self.keymapping.captured(crate::keymap::screens::chord(stroke), out);
                    }
                    Some(stroke) if self.session.front() == crate::session::running::Front::Shelf => {
                        self.shelf.find(stroke, &mut self.pointer, out);
                    }
                    Some(stroke) => out.effects.push(Effect::Key(stroke)),
                    None => {}
                }
            }
            Id::Time(id) => self.sys.on_time(id, self.scrolls.get_mut(ListId::Time), out),
            Id::Device(id) => {
                self.device.tap(id, self.clock.mono(), &mut self.pointer, self.shelf.selected(), out);
            }
            Id::Battery(id) => {
                self.sys.on_battery(id, &self.session, out);
            }
            Id::PadMouse => {
                self.pad.toggle_mouse();
            }
            Id::Wake => {
                self.bottom.wake();
            }
        }
    }

    // Queries across areas.

    /// A library entry by its id. An entry that is still running after leaving the library, because its
    /// file was deleted, is returned as it was.
    pub fn entry(&self, id: &str) -> Option<&Entry> {
        self.session.entry(self.shelf.library(), id)
    }

    /// Whether the dock menu offers Open another for `id`. It does when `id` is running and another launch
    /// of it makes sense (see [`crate::Entry::another`]).
    pub fn opens_another(&self, id: &str) -> bool {
        // Not for an entry handed over to another program, such as Android's: launching it again just
        // shows the one already running.
        self.session.running(id).is_some() && self.entry(id).is_some_and(|g| g.another && !g.runner.as_ref().is_some_and(|r| r.handover))
    }

    /// The settings in force: those of the program in force (see [`crate::session::program`]), or else
    /// the global ones.
    pub fn in_force(&self) -> Values {
        self.session.program().as_ref().map_or(self.prefs.settings().global, |id| self.settings_of(id))
    }

    /// The level that entry `id` takes its settings from before its own, which is its runner's, if it
    /// has one.
    fn runner_of(&self, id: &str) -> Option<&str> {
        self.entry(id).and_then(|g| g.runner.as_ref()).map(|r| &*r.key)
    }

    /// The settings entry or level `id` runs with: the global ones, then its runner's, then its own.
    pub fn settings_of(&self, id: &str) -> Values {
        self.prefs.settings().of_in(id, self.runner_of(id).as_slice())
    }

    /// The settings panel as it is now (see [`Panel::new`]).
    pub fn panel(&self) -> Panel<'_> {
        Panel::new(&self.session, &self.shelf, &self.prefs, &self.keymapping, &self.curving)
    }

    /// Whether the second screen is dark (see [`Bottom::dark`]).
    pub fn bottom_dark(&self) -> bool {
        self.bottom.dark(self.session.program().is_some(), self.in_force().choice::<BottomWhenIdle>())
    }

    /// What the second screen's power follows, as things are now.
    fn power(&self) -> Power {
        Power {
            any_down: self.session.windows_down().next().is_some(),
            in_game: self.session.program().is_some(),
            when_idle: self.in_force().choice::<BottomWhenIdle>(),
            discharging: self.sys.status().discharging,
            refreshing: *self.device.refresh() == crate::device::Refresh::Running,
        }
    }

    /// What the pad's Back button does on ClamShell's screens, as the target it activates. In the editor
    /// it is Cancel. In the dock's menu or move mode it is the dock's Back. On the keymap screens it goes
    /// back one screen, or gives up picking a control. Nothing else has a back of its own. On the edit
    /// screen while the pad listens, Back comes in as an input to remap.
    pub fn pad_back(&self) -> Option<crate::Id> {
        if self.bottom.editor().is_some() {
            return Some(crate::Id::Editor(crate::EditorId::Cancel));
        }
        if (self.dock.menu().is_some() || self.dock.moving().is_some()) && self.bottom.tab() == crate::Tab::Dock {
            return Some(crate::Id::Dock(crate::DockId::Back));
        }
        if self.curving.screen_in(self.bottom.tab()).is_some() {
            return Some(crate::Id::Profile(ProfileKind::Curve, ProfileId::Back));
        }
        if self.prefs.menu_in(self.bottom.tab()).is_some() {
            return Some(crate::Id::Settings(SettingsId::Back));
        }
        self.keymapping.screen_in(self.bottom.tab()).map(|_| crate::Id::Profile(ProfileKind::Keymap, ProfileId::Back))
    }

    /// Where shelf and the second screen are, to come back to.
    pub(crate) fn resume(&self) -> Resume {
        Resume::of(&self.shelf, &self.bottom)
    }

    /// Whether the pad listens for inputs (see [`Keymapping::listening`]).
    pub(crate) fn listening(&self) -> bool {
        self.keymapping.listening(self.bottom.tab(), self.bottom.editor().is_some())
    }

    /// Profile `id` of `kind`'s name as shown (see [`crate::profiles::Profiles::shown_name`]).
    pub fn profile_name(&self, kind: ProfileKind, id: u16) -> &str {
        match kind {
            ProfileKind::Keymap => self.keymapping.name(id),
            ProfileKind::Curve => self.curving.name(id),
        }
    }

    /// Whether profile `id` of `kind` is a game's own.
    pub fn profile_is_own(&self, kind: ProfileKind, id: u16) -> bool {
        match kind {
            ProfileKind::Keymap => self.keymapping.is_own(id),
            ProfileKind::Curve => self.curving.curves().is_own(id),
        }
    }

    /// Whether profile `id` of `kind` is there.
    pub fn profile_exists(&self, kind: ProfileKind, id: u16) -> bool {
        match kind {
            ProfileKind::Keymap => self.keymapping.keymaps().get(id).is_some(),
            ProfileKind::Curve => self.curving.curves().get(id).is_some(),
        }
    }

    /// What the keymaps' screens read from the settings panel (see [`PickerCx`]).
    fn keymap_cx(&self) -> PickerCx {
        self.picker_cx(ProfileKind::Keymap)
    }

    /// What `kind`'s picker reads from the settings panel (see [`PickerCx`]).
    fn picker_cx(&self, kind: ProfileKind) -> PickerCx {
        let panel = self.panel();
        PickerCx { entry: panel.editing().map(|g| g.id.clone()), chosen: panel.profile_chosen(kind) }
    }

    // Lists.

    fn scroll_mut(&mut self, list: ListId) -> Option<&mut Scroll> {
        match list {
            ListId::Shelf => self.shelf.scroll_mut(),
            _ => self.scrolls.get_mut(list),
        }
    }

    /// A list scrolled or was laid out.
    fn on_list(&mut self, event: ListEvent) {
        match event {
            ListEvent::Scroll(list, rows) => self.move_list(list, |s, len| s.scroll_by(rows, len)),
            ListEvent::Visible(list, rows) => self.move_list(list, |s, len| s.set_visible(rows, len)),
        }
    }

    /// A tap or long press on a row, handled by the list's feature.
    fn on_row(&mut self, list: ListId, i: usize, part: RowPart, long: bool, out: &mut Out) {
        match list {
            ListId::Wifi => self.radios.tap_network(i, long, self.bottom.editors_mut(), out),
            ListId::Bluetooth => self.radios.tap_device(i, long, out),
            ListId::Streams => self.levels.on_stream(i, part, out),
            // A picker's Reset row is the settings': the level edited goes back to the one below's choice.
            ListId::Keymaps | ListId::Curves if self.panel().is_profile_reset(ProfileKind::of_list(list), i) => {
                if !long {
                    self.change(ProfileKind::of_list(list).setting(), None, out);
                }
            }
            ListId::Keymaps => self.on_keymap_row(i, part, long, out),
            ListId::Curves => {
                let cx = self.picker_cx(ProfileKind::Curve);
                self.curving.on_row(i, part, long, &cx, self.bottom.editors_mut(), out);
            }
            ListId::Values if !long => self.on_menu_row(i, out),
            ListId::Values => {}
            ListId::Layers => self.on_layer_row(i, part, long, out),
            ListId::Time => self.sys.on_time_row(i, self.scrolls.get_mut(ListId::Time), out),
            // Its tiles are targets of their own.
            ListId::Dock => {}
            // A shelf game's row is its game's target, and a backlight's row is only its level.
            ListId::Shelf | ListId::Backlights => {}
        }
    }

    /// The pad's focus moved to `id`. If it is a list row, the list scrolls to show it.
    fn show_focused(&mut self, id: &Id) {
        if let Some((list, i)) = ListId::of(id) {
            self.move_list(list, |s, len| s.show(i, len));
        }
    }

    /// Moves `list` by `f`, given its scroll and how many rows it has. If it moved, the item now at the top
    /// of its view is remembered, to keep there when the list changes.
    fn move_list(&mut self, list: ListId, f: impl FnOnce(&mut Scroll, usize) -> bool) {
        let len = self.lists().len(list);
        if !self.scroll_mut(list).is_some_and(|s| f(s, len)) {
            return;
        }
        let lists = self.lists();
        let key = lists.rows(list).key(lists.scroll(list).top());
        if let Some(s) = self.scroll_mut(list) {
            s.anchor(key);
        }
    }

    /// Runs after every event, since any may change the lists' contents. Each scrolled list keeps its top
    /// item where it is, while that item is still there, and stays within its ends.
    fn follow_lists(&mut self) {
        for list in ListId::ALL {
            let lists = self.lists();
            let len = lists.len(list);
            let found = lists.scroll(list).anchored().and_then(|key| lists.rows(list).find(key));
            if let Some(s) = self.scroll_mut(list) {
                s.follow(found, len);
            }
        }
    }

    // Tabs, the editor and the device's keys.

    /// The user chose a tab (see [`Bottom::choose_tab`]).
    fn choose_tab(&mut self, tab: Tab) {
        if let Some(left) = self.bottom.choose_tab(tab) {
            self.left_tab(left);
        }
    }

    /// A text field wants typing, or is done (see [`Bottom::text_input`]). While the second screen shows
    /// a window moved there, the keyboard doesn't come up over it: its tab is a tap away.
    fn text_input(&mut self, active: bool) {
        if active && self.bottom.shows_windows(&self.session) {
            return;
        }
        if let Some(left) = self.bottom.text_input(active) {
            self.left_tab(left);
        }
    }

    /// Opens `editor` on the keyboard tab (see [`Bottom::open_on_keyboard`]).
    fn edit_on_keyboard(&mut self, editor: crate::Editor) {
        if let Some(left) = self.bottom.open_on_keyboard(editor, self.clock.mono()) {
            self.left_tab(left);
        }
    }

    /// Goes back to the tab the keyboard came up from, if it did.
    fn back_from_keyboard(&mut self) {
        if let Some(left) = self.bottom.back_from_keyboard() {
            self.left_tab(left);
        }
    }

    /// The second screen's tab changed from `left`, and its watch follows (see
    /// [`crate::sync::Sent::watches`]). What the tab left had going ends: the dock's menu or move, any
    /// level row being set, and the modifiers armed on the keymaps' capture screen. The time tab opens on
    /// its main page again.
    fn left_tab(&mut self, left: Tab) {
        if self.keymapping.capturing(left, false) {
            self.keyboard.take_modifiers();
        }
        self.dock.close();
        self.levels.release();
        self.sys.left_tab();
    }

    /// The editor's Save or Cancel, by its button or a key. Either closes it. Save hands the text to its
    /// purpose: the environment is saved, the network joined with the password, the keymap or file named,
    /// or the account password used. An editor on the keyboard tab then goes back to the tab the keyboard
    /// came up from, unless another editor took its place there (the new password after the current one).
    fn editor_done(&mut self, done: Done, out: &mut Out) {
        let Some(editor) = self.bottom.close_editor() else {
            return;
        };
        let on_keyboard = editor.tab() == Tab::Keyboard;
        match (done, editor.purpose) {
            (Done::Cancel, _) => {}
            (Done::Save, Purpose::Env { entry }) => {
                self.prefs.set_env(entry, &editor.text);
                out.effects.push(self.prefs.save_effect());
            }
            (Done::Save, Purpose::Password { ssid }) => self.radios.join(ssid, Some(editor.text), out),
            (Done::Save, Purpose::KeymapName(action)) => self.keymapping.named(action, &editor.text, out),
            (Done::Save, Purpose::ProfileName(ProfileKind::Keymap, action)) => self.keymapping.named_profile(action, &editor.text, out),
            (Done::Save, Purpose::ProfileName(ProfileKind::Curve, action)) => self.curving.named_profile(action, &editor.text, out),
            (Done::Save, Purpose::FileName(purpose)) => {
                if let Some(op) = crate::shelf::files::named(purpose, &editor.text) {
                    self.shelf.run_file_op(op, out);
                }
            }
            (Done::Save, Purpose::AccountPassword(step)) => crate::device::account::password_typed(step, editor.text, self.bottom.editors_mut(), out),
        }
        if on_keyboard && self.bottom.editor().is_none() {
            self.back_from_keyboard();
        }
    }

    /// Calibrates the gyro, with a notice to hold the device still.
    fn calibrate_gyro(&mut self, out: &mut Out) {
        self.bottom.notice_mut().show(NoticeKind::Gyro(None), self.clock.mono());
        out.effects.push(Effect::CalibrateGyro);
    }

    /// A hardware key. A level key steps the level and shows it. The others take a screenshot or run
    /// their Home combination.
    fn on_device_key(&mut self, key: DeviceKey, out: &mut Out) {
        let (level, direction) = match key {
            DeviceKey::VolumeUp => (Level::Volume, 1),
            DeviceKey::VolumeDown => (Level::Volume, -1),
            DeviceKey::BrightnessUp => (Level::Brightness, 1),
            DeviceKey::BrightnessDown => (Level::Brightness, -1),
            // The level is shown only when the mute changed.
            DeviceKey::Mute => {
                if self.levels.toggle_mute(out) {
                    self.bottom.notice_mut().show(NoticeKind::Level(Control::Volume), self.clock.mono());
                }
                return;
            }
            DeviceKey::Screenshot => return out.effects.push(Effect::Screenshot),
            DeviceKey::BottomView => return self.choose_tab(self.bottom.view_toggled()),
            DeviceKey::CloseWindow => return self.session.close_front_window(self.bottom.shows_windows(&self.session), out),
            DeviceKey::Quit => return self.session.quit_front(self.bottom.shows_windows(&self.session), out),
            DeviceKey::BottomPower => return self.bottom.toggle_power(self.power(), self.clock.mono()),
            DeviceKey::WindowDown => return self.bottom.window_down(&mut self.session, self.clock.mono(), out),
            DeviceKey::WindowUp => return self.bottom.window_up(&mut self.session, self.clock.mono(), out),
            DeviceKey::WindowPrevious => return self.session.step_window(false, out),
            DeviceKey::WindowNext => return self.session.step_window(true, out),
        };
        let control = if level == Level::Volume { Control::Volume } else { Control::Brightness };
        self.bottom.notice_mut().show(NoticeKind::Level(control), self.clock.mono());
        self.levels.change(self.clock.mono(), |l| l.step(level, direction, out));
    }

    fn on_slide(&mut self, slide: Slide, out: &mut Out) {
        match slide {
            Slide::Start { control: Control::Param(param), x, width } => {
                self.keymapping.param_slide_start(param, x, width);
            }
            Slide::Start { control: Control::Curve(param), x, width } => {
                self.curving.slide_start(param, x, width);
            }
            Slide::Start { control, x, width } => {
                self.levels.slide_start(control, x, width);
            }
            Slide::To { x } if self.levels.slider().is_none() => {
                self.keymapping.param_slide_to(x);
                self.curving.slide_to(x);
            }
            Slide::To { x } => {
                self.levels.change(self.clock.mono(), |l| l.slide_to(x, out));
            }
            Slide::End => {
                self.levels.slide_end();
                self.keymapping.param_slide_end(out);
                self.curving.slide_end(out);
            }
        }
    }

    // Launches.

    /// Shelf's action on its selected entry, or a tap on a stopped dock entry: shows the entry if it runs,
    /// or else starts it. A group isn't started; it is shown open in shelf.
    fn play(&mut self, entry: Arc<str>, out: &mut Out) {
        // A browsed file that is also a library entry plays as that entry.
        let entry = self.shelf.entry_of_file(&entry).unwrap_or(entry);
        if self.session.running(&entry).is_some() {
            self.show(&entry, out);
            return;
        }
        let title = self.shelf.entry(&entry).map(|g| g.title.clone());
        let mark = out.effects.len();
        self.launch(entry, None, Start::Front, out);
        // The notice comes at once, since a start may take a while: Android's first start, a Steam
        // game's, or Proton's setup.
        if let Some(title) = title.filter(|_| out.effects.iter().skip(mark).any(|e| matches!(e, Effect::Launch { .. }))) {
            self.bottom.notice_mut().show(crate::bottom::notice::NoticeKind::Starting(title), self.clock.mono());
        }
    }

    /// Shows running entry `id` (see [`Session::show`]), on the bottom screen if all its windows are there.
    fn show(&mut self, id: &str, out: &mut Out) {
        let below = self.session.windows_of(id).find(|w| w.down).map(|w| w.id);
        if let Some(window) = self.session.show(id, &self.clock, self.shelf.library(), self.bottom.notice_mut(), out) {
            self.bottom.show_program_down(window, &mut self.session, out);
        } else if let Some(window) = below.filter(|_| !self.bottom.shows_windows(&self.session)) {
            self.bottom.show_program_down(window, &mut self.session, out);
        }
    }

    /// The dock menu's Open another: launches `id` again, if that is offered (see [`State::opens_another`]).
    fn open_another(&mut self, id: Arc<str>, out: &mut Out) {
        if self.opens_another(&id) {
            self.launch(id, None, Start::Front, out);
        }
    }

    /// Launches `entry` (see [`Session::launch`]), or does what the session says instead: the Device view
    /// is opened, a group shown open in shelf, or a running entry shown.
    pub(crate) fn launch(&mut self, entry: Arc<str>, command: Option<Launch>, start: Start, out: &mut Out) {
        let tab = self.bottom.chosen_tab();
        let request = LaunchRequest { entry: entry.clone(), command, start };
        match self.session.launch(request, &self.shelf, tab, self.prefs.settings(), &self.clock, out) {
            Some(Instead::Device) => {
                self.session.cancel_front_launch();
                self.device.open(&mut self.pointer, &mut self.session, out);
            }
            Some(Instead::Group) if self.shelf.reveal(&entry) => {
                self.session.cancel_front_launch();
                self.shelf.moved(&mut self.pointer);
                self.session.command(WindowCommand::ShowShelf, out);
            }
            Some(Instead::Show) => self.show(&entry, out),
            Some(Instead::Group) | None => {}
        }
    }

    /// Enqueue was pressed on what shelf's details show. The entries are added to the running launch of
    /// their runner, or else launched (see [`crate::session::enqueue`]).
    fn enqueue(&mut self, out: &mut Out) {
        let Some(enqueued) = self.shelf.take_enqueued() else {
            return;
        };
        if self.session.enqueue(&enqueued, self.bottom.notice_mut(), self.clock.mono(), out) {
            return;
        }
        if enqueued.alone {
            // A file alone is opened as A opens it, which includes resuming where it was left.
            self.play(enqueued.entry, out);
        } else {
            let entry = self.shelf.entry_of_file(&enqueued.entry).unwrap_or(enqueued.entry);
            self.launch(entry, Some(enqueued.runner.command_for(&enqueued.files)), Start::Front, out);
        }
    }

    // Settings and keymaps.

    /// A tap or long press in the settings tab.
    fn on_settings(&mut self, id: SettingsId, long: bool, out: &mut Out) {
        match id {
            // A long press opens the tile's menu: a profile tile's picker, or the values.
            SettingsId::Tile(Setting::Keymap) if long => self.keymapping.open_picker(),
            SettingsId::Tile(Setting::Curve) if long => self.curving.open_picker(),
            SettingsId::Tile(setting) if long => self.prefs.open_menu(setting),
            SettingsId::Tile(setting @ (Setting::Keymap | Setting::Curve)) => self.next_profile(ProfileKind::of(setting), out),
            SettingsId::Tile(setting) => {
                let (len, now) = (self.menu_values_len(setting), self.panel().tile_value(setting));
                let at = (0..len).find(|i| self.menu_value(setting, *i) == Some(now));
                if let Some(next) = self.menu_value(setting, at.map_or(0, |i| (i + 1) % len.max(1))) {
                    self.change(setting, Some(next), out);
                }
            }
            SettingsId::Back => self.prefs.close_menu(),
            SettingsId::Env => {
                let entry = self.panel().editing().map(|g| g.id.clone());
                self.prefs.open_env_editor(entry, self.bottom.editors_mut());
            }
            SettingsId::Scope(scope) => self.prefs.set_scope(scope),
            SettingsId::Reset => {
                if let Some(id) = self.panel().editing().map(|g| g.id.clone()) {
                    self.prefs.reset(&id);
                }
                out.effects.push(self.prefs.save_effect());
            }
        }
    }

    /// The values a tile steps through and its menu lists, as their number and the one at `i`: the
    /// screen's announced rates after "as is" for the refresh rate (see [`crate::device::display::DisplayModes`]), the setting's
    /// choices for the others. Drawing reads them every frame, so nothing is allocated.
    fn menu_values_len(&self, setting: Setting) -> usize {
        match setting {
            Setting::Refresh => 1 + self.device.display_modes().rates().len(),
            _ => setting.choices().len(),
        }
    }

    fn menu_value(&self, setting: Setting, i: usize) -> Option<u16> {
        match (setting, i) {
            (Setting::Refresh, 0) => Some(0),
            (Setting::Refresh, i) => self.device.display_modes().rates().get(i - 1).copied(),
            _ => setting.choices().get(i).copied(),
        }
    }

    /// The rows of the open tile's menu: its values, then Reset when there is something to reset.
    pub(crate) fn menu_rows_len(&self) -> usize {
        self.prefs.menu().map_or(0, |setting| self.menu_values_len(setting) + usize::from(self.panel().resettable(setting)))
    }

    /// Row `i` of the open tile's menu: a value, or with `None`, Reset.
    pub fn menu_row(&self, i: usize) -> Option<Option<u16>> {
        let setting = self.prefs.menu()?;
        match self.menu_value(setting, i) {
            Some(value) => Some(Some(value)),
            None => (i == self.menu_values_len(setting) && self.panel().resettable(setting)).then_some(None),
        }
    }

    /// A row of the open tile's menu: its value is set where the panel edits, or Reset drops the level's
    /// own; either closes the menu.
    fn on_menu_row(&mut self, i: usize, out: &mut Out) {
        let (Some(setting), Some(value)) = (self.prefs.menu(), self.menu_row(i)) else {
            return;
        };
        self.change(setting, value, out);
        self.prefs.close_menu();
    }

    /// A profile tile's tap: the next of `kind`'s profiles where the panel edits, after Default (or Off),
    /// the level's own, and the named ones in order, then Default again.
    fn next_profile(&mut self, kind: ProfileKind, out: &mut Out) {
        let entry = self.panel().editing().map(|g| g.id.clone());
        let ids: Vec<u16> = match kind {
            ProfileKind::Keymap => profile_order(self.keymapping.keymaps(), entry.as_deref()),
            ProfileKind::Curve => profile_order(self.curving.curves(), entry.as_deref()),
        };
        let now = self.panel().profile_chosen(kind);
        let i = ids.iter().position(|id| *id == now).map_or(0, |i| (i + 1) % ids.len().max(1));
        if let Some(next) = ids.get(i) {
            self.choose_profile(kind, *next, out);
        }
    }

    /// Changes `setting` where the panel edits, in the game's overrides or the global settings. It is set
    /// to `value`, or back to the default for `None`. The change is saved, and applied after the event
    /// if the settings in force changed (see [`crate::sync`]).
    fn change(&mut self, setting: Setting, value: Option<u16>, out: &mut Out) {
        let entry = self.panel().editing().map(|g| g.id.clone());
        self.prefs.set(entry.as_ref(), setting, value);
        out.effects.push(self.prefs.save_effect());
    }

    /// Chooses profile `id` of `kind` where the panel edits, and saves the settings.
    fn choose_profile(&mut self, kind: ProfileKind, id: u16, out: &mut Out) {
        let entry = self.panel().editing().map(|g| g.id.clone());
        self.prefs.choose(entry.as_ref(), kind.setting(), id);
        out.effects.push(self.prefs.save_effect());
    }

    /// The keymaps' screens' buttons, but Back (the picker's, [`ProfileId::Back`]).
    fn on_keymap(&mut self, id: KeymapId, long: bool, out: &mut Out) {
        let cx = self.keymap_cx();
        self.keymapping.on(id, long, &cx, self.bottom.editors_mut(), out);
    }

    /// A kind's picker's buttons. Back goes back a screen; the others are the picker's.
    fn on_profile(&mut self, kind: ProfileKind, id: ProfileId, out: &mut Out) {
        match kind {
            ProfileKind::Keymap if id == ProfileId::Back => self.keymapping.back(&mut self.keyboard),
            ProfileKind::Keymap => self.keymapping.on_picker(id, self.prefs.settings(), self.bottom.editors_mut(), out),
            ProfileKind::Curve if id == ProfileId::Back => self.curving.back(),
            ProfileKind::Curve => self.curving.on_picker(id, self.prefs.settings(), self.bottom.editors_mut(), out),
        }
    }

    /// A row of the picker (see [`Keymapping::on_row`]).
    fn on_keymap_row(&mut self, i: usize, part: RowPart, long: bool, out: &mut Out) {
        let cx = self.keymap_cx();
        self.keymapping.on_row(i, part, long, &cx, self.bottom.editors_mut(), out);
    }

    /// A row of the list of layers, as the button it stands for.
    fn on_layer_row(&mut self, i: usize, part: RowPart, long: bool, out: &mut Out) {
        if let Some(id) = self.keymapping.layer_row_id(i, part) {
            self.on_keymap(id, long, out);
        }
    }

    /// What the keymaps' screens ask of the keyboard.
    fn keymap_event(&mut self, event: KeymapEvent, out: &mut Out) {
        match event {
            KeymapEvent::Capture => self.keymapping.capture(&mut self.keyboard, out),
        }
    }

    /// What a kind's picker asks of the settings.
    fn profile_event(&mut self, kind: ProfileKind, event: ProfileEvent, out: &mut Out) {
        match event {
            ProfileEvent::Choose(id) => self.choose_profile(kind, id, out),
            // The settings are saved before the profiles.
            ProfileEvent::MadeOwn(id) => {
                self.choose_profile(kind, id, out);
                out.effects.push(match kind {
                    ProfileKind::Keymap => self.keymapping.save(),
                    ProfileKind::Curve => self.curving.save_effect(),
                });
            }
            ProfileEvent::Deleted(id) => {
                self.prefs.forget(kind.setting(), id);
                out.effects.push(self.prefs.save_effect());
            }
        }
    }

    // Setting up tests.

    /// Marks the UI up, as Start does, with what the outside would be sent for it dropped.
    #[cfg(test)]
    pub(crate) fn ui_up(&mut self) {
        self.sent.ui_started();
        self.sync(&mut Out::default(), 0);
    }

    /// Handles `event` without the steps that follow it, and returns the effects it pushed.
    #[cfg(test)]
    pub(crate) fn handle(&mut self, event: Event) -> Vec<Effect> {
        let mut out = Out::default();
        self.apply_event(event, &mut out);
        out.effects
    }

    /// Handles `events` as if a rule had queued them, without the steps that follow, and returns the
    /// effects.
    #[cfg(test)]
    pub(crate) fn handle_as_queued(&mut self, events: Vec<Event>) -> Vec<Effect> {
        let mut out = Out { effects: Vec::new(), events };
        self.handle_queued(0, &mut out);
        out.effects
    }

    #[cfg(test)]
    pub(crate) fn settings_mut(&mut self) -> &mut crate::settings::Settings {
        self.prefs.settings_mut()
    }

    /// Has `running` run in scope `unit` (see [`Session::add_run`]).
    #[cfg(test)]
    pub(crate) fn add_run(&mut self, running: crate::Running, unit: crate::Instance) {
        self.session.add_run(running, unit);
    }

    #[cfg(test)]
    pub(crate) fn editor_mut(&mut self) -> Option<&mut crate::Editor> {
        self.bottom.editor_mut()
    }
}

/// The ids a profile tile steps through for `entry`'s level: Default (or Off), the level's own if it has
/// one, then the named profiles in order.
fn profile_order<B: Clone + Default + PartialEq>(profiles: &crate::profiles::Profiles<B>, entry: Option<&str>) -> Vec<u16> {
    let own = entry.and_then(|e| profiles.own_of(e)).map(|p| p.id);
    std::iter::once(0).chain(own).chain(profiles.named().map(|p| p.id)).collect()
}
