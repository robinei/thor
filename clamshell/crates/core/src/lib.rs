//! The program's state and its rules. No I/O: every change comes in as an [`Event`], and what the outside
//! world must do comes back as [`Effect`]s. The UI and the policy threads only read the state and send
//! events, so all of this is unit-testable.

pub mod keyboard;
pub mod apps;
pub mod controls;
pub mod editor;
pub mod metrics;
mod panel;
pub mod power;
mod resume;
pub mod settings;
mod shelf;
mod tasks;
pub mod windows;
pub mod touch;
pub mod wifi;

use std::sync::Arc;

use controls::{Control, Slider};
use keyboard::{Key, KeyPos, Keyboard, Stroke};
use settings::{Setting, Settings, Values};

pub use editor::{Editor, count_env, parse_env};

pub use clamshell_library::{Collection, Game, Launch, Library};

/// Second-screen modes (docs/plans.md, "Second screen"), chosen from the bar; the last one chosen stays.
/// Wi-Fi and Battery open from their status icons in the bar, and a second tap there goes back to the
/// mode before.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum Mode {
    Settings,
    Launcher,
    Keyboard,
    Touchpad,
    #[default]
    Sysinfo,
    Wifi,
    Battery,
}

impl Mode {
    /// The modes with their own icon on the left of the bar, in order.
    pub const BAR: [Mode; 5] = [Mode::Settings, Mode::Launcher, Mode::Keyboard, Mode::Touchpad, Mode::Sysinfo];

    pub fn name(self) -> &'static str {
        match self {
            Mode::Settings => "settings",
            Mode::Launcher => "launcher",
            Mode::Keyboard => "keyboard",
            Mode::Touchpad => "touchpad",
            Mode::Sysinfo => "sysinfo",
            Mode::Wifi => "wifi",
            Mode::Battery => "battery",
        }
    }

    pub fn from_name(name: &str) -> Option<Mode> {
        Mode::BAR.into_iter().chain([Mode::Wifi, Mode::Battery]).find(|m| m.name() == name)
    }
}

/// A game's power profile: how fast CPU and GPU may run (frequency caps; the Thor has no TDP setting).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum Power {
    Battery,
    #[default]
    Balanced,
    Performance,
}

impl Power {
    pub const ALL: [Power; 3] = [Power::Battery, Power::Balanced, Power::Performance];

    pub fn label(self) -> &'static str {
        match self {
            Power::Battery => "Battery",
            Power::Balanced => "Balanced",
            Power::Performance => "Performance",
        }
    }

    /// The power profile a [`Setting::Power`] value stands for.
    pub fn from_value(v: u16) -> Power {
        match v {
            0 => Power::Battery,
            1 => Power::Balanced,
            _ => Power::Performance,
        }
    }
}

/// What the settings panel edits: the global settings, or the game in context's overrides.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum Scope {
    #[default]
    Game,
    Global,
}

/// Stable ids of everything that can be focused or tapped, on either screen. Focus is kept by id, so it
/// survives redraws and layout changes.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum Id {
    Game(Arc<str>),
    /// A collection's tab in shelf, by position.
    Tab(u16),
    Mode(Mode),
    Launch,
    /// A settings tile: a tap steps to the next value, a long press goes back to the default.
    Tile(Setting),
    /// The environment variables tile, and the editor's buttons.
    Env,
    EditorSave,
    EditorCancel,
    /// The dark second screen (while a game runs): a tap anywhere lights it.
    Wake,
    /// A launcher app, by position.
    App(u16),
    /// The launcher's first tile: the running game (or a placeholder when none runs).
    GameTile,
    /// The "+" on a running app that allows several instances: another one.
    NewInstance(u16),
    /// The settings panel's scope tabs.
    Scope(Scope),
    /// Drops the game in context's overrides.
    Reset,
    /// An on-screen keyboard key, by position.
    Key(KeyPos),
    /// The pad mouse toggle.
    PadMouse,
    /// Volume or brightness in the bar: a touch brings up its slider.
    Control(Control),
    /// The Wi-Fi mode's radio switch, and a network by position.
    WifiPower,
    Network(u16),
}

