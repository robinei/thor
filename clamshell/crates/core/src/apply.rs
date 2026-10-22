//! Applying an event: which feature's rules handle it, and then everything that follows from it
//! elsewhere. The follow-up steps run in an order where each one finds the steps it depends on settled.

use crate::bottom::editor::Done;
use crate::kit::replace;
use crate::levels::slider::Control;
use crate::out::Out;
use crate::resume::SAVE_AFTER;
use crate::settings::MouseAtLaunch;
use crate::{
    BtId, Dirty, EditorId, Effect, EntryAct, Event, Id, Level, Library, ListId, PadRoute, PointerEvent, Reading, Save, SettingsId, ShelfId, State, Tab, WifiId,
    WindowCommand,
};

impl State {
    /// Applies one event and returns which screens changed and what must happen outside. After the event
    /// itself, everything that follows from it is applied: the pad mouse follows the game in front, the
    /// pad is routed to where it is wanted, and idle moves on or back.
    pub fn apply(&mut self, event: Event) -> (Dirty, Vec<Effect>) {
        let session_input = matches!(event, Event::Tick { .. } | Event::Windows(_) | Event::Units(_) | Event::Tray(_) | Event::Launch(_) | Event::Library(_));
        let mut out = Out::default();
        self.apply_event(event, &mut out);
        if session_input && self.reconcile(&mut out) {
            out.mark(Dirty::BOTTOM);
        }
        if self.bottom.follow_dim(&self.session, &self.clock) {
            out.mark(Dirty::BOTTOM);
        }
        out.effects.extend(self.follow_shown());
        out.effects.extend(self.follow_browse());
        out.effects.extend(self.follow_idle());
        out.effects.extend(self.follow_watch());
        let (changed, applied) = self.follow_program();
        out.effects.extend(applied);
        // This comes after the program in force is settled, since it reads that program's setting and
        // darkness. If it changed anything, the watch is followed again.
        let power = self.bottom.follow_power(self.power(), self.clock.mono());
        if !power.is_empty() {
            out.effects.extend(power);
            out.effects.extend(self.follow_watch());
        }
        if changed && self.follow_pad_mouse() {
            out.mark(Dirty::BOTH);
        }
        out.effects.extend(self.follow_keymap(false));
        if let Some(route) = self.reconcile_pad() {
            out.effects.push(Effect::PadRoute(route));
            // The focus ring shows where the pad is.
            if route != PadRoute::Ui && self.pointer.hide_focus() {
                out.mark(Dirty::BOTH);
            }
        }
        if self.bottom.take_unsaved() {
            self.saves.resume.touch(self.clock.mono());
        }
        (out.dirty, out.effects)
    }

    /// Runs after any change to what the session follows (the windows, systemd's scopes, the tray, the
    /// launcher's messages, the library), and every second. It applies everything that follows, in this
    /// order, since each step reads what the earlier ones settled:
    ///
    /// 1. systemd's scopes: entries found running, found ones promoted to launches, and ours ended
    ///    ([`State::follow_units`]). The first time, the entries to start with ClamShell are started.
    /// 2. The windows: entries started or ended by them, windows moved to their entries, and programs
    ///    found by them ([`State::follow_windows`]; `known` is the set of windows the last pass saw).
    ///    Then the rules that send windows down.
    /// 3. In the background: entries paused, thawed or ended by their setting. This reads 1 and 2.
    /// 4. What is shown: a launch's first window, a workspace whose window came back, and going back to
    ///    what was before when the one in front closed.
    /// 5. Quitting entries are ended once their windows are gone. This reads 1 to 3.
    /// 6. The dock: running entries are added to it and ended ones removed.
    ///
    /// A step does nothing when nothing changed for it. Returns whether the running entries changed.
    fn reconcile(&mut self, out: &mut Out) -> bool {
        let before: Vec<std::sync::Arc<str>> = self.session.running.iter().map(|r| r.id.clone()).collect();
        let known = std::mem::take(&mut self.session.followed_windows);
        let was = self.session.followed_front.take();
        out.effects.extend(self.follow_units());
        if self.session.units_known {
            out.effects.extend(self.start_autostart());
        }
        let by_windows = self.follow_windows(&known);
        out.effects.extend(self.bottom.follow_window_rules(&self.session));
        out.effects.extend(self.follow_background());
        out.effects.extend(self.session.show_pending());
        out.effects.extend(self.session.show_reopened(self.clock.mono()));
        out.effects.extend(self.follow_quitting());
        out.effects.extend(self.follow_closed(was));
        // After handling the front one closing, so that a window just taken by a browsed file is shown
        // over it.
        out.effects.extend(by_windows);
        let docked = self.follow_dock();
        self.session.followed_windows = self.session.windows.iter().map(|w| w.id).collect();
        self.session.followed_front = self.session.windows.iter().find(|w| w.focused && !w.down).map(|w| w.workspace.clone());
        let after = self.session.running.iter().map(|r| &r.id);
        docked || !after.eq(before.iter())
    }

