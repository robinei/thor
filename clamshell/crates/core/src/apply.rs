//! Applying an event in three phases: the feature's rules handle it, the state is settled, which brings
//! everything that follows from it elsewhere up to date in an order where each step finds the ones it
//! depends on settled, and the outside is told what changed for it (see [`crate::sync`]).

use crate::bottom::editor::Done;
use crate::levels::slider::Control;
use crate::out::Out;
use crate::resume::SAVE_AFTER;
use crate::settings::MouseAtLaunch;
use crate::sync::Resend;
use crate::{
    BtId, Dirty, EditorId, Effect, EntryAct, Event, Id, Level, Library, ListId, PadRoute, PointerEvent, Reading, Save, SettingsId, ShelfId, State, Tab, WifiId,
};

/// What the steps after an event last saw (see [`State::settle`]).
#[derive(Debug, Default)]
pub(crate) struct Settled {
    /// The session's version (see [`crate::Session::version`]).
    session: u64,
    /// The running entries' ids before the session's steps, kept so that the list is reused.
    running: Vec<std::sync::Arc<str>>,
}

impl Settled {
    /// As at start: the session's steps run on the first change to what it follows.
    pub(crate) fn new(session: &crate::Session) -> Settled {
        Settled { session: session.version(), running: Vec::new() }
    }
}

impl State {
    /// Applies one event and returns which screens changed and what must happen outside. After the event
    /// itself, the state is settled ([`State::settle`]), and then the outside is told what changed for it
    /// ([`State::sync`]).
    pub fn apply(&mut self, event: Event) -> (Dirty, Vec<Effect>) {
        let mut out = Out::default();
        self.apply_event(event, &mut out);
        self.settle(&mut out);
        self.sync(&mut out);
        // Both are taken, so neither is left marked.
        if self.shelf.take_unsaved() | self.bottom.take_unsaved() {
            self.resume.touch(self.clock.mono());
        }
        // The follow-up steps don't queue events: one queued here would never be handled.
        debug_assert!(out.events.is_empty(), "an event queued after the event was handled: {:?}", out.events);
        (out.dirty, out.effects)
    }

    /// Brings the state up to date with what happened. The steps run in this order, since each reads
    /// what the earlier ones settled. The first six follow the session's inputs (the windows, systemd's
    /// scopes, the tray, the launcher's messages, the library and the clock), and run only when one
    /// changed, which [`crate::Session::version`] counts:
    ///
    /// 1. systemd's scopes: entries found running, found ones promoted to launches, and ours ended
    ///    ([`crate::Session::follow_units`]). The first time, the entries to start with ClamShell are
    ///    started.
    /// 2. The windows: entries started by their app ids ([`crate::Session::start_by_app_id`]), with Steam's
    ///    games launched before the windows are followed, since a starting launch's window is left where
    ///    it is; then windows moved to their entries, programs found by them, and entries ended by them
    ///    ([`crate::Session::follow_windows`], which compares with the windows the last pass saw). Then the
    ///    rules that send windows down. The windows' effects come after those of steps 3 and 4.
    /// 3. In the background: entries paused, thawed or ended by their setting. This reads 1 and 2.
    /// 4. What is shown: a launch's first window, a workspace whose window came back, and going back to
    ///    what was before when the one in front closed.
    /// 5. Quitting entries are ended once their windows are gone. This reads 1 to 3.
    /// 6. The dock: running entries are added to it and ended ones removed.
    ///
    /// The rest run after every event:
    ///
    /// 7. The second screen's dimming.
    /// 8. The browsed folder's selection and armed action.
    /// 9. The idle stage.
    /// 10. The program in force.
    /// 11. The second screen turning off or on by itself, which reads the program in force's setting and
    ///     darkness.
    /// 12. The pad mouse, when the program in force changed.
    /// 13. What claims the pad, and where it goes. The focus ring shows where the pad is, so it is hidden
    ///     when the pad leaves ClamShell's screens.
    ///
    /// A step does nothing when nothing changed for it.
    fn settle(&mut self, out: &mut Out) {
        if self.session.version() != self.settled.session {
            self.settled.session = self.session.version();
            let mut before = std::mem::take(&mut self.settled.running);
            before.clear();
            before.extend(self.session.running_entries().iter().map(|r| r.id.clone()));
            if self.session.follow_units(&self.clock, &mut self.shelf, self.bottom.notice_mut(), out) {
                self.bottom.unwake();
            }
            let mut due = Vec::new();
            self.session.autostart_due(self.shelf.library(), &mut due);
            // They start in the background, so what was going to be shown still is.
            for id in due {
                self.launch(id, None, false, out);
            }
            // The windows' effects are moved after those of the steps that follow them, below.
            let windows_from = out.effects.len();
            let mut steam = Vec::new();
            if self.session.start_by_app_id(&self.clock, self.shelf.library(), &mut steam) {
                self.bottom.unwake();
            }
            // Before following the windows, which leaves a starting launch's windows where they are.
            for id in steam {
                self.launch(id, None, true, out);
            }
            self.session.follow_windows(&self.clock, &mut self.shelf, out);
            let by_windows = out.effects.len() - windows_from;
            self.bottom.follow_window_rules(&self.session, out);
            self.session.follow_background(self.clock.mono(), self.shelf.library(), self.prefs.settings(), out);
            self.session.show_pending(out);
            self.session.show_reopened(self.clock.mono(), out);
            self.session.follow_quitting(self.shelf.library(), out);
            self.session.follow_closed(self.clock.mono(), out);
            // After handling the front one closing, so that a window just taken by a browsed file is shown
            // over it.
            if let Some(effects) = out.effects.get_mut(windows_from..) {
                effects.rotate_left(by_windows);
            }
            let docked = self.dock.follow(&self.session);
            self.session.followed();
            // The dock and the running entries are shown on the second screen.
            if docked || !self.session.running_entries().iter().map(|r| &r.id).eq(before.iter()) {
                out.mark(Dirty::BOTTOM);
            }
            self.settled.running = before;
        }
        if self.bottom.follow_dim(&self.session, &self.clock) {
            out.mark(Dirty::BOTTOM);
        }
        self.shelf.follow_folder();
        self.follow_idle(out);
        let changed = self.session.follow_program();
        self.bottom.follow_power(self.power(), self.clock.mono());
        if changed && self.follow_pad_mouse() {
            out.mark(Dirty::BOTH);
        }
        if self.reconcile_pad().is_some_and(|route| route != PadRoute::Ui) && self.pointer.hide_focus() {
            out.mark(Dirty::BOTH);
        }
    }

