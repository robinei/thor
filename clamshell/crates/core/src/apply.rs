//! Applying an event: which feature's rules handle it, and then everything that follows from it
//! elsewhere. The follow-up steps run in an order where each one finds the steps it depends on settled.

use crate::bottom::keyboard::Key;
use crate::kit::replace;
use crate::levels::slider::Control;
use crate::mutation::{EntryAct, Mutation};
use crate::resume::SAVE_AFTER;
use crate::settings::MouseAtLaunch;
use crate::{
    BtId, Dirty, Effect, Event, Hold, Id, Level, Library, ListId, PadRoute, PointerEvent, Reading, Save, SettingsId, ShelfId, State, Tab, WifiId,
    WindowCommand, device,
};

impl State {
    /// Applies one event and returns which screens changed and what must happen outside. After the event
    /// itself, everything that follows from it is applied: the pad mouse follows the game in front, the
    /// pad is routed to where it is wanted, and idle moves on or back.
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
        // This comes after the program in force is settled, since it reads that program's setting and
        // darkness. If it changed anything, the watch is followed again.
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
        effects.extend(self.session.show_reopened(self.clock.mono()));
        effects.extend(self.follow_quitting());
        effects.extend(self.follow_closed(was));
        // After handling the front one closing, so that a window just taken by a browsed file is shown
        // over it.
        effects.extend(by_windows);
        let docked = self.follow_dock();
        self.session.followed_windows = self.session.windows.iter().map(|w| w.id).collect();
        self.session.followed_front = self.session.windows.iter().find(|w| w.focused && !w.down).map(|w| w.workspace.clone());
        let after = self.session.running.iter().map(|r| &r.id);
        (docked || !after.eq(before.iter()), effects)
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
            // On a screen that listens for input, Home leaves it. Home is never taken as an input.
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
            // Never applied here: the UI handles pad navigation itself.
            Event::Pad(_) => (Dirty::NONE, vec![]),
            Event::TextInput(active) => self.text_input_changed(active),
            Event::Pointing(pointing) => self.pointing(pointing),
            // Sleep may have reset what the settings set, such as frequency caps and cores, so apply them
            // again.
            Event::Resumed => {
                self.idle.resumed(self.clock.mono());
                (Dirty::NONE, vec![Effect::Apply { entry: self.session.program.clone(), settings: self.in_force() }])
            }
            Event::Idle(event) => {
                self.idle.heard(event, self.clock.mono());
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
                // The tool turned the bottom screen off while it ran and then restored it. Set it to what
                // ClamShell wants.
                (Dirty::TOP, vec![Effect::Window(WindowCommand::BottomPower(!self.bottom_off()))])
            }
            Event::GyroCalibrated(ok) => {
                self.notify(crate::bottom::notice::NoticeKind::Gyro(Some(ok)));
                (Dirty::BOTTOM, vec![])
            }
            Event::AccountChanged(result) => {
                self.on_account_changed(result);
                (Dirty::BOTTOM, vec![])
            }
            Event::Quit => (Dirty::NONE, vec![Effect::Save(Save::Resume(self.resume())), Effect::Quit]),
        }
    }

    /// A reading from a system thread. After a status reading, the battery warning is checked.
    fn reading_came(&mut self, reading: Reading) -> (Dirty, Vec<Effect>) {
        let status = matches!(reading, Reading::Status(_));
        let dirty = self.on_reading(reading);
        let effects = if status { self.sys.battery_warning().into_iter().collect() } else { vec![] };
        (if self.follow_lists() { Dirty::BOTTOM } else { dirty }, effects)
    }

    /// A touch or a pad focus change. Focusing or pressing a target on the second screen counts as using
    /// that screen.
    fn pointer_event(&mut self, e: PointerEvent) -> (Dirty, Vec<Effect>) {
        let used = match &e {
            PointerEvent::Focus(id) | PointerEvent::Activate(id) | PointerEvent::LongPress(id) | PointerEvent::Down(id) => id.screen() == Dirty::BOTTOM,
            _ => false,
        };
        let lit = if used { self.bottom_used() } else { Dirty::NONE };
        let (dirty, effects) = self.on_pointer(e);
        (dirty.or(lit), effects)
    }

    /// The library was read again. New problems are reported once and shelf is reloaded. Then what
    /// depends on its entries is updated: runners' pauses and keymaps, landing rules, and pins of entries
    /// that are gone.
    fn library_read(&mut self, library: Library) -> Vec<Effect> {
        let problems = &library.problems;
        if !problems.is_empty() && *problems != self.shelf.library().problems {
            let first = problems.first().cloned().unwrap_or_default();
            self.notify(crate::bottom::notice::NoticeKind::Problems(first, problems.len()));
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

    /// Applies what an area's rule asked of the rest of the state, in order, and returns its effects in
    /// the order they were asked for.
    pub(crate) fn settle(&mut self, mutations: Vec<Mutation>) -> Vec<Effect> {
        let mut effects = Vec::new();
        for m in mutations {
            match m {
                Mutation::Effect(e) => effects.push(e),
                Mutation::Notify(what) => self.notify(what),
                Mutation::OpenEditor(editor) => self.bottom.open_editor(editor),
                Mutation::Act(id, EntryAct::Quit) => effects.extend(self.quit(&id)),
                Mutation::Act(id, EntryAct::ForceKill) => effects.extend(self.force_kill(&id)),
            }
        }
        effects
    }

    /// The UI is up. Sends what was waiting for it: the dock if pins were dropped at start, the
    /// brightness, the pad's route, the keymaps and the down rules. The open tab's watch starts from now
    /// (see [`State::follow_watch`]).
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
        effects.push(self.window_rules.effect());
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

    /// Contacts on the touchpad tab, passed on to the pointer, or none.
    fn pointing(&mut self, pointing: crate::bottom::touch::Pointing) -> (Dirty, Vec<Effect>) {
        let before = self.bottom.pointing;
        let effects = match pointing {
            crate::bottom::touch::Pointing::Contacts(contacts) => {
                self.bottom.pointing = true;
                vec![Effect::Touchpad(contacts)]
            }
            crate::bottom::touch::Pointing::Idle => {
                self.bottom.pointing = false;
                vec![]
            }
        };
        (if self.bottom.pointing == before { Dirty::NONE } else { Dirty::BOTTOM }, effects)
    }

    /// Progress or the end of a stick calibration. A finished calibration is saved.
    fn sticks_event(&mut self, event: device::SticksEvent) -> Vec<Effect> {
        let done = matches!(event, device::SticksEvent::Done(_, Some(_)));
        self.on_sticks(event);
        if done { vec![self.sticks_saved()] } else { vec![] }
    }

    /// The clock ticked. Once a minute, the bar's clock and both screens' burn-in orbit move on. Changes
    /// are saved once they have stopped for a moment.
    fn tick(&mut self, unix_secs: u64, mono_secs: u64) -> (Dirty, Vec<Effect>) {
        // The first tick. Anything idle heard before it was timed at 0, so count from now.
        if !self.clock.read() {
            self.idle.resumed(mono_secs);
        }
        let minute_changed = self.clock.tick(unix_secs, mono_secs);
        let mut effects = vec![];
        if self.saves.resume.due(mono_secs, SAVE_AFTER) {
            effects.push(Effect::Save(Save::Resume(self.resume())));
        }
        effects.extend(self.levels.save_due(mono_secs));
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

    /// A reading of the system. It is stored, and the second screen is redrawn if it shows what changed.
    fn on_reading(&mut self, reading: Reading) -> Dirty {
        let shown = self.bottom.tab.shows(&reading);
        let changed = match reading {
            // The volume, mute and brightness go to the levels, where a finger on a slider wins over the
            // reading.
            Reading::Status(status) => {
                let levels = self.levels.read(status.volume, status.muted, status.brightness);
                self.sys.read_status(status) | levels
            }
            Reading::Clipboard(text) => replace(&mut self.clipboard, text),
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
                let changed = replace(&mut self.sys.account, account);
                return if changed && self.device.open { Dirty::TOP } else { Dirty::NONE };
            }
        };
        if changed && shown { Dirty::BOTTOM } else { Dirty::NONE }
    }

    fn on_pointer(&mut self, event: PointerEvent) -> (Dirty, Vec<Effect>) {
        // Input that reached ClamShell itself, such as the pad driving the screens.
        self.idle.own_input(self.clock.mono());
        match event {
            PointerEvent::Focus(id) => {
                self.show_focused(&id);
                self.pointer.focus = Some(id);
                self.pointer.show_focus = true;
                (Dirty::BOTH, vec![])
            }
            PointerEvent::HideFocus => (if self.pointer.hide_focus() { Dirty::BOTH } else { Dirty::NONE }, vec![]),
            // A press that woke the screens does nothing else.
            PointerEvent::Activate(_) | PointerEvent::LongPress(_) if self.idle.swallows_press(self.clock.mono()) => (Dirty::NONE, vec![]),
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

    /// A tap or long press on `id`, resolved to what the press landed on. It gets the focus, and its
    /// subsystem acts on it.
    fn activate(&mut self, id: &Id, long: bool) -> (Dirty, Vec<Effect>) {
        // The list item the press came down on is gone.
        let Some(id) = self.resolve(id) else {
            return (Dirty::BOTTOM, vec![]);
        };
        self.pointer.focus = Some(id.clone());
        match id {
            // A hold on the selected entry with the pad, while its focus is showing, moves the pad to the
            // entry's actions and focuses the first. Up and down go through them, A runs one, and B goes
            // back to the list.
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
            Id::Wifi(WifiId::Power) => (Dirty::BOTTOM, vec![self.radios.power_wifi()]),
            Id::Bluetooth(BtId::Power) => (Dirty::BOTTOM, vec![self.radios.power_bluetooth()]),
            Id::Audio(id) => (Dirty::BOTTOM, self.levels.on_audio(id)),
            Id::Row { list, i, part } => (list.screens(), self.on_row(list, usize::from(i), part, long)),
            Id::Keymap(id) => (Dirty::BOTTOM, self.on_keymap(id, long)),
            Id::Tab(tab) => (Dirty::BOTH, self.choose_tab(tab)),
            // A tap opens the tab. Slides are handled by the UI and don't end in a tap.
            Id::Control(control) if !long => {
                let tab = match control {
                    Control::Volume => Tab::Volume,
                    Control::Brightness => Tab::Brightness,
                    // A keymap parameter is held or slid, not tapped.
                    Control::Param(_) => return (Dirty::NONE, vec![]),
                };
                (Dirty::BOTH, self.choose_tab(tab))
            }
            Id::Control(_) | Id::Level(_) | Id::List(_) | Id::Blank => (Dirty::NONE, vec![]),
            Id::Key(pos) => {
                let stroke = self.keyboard.press(pos, long);
                // The key goes to the editor while it is open, to finding an entry while shelf is in
                // front, and otherwise to the focused app.
                let effects = match stroke {
                    Some(stroke) if self.bottom.editor().is_some() => self.edit(stroke),
                    Some(stroke) if self.capturing() => self.captured(State::chord(stroke)),
                    Some(stroke) if self.session.front() == crate::session::running::Front::Shelf => return (Dirty::BOTH, self.find(stroke)),
                    Some(stroke) => vec![Effect::Key(stroke)],
                    None => vec![],
                };
                (Dirty::BOTTOM, effects)
            }
            Id::BottomWindows => match self.hidden_down() {
                Some(window) => (Dirty::BOTH, self.show_program_down(window)),
                None => (Dirty::NONE, vec![]),
            },
            Id::Time(id) => self.sys.on_time(id, self.lists.get_mut(ListId::Time)),
            Id::Device(id) => (Dirty::BOTH, self.on_device(id)),
            Id::Battery(id) => {
                let mutations = self.sys.on_battery(id, &self.session);
                (Dirty::BOTTOM, self.settle(mutations))
            }
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
}
