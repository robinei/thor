//! The program's state and its rules. No I/O: every change comes in as an [`Event`], and what the outside
//! world must do comes back as [`Effect`]s. The UI and the system threads only read the state and send
//! events, so all of this is unit-testable.
//!
//! The state is grouped by what it is about: shelf ([`Shelf`]), the second screen ([`Bottom`]), what runs
//! and is in front ([`Session`]), the dock's apps ([`Apps`]), volume and brightness ([`Levels`]), the
//! radios ([`Radios`]), the controller ([`Pad`]), focus and presses ([`Pointer`]), and the system's
//! readings ([`Sys`]). Each owns its own rules; what involves two of them is [`State`]'s.

pub mod apps;
pub mod battery;
pub mod bluetooth;
pub mod editor;
mod effect;
mod event;
mod game;
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
mod radios;
mod resume;
pub mod scroll;
mod session;
pub mod settings;
mod shelf;
pub mod slider;
mod sys;
mod tab;
mod tasks;
pub mod touch;
pub mod wifi;
pub mod windows;

use std::sync::Arc;

use keyboard::{Key, Keyboard};
use resume::{SAVE_AFTER, Saves};
use settings::{Setting, Settings};
use slider::Control;

pub use apps::Apps;
pub use editor::{Editor, count_env, parse_env};
pub use effect::{Dirty, Effect, Save, Watch, WindowCommand};
pub use event::{Event, GameEvent, LevelEvent, PointerEvent, Reading, ShelfEvent, Slide};
pub use game::{Front, Launching, Origin, Queued, RunningGame};
pub use id::{AudioId, BtId, DockId, EditorId, Hold, Id, KeymapId, SettingsId, ShelfId, Target, WifiId};
pub use instance::Instance;
pub use levels::{Brightness, Level, Levels, Sound};
pub use pad::{Claimant, Pad, PadInput, PadMouse, PadRoute};
pub use panel::{Scope, TileOrigin};
pub use pointer::Pointer;
pub use radios::{RadioStatus, Radios};
pub use scroll::{ListEvent, ListId, Lists, RowPart, Scroll};
pub use session::{Pending, Session};
pub use shelf::{Cursor, Shelf};
pub use sys::{Status, Sys, TouchpadProblem};
pub use tab::{Bottom, Tab};

pub use clamshell_library::{Collection, Game, Launch, Library};

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
    pub apps: Apps,
    /// Where shelf and the second screen were (see [`State::resume_text`]).
    pub resume: Option<String>,
    /// The tab to start in instead (a development option).
    pub start_tab: Option<Tab>,
    /// Start with the pad mouse on.
    pub pad_mouse: bool,
    /// ClamShell is the device's own session: it may put the device to sleep when idle.
    pub device_session: bool,
}