    /// Called when the program in force changed. Turns the pad mouse on if the program wants it
    /// (Setting::PadMouse), and off again if the program had turned it on. Returns whether it changed.
    fn follow_pad_mouse(&mut self) -> bool {
        let wanted = self.session.program().is_some() && self.in_force().choice::<MouseAtLaunch>() == MouseAtLaunch::On;
        self.pad.follow_game(wanted)
    }

    /// Works out where the pad goes as things are now. Returns the new route if it changed.
    pub(crate) fn reconcile_pad(&mut self) -> Option<PadRoute> {
        let listening = self.listening();
        let moving = self.dock.moving().is_some() && self.bottom.tab() == Tab::Dock;
        self.pad.reconcile(self.session.front(), self.bottom.tab(), self.pointer.show_focus(), listening, moving)
    }

    fn apply_event(&mut self, event: Event, out: &mut Out) {
        match event {
            Event::Tick { unix_secs, mono_secs } => self.tick(unix_secs, mono_secs, out),
            Event::Reading(reading) => self.reading_came(reading, out),
            Event::List(e) => {
                if self.on_list(e) {
                    out.mark(e.list().screens());
                }
            }
            Event::Pointer(e) => self.pointer_event(e, out),
            Event::Slide(slide) => self.on_slide(slide, out),
            Event::Level(e) => self.on_level(e, out),
            Event::Shelf(e) => self.shelf.on_event(e, &mut self.pointer, out),
            Event::Launch(e) => self.on_launch_event(e, out),
            Event::Windows(windows) => {
                self.session.set_windows(windows);
                out.mark(Dirty::BOTH);
            }
            Event::Meta { id, meta } => {
                if self.shelf.apply_meta(&id, &meta) {
                    out.mark(Dirty::BOTH);
                }
            }
            Event::Library(library) => {
                out.mark(Dirty::BOTH);
                self.library_read(*library, out);
                self.sent.resend(Resend::Shown);
            }
            Event::Files(e) => {
                out.mark(Dirty::BOTH);
                self.shelf.on_files(e, &mut self.pointer, self.bottom.notice_mut(), self.clock.mono(), out);
            }
            Event::Fetching(on) => {
                self.shelf.set_fetching(on);
                self.sent.resend(Resend::Shown);
            }
            Event::Tray(items) => {
                if self.session.set_tray(items, &self.clock, self.shelf.library(), out) {
                    out.mark(Dirty::BOTH);
                }
            }
            Event::Units(units) => {
                self.session.set_units(units);
                out.mark(Dirty::BOTTOM);
            }
            Event::Start => {
                out.mark(Dirty::BOTH);
                self.ui_started(out);
            }
            // On a screen that listens for input, Home leaves it. Home is never taken as an input.
            Event::Home if self.listening() => {
                self.keymap_back();
                out.mark(Dirty::BOTTOM);
            }
            // When the Device view is in shelf's place, Home goes back to shelf.
            Event::Home if self.session.shelf_in_front() && self.device.is_open() => self.close_device(),
            Event::Home => self.session.home(out),
            Event::Listened(input) => {
                out.mark(Dirty::BOTTOM);
                self.keymapping.listened(input, self.pad.route() == PadRoute::Listen, out);
            }
            Event::PadSwitch => self.pad.switch(),
            // Never applied here: the UI handles pad navigation itself.
            Event::Pad(_) => {}
            Event::TextInput(active) => {
                let (dirty, changed) = self.bottom.text_input(active);
                if changed {
                    self.left_tab();
                }
                out.mark(dirty);
            }
            Event::Pointing(pointing) => self.bottom.point(pointing, out),
            Event::Resumed => {
                self.idle.resumed(self.clock.mono());
                self.sent.resend(Resend::Resumed);
            }
            Event::Idle(event) => self.idle.heard(event, self.clock.mono()),
            Event::Device(key) => {
                out.mark(Dirty::BOTTOM);
                self.on_device_key(key, out);
            }
            Event::BottomTouched => out.mark(self.bottom.used(&self.session, &self.clock)),
            Event::Frozen(keys) => {
                self.session.found_frozen(keys);
                out.mark(Dirty::BOTTOM);
            }
            Event::Paste => {
                self.bottom.paste();
                out.mark(Dirty::BOTTOM);
            }
            Event::Screenshot(result) => {
                self.on_screenshot(result);
                out.mark(Dirty::BOTTOM);
            }
            Event::Sticks(event) => {
                out.mark(Dirty::TOP);
                self.device.sticks_event(event, out);
            }
            Event::RefreshCalibrated(result) => {
                self.device.refresh_done(result);
                out.mark(Dirty::TOP);
                self.sent.resend(Resend::RefreshCalibrated);
            }
            Event::GyroCalibrated(ok) => {
                self.bottom.notify(crate::bottom::notice::NoticeKind::Gyro(Some(ok)), self.clock.mono());
                out.mark(Dirty::BOTTOM);
            }
            Event::AccountChanged(result) => {
                // What the helper did, or why it failed, shown for a moment.
                self.bottom.notify(crate::bottom::notice::NoticeKind::Account(result), self.clock.mono());
                out.mark(Dirty::BOTTOM);
            }
            Event::ChooseTab(tab) => {
                self.choose_tab(tab);
                out.mark(Dirty::BOTH);
            }
            Event::Entry(id, act) => {
                out.mark(Dirty::BOTTOM);
                match act {
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
                }
            }
            Event::Enqueue => {
                out.mark(Dirty::BOTTOM);
                self.enqueue(out);
            }
            Event::Keymap(e) => {
                out.mark(Dirty::BOTTOM);
                self.keymap_event(e, out);
            }
            Event::EditOnKeyboard(editor) => {
                if self.bottom.open_on_keyboard(editor) {
                    self.left_tab();
                }
                out.mark(Dirty::BOTH);
            }
            Event::Quit => {
                out.effects.push(Effect::Save(Save::Resume(self.resume())));
                out.effects.push(Effect::Quit);
            }
        }
        // What the event's rules asked of State, in order, after the effects they pushed.
        for queued in std::mem::take(&mut out.events) {
            self.apply_event(queued, out);
        }
    }