/// The system's status as the bar shows it (policy threads read it from the system).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Status {
    /// Battery charge 0-100, if there is a battery.
    pub battery: Option<u8>,
    pub charging: bool,
    /// Wi-Fi link quality 0-100 while connected; `None` when not.
    pub wifi: Option<u8>,
    /// Output volume 0-100, if known, and whether it is muted.
    pub volume: Option<u8>,
    pub muted: bool,
    /// The screens' brightness 0-100, if there is a backlight.
    pub brightness: Option<u8>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Event {
    /// Wall clock, once a second (policy thread).
    Tick { unix_secs: u64 },
    /// Battery, Wi-Fi, volume (policy thread, when they change).
    Status(Status),
    /// CPU, GPU, memory, temperatures, power (policy thread, once a second).
    Metrics(metrics::Metrics),
    /// A running game's frame rate and frame time in ms (MangoHud), or none.
    Frames(Option<(f32, f32)>),
    /// Tap, click, or the pad's A on a focused item.
    Activate(Id),
    /// A press held longer than a tap (locks a keyboard modifier; otherwise as `Activate`).
    LongPress(Id),
    /// A finger, button or the pad's A went down on / up from a target: shown as pressed meanwhile.
    Down(Id),
    Up(Id),
    /// The pad moved focus (the UI finds the target, since only it knows the layout). Shows the focus.
    Focus(Id),
    /// Touch or mouse input began: the focus ring is hidden until the pad moves focus again.
    HideFocus,
    /// The pad moved through shelf's list: by this many games (a page is the rows on screen).
    ListStep(i32),
    /// The pad switched shelf's tab: by this many collections.
    TabStep(i32),
    /// How many list rows shelf shows (from the layout), so the selection stays on screen.
    Rows(usize),
    /// The pad's Y (desktop: F): the selected game in or out of the favourites.
    ToggleFavourite,
    /// The launcher started a game, or it ended (start in epoch seconds, duration in seconds), or it
    /// couldn't be started.
    Started(Arc<str>),
    Played { game: Arc<str>, start: u64, duration: u64 },
    LaunchFailed(Arc<str>),
    /// The compositor's windows, after any change.
    Windows(Vec<windows::Window>),
    /// Our running scopes (instances and games), by name without `.scope`, after any change.
    Units(Vec<String>),
    /// A finger came down on a control (`x` in the second screen's layout units, which is `width`
    /// wide), moved, and (with `Up`) lifted.
    SlideStart { control: Control, x: f32, width: f32 },
    SlideTo { x: f32 },
    /// Wi-Fi as NetworkManager has it, after any change (while the Wi-Fi mode is open).
    Wifi(wifi::Wifi),
    /// The charge history and what is using the machine (every few seconds while the battery mode is open).
    Power(power::Power),
    /// The pad's home button.
    Home,
    /// The system woke from sleep.
    Resumed,
    Quit,
}

/// What the state asks the outside world to do.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Effect {
    /// Start this game (its launch command or Steam id, and its file) with these settings and extra
    /// environment.
    Launch {
        game: Arc<str>,
        settings: Values,
        env: Vec<(String, String)>,
        launch: Launch,
        file: Option<std::path::PathBuf>,
    },
    /// The settings in force changed: the running game's (`game`), or the global ones while none runs
    /// (also when a game ends). Apply them now.
    Apply { game: Option<Arc<str>>, settings: Values },
    /// The games' settings changed: write them (see [`Settings::text`]).
    SaveSettings(String),
    /// The favourites changed: write them (favorites.txt).
    SaveFavourites(Vec<String>),
    /// Where shelf is (see [`State::resume_text`]): write it, to start there next time.
    SaveResume(String),
    /// A key press for the focused app (virtual keyboard).
    Key(Stroke),
    /// Touchpad mode started (true) or ended: the input side grabs or releases the touchscreen.
    Touchpad(bool),
    /// The pad mouse turned on or off: the input side grabs the controller and drives a virtual mouse
    /// with it, or releases it to games.
    PadMouse(bool),
    /// Start a launcher app's command as a new instance: a scope and a workspace of this name
    /// (`clamshell-app-<key>-<secs>-<n>`).
    RunApp { name: String, command: Vec<String> },
    /// Show this workspace (a launcher instance's, or the game's).
    FocusWorkspace(String),
    /// Ask these windows to close (as their own close buttons would): an instance's.
    CloseWindows(Vec<i64>),
    /// Quit the game: close its windows, then end its scopes (ours only) if it doesn't go.
    QuitGame { windows: Vec<i64>, units: Vec<String> },
    /// Bring shelf's window to the front.
    ShowShelf,
    /// Output volume (0-100), mute, the screens' brightness (0-100).
    SetVolume(u8),
    SetMute(bool),
    SetBrightness(u8),
    Wifi(wifi::Command),
    /// The battery mode opened (read the charge history and the spenders) or closed.
    WatchPower(bool),
    Quit,
}

