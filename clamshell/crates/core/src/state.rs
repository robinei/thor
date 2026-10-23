//! [`State`]: the areas, and what runs across them. Its impl builds the areas, gives each to outside code
//! to read, applies events in three phases (handle, settle, sync: see [`State::apply`]), and carries out
//! what needs more than one area.

use std::sync::Arc;

use crate::bottom::Bottom;
use crate::bottom::editor::{Done, Purpose};
use crate::bottom::keyboard::Keyboard;
use crate::bottom::notice::NoticeKind;
use crate::bottom::screen::Power;
use crate::device::Device;
use crate::device::buttons::DeviceKey;
use crate::device::idle::Idle;
use crate::keymap::screens::{KeymapCx, Keymapping};
use crate::kit::debounce::Debounce;
use crate::levels::slider::Control;
use crate::nav::lists::Scrolls;
use crate::out::Out;
use crate::resume::SAVE_AFTER;
use crate::session::running::OwnView;
use crate::settings::{BottomWhenIdle, MouseAtLaunch, Setting, Values};
use crate::sync::{Resend, Sent, Wanted};
use crate::{
    BtId, Clock, Dock, EditorId, Effect, Entry, EntryAct, Event, Id, Init, KeymapEvent, KeymapId, Launch, LaunchEvent, Level, LevelEvent, Levels, Library,
    ListEvent, ListId, Lists, Pad, PadRoute, Panel, Pointer, PointerEvent, Prefs, Radios, Reading, Resume, RowPart, Save, Scroll, Session, SettingsId, Shelf,
    Slide, Sys, Tab, Watch, WifiId, WindowCommand,
};

/// Everything ClamShell knows, as its areas (see the [crate docs](crate)).
#[derive(Debug, Default)]
pub struct State {
    shelf: Shelf,
    bottom: Bottom,
    session: Session,
    dock: Dock,
    /// The Device view on the main screen (see [`crate::device`]).
    device: Device,
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
    /// Where the lists but shelf are scrolled.
    scrolls: Scrolls,
    clock: Clock,
    /// Where shelf and the second screen are ([`State::resume`]), saved once they stop changing for a
    /// moment. Shelf and the bottom screen say when they changed; `apply` touches this at its end.
    resume: Debounce,
    idle: Idle,
    /// What the steps after an event last saw.
    settled: Settled,
    /// What the outside was last sent (see [`crate::sync`]).
    sent: Sent,
    /// The events the rules queue for State, lent to `apply`'s [`Out`] and returned empty, so that its
    /// room is kept.
    queue: Vec<Event>,
    /// The windows' effects while the session's steps run, put after the later steps' (see
    /// [`State::settle`]). Kept empty, so that its room is reused.
    window_effects: Vec<Effect>,
}

impl State {
    // Construction and the areas.