    /// A reading from a system thread. After a status reading, the battery warning is checked.
    fn reading_came(&mut self, reading: Reading, out: &mut Out) {
        let status = matches!(reading, Reading::Status(_));
        let dirty = self.on_reading(reading);
        if status {
            self.sys.battery_warning(out);
        }
        out.mark(if self.follow_lists() { Dirty::BOTTOM } else { dirty });
    }

    /// A touch or a pad focus change. Focusing or pressing a target on the second screen counts as using
    /// that screen.
    fn pointer_event(&mut self, e: PointerEvent, out: &mut Out) {
        let used = match &e {
            PointerEvent::Focus(id) | PointerEvent::Activate(id) | PointerEvent::LongPress(id) | PointerEvent::Down(id) => id.screen() == Dirty::BOTTOM,
            _ => false,
        };
        if used {
            out.mark(self.bottom.used(&self.session, &self.clock));
        }
        self.on_pointer(e, out);
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
        out.effects.extend(self.follow_runner_keymaps());
        out.effects.push(crate::session::running::land_rules(self.shelf.library()));
        if self.dock.drop_gone(self.shelf.library(), &self.session) {
            out.effects.push(Effect::Save(Save::Dock(self.dock.pinned_ids())));
        }
    }

    /// The UI is up. Sends what was waiting for it: the dock if pins were dropped at start, the
    /// brightness, the runners' keymaps and the down rules, and after the event the pad's route and
    /// keymap. The open tab's watch starts from now (see [`crate::sync::Sent::ui_started`]).
    fn ui_started(&mut self, out: &mut Out) {
        self.sent.ui_started();
        self.sent.resend(Resend::Start);
        if self.dock.take_unsaved() {
            out.effects.push(Effect::Save(Save::Dock(self.dock.pinned_ids())));
        }
        out.effects.extend(self.levels.brightness_effect());
        out.effects.extend(self.follow_runner_keymaps());
        out.effects.push(self.bottom.window_rules().effect());
    }