    /// Called when the program in force changed. Turns the pad mouse on if the program wants it
    /// (Setting::PadMouse), and off again if the program had turned it on. Returns whether it changed.
    fn follow_pad_mouse(&mut self) -> bool {
        let wanted = self.session.program.is_some() && self.in_force().choice::<MouseAtLaunch>() == MouseAtLaunch::On;
        self.pad.follow_game(wanted)
    }

    /// Works out where the pad goes as things are now. Returns the new route if it changed.
    pub(crate) fn reconcile_pad(&mut self) -> Option<PadRoute> {
        let listening = self.listening();
        let moving = self.dock.moving.is_some() && self.bottom.tab() == Tab::Dock;
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
            Event::Slide(slide) => {
                let (dirty, effects) = self.on_slide(slide);
                out.mark(dirty);
                out.effects.extend(effects);
            }
            Event::Level(e) => {
                let (dirty, effects) = self.on_level(e);
                out.mark(dirty);
                out.effects.extend(effects);
            }
            Event::Shelf(e) => {
                let (dirty, effects) = self.on_shelf_event(e);
                out.mark(dirty);
                out.effects.extend(effects);
            }
            Event::Launch(e) => {
                let (dirty, effects) = self.on_launch_event(e);
                out.mark(dirty);
                out.effects.extend(effects);
            }
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
            }
            Event::Files(e) => {
                out.mark(Dirty::BOTH);
                out.effects.extend(self.on_files(e));
            }
            Event::Fetching(on) => {
                self.fetching = on;
                self.shown = None;
            }
            Event::Tray(items) => {
                if let Some(effects) = self.set_tray(items) {
                    out.mark(Dirty::BOTH);
                    out.effects.extend(effects);
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
            Event::Home => out.effects.extend(self.home()),
            Event::Listened(input) => {
                out.mark(Dirty::BOTTOM);
                out.effects.extend(self.listened(input));
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
            Event::Pointing(pointing) => {
                let (dirty, effects) = self.bottom.point(pointing);
                out.mark(dirty);
                out.effects.extend(effects);
            }
            // Sleep may have reset what the settings set, such as frequency caps and cores, so apply them
            // again.
            Event::Resumed => {
                self.idle.resumed(self.clock.mono());
                out.effects.push(Effect::Apply { entry: self.session.program.clone(), settings: self.in_force() });
            }
            Event::Idle(event) => self.idle.heard(event, self.clock.mono()),
            Event::Device(key) => {
                out.mark(Dirty::BOTTOM);
                out.effects.extend(self.on_device_key(key));
            }
            Event::BottomTouched => out.mark(self.bottom.used(&self.session, &self.clock)),
            Event::Frozen(keys) => {
                self.found_frozen(keys);
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
                out.effects.extend(self.device.sticks_event(event));
            }
            Event::RefreshCalibrated(result) => {
                self.device.refresh_done(result);
                // The tool turned the bottom screen off while it ran and then restored it. Set it to what
                // ClamShell wants.
                out.mark(Dirty::TOP);
                out.effects.push(Effect::Window(WindowCommand::BottomPower(!self.bottom.off())));
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
                    EntryAct::Quit => out.effects.extend(self.quit(&id)),
                    EntryAct::ForceKill => out.effects.extend(self.force_kill(&id)),
                }
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
            out.effects.extend(self.sys.battery_warning());
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
        let problems = &library.problems;
        if !problems.is_empty() && *problems != self.shelf.library().problems {
            let first = problems.first().cloned().unwrap_or_default();
            self.bottom.notify(crate::bottom::notice::NoticeKind::Problems(first, problems.len()), self.clock.mono());
        }
        if self.shelf.reload(library) {
            self.moved_collection();
        }
        self.follow_pausable_runners();
        out.effects.extend(self.follow_runner_keymaps());
        out.effects.push(self.land_rules());
        if self.drop_gone_pins() {
            out.effects.push(Effect::Save(Save::Dock(self.dock.pinned_ids())));
        }
        self.shown = None;
    }

    /// The UI is up. Sends what was waiting for it: the dock if pins were dropped at start, the
    /// brightness, the pad's route, the keymaps and the down rules. The open tab's watch starts from now
    /// (see [`State::follow_watch`]).
    fn ui_started(&mut self, out: &mut Out) {
        self.watches.ui_started();
        if std::mem::take(&mut self.pins_dropped) {
            out.effects.push(Effect::Save(Save::Dock(self.dock.pinned_ids())));
        }
        out.effects.extend(self.levels.brightness_effect());
        out.effects.push(Effect::PadRoute(self.pad.route()));
        out.effects.extend(self.follow_runner_keymaps());
        out.effects.extend(self.follow_keymap(true));
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
        if self.saves.resume.due(mono_secs, SAVE_AFTER) {
            out.effects.push(Effect::Save(Save::Resume(self.resume())));
        }
        out.effects.extend(self.levels.save_due(mono_secs));
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
            Reading::Icon { entry, path } => {
                let running = self.session.running.iter_mut().find(|r| r.id == entry);
                running.and_then(|r| r.entry.as_mut()).is_some_and(|g| replace(&mut g.icon, Some(path)))
            }
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
                if let Some(first) = self.actions().as_slice().first() {
                    self.pointer.focus_on(Id::Shelf(first.id()));
                }
                out.mark(Dirty::BOTH);
            }
            Id::Shelf(id) => {
                out.mark(Dirty::BOTH);
                out.effects.extend(self.on_shelf(id));
            }
            Id::Dock(id) => {
                out.mark(Dirty::BOTTOM);
                out.effects.extend(self.on_dock(id, long));
            }
            Id::Settings(id) => {
                out.mark(if matches!(id, SettingsId::Tile(_) | SettingsId::Reset) { Dirty::BOTH } else { Dirty::BOTTOM });
                out.effects.extend(self.on_settings(id, long));
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
                out.effects.extend(self.levels.on_audio(id));
            }
            Id::Row { list, i, part } => {
                out.mark(list.screens());
                self.on_row(list, usize::from(i), part, long, out);
            }
            Id::Keymap(id) => {
                out.mark(Dirty::BOTTOM);
                out.effects.extend(self.on_keymap(id, long));
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
                    Some(stroke) if self.capturing() => out.effects.extend(self.captured(State::chord(stroke))),
                    Some(stroke) if self.session.front() == crate::session::running::Front::Shelf => {
                        out.mark(Dirty::BOTH);
                        out.effects.extend(self.find(stroke));
                    }
                    Some(stroke) => out.effects.push(Effect::Key(stroke)),
                    None => {}
                }
            }
            Id::BottomWindows => {
                if let Some(window) = self.bottom.hidden_down(&self.session) {
                    out.mark(Dirty::BOTH);
                    out.effects.extend(self.bottom.show_program_down(window));
                }
            }
            Id::Time(id) => {
                let (dirty, effects) = self.sys.on_time(id, self.lists.get_mut(ListId::Time));
                out.mark(dirty);
                out.effects.extend(effects);
            }
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
