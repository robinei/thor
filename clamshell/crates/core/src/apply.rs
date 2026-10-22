//! Applying an event: which feature's rules it goes to, and what follows from it everywhere after, in
//! the order each step needs the ones before settled.

use crate::bottom::keyboard::Key;
use crate::kit::replace;
use crate::levels::slider::Control;
use crate::resume::SAVE_AFTER;
use crate::settings::MouseAtLaunch;
use crate::{
    Dirty, Effect, Event, Hold, Id, Level, Library, PadRoute, PointerEvent, Reading, Save, SettingsId, ShelfId, State, Status, Tab, WindowCommand, device,
};

impl State {
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

    /// The program in force changed: the pad mouse on if it wants it (Setting::PadMouse), off again if it
    /// was the program's. Whether it changed.
    fn follow_pad_mouse(&mut self) -> bool {
        let wanted = self.session.program.is_some() && self.in_force().choice::<MouseAtLaunch>() == MouseAtLaunch::On;
        self.pad.follow_game(wanted)
    }

    /// Where the pad goes, as things are now; the new route, if it changed.
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
}