    /// The clock ticked. Once a minute, the bar's clock and both screens' burn-in orbit move on. Changes
    /// are saved once they have stopped for a moment.
    fn tick(&mut self, unix_secs: u64, mono_secs: u64, out: &mut Out) {
        // The first tick. Anything idle heard before it was timed at 0, so count from now.
        if !self.clock.read() {
            self.idle.resumed(mono_secs);
        }
        let minute_changed = self.clock.tick(unix_secs, mono_secs);
        // The session's steps follow the time.
        self.session.changed();
        if self.resume.due(mono_secs, SAVE_AFTER) {
            out.effects.push(Effect::Save(Save::Resume(self.resume())));
        }
        self.levels.save_due(mono_secs, out);
        let notice_gone = self.bottom.expire_notice(mono_secs);
        if minute_changed {
            out.mark(Dirty::BOTH);
        } else if notice_gone {
            out.mark(Dirty::BOTTOM);
        }
    }

    /// A reading of the system. It is stored, and the second screen is redrawn if it shows what changed.
    fn on_reading(&mut self, reading: Reading) -> Dirty {
        let shown = self.bottom.tab().shows(&reading);
        let changed = match reading {
            // The volume, mute and brightness go to the levels, where a finger on a slider wins over the
            // reading.
            Reading::Status(status) => {
                let levels = self.levels.read(status.volume, status.muted, status.brightness);
                self.sys.read_status(status) | levels
            }
            Reading::Clipboard(text) => self.bottom.read_clipboard(text),
            Reading::Icon { entry, path } => self.session.set_icon(&entry, path),
            // The graphs move on with every reading.
            Reading::Metrics(metrics) => {
                self.sys.read_metrics(metrics);
                true
            }
            Reading::Frames { fps, frame_ms } => {
                self.sys.frames(fps, frame_ms);
                true
            }
            Reading::Wifi(wifi) => self.radios.read_wifi(wifi),
            Reading::Bluetooth(bluetooth) => self.radios.read_bluetooth(bluetooth),
            Reading::Battery(battery) => self.sys.read_battery(battery),
            Reading::Audio(audio) => self.levels.read_audio(audio),
            Reading::Backlights(backlights) => self.levels.read_backlights(backlights),
            Reading::Time(time) => self.sys.read_time(time),
            Reading::Account(account) => {
                // The account is shown in the Device view, on the main screen.
                return if self.device.read_account(account) { Dirty::TOP } else { Dirty::NONE };
            }
        };
        if changed && shown { Dirty::BOTTOM } else { Dirty::NONE }
    }

    fn on_pointer(&mut self, event: PointerEvent, out: &mut Out) {
        // Input that reached ClamShell itself, such as the pad driving the screens.
        self.idle.own_input(self.clock.mono());
        match event {
            PointerEvent::Focus(id) => {
                self.show_focused(&id);
                self.pointer.focus_on(id);
                self.pointer.show();
                out.mark(Dirty::BOTH);
            }
            PointerEvent::HideFocus => {
                if self.pointer.hide_focus() {
                    out.mark(Dirty::BOTH);
                }
            }
            // A press that woke the screens does nothing else.
            PointerEvent::Activate(_) | PointerEvent::LongPress(_) if self.idle.swallows_press(self.clock.mono()) => {}
            PointerEvent::Activate(id) => self.activate(&id, false, out),
            PointerEvent::LongPress(id) => self.activate(&id, true, out),
            PointerEvent::Down(id) => {
                out.mark(id.screen());
                if let Id::Key(pos) = id {
                    self.keyboard.down(pos);
                }
                let item = self.item_key(&id);
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
                out.mark(id.screen());
            }
        }
    }