    pub fn new(init: Init) -> State {
        let mut s = State {
            shelf: Shelf::new(init.library),
            prefs: Prefs::new(init.settings),
            keymapping: Keymapping::new(init.keymaps),
            bottom: Bottom::new(init.window_rules),
            sys: Sys::new(init.twelve_hour),
            levels: Levels::new(init.brightness),
            session: Session::new(init.autostart, init.device_session),
            dock: init.dock,
            device: Device::new(init.sticks, init.first_start),
            ..State::default()
        };
        if let Some(resume) = &init.resume {
            resume.restore(&mut s.shelf, &mut s.bottom);
        }
        s.dock.drop_gone_at_start(s.shelf.library(), &s.session);
        s.session.follow_pausable_runners(s.shelf.library());
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
        s.settled = Settled::new(&s.session);
        s.sent = Sent::new(&s.wanted());
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

    pub fn dock(&self) -> &Dock {
        &self.dock
    }

    pub fn device(&self) -> &Device {
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

    /// The lists, their rows and where each is scrolled (see [`Lists`]).
    pub fn lists(&self) -> Lists<'_> {
        Lists {
            scrolls: &self.scrolls,
            shelf: &self.shelf,
            session: &self.session,
            prefs: &self.prefs,
            keymapping: &self.keymapping,
            radios: &self.radios,
            levels: &self.levels,
            sys: &self.sys,
            dock: &self.dock,
            pointer: &self.pointer,
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

    fn apply_event(&mut self, event: Event, out: &mut Out) {
        // Where the events this one queues start.
        let mark = out.events.len();
        match event {
            Event::Tick { unix_secs, mono_secs } => self.tick(unix_secs, mono_secs, out),
            Event::Reading(reading) => self.on_reading(reading, out),
            Event::List(e) => {
                self.on_list(e);
            }
            Event::Pointer(e) => self.on_pointer(e, out),
            Event::Slide(slide) => self.on_slide(slide, out),
            // A level row set is the one the finger landed on, and nothing once it is gone.
            Event::Level(LevelEvent::Set(level, value)) => {
                if let Some(level) = self.lists().resolve_level(level) {
                    self.levels.on_event(LevelEvent::Set(level, value), self.clock.mono(), out);
                }
            }
            Event::Level(e) => self.levels.on_event(e, self.clock.mono(), out),
            Event::Shelf(e) => self.shelf.on_event(e, &mut self.pointer, out),
            Event::Launch(e) => self.on_launch_event(e),
            Event::Windows(windows) => {
                self.session.set_windows(windows);
            }
            Event::Meta { id, meta } => {
                self.shelf.apply_meta(&id, &meta);
            }
            Event::Library(library) => {
                self.library_read(*library, out);
                self.sent.resend(Resend::Shown);
            }
            Event::Files(e) => {
                self.shelf.on_files(e, &mut self.pointer, self.bottom.notice_mut(), self.clock.mono(), out);
            }
            Event::Fetching(on) => {
                self.shelf.set_fetching(on);
                self.sent.resend(Resend::Shown);
            }
            Event::Tray(items) => {
                self.session.set_tray(items, &self.clock, self.shelf.library(), out);
            }
            Event::Units(units) => {
                self.session.set_units(units);
            }
            Event::Start => {
                self.ui_started(out);
            }
            // On a screen that listens for input, Home leaves it. Home is never taken as an input.
            Event::Home if self.listening() => {
                self.keymapping.back(&mut self.keyboard);
            }
            // When the Device view is in shelf's place, Home goes back to shelf.
            Event::Home if self.session.shelf_in_front() && self.device.is_open() => self.device.close(&mut self.pointer, self.shelf.selected()),
            Event::Home => self.session.home(out),
            Event::Listened(input) => {
                self.keymapping.listened(input, self.pad.route() == PadRoute::Listen, out);
            }
            Event::PadSwitch => self.pad.switch(),
            // Never applied here: the UI handles pad navigation itself.
            Event::Pad(_) => {}
            Event::TextInput(active) => {
                if self.bottom.text_input(active) {
                    self.left_tab();
                }
            }
            Event::Pointing(pointing) => self.bottom.point(pointing, out),
            Event::Resumed => {
                self.idle.resumed(self.clock.mono());
                self.sent.resend(Resend::Resumed);
            }
            Event::Idle(event) => self.idle.heard(event, self.clock.mono()),
            Event::Device(key) => {
                self.on_device_key(key, out);
            }
            Event::BottomTouched => self.bottom.used(&self.session, &self.clock),
            Event::Frozen(keys) => {
                self.session.found_frozen(keys);
            }
            Event::Paste => {
                self.bottom.paste();
            }
            Event::Screenshot(result) => {
                self.bottom.notify(crate::bottom::notice::NoticeKind::Screenshot(result), self.clock.mono());
            }
            Event::Sticks(event) => {
                self.device.sticks_event(event, out);
            }
            Event::RefreshCalibrated(result) => {
                self.device.refresh_done(result);
                self.sent.resend(Resend::RefreshCalibrated);
            }
            Event::GyroCalibrated(ok) => {
                self.bottom.notify(crate::bottom::notice::NoticeKind::Gyro(Some(ok)), self.clock.mono());
            }
            Event::AccountChanged(result) => {
                // What the helper did, or why it failed, shown for a moment.
                self.bottom.notify(crate::bottom::notice::NoticeKind::Account(result), self.clock.mono());
            }
            Event::ChooseTab(tab) => {
                self.choose_tab(tab);
            }
            Event::Entry(id, act) => match act {
                EntryAct::Play => self.play(id, out),
                EntryAct::Show => self.session.show(&id, &self.clock, self.shelf.library(), self.bottom.notice_mut(), out),
                EntryAct::OpenAnother => {
                    if self.opens_another(&id) {
                        self.launch(id, None, true, out);
                    }
                }
                EntryAct::Suspend => self.session.suspend(&id, out),
                EntryAct::ToggleAutostart => self.session.toggle_autostart(id, out),
                EntryAct::TogglePin => self.dock.toggle_pin(id, &self.session, out),
                EntryAct::Quit => self.session.quit(&id, self.shelf.library(), out),
                EntryAct::ForceKill => self.session.force_kill(&id, self.shelf.library(), out),
            },
            Event::Enqueue => {
                self.enqueue(out);
            }
            Event::Keymap(e) => {
                self.keymap_event(e, out);
            }
            Event::EditOnKeyboard(editor) => {
                if self.bottom.open_on_keyboard(editor) {
                    self.left_tab();
                }
            }
            Event::Quit => {
                out.effects.push(Effect::Save(Save::Resume(self.resume())));
                out.effects.push(Effect::Quit);
            }
        }
        self.handle_queued(mark, out);
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
    /// what the earlier ones settled. The first eight follow the session's inputs (the windows, systemd's
    /// scopes, the tray, the launcher's messages, the library and the clock), and run only when one
    /// changed, which [`crate::Session::version`] counts:
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
    /// The rest run after every event:
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
    ///
    /// A step does nothing when nothing changed for it.
    fn settle(&mut self, out: &mut Out) {
        if self.session.version() != self.settled.session {
            self.settled.session = self.session.version();
            if self.session.follow_units(&self.clock, &mut self.shelf, self.bottom.notice_mut(), out) {
                self.bottom.unwake();
            }
            let mut due = Vec::new();
            self.session.autostart_due(self.shelf.library(), &mut due);
            // They start in the background, so what was going to be shown still is.
            for id in due {
                self.launch(id, None, false, out);
            }
            // The windows' effects go after those of the steps that follow them, below.
            let mut windows = Out { effects: std::mem::take(&mut self.window_effects), events: Vec::new() };
            let mut steam = Vec::new();
            if self.session.start_by_app_id(&self.clock, self.shelf.library(), &mut steam) {
                self.bottom.unwake();
            }
            // Before following the windows, which leaves a starting launch's windows where they are.
            for id in steam {
                self.launch(id, None, true, &mut windows);
            }
            self.session.follow_windows(&self.clock, &mut self.shelf, &mut windows);
            self.bottom.follow_window_rules(&self.session, out);
            self.session.follow_background(self.clock.mono(), self.shelf.library(), self.prefs.settings(), out);
            self.session.show_pending(out);
            self.session.show_reopened(self.clock.mono(), out);
            self.session.follow_quitting(self.shelf.library(), out);
            self.session.follow_closed(self.clock.mono(), out);
            // After handling the front one closing, so that a window just taken by a browsed file is shown
            // over it.
            out.effects.append(&mut windows.effects);
            out.events.append(&mut windows.events);
            self.window_effects = windows.effects;
            self.dock.follow(&self.session);
            self.session.followed();
        }
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
        self.pad.reconcile(self.session.front(), self.bottom.tab(), self.pointer.show_focus(), listening, moving)
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
            shown: self.shelf.fetch_first(),
            browsed: self.shelf.browsed(),
            hidden: self.shelf.files().hidden(),
            running: self.session.running_entries(),
            stage: self.idle.stage(),
            levels: &self.levels,
            program: self.session.program(),
            settings,
            bottom_on: !self.bottom.off(),
            bottom_seen: !self.bottom.shows_windows(&self.session) && !self.bottom.off(),
            tab,
            device_open: self.device.is_open(),
            keymap: self.keymapping.version_of(settings.get(Setting::Keymap)),
            keymapping: &self.keymapping,
            route: self.pad.route(),
        }
    }

    /// Sends the outside what differs from what it was last sent.
    fn sync(&mut self, out: &mut Out) {
        let mut sent = std::mem::take(&mut self.sent);
        sent.sync(&self.wanted(), out);
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
        // The session's steps follow the time.
        self.session.changed();
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
    /// that are gone.
    fn library_read(&mut self, library: Library, out: &mut Out) {
        self.session.changed();
        let problems = &library.problems;
        if !problems.is_empty() && *problems != self.shelf.library().problems {
            let first = problems.first().cloned().unwrap_or_default();
            self.bottom.notify(crate::bottom::notice::NoticeKind::Problems(first, problems.len()), self.clock.mono());
        }
        if self.shelf.reload(library) {
            self.shelf.moved(&mut self.pointer);
        }
        self.session.follow_pausable_runners(self.shelf.library());
        self.keymapping.follow_runner_keymaps(self.shelf.library(), &mut self.prefs, out);
        out.effects.push(crate::session::running::land_rules(self.shelf.library()));
        if self.dock.drop_gone(self.shelf.library(), &self.session) {
            out.effects.push(self.dock.save_effect());
        }
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
        self.keymapping.follow_runner_keymaps(self.shelf.library(), &mut self.prefs, out);
        out.effects.push(self.bottom.window_rules().effect());
    }

    fn on_launch_event(&mut self, event: LaunchEvent) {
        self.session.changed();
        match event {
            LaunchEvent::Position { entry: game, position } => self.shelf.set_position(&game, position, self.clock.unix()),
            // A launch that never started doesn't change what runs, but it is no longer starting.
            LaunchEvent::LaunchFailed(game) => self.session.launch_failed(game, self.shelf.library(), &self.clock, self.bottom.notice_mut()),
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
            PointerEvent::Activate(id) => self.activate(&id, false, out),
            PointerEvent::LongPress(id) => self.activate(&id, true, out),
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
    fn activate(&mut self, id: &Id, long: bool, out: &mut Out) {
        // The list item the press came down on is gone.
        let Some(id) = self.lists().resolve(id) else {
            return;
        };
        self.pointer.focus_on(id.clone());
        match id {
            Id::Shelf(id) => {
                self.shelf.on_target(id, long, &mut self.pointer, out);
            }
            Id::Dock(id) => {
                self.dock.on(id, long, &self.session, &mut self.bottom, out);
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
            Id::Tab(tab) => {
                self.choose_tab(tab);
            }
            // A tap opens the tab. Slides are handled by the UI and don't end in a tap.
            Id::Control(control) if !long => {
                let tab = match control {
                    Control::Volume => Tab::Volume,
                    Control::Brightness => Tab::Brightness,
                    // A keymap parameter is held or slid, not tapped.
                    Control::Param(_) => return,
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
            Id::BottomWindows => {
                if let Some(window) = self.bottom.hidden_down(&self.session) {
                    self.bottom.show_program_down(window, out);
                }
            }
            Id::Time(id) => self.sys.on_time(id, self.scrolls.get_mut(ListId::Time), out),
            Id::Device(id) => {
                self.device.tap(id, self.clock.mono(), &mut self.bottom, &mut self.pointer, self.shelf.selected(), out);
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
        Panel::new(&self.session, &self.shelf, &self.prefs, &self.keymapping)
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
        self.keymapping.shown(self.bottom.tab()).map(|_| crate::Id::Keymap(crate::KeymapId::Back))
    }

    /// Where shelf and the second screen are, to come back to.
    pub(crate) fn resume(&self) -> Resume {
        Resume::of(&self.shelf, &self.bottom)
    }

    /// Whether the pad listens for inputs (see [`Keymapping::listening`]).
    pub(crate) fn listening(&self) -> bool {
        self.keymapping.listening(self.bottom.tab(), self.bottom.editor().is_some())
    }

    /// What the keymaps' screens read from the settings panel and the clock (see [`KeymapCx`]).
    fn keymap_cx(&self) -> KeymapCx {
        let panel = self.panel();
        KeymapCx { game: panel.editing().map(|g| g.id.clone()), chosen: panel.keymap_chosen(), now: self.clock.mono() }
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
            ListId::Keymaps => self.on_keymap_row(i, part, long, out),
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
        if self.bottom.choose_tab(tab) {
            self.left_tab();
        }
    }

    /// Goes back to the tab the keyboard came up from, if it did.
    fn back_from_keyboard(&mut self) {
        if self.bottom.back_from_keyboard() {
            self.left_tab();
        }
    }

    /// The second screen's tab changed, and its watch follows (see [`crate::sync::Sent::watches`]). What the
    /// tab left had going ends: the dock's menu or move, and any level row being set. The time tab opens
    /// on its main page again.
    fn left_tab(&mut self) {
        self.dock.close();
        self.levels.release();
        self.sys.reset_time_page();
    }

    /// The editor's Save or Cancel, by its button or a key. Either closes it. Save hands the text to its
    /// purpose: the environment is saved, the network joined with the password, the keymap or file named,
    /// or the account password used. Cancelling a file's name goes back to the tab the keyboard came up
    /// from.
    fn editor_done(&mut self, done: Done, out: &mut Out) {
        let Some(editor) = self.bottom.close_editor() else {
            return;
        };
        match (done, editor.purpose) {
            // The keyboard goes back to the tab it came up from.
            (Done::Cancel, Purpose::FileName(_)) => self.back_from_keyboard(),
            (Done::Cancel, _) => {}
            (Done::Save, Purpose::Env { entry: game }) => {
                self.prefs.set_env(game, &editor.text);
                out.effects.push(self.prefs.save_effect());
            }
            (Done::Save, Purpose::Password { ssid }) => self.radios.join(ssid, Some(editor.text), out),
            (Done::Save, Purpose::KeymapName(action)) => self.keymapping.named(action, &editor.text, out),
            (Done::Save, Purpose::FileName(purpose)) => {
                self.back_from_keyboard();
                out.effects.extend(crate::shelf::files::named(purpose, &editor.text).map(Effect::Files));
            }
            (Done::Save, Purpose::AccountPassword(step)) => crate::device::account::password_typed(step, editor.text, self.bottom.editors_mut(), out),
        }
    }

    /// A hardware key. A level key steps the level and shows it. The others take a screenshot or run
    /// their Home combination.
    fn on_device_key(&mut self, key: DeviceKey, out: &mut Out) {
        let (level, direction) = match key {
            DeviceKey::VolumeUp => (Level::Volume, 1),
            DeviceKey::VolumeDown => (Level::Volume, -1),
            DeviceKey::BrightnessUp => (Level::Brightness, 1),
            DeviceKey::BrightnessDown => (Level::Brightness, -1),
            DeviceKey::Mute => {
                self.bottom.notify(NoticeKind::Level(Control::Volume), self.clock.mono());
                return self.levels.toggle_mute(out);
            }
            DeviceKey::Screenshot => return out.effects.push(Effect::Screenshot),
            DeviceKey::BottomView => return self.bottom.toggle_view(&self.session),
            DeviceKey::CloseWindow => return self.session.close_front_window(self.bottom.shows_windows(&self.session), out),
            DeviceKey::Quit => return self.session.quit_front(self.bottom.shows_windows(&self.session), out),
            DeviceKey::BottomPower => return self.bottom.toggle_power(self.power(), self.clock.mono()),
            DeviceKey::WindowDown => return self.bottom.window_down(&self.session, self.clock.mono(), out),
            DeviceKey::WindowUp => return self.bottom.window_up(&self.session, self.clock.mono(), out),
        };
        let control = if level == Level::Volume { Control::Volume } else { Control::Brightness };
        self.bottom.notify(NoticeKind::Level(control), self.clock.mono());
        self.levels.change(self.clock.mono(), |l| l.step(level, direction, out));
    }

    fn on_slide(&mut self, slide: Slide, out: &mut Out) {
        match slide {
            Slide::Start { control: Control::Param(param), x, width } => {
                self.keymapping.param_slide_start(param, x, width);
            }
            Slide::Start { control, x, width } => {
                self.levels.slide_start(control, x, width);
            }
            Slide::To { x } if self.levels.slider().is_none() => {
                self.keymapping.param_slide_to(x);
            }
            Slide::To { x } => {
                self.levels.change(self.clock.mono(), |l| l.slide_to(x, out));
            }
            Slide::End => {
                self.levels.slide_end();
                self.keymapping.param_slide_end(out);
            }
        }
    }

    // Launches.

    /// Shelf's action on its selected entry, or a tap on a stopped dock entry: shows the entry if it runs,
    /// or else starts it. A group isn't started; it is shown open in shelf.
    fn play(&mut self, game: Arc<str>, out: &mut Out) {
        // A browsed file that is also a library entry plays as that entry.
        let game = self.shelf.entry_of_file(&game).unwrap_or(game);
        if self.session.running(&game).is_some() {
            self.session.show(&game, &self.clock, self.shelf.library(), self.bottom.notice_mut(), out);
            return;
        }
        let title = self.shelf.entry(&game).map(|g| g.title.clone());
        let mark = out.effects.len();
        self.launch(game, None, true, out);
        // The notice comes at once, since a start may take a while: Android's first start, a Steam
        // game's, or Proton's setup.
        if let Some(title) = title.filter(|_| out.effects.iter().skip(mark).any(|e| matches!(e, Effect::Launch { .. }))) {
            self.bottom.notify(crate::bottom::notice::NoticeKind::Starting(title), self.clock.mono());
        }
    }

    /// Launches `game` (see [`Session::launch`]). The Device view is opened, and a group shown open in
    /// shelf.
    pub(crate) fn launch(&mut self, game: Arc<str>, command: Option<Launch>, shown: bool, out: &mut Out) {
        let tab = self.bottom.chosen_tab();
        match self.session.launch(game.clone(), command, shown, &self.shelf, tab, self.prefs.settings(), &self.clock, self.bottom.notice_mut(), out) {
            Some(OwnView::Device) => self.device.open(&mut self.pointer, out),
            Some(OwnView::Group) if self.shelf.reveal(&game) => {
                self.shelf.moved(&mut self.pointer);
                out.effects.push(Effect::Window(WindowCommand::ShowShelf));
            }
            Some(OwnView::Group) | None => {}
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
            self.launch(entry, Some(enqueued.runner.command_for(&enqueued.files)), true, out);
        }
    }

    // Settings and keymaps.

    /// A tap or long press in the settings tab.
    fn on_settings(&mut self, id: SettingsId, long: bool, out: &mut Out) {
        match id {
            SettingsId::Tile(Setting::Keymap) if !long => self.keymapping.open_picker(),
            SettingsId::Tile(setting) => {
                let value = if long { None } else { Some(setting.next(self.panel().tile_value(setting))) };
                self.change(setting, value, out);
            }
            SettingsId::Env => {
                let game = self.panel().editing().map(|g| g.id.clone());
                self.prefs.open_env_editor(game, self.bottom.editors_mut());
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

    /// Changes `setting` where the panel edits, in the game's overrides or the global settings. It is set
    /// to `value`, or back to the default for `None`. The change is saved, and applied after the event
    /// if the settings in force changed (see [`crate::sync`]).
    fn change(&mut self, setting: Setting, value: Option<u16>, out: &mut Out) {
        let game = self.panel().editing().map(|g| g.id.clone());
        self.prefs.set(game.as_ref(), setting, value);
        out.effects.push(self.prefs.save_effect());
    }

    /// The keymaps' screens' buttons. Back goes back a screen; the others are the screen's.
    fn on_keymap(&mut self, id: KeymapId, long: bool, out: &mut Out) {
        if id == KeymapId::Back {
            self.keymapping.back(&mut self.keyboard);
            return;
        }
        let cx = self.keymap_cx();
        self.keymapping.on(id, long, &cx, self.prefs.settings(), &mut self.bottom, out);
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

    /// What the keymaps' screens ask of the settings and the keyboard.
    fn keymap_event(&mut self, event: KeymapEvent, out: &mut Out) {
        match event {
            KeymapEvent::Choose(id) => self.change(Setting::Keymap, Some(id), out),
            // The settings are saved before the keymaps.
            KeymapEvent::MadeOwn(id) => {
                self.change(Setting::Keymap, Some(id), out);
                out.effects.push(self.keymapping.save());
            }
            KeymapEvent::Deleted(id) => {
                self.prefs.forget_keymap(id);
                out.effects.push(self.prefs.save_effect());
            }
            KeymapEvent::Capture => self.keymapping.capture(&mut self.keyboard, out),
        }
    }

    // Setting up tests.

    /// Marks the UI up, as Start does, with what the outside would be sent for it dropped.
    #[cfg(test)]
    pub(crate) fn ui_up(&mut self) {
        self.sent.ui_started();
        self.sync(&mut Out::default());
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

/// What the steps after an event last saw (see [`State::settle`]).
#[derive(Debug, Default)]
pub(crate) struct Settled {
    /// The session's version (see [`crate::Session::version`]).
    session: u64,
}

impl Settled {
    /// As at start: the session's steps run on the first change to what it follows.
    pub(crate) fn new(session: &crate::Session) -> Settled {
        Settled { session: session.version() }
    }
}