/// Which screens need a new frame after an event.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Dirty {
    pub top: bool,
    pub bottom: bool,
}

impl Dirty {
    pub const BOTH: Dirty = Dirty { top: true, bottom: true };
    pub const BOTTOM: Dirty = Dirty { top: false, bottom: true };
    pub const NONE: Dirty = Dirty { top: false, bottom: false };
}

/// A tab's place in its list: the selected game's position, and the first row on screen.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Cursor {
    pub index: usize,
    pub top: usize,
}

#[derive(Debug, Default)]
pub struct State {
    pub library: Library,
    /// The selected collection (shelf's tab), and each collection's cursor.
    pub tab: usize,
    pub cursors: Vec<Cursor>,
    /// List rows on screen (reported by the UI).
    pub rows: usize,
    /// Id of the selected game (shelf's list on top, its settings below): the current tab's cursor.
    pub selected: Option<Arc<str>>,
    /// Every game's settings and environment.
    pub settings: Settings,
    /// The environment being edited (for the game, or all games), if the editor is open.
    pub editor: Option<Editor>,
    /// The dark second screen was tapped awake (until the next game).
    pub woken: bool,
    /// The launcher's apps, what is wrong with their file (the defaults are used then), and how many
    /// of each run.
    pub apps: Vec<apps::App>,
    pub apps_error: Option<String>,
    /// Instances started and not yet listed by systemd (name, epoch seconds), so a second tap doesn't
    /// start another; dropped once listed, or after a while if the start failed.
    starting: Vec<(String, u64)>,
    /// Numbers new instances' names (with the time, unique across restarts).
    started: u64,
    /// The workspace to show when its first window appears: the instance or game just started.
    pending_show: Option<String>,
    /// The compositor's windows, and launcher instances (workspace names) by when they were last in
    /// front, most recent first.
    pub windows: Vec<windows::Window>,
    /// Our running scopes (systemd's list).
    pub units: Vec<String>,
    recent: Vec<String>,
    /// The pad mouse was turned on by a launch (Setting::PadMouse), to turn off when the game ends.
    pad_mouse_by_launch: bool,
    /// What the settings panel edits (global whenever there is no game in context).
    pub scope: Scope,
    pub mode: Mode,
    /// The last mode chosen from the bar's left (where a second tap on Wi-Fi goes back to).
    bar_mode: Mode,
    /// Volume or brightness being slid, if any.
    pub slider: Option<Slider>,
    pub wifi: wifi::Wifi,
    pub power: power::Power,
    pub focus: Option<Id>,
    /// The focus ring is shown only while navigating with the pad.
    pub show_focus: bool,
    pub unix_secs: u64,
    pub keyboard: Keyboard,
    /// The target being pressed (drawn lit), if any.
    pub pressed: Option<Id>,
    pub status: Status,
    /// The controller drives the pointer (independent of the second screen's mode).
    pub pad_mouse: bool,
    pub metrics: metrics::Metrics,
    pub history: metrics::History,
    /// The game being played, if any (one at a time).
    pub running: Option<Arc<str>>,
    /// When the tab or a selection last changed and isn't saved yet (epoch seconds).
    resume_dirty: Option<u64>,
}