    /// A tap or long press on `id`, resolved to what the press landed on. It gets the focus, and its
    /// subsystem acts on it.
    fn activate(&mut self, id: &Id, long: bool, out: &mut Out) {
        // The list item the press came down on is gone.
        let Some(id) = self.resolve(id) else {
            out.mark(Dirty::BOTTOM);
            return;
        };
        self.pointer.focus_on(id.clone());
        match id {
            // A hold on the selected entry with the pad, while its focus is showing, moves the pad to the
            // entry's actions and focuses the first. Up and down go through them, A runs one, and B goes
            // back to the list.
            Id::Shelf(ShelfId::Entry(game)) if long && self.pointer.show_focus() && self.shelf.selected() == Some(&game) => {
                if let Some(first) = self.shelf.actions().as_slice().first() {
                    self.pointer.focus_on(Id::Shelf(first.id()));
                }
                out.mark(Dirty::BOTH);
            }
            Id::Shelf(id) => {
                out.mark(Dirty::BOTH);
                self.shelf.on_target(id, &mut self.pointer, out);
            }
            Id::Dock(id) => {
                out.mark(Dirty::BOTTOM);
                self.dock.on(id, long, &self.session, &mut self.bottom, out);
            }
            Id::Settings(id) => {
                out.mark(if matches!(id, SettingsId::Tile(_) | SettingsId::Reset) { Dirty::BOTH } else { Dirty::BOTTOM });
                self.on_settings(id, long, out);
            }
            Id::Editor(id) => {
                let done = match id {
                    EditorId::Save => Done::Save,
                    EditorId::Cancel => Done::Cancel,
                };
                out.mark(Dirty::BOTTOM);
                self.editor_done(done, out);
            }
            Id::Wifi(WifiId::Power) => {
                out.mark(Dirty::BOTTOM);
                self.radios.power_wifi(out);
            }
            Id::Bluetooth(BtId::Power) => {
                out.mark(Dirty::BOTTOM);
                self.radios.power_bluetooth(out);
            }
            Id::Audio(id) => {
                out.mark(Dirty::BOTTOM);
                self.levels.on_audio(id, out);
            }
            Id::Row { list, i, part } => {
                out.mark(list.screens());
                self.on_row(list, usize::from(i), part, long, out);
            }
            Id::Keymap(id) => {
                out.mark(Dirty::BOTTOM);
                self.on_keymap(id, long, out);
            }
            Id::Tab(tab) => {
                self.choose_tab(tab);
                out.mark(Dirty::BOTH);
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
                out.mark(Dirty::BOTH);
            }
            Id::Control(_) | Id::Level(_) | Id::List(_) | Id::Blank => {}
            Id::Key(pos) => {
                let stroke = self.keyboard.press(pos, long);
                // The key goes to the editor while it is open, to finding an entry while shelf is in
                // front, and otherwise to the focused app.
                out.mark(Dirty::BOTTOM);
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
                        out.mark(Dirty::BOTH);
                        self.shelf.find(stroke, &mut self.pointer, out);
                    }
                    Some(stroke) => out.effects.push(Effect::Key(stroke)),
                    None => {}
                }
            }
            Id::BottomWindows => {
                if let Some(window) = self.bottom.hidden_down(&self.session) {
                    out.mark(Dirty::BOTH);
                    self.bottom.show_program_down(window, out);
                }
            }
            Id::Time(id) => self.sys.on_time(id, self.lists.get_mut(ListId::Time), out),
            Id::Device(id) => {
                out.mark(Dirty::BOTH);
                self.device.tap(id, self.clock.mono(), &mut self.bottom, &mut self.pointer, self.shelf.selected(), out);
            }
            Id::Battery(id) => {
                out.mark(Dirty::BOTTOM);
                self.sys.on_battery(id, &self.session, out);
            }
            Id::PadMouse => {
                self.pad.toggle_mouse();
                out.mark(Dirty::BOTTOM);
            }
            Id::Wake => {
                self.bottom.wake();
                out.mark(Dirty::BOTTOM);
            }
        }
    }
}