#[derive(Debug, Default)]
pub struct State {
    shelf: Shelf,
    bottom: Bottom,
    session: Session,
    apps: Apps,
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
            levels: Levels::new(init.brightness),
            apps: init.apps,
            device_session: init.device_session,
            ..State::default()
        };
        if let Some(text) = &init.resume {
            s.restore(text);
        }
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

    pub fn apps(&self) -> &Apps {
        &self.apps
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

    /// A game of the library, by its id.
    pub fn game(&self, id: &str) -> Option<&Game> {
        self.shelf.game(id)
    }

    /// The game: of the games running, the one most recently in front.
    pub fn current_game(&self) -> Option<&Arc<str>> {
        self.session.current_game()
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
        let game = self.session.current.clone();
        let (mut dirty, mut effects) = self.apply_event(event);
        effects.extend(self.follow_idle());
        if self.session.current != game && self.follow_pad_mouse() {
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

    /// The game in front changed: the pad mouse on if it wants it (Setting::PadMouse), off again if it
    /// was the game's. Whether it changed.
    fn follow_pad_mouse(&mut self) -> bool {
        let wanted = self.session.current.is_some() && self.in_force().get(Setting::PadMouse) == 1;
        self.pad.follow_game(wanted)
    }

    /// Where the pad goes, as things are now; the new route, if it changed.
    fn reconcile_pad(&mut self) -> Option<PadRoute> {
        let listening = self.listening();
        self.pad.reconcile(self.session.front(), self.bottom.tab, self.pointer.show_focus, listening)
    }

    fn apply_event(&mut self, event: Event) -> (Dirty, Vec<Effect>) {
        match event {
            Event::Tick { unix_secs } => self.tick(unix_secs),
            Event::Reading(reading) => {
                let dirty = self.on_reading(reading);
                (if self.follow_lists() { Dirty::BOTTOM } else { dirty }, vec![])
            }
            Event::List(e) => (if self.on_list(e) { e.list().screens() } else { Dirty::NONE }, vec![]),
            Event::Pointer(e) => self.on_pointer(e),
            Event::Slide(slide) => self.on_slide(slide),
            Event::Level(e) => self.on_level(e),
            Event::Shelf(e) => self.on_shelf_event(e),
            Event::Game(e) => self.on_game(e),
            Event::Windows(windows) => {
                self.session.set_windows(windows);
                let mut effects = self.follow_games();
                effects.extend(self.session.show_pending());
                (Dirty::BOTH, effects)
            }
            Event::Units(units) => {
                self.session.set_units(units);
                (Dirty::BOTTOM, vec![])
            }
            Event::Start => {
                let mut effects: Vec<Effect> = self.bottom.tab.watch().map(|w| Effect::Watch(w, true)).into_iter().collect();
                effects.extend(self.levels.brightness_effect());
                effects.push(Effect::PadRoute(self.pad.route()));
                effects.extend(self.follow_keymap(true));
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
                let before = self.bottom.tab;
                let effects = self.text_input(active);
                (if self.bottom.tab == before { Dirty::NONE } else { Dirty::BOTH }, effects)
            }
            // Sleep may have reset what the settings set (frequency caps, cores): again.
            // Sleep may have reset what the settings set (frequency caps, cores): again.
            Event::Resumed => {
                self.idle.resumed(self.unix_secs);
                (Dirty::NONE, vec![Effect::Apply { game: self.session.current.clone(), settings: self.in_force() }])
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
            Event::Quit => (Dirty::NONE, vec![Effect::Save(Save::Resume(self.resume_text())), Effect::Quit]),
        }
    }

    /// The clock: once a minute the bar's clock and both screens' burn-in orbit step; starts systemd
    /// never listed are forgotten; what changed is saved once changes have stopped for a moment.
    fn tick(&mut self, unix_secs: u64) -> (Dirty, Vec<Effect>) {
        let minute_changed = unix_secs / 60 != self.unix_secs / 60;
        // The clock's first word: what idle heard before it was dated 0, as if decades ago. Count from now.
        if self.unix_secs == 0 {
            self.idle.resumed(unix_secs);
        }
        self.session.forget_failed_starts(unix_secs);
        self.unix_secs = unix_secs;
        let mut effects = vec![];
        if self.saves.resume.due(unix_secs, SAVE_AFTER) {
            effects.push(Effect::Save(Save::Resume(self.resume_text())));
        }
        if self.saves.brightness.due(unix_secs, SAVE_AFTER) {
            effects.push(Effect::Save(Save::Brightness(self.levels.brightness().text())));
        }
        let notice_gone = self.expire_notice(unix_secs);
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
            Reading::TouchpadProblem(problem) => replace(&mut self.sys.touchpad_problem, problem),
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
        // Input that reached ClamShell itself (the compositor may not see it: touchpad mode's bar).
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
            Id::Shelf(id) => (Dirty::BOTH, self.on_shelf(id)),
            Id::Dock(DockId::StartGame) => (Dirty::NONE, self.on_dock(DockId::StartGame, long)),
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
            Id::Control(_) | Id::Level(_) | Id::List(_) => (Dirty::NONE, vec![]),
            Id::Key(pos) => {
                let stroke = self.keyboard.press(pos, long);
                // Into the editor while it is open, else to the focused app.
                let effects = match stroke {
                    Some(stroke) if self.bottom.editor().is_some() => self.edit(stroke),
                    Some(stroke) if self.capturing() => self.captured(State::chord(stroke)),
                    Some(stroke) => vec![Effect::Key(stroke)],
                    None => vec![],
                };
                (Dirty::BOTTOM, effects)
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

    /// Shelf's targets.
    fn on_shelf(&mut self, id: ShelfId) -> Vec<Effect> {
        match id {
            ShelfId::Game(game) => {
                // Activating the selected game again plays it (starts, resumes, or switches to it).
                if self.shelf.selected() == Some(&game) {
                    return self.play(game);
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
            ShelfId::Launch => match self.shelf.selected().cloned() {
                Some(game) => self.play(game),
                None => vec![],
            },
            ShelfId::StartKeep => self.start_beside(false),
            ShelfId::StartClose => self.start_beside(true),
        }
    }

    /// Shelf went to another collection: the place is saved, and a focus in shelf's list follows.
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
            ShelfEvent::ToggleFavourite => match self.shelf.toggle_favourite() {
                Some(moved) => {
                    if moved {
                        self.saves.resume.touch(self.unix_secs);
                    }
                    let ids = self.shelf.library().favourite_ids().map(str::to_owned).collect();
                    (Dirty::BOTH, vec![Effect::Save(Save::Favourites(ids))])
                }
                None => (Dirty::NONE, vec![]),
            },
        }
    }

    fn on_game(&mut self, event: GameEvent) -> (Dirty, Vec<Effect>) {
        match event {
            GameEvent::Started(game) => {
                self.game_started(game);
                (Dirty::BOTH, vec![])
            }
            GameEvent::Played { game, start, duration } => (Dirty::BOTH, self.game_played(&game, start, duration)),
            GameEvent::LaunchFailed(game) => {
                self.session.launching.retain(|l| l.game != game);
                if self.session.running_game(&game).is_some() {
                    return (Dirty::BOTH, self.game_gone(&game));
                }
                (Dirty::BOTH, vec![])
            }
        }
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

    /// Changes the second screen's tab. The touchpad and the watches (Wi-Fi, Bluetooth, battery,
    /// audio, backlights) start and stop with their tabs; a level being set ends. What the tab left had
    /// open (an editor, the keymaps' screens) stays for coming back to.
    fn set_tab(&mut self, tab: Tab) -> Vec<Effect> {
        let before = self.bottom.tab;
        if tab == before {
            return vec![];
        }
        self.saves.resume.touch(self.unix_secs);
        self.bottom.tab = tab;
        self.levels.leave();
        let watch = |tab: Tab, on| tab.watch().map(|w| Effect::Watch(w, on));
        watch(before, false).into_iter().chain(watch(tab, true)).collect()
    }

    /// An app's text field wants typing (or is done): the keyboard comes up, and goes back to the tab it
    /// came up from when the field is done, unless the user chose a tab meanwhile. Not for a game (SDL
    /// asks for text input whether or not it needs it); the keyboard is a tap away there.
    fn text_input(&mut self, active: bool) -> Vec<Effect> {
        if active {
            // (Not over an open editor: what is typed there would be lost.)
            if self.bottom.tab == Tab::Keyboard || self.session.front() == Front::Game || self.bottom.editor().is_some() {
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

    /// The home button: from anything to shelf; from shelf back to what was in front before it (the
    /// game, an app's instance), or without that history (no compositor's word) to the running game.
    fn home(&mut self) -> Vec<Effect> {
        let session = &self.session;
        let front = session.windows.iter().find(|w| w.focused);
        let shelf_in_front = front.is_none_or(|w| w.app_id == apps::SHELF_APP_ID);
        if !shelf_in_front {
            return vec![Effect::Window(WindowCommand::ShowShelf)];
        }
        let before = session.recent.iter().find(|ws| !Instance::Shelf.is(ws)).cloned();
        let game = session.current.as_deref().and_then(|id| session.running_game(id)).map(|g| g.instance.name());
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

    pub fn state() -> State {
        State::new(init())
    }

    pub fn win(id: i64, app_id: &str, workspace: &str, focused: bool) -> Window {
        Window { id, app_id: app_id.into(), title: String::new(), focused, unit: None, workspace: workspace.into() }
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

    /// `game` ended (a short play).
    pub fn played(s: &mut State, game: &str) -> Vec<Effect> {
        s.apply(GameEvent::Played { game: game.into(), start: 0, duration: 1 }.into()).1
    }

    /// Showing `instance`'s workspace.
    pub fn focus(instance: &Instance) -> Effect {
        Effect::Window(WindowCommand::Focus(instance.name()))
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
    fn home_goes_to_shelf_and_back_to_what_was_in_front() {
        let mut s = state();
        let shelf = |focused| win(1, apps::SHELF_APP_ID, &Instance::Shelf.name(), focused);
        let landing = Instance::Landing.name();
        let home = |s: &mut State| s.apply(Event::Home).1;
        assert_eq!(home(&mut s), vec![], "nothing before shelf: it stays");
        s.apply(GameEvent::Started("zelda-lttp".into()).into());
        assert_eq!(home(&mut s), vec![focus(&Instance::Landing)], "no history: the game");
        // A terminal in front, then shelf: home goes back to the terminal.
        let terminal = "clamshell-app-terminal-1-0";
        s.apply(Event::Windows(vec![shelf(false), win(2, "foot", terminal, true)]));
        assert_eq!(home(&mut s), vec![Effect::Window(WindowCommand::ShowShelf)]);
        s.apply(Event::Windows(vec![shelf(true), win(2, "foot", terminal, false)]));
        assert_eq!(home(&mut s), vec![Effect::Window(WindowCommand::Focus(terminal.into()))]);
        // The terminal closed: back to the game, which was in front before.
        s.apply(Event::Windows(vec![shelf(false), win(3, "retroarch", &landing, true)]));
        s.apply(Event::Windows(vec![shelf(true), win(3, "retroarch", &landing, false)]));
        assert_eq!(home(&mut s), vec![focus(&Instance::Landing)]);
    }

    #[test]
    fn home_goes_back_to_a_workspace_that_isnt_ours() {
        let mut s = state();
        let shelf = |focused| win(1, apps::SHELF_APP_ID, &Instance::Shelf.name(), focused);
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