/// Seconds an instance may take to show up in systemd's list before its start counts as failed.
const STARTING_FOR: u64 = 15;

/// What holding a target does once the press lasts beyond a tap.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Hold {
    /// Activate now and again at the repeat rate until released (keyboard keys).
    Repeat,
    /// One long press, then nothing on release (locks a modifier).
    Long,
}

/// Which screen a target is drawn on.
fn screen_of(id: &Id) -> Dirty {
    match id {
        Id::Game(_) | Id::Tab(_) => Dirty::BOTH,
        Id::Mode(_)
        | Id::Tile(_)
        | Id::Env
        | Id::EditorSave
        | Id::EditorCancel
        | Id::Wake
        | Id::App(_)
        | Id::GameTile
        | Id::NewInstance(_)
        | Id::Scope(_)
        | Id::Reset
        | Id::Launch
        | Id::Key(_)
        | Id::PadMouse
        | Id::Control(_)
        | Id::WifiPower
        | Id::Network(_) => Dirty::BOTTOM,
    }
}

impl State {
    pub fn new(library: Library) -> Self {
        let cursors = vec![Cursor::default(); library.collections.len()];
        let mut s = State { library, cursors, rows: 1, ..State::default() };
        s.set_apps(apps::defaults(), None);
        s.sync();
        s.focus = s.selected.clone().map(Id::Game);
        s
    }

    /// The launcher's apps (from their file), and what is wrong with it if anything.
    pub fn set_apps(&mut self, apps: Vec<apps::App>, error: Option<String>) {
        self.apps = apps;
        self.apps_error = error;
    }

    /// Applies one event; returns which screens changed and what must happen outside.
    pub fn apply(&mut self, event: Event) -> (Dirty, Vec<Effect>) {
        match event {
            Event::Tick { unix_secs } => {
                // Once a minute: the bar's clock, and both screens' burn-in orbit step.
                let minute_changed = unix_secs / 60 != self.unix_secs / 60;
                // Starts systemd never listed failed: forget them.
                self.starting.retain(|(_, at)| unix_secs < at + STARTING_FOR);
                self.unix_secs = unix_secs;
                // The place, once moving has stopped for a moment.
                let effects = match self.resume_dirty {
                    Some(since) if unix_secs >= since + resume::RESUME_AFTER => {
                        self.resume_dirty = None;
                        vec![Effect::SaveResume(self.resume_text())]
                    }
                    _ => vec![],
                };
                (if minute_changed { Dirty::BOTH } else { Dirty::NONE }, effects)
            }
            Event::Status(status) => {
                let changed = status != self.status;
                self.status = status;
                (if changed { Dirty::BOTTOM } else { Dirty::NONE }, vec![])
            }
            Event::Metrics(mut m) => {
                // The frame rate comes separately (Frames); keep it.
                m.fps = self.metrics.fps;
                m.frame_ms = self.metrics.frame_ms;
                self.metrics = m;
                self.history.push(&m);
                // Only the sysinfo and battery modes show them.
                (if matches!(self.mode, Mode::Sysinfo | Mode::Battery) { Dirty::BOTTOM } else { Dirty::NONE }, vec![])
            }
            Event::Frames(frames) => {
                self.metrics.fps = frames.map(|f| f.0);
                self.metrics.frame_ms = frames.map(|f| f.1);
                (if self.mode == Mode::Sysinfo { Dirty::BOTTOM } else { Dirty::NONE }, vec![])
            }
            Event::Focus(id) => {
                self.focus = Some(id);
                self.show_focus = true;
                (Dirty::BOTH, vec![])
            }
            Event::HideFocus => {
                let was = std::mem::replace(&mut self.show_focus, false);
                (if was { Dirty::BOTH } else { Dirty::NONE }, vec![])
            }
            Event::Activate(id) => {
                self.focus = Some(id.clone());
                self.activate(id, false)
            }
            Event::LongPress(id) => {
                self.focus = Some(id.clone());
                self.activate(id, true)
            }
            Event::ListStep(step) => {
                self.step_list(step);
                self.show_focus = true;
                (Dirty::BOTH, vec![])
            }
            Event::TabStep(step) => {
                let n = self.library.collections.len() as i64;
                if n > 0 {
                    self.set_tab((self.tab as i64 + i64::from(step)).rem_euclid(n) as usize);
                }
                self.show_focus = true;
                (Dirty::BOTH, vec![])
            }
            Event::Rows(rows) => {
                if rows == self.rows {
                    return (Dirty::NONE, vec![]);
                }
                let before = self.cursor();
                self.rows = rows.max(1);
                self.sync();
                (if self.cursor() != before { Dirty::BOTH } else { Dirty::NONE }, vec![])
            }
            Event::ToggleFavourite => {
                let Some(id) = self.selected.clone() else {
                    return (Dirty::NONE, vec![]);
                };
                self.relayout(|lib| {
                    if let Some(g) = lib.games.iter_mut().find(|g| g.id == id) {
                        g.favourite = !g.favourite;
                    }
                    lib.rebuild_derived();
                });
                let ids = self.library.favourite_ids().map(str::to_owned).collect();
                (Dirty::BOTH, vec![Effect::SaveFavourites(ids)])
            }
            Event::Started(game) => {
                self.running = Some(game);
                self.woken = false;
                // The pad mouse, if the game wants it at launch.
                if self.current().get(Setting::PadMouse) == 1 && !self.pad_mouse {
                    self.pad_mouse = true;
                    self.pad_mouse_by_launch = true;
                    return (Dirty::BOTH, vec![Effect::PadMouse(true)]);
                }
                (Dirty::BOTH, vec![])
            }
            Event::Played { game, start, duration } => {
                self.running = None;
                self.relayout(|lib| {
                    if let Some(g) = lib.games.iter_mut().find(|g| g.id == game) {
                        g.plays.add(start, duration);
                    }
                    lib.rebuild_derived();
                });
                (Dirty::BOTH, self.game_ended())
            }
            Event::Units(units) => {
                self.units = units;
                let units = &self.units;
                self.starting.retain(|(name, _)| !units.contains(name));
                (Dirty::BOTTOM, vec![])
            }
            Event::Windows(windows) => {
                self.set_windows(windows);
                (Dirty::BOTTOM, self.show_pending())
            }
            Event::LaunchFailed(game) => {
                if self.running.as_ref() == Some(&game) {
                    self.running = None;
                }
                (Dirty::BOTH, self.game_ended())
            }
            Event::Down(id) => {
                let dirty = screen_of(&id);
                self.pressed = Some(id);
                (dirty, vec![])
            }
            Event::Up(id) => {
                if self.pressed.as_ref() == Some(&id) {
                    self.pressed = None;
                }
                if matches!(id, Id::Control(_)) {
                    self.slide_end();
                }
                (screen_of(&id), vec![])
            }
            Event::SlideStart { control, x, width } => {
                self.slide_start(control, x, width);
                (Dirty::BOTTOM, vec![])
            }
            Event::SlideTo { x } => {
                let effects = self.slide_to(x);
                (if effects.is_empty() { Dirty::NONE } else { Dirty::BOTTOM }, effects)
            }
            Event::Wifi(wifi) => {
                self.set_wifi(wifi);
                (Dirty::BOTTOM, vec![])
            }
            Event::Power(power) => {
                self.power = power;
                (if self.mode == Mode::Battery { Dirty::BOTTOM } else { Dirty::NONE }, vec![])
            }
            Event::Home => (Dirty::NONE, self.home()),
            // Sleep may have reset what the settings set (frequency caps, cores): again.
            Event::Resumed => (Dirty::NONE, vec![Effect::Apply { game: self.running.clone(), settings: self.current() }]),
            Event::Quit => (Dirty::NONE, vec![Effect::SaveResume(self.resume_text()), Effect::Quit]),
        }
    }

    /// What holding `id` does: keyboard keys repeat, modifiers and everything else get one long press.
    pub fn hold(&self, id: &Id) -> Hold {
        match id {
            Id::Key(pos) => match self.keyboard.effective(*pos) {
                Some(Key::Mod(_)) | None => Hold::Long,
                Some(_) => Hold::Repeat,
            },
            _ => Hold::Long,
        }
    }

    fn activate(&mut self, id: Id, long: bool) -> (Dirty, Vec<Effect>) {
        match id {
            Id::Game(game) => {
                // Activating the selected game again launches it, as A on a list entry does in shelf.
                if self.selected.as_ref() == Some(&game) {
                    return (Dirty::BOTH, self.launch_effects(game));
                }
                let position = self.collection().and_then(|c| {
                    c.games.iter().position(|&i| self.library.games.get(i).is_some_and(|g| g.id == game))
                });
                if let (Some(p), Some(c)) = (position, self.cursors.get_mut(self.tab)) {
                    c.index = p;
                }
                self.sync();
                (Dirty::BOTH, vec![])
            }
            Id::Tab(tab) => {
                self.set_tab(usize::from(tab));
                (Dirty::BOTH, vec![])
            }
            Id::Mode(mode) => (Dirty::BOTH, self.set_mode(mode)),
            Id::Tile(setting) => {
                let value = if long { None } else { Some(setting.next(self.tile_value(setting))) };
                (Dirty::BOTH, self.change(setting, value))
            }
            Id::Env => {
                self.open_editor();
                (Dirty::BOTTOM, vec![])
            }
            Id::EditorSave => (Dirty::BOTTOM, self.save_editor()),
            Id::EditorCancel => {
                self.editor = None;
                (Dirty::BOTTOM, vec![])
            }
            Id::Wake => {
                self.woken = true;
                (Dirty::BOTTOM, vec![])
            }
            Id::Scope(scope) => {
                self.scope = scope;
                (Dirty::BOTTOM, vec![])
            }
            Id::Reset => (Dirty::BOTH, self.reset()),
            Id::Launch => match self.selected.clone() {
                Some(game) => (Dirty::BOTH, self.launch_effects(game)),
                None => (Dirty::BOTH, vec![]),
            },
            Id::App(i) => (Dirty::BOTTOM, self.tap_app(usize::from(i), long)),
            Id::GameTile => (Dirty::BOTTOM, self.tap_game(long)),
            Id::NewInstance(i) => (Dirty::BOTTOM, self.new_instance(usize::from(i))),
            Id::PadMouse => {
                self.pad_mouse = !self.pad_mouse;
                (Dirty::BOTTOM, vec![Effect::PadMouse(self.pad_mouse)])
            }
            Id::Control(control) if !long => (Dirty::BOTTOM, self.tap_control(control)),
            Id::Control(_) => (Dirty::NONE, vec![]),
            Id::WifiPower => (Dirty::BOTTOM, vec![Effect::Wifi(wifi::Command::Enable(!self.wifi.enabled))]),
            Id::Network(i) => (Dirty::BOTTOM, self.tap_network(usize::from(i), long)),
            Id::Key(pos) => {
                let stroke = self.keyboard.press(pos, long);
                // Into the environment editor while it is open, else to the focused app.
                let effects = match stroke {
                    Some(stroke) if self.editor.is_some() => self.edit(stroke),
                    Some(stroke) => vec![Effect::Key(stroke)],
                    None => vec![],
                };
                (Dirty::BOTTOM, effects)
            }
        }
    }

    /// Changes the second screen's mode: Wi-Fi a second time goes back to the mode before it. The
    /// touchpad and the Wi-Fi watch start and stop with their modes; an open editor closes.
    fn set_mode(&mut self, mode: Mode) -> Vec<Effect> {
        let before = self.mode;
        // The status modes (opened from status icons) toggle; the bar's modes are remembered.
        let status_mode = |m: Mode| !Mode::BAR.contains(&m);
        self.mode = if status_mode(mode) && before == mode { self.bar_mode } else { mode };
        if !status_mode(self.mode) {
            self.bar_mode = self.mode;
        }
        if self.mode != before {
            self.editor = None;
        }
        self.resume_dirty = Some(self.unix_secs);
        let mut effects = vec![];
        let battery = |m: Mode| m == Mode::Battery;
        if battery(before) != battery(self.mode) {
            effects.push(Effect::WatchPower(battery(self.mode)));
        }
        let wifi = |m: Mode| m == Mode::Wifi;
        if wifi(before) != wifi(self.mode) {
            effects.push(Effect::Wifi(wifi::Command::Watch(wifi(self.mode))));
        }
        let touchpad = |m: Mode| m == Mode::Touchpad;
        if touchpad(before) != touchpad(self.mode) {
            effects.push(Effect::Touchpad(touchpad(self.mode)));
        }
        effects
    }

    /// The home button: from shelf to the running game, from anything else to shelf.
    fn home(&mut self) -> Vec<Effect> {
        let front = self.windows.iter().find(|w| w.focused);
        let shelf_in_front = front.is_none_or(|w| w.app_id == apps::SHELF_APP_ID);
        if !shelf_in_front {
            return vec![Effect::ShowShelf];
        }
        match self.running {
            Some(_) => vec![Effect::FocusWorkspace(windows::GAME_WORKSPACE.into())],
            None => vec![],
        }
    }

    /// Back to the global settings, and the pad mouse off if the launch turned it on.
    fn game_ended(&mut self) -> Vec<Effect> {
        let mut effects = vec![Effect::Apply { game: None, settings: self.settings.global }];
        if self.pad_mouse_by_launch {
            self.pad_mouse_by_launch = false;
            if self.pad_mouse {
                self.pad_mouse = false;
                effects.push(Effect::PadMouse(false));
            }
        }
        effects
    }

    /// Launching `game`, unless one is already running.
    fn launch_effects(&mut self, game: Arc<str>) -> Vec<Effect> {
        if self.running.is_some() {
            return vec![];
        }
        let Some(g) = self.library.games.iter().find(|g| g.id == game) else {
            return vec![];
        };
        let settings = self.settings.of(&game);
        let env = self.settings.env_of(&game);
        // Shown once its first window appears.
        self.pending_show = Some(windows::GAME_WORKSPACE.into());
        vec![
            Effect::SaveResume(self.resume_text()),
            Effect::Launch { game, settings, env, launch: g.launch.clone(), file: g.file.clone() },
        ]
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::indexing_slicing)]
mod tests {
    use super::*;

    pub(crate) fn state() -> State {
        State::new(clamshell_library::demo_library())
    }

    #[test]
    fn starts_with_first_game_selected_and_focused() {
        let s = state();
        assert_eq!(s.selected.as_deref(), Some("zelda-lttp"));
        assert_eq!(s.focus, Some(Id::Game("zelda-lttp".into())));
    }

    #[test]
    fn selecting_then_activating_again_launches() {
        let mut s = state();
        let (_, fx) = s.apply(Event::Activate(Id::Game("ff6".into())));
        assert!(fx.is_empty());
        let (_, fx) = s.apply(Event::Activate(Id::Game("ff6".into())));
        assert_eq!(
            fx.last(),
            Some(&Effect::Launch { game: "ff6".into(), settings: Values::default(), env: vec![], launch: Launch::None, file: None })
        );
    }

    #[test]
    fn keys_repeat_and_modifiers_lock_when_held() {
        use keyboard::{Modifier, find};
        let s = state();
        assert_eq!(s.hold(&Id::Key(find(Key::Char('a')).unwrap())), Hold::Repeat);
        assert_eq!(s.hold(&Id::Key(find(Key::Mod(Modifier::Shift)).unwrap())), Hold::Long);
        assert_eq!(s.hold(&Id::Launch), Hold::Long);
    }

    #[test]
    fn down_and_up_light_a_target() {
        let mut s = state();
        s.apply(Event::Down(Id::Launch));
        assert_eq!(s.pressed, Some(Id::Launch));
        s.apply(Event::Up(Id::Launch));
        assert_eq!(s.pressed, None);
    }

    #[test]
    fn status_redraws_the_bottom_only_when_it_changes() {
        let mut s = state();
        let status = Status { battery: Some(50), ..Status::default() };
        assert_eq!(s.apply(Event::Status(status)).0, Dirty::BOTTOM);
        assert_eq!(s.apply(Event::Status(status)).0, Dirty::NONE);
    }

    #[test]
    fn touchpad_mode_switches_the_touchscreen() {
        let mut s = state();
        let (_, fx) = s.apply(Event::Activate(Id::Mode(Mode::Touchpad)));
        assert_eq!(fx, vec![Effect::Touchpad(true)]);
        assert_eq!(s.apply(Event::Activate(Id::Mode(Mode::Wifi))).1.last(), Some(&Effect::Touchpad(false)));
        assert_eq!(s.apply(Event::Activate(Id::Mode(Mode::Wifi))).1.last(), Some(&Effect::Touchpad(true)), "back");
        let (_, fx) = s.apply(Event::Activate(Id::Mode(Mode::Keyboard)));
        assert_eq!(fx, vec![Effect::Touchpad(false)]);
        let (_, fx) = s.apply(Event::Activate(Id::Mode(Mode::Keyboard)));
        assert!(fx.is_empty());
    }

    #[test]
    fn pad_mouse_toggles_independently_of_the_mode() {
        let mut s = state();
        s.apply(Event::Activate(Id::Mode(Mode::Keyboard)));
        let (_, fx) = s.apply(Event::Activate(Id::PadMouse));
        assert_eq!(fx, vec![Effect::PadMouse(true)]);
        assert_eq!(s.mode, Mode::Keyboard);
        let (_, fx) = s.apply(Event::Activate(Id::PadMouse));
        assert_eq!(fx, vec![Effect::PadMouse(false)]);
    }

    #[test]
    fn the_focus_ring_shows_for_the_pad_only() {
        let mut s = state();
        assert!(!s.show_focus);
        s.apply(Event::Focus(Id::Launch));
        assert!(s.show_focus);
        assert_eq!(s.apply(Event::HideFocus).0, Dirty::BOTH);
        assert!(!s.show_focus);
        assert_eq!(s.apply(Event::HideFocus).0, Dirty::NONE);
    }

    #[test]
    fn one_game_at_a_time_and_its_play_counts() {
        let mut s = state();
        let first = s.selected.clone().unwrap();
        s.apply(Event::Started(first.clone()));
        let (_, fx) = s.apply(Event::Activate(Id::Launch));
        assert!(fx.is_empty(), "no second launch while one runs");
        s.apply(Event::Played { game: first.clone(), start: 1000, duration: 600 });
        assert_eq!(s.running, None);
        let g = s.library.games.iter().find(|g| g.id == first).unwrap();
        assert_eq!((g.plays.count, g.plays.seconds), (1, 600));
        assert!(s.library.collections.iter().any(|c| c.name == "Recent"));
    }

    #[test]
    fn home_goes_between_shelf_and_the_game() {
        let mut s = state();
        let window = |app_id: &str, focused: bool| windows::Window {
            id: 1,
            app_id: app_id.into(),
            title: String::new(),
            focused,
            unit: None,
            workspace: String::new(),
        };
        assert_eq!(s.apply(Event::Home).1, vec![], "nothing runs: shelf stays");
        s.apply(Event::Started("zelda-lttp".into()));
        s.apply(Event::Windows(vec![window(apps::SHELF_APP_ID, true)]));
        assert_eq!(s.apply(Event::Home).1, vec![Effect::FocusWorkspace(windows::GAME_WORKSPACE.into())]);
        s.apply(Event::Windows(vec![window(apps::SHELF_APP_ID, false), window("retroarch", true)]));
        assert_eq!(s.apply(Event::Home).1, vec![Effect::ShowShelf]);
    }

    #[test]
    fn tick_redraws_once_a_minute() {
        let mut s = state();
        let (dirty, _) = s.apply(Event::Tick { unix_secs: 60 });
        assert_eq!(dirty, Dirty::BOTH);
        let (dirty, _) = s.apply(Event::Tick { unix_secs: 119 });
        assert_eq!(dirty, Dirty::NONE);
    }
}
