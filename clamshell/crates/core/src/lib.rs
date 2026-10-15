//! The program's state and its rules. No I/O: every change comes in as an [`Event`], and what the outside
//! world must do comes back as [`Effect`]s. The UI and the policy threads only read the state and send
//! events, so all of this is unit-testable.

pub mod keyboard;
pub mod apps;
pub mod bluetooth;
pub mod controls;
pub mod editor;
mod game;
mod items;
pub mod metrics;
pub mod mixer;
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
use items::ItemKey;
use keyboard::{Key, KeyPos, Keyboard, Stroke};
use settings::{Setting, Settings, Values};

pub use editor::{Editor, count_env, parse_env};
pub use game::{Front, RunningGame};

pub use clamshell_library::{Collection, Game, Launch, Library};

/// The second screen's tabs (the code says mode), chosen from the bar; the last one chosen stays, also
/// across restarts. Volume and brightness open from their controls, Bluetooth, Wi-Fi and Battery from
/// their status icons; all are tabs alike.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum Mode {
    Settings,
    /// The apps at hand and what runs (the game first).
    Dock,
    Keyboard,
    Touchpad,
    #[default]
    Sysinfo,
    Volume,
    Brightness,
    Wifi,
    Bluetooth,
    Battery,
}

impl Mode {
    /// The tabs with a plain icon, at the bar's start, in order (the others' icons show a level or a
    /// status).
    pub const BAR: [Mode; 5] = [Mode::Settings, Mode::Dock, Mode::Keyboard, Mode::Touchpad, Mode::Sysinfo];
    /// Every tab, in the bar's order.
    pub const TABS: [Mode; 10] = [
        Mode::Settings,
        Mode::Dock,
        Mode::Keyboard,
        Mode::Touchpad,
        Mode::Sysinfo,
        Mode::Volume,
        Mode::Brightness,
        Mode::Bluetooth,
        Mode::Wifi,
        Mode::Battery,
    ];

    pub fn name(self) -> &'static str {
        match self {
            Mode::Settings => "settings",
            Mode::Dock => "dock",
            Mode::Keyboard => "keyboard",
            Mode::Touchpad => "touchpad",
            Mode::Sysinfo => "sysinfo",
            Mode::Volume => "volume",
            Mode::Brightness => "brightness",
            Mode::Wifi => "wifi",
            Mode::Battery => "battery",
            Mode::Bluetooth => "bluetooth",
        }
    }

    /// What the tab watches while open (the system read for it), turned on or off; the touchpad grabs the
    /// touchscreen.
    pub fn watch(self, on: bool) -> Option<Effect> {
        match self {
            Mode::Touchpad => Some(Effect::Touchpad(on)),
            Mode::Volume => Some(Effect::Audio(mixer::Command::Watch(on))),
            Mode::Brightness => Some(Effect::WatchBacklights(on)),
            Mode::Wifi => Some(Effect::Wifi(wifi::Command::Watch(on))),
            Mode::Bluetooth => Some(Effect::Bluetooth(bluetooth::Command::Watch(on))),
            Mode::Battery => Some(Effect::WatchPower(on)),
            Mode::Settings | Mode::Dock | Mode::Keyboard | Mode::Sysinfo => None,
        }
    }

    pub fn from_name(name: &str) -> Option<Mode> {
        // ("launcher": the dock's old name, in saved places and scripts.)
        let name = if name == "launcher" { "dock" } else { name };
        Mode::TABS.into_iter().find(|m| m.name() == name)
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

/// Where the controller's input goes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum PadRoute {
    /// The game's virtual pad.
    #[default]
    Game,
    /// ClamShell's own screens, as [`Event::Pad`]: shelf's and the second screen, or the second screen
    /// alone.
    Ui,
    /// Typing into the app in front (D-pad arrows, A Enter, B Backspace): the keyboard is open.
    Keys,
    /// The pad mouse.
    Mouse,
}

/// The controller driving ClamShell's screens.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PadInput {
    /// The D-pad or left stick (repeated while held).
    Up,
    Down,
    Left,
    Right,
    /// A pressed or released: activates the focused target (holds act as for touch).
    A(bool),
    /// B: cancels the editor.
    B,
    /// Y: the selected game in or out of the favourites.
    Y,
    /// The shoulder buttons: the previous or next tab (shelf) or mode (the second screen).
    Shoulder(i32),
    /// View/Select: focus to the other screen.
    Switch,
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
    /// A dock app, by position.
    App(u16),
    /// A running game's tile in the dock, by position among the running games.
    GameTile(u16),
    /// The dock's first tile while no game runs: to shelf, to start one.
    StartGame,
    /// Shelf's choice when starting a game while another runs: keep that one running, or close it.
    StartKeep,
    StartClose,
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
    /// Volume or brightness in the bar: a tap opens its tab, a hold or a slide brings up its slider.
    Control(Control),
    /// A level row in the volume or brightness tab: the finger sets it.
    Level(mixer::Level),
    /// The volume tab's mute button, an app's mute button, and an output, by position.
    MuteOutput,
    StreamMute(u16),
    Output(u16),
    /// The Wi-Fi mode's radio switch, and a network by position.
    WifiPower,
    Network(u16),
    /// The Bluetooth mode's radio switch, and a device by position.
    BluetoothPower,
    Device(u16),
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
    /// The first backlight's level 0-100 as read, if there is one: the brightness until ClamShell sets
    /// one (see [`mixer::Brightness`]).
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
    /// The finger that slides lifted.
    SlideEnd,
    SlideTo { x: f32 },
    /// Wi-Fi as NetworkManager has it, after any change (while the Wi-Fi mode is open).
    Wifi(wifi::Wifi),
    /// Bluetooth as BlueZ has it, after any change.
    Bluetooth(bluetooth::Bluetooth),
    /// The charge history and what is using the machine (every few seconds while the battery mode is open).
    Power(power::Power),
    /// The outputs and the apps playing (while the volume tab is open), the backlights (while the
    /// brightness tab is open).
    Audio(mixer::Audio),
    /// The backlights' names.
    Backlights(Vec<String>),
    /// A level row set by the finger, or stepped by the pad (-1 or 1).
    SetLevel(mixer::Level, u8),
    LevelStep(mixer::Level, i16),
    /// The UI is up: what the state in force needs at start (the tab's watch, the screens' brightness,
    /// where the pad goes).
    Start,
    /// Touchpad mode works, or why it doesn't (no touchscreen, the grab refused).
    TouchpadProblem(Option<String>),
    /// The pad's home button.
    Home,
    /// View, away from shelf: the pad between the second screen and what else wants it (the pad mouse,
    /// typing).
    PadSwitch,
    /// A text field of the app in front wants typing (true), or no longer does (the Wayland side).
    TextInput(bool),
    /// The pad, while it drives ClamShell (the UI handles these; the state ignores them).
    Pad(PadInput),
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
        /// Its scope's and workspace's name (`clamshell-game-<secs>-<n>`).
        workspace: String,
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
    /// Where shelf and the second screen are (see [`State::resume_text`]): write it, to start there
    /// next time.
    SaveResume(String),
    /// The screens' brightness (see [`mixer::Brightness::text`]): write it, to apply at the next start.
    SaveBrightness(String),
    /// A key press for the focused app (virtual keyboard).
    Key(Stroke),
    /// Touchpad mode started (true) or ended: the input side grabs or releases the touchscreen.
    Touchpad(bool),
    /// Where the controller's input goes now.
    PadRoute(PadRoute),
    /// Start a launcher app's command as a new instance: a scope and a workspace of this name
    /// (`clamshell-app-<key>-<secs>-<n>`).
    RunApp { name: String, command: Vec<String> },
    /// Show this workspace (a launcher instance's, or the game's).
    FocusWorkspace(String),
    /// Ask these windows to close (as their own close buttons would): an instance's.
    CloseWindows(Vec<i64>),
    /// Move these windows to this workspace (a Steam game's, from where they landed).
    MoveWindows { windows: Vec<i64>, workspace: String },
    /// Quit the game: close its windows, then end its scopes (ours only) if it doesn't go.
    QuitGame { windows: Vec<i64>, units: Vec<String> },
    /// Bring shelf's window to the front.
    ShowShelf,
    /// Record a play that ClamShell didn't start (a game Steam started): in stats.db.
    RecordPlay { game: Arc<str>, start: u64, duration: u64 },
    /// Output volume (0-100), mute.
    SetVolume(u8),
    SetMute(bool),
    /// The screens' brightness: each backlight to `level` times its factor (both 0-100; a backlight
    /// without one: 100).
    Brightness { level: u8, factors: Vec<(String, u8)> },
    Wifi(wifi::Command),
    Bluetooth(bluetooth::Command),
    /// The battery mode opened (read the charge history and the spenders) or closed.
    WatchPower(bool),
    Audio(mixer::Command),
    /// The brightness tab opened (read the backlights) or closed.
    WatchBacklights(bool),
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
    /// The pad mouse was turned on for the game in front (its Setting::PadMouse), to turn off when
    /// another (or none) is; turning it off or on by hand makes it the user's.
    pad_mouse_by_game: bool,
    /// Where the pad goes (see [`Effect::PadRoute`]).
    pub pad_route: PadRoute,
    /// View moved the pad to the second screen, away from what else wants it (the pad mouse, typing).
    pad_on_bottom: bool,
    /// What else wanted the pad when last looked (see `pad_wanted_by`).
    pad_contender: Option<PadRoute>,
    /// The games that run, in the order they started (`running` is the one most recently in front),
    /// and the launches asked for that haven't started.
    pub games: Vec<RunningGame>,
    launching: Vec<game::Launching>,
    /// The game to start once the one closed for it has ended: (the one closed, the one to start).
    queued: Option<(Arc<str>, Arc<str>)>,
    /// What the settings panel edits (global whenever there is no game in context).
    pub scope: Scope,
    pub mode: Mode,
    /// The mode the keyboard was brought up from by an app's text field: back there when the field is
    /// done, unless the user chose a mode meanwhile.
    raised_from: Option<Mode>,
    /// Volume or brightness being slid, if any.
    pub slider: Option<Slider>,
    pub wifi: wifi::Wifi,
    pub bluetooth: bluetooth::Bluetooth,
    pub audio: mixer::Audio,
    /// The backlights' names (read while the brightness tab is open).
    pub backlights: Vec<String>,
    pub brightness: mixer::Brightness,
    /// The level row a finger is on.
    dragging: Option<mixer::Level>,
    pub power: power::Power,
    /// Why touchpad mode doesn't work, if it doesn't.
    pub touchpad_problem: Option<String>,
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
    /// The game: of the games running, the one most recently in front (its settings are in force).
    pub running: Option<Arc<str>>,
    /// When the tab or a selection last changed and isn't saved yet (epoch seconds).
    resume_dirty: Option<u64>,
    /// When the brightness last changed and isn't saved yet.
    brightness_dirty: Option<u64>,
    /// The list item a press is on, by what it is (not where), so a list that changes under the
    /// finger doesn't redirect it.
    press_key: Option<(Id, ItemKey)>,
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
        Id::Game(_) | Id::Tab(_) | Id::StartKeep | Id::StartClose => Dirty::BOTH,
        Id::Mode(_)
        | Id::Tile(_)
        | Id::Env
        | Id::EditorSave
        | Id::EditorCancel
        | Id::Wake
        | Id::App(_)
        | Id::GameTile(_)
        | Id::StartGame
        | Id::NewInstance(_)
        | Id::Scope(_)
        | Id::Reset
        | Id::Launch
        | Id::Key(_)
        | Id::PadMouse
        | Id::Control(_)
        | Id::Level(_)
        | Id::MuteOutput
        | Id::StreamMute(_)
        | Id::Output(_)
        | Id::WifiPower
        | Id::Network(_)
        | Id::BluetoothPower
        | Id::Device(_) => Dirty::BOTTOM,
    }
}

impl State {
    pub fn new(library: Library) -> Self {
        let cursors = vec![Cursor::default(); library.collections.len()];
        let mut s = State { library, cursors, rows: 1, ..State::default() };
        s.set_apps(apps::defaults(), None);
        s.sync();
        s.focus = s.selected.clone().map(Id::Game);
        s.pad_contender = s.pad_wanted_by();
        s.pad_route = s.wanted_pad_route();
        s
    }

    /// The launcher's apps (from their file), and what is wrong with it if anything.
    pub fn set_apps(&mut self, apps: Vec<apps::App>, error: Option<String>) {
        self.apps = apps;
        self.apps_error = error;
    }

    /// Applies one event; returns which screens changed and what must happen outside.
    pub fn apply(&mut self, event: Event) -> (Dirty, Vec<Effect>) {
        let running = self.running.clone();
        let (mut dirty, mut effects) = self.apply_event(event);
        if self.running != running && self.follow_pad_mouse() {
            dirty = Dirty::BOTH;
        }
        // Something else wants the pad now: it gets it. The pad mouse always takes it; otherwise a pad
        // being used on the second screen (its focus ring showing) with nothing else around stays there
        // (the keyboard coming up under it is typed on with the pad).
        let contender = self.pad_wanted_by();
        if contender != self.pad_contender {
            let in_use = self.pad_route == PadRoute::Ui && self.show_focus;
            let stays = self.pad_contender.is_none() && in_use && contender != Some(PadRoute::Mouse);
            self.pad_on_bottom = stays;
            self.pad_contender = contender;
        }
        let route = self.wanted_pad_route();
        if route != self.pad_route {
            self.pad_route = route;
            effects.push(Effect::PadRoute(route));
            // The focus ring shows where the pad is.
            if route != PadRoute::Ui && self.show_focus {
                self.show_focus = false;
                return (Dirty::BOTH, effects);
            }
        }
        (dirty, effects)
    }

    /// The game in front changed: the pad mouse on if it wants it (Setting::PadMouse), off again when
    /// another game (or none) is in front, unless the user switched it meanwhile. Whether it changed.
    fn follow_pad_mouse(&mut self) -> bool {
        let wanted = self.running.is_some() && self.current().get(Setting::PadMouse) == 1;
        if wanted && !self.pad_mouse {
            self.pad_mouse = true;
            self.pad_mouse_by_game = true;
            return true;
        }
        if !wanted && self.pad_mouse && self.pad_mouse_by_game {
            self.pad_mouse = false;
            self.pad_mouse_by_game = false;
            return true;
        }
        false
    }

    /// What wants the pad besides the second screen: the pad mouse while on; else shelf (`Ui`), the game
    /// in front, or typing into an app while the keyboard is open; nothing for an app otherwise.
    fn pad_wanted_by(&self) -> Option<PadRoute> {
        if self.pad_mouse {
            return Some(PadRoute::Mouse);
        }
        match (self.front(), self.mode) {
            (Front::Shelf, _) => Some(PadRoute::Ui),
            (Front::Game, _) => Some(PadRoute::Game),
            (Front::App, Mode::Keyboard) => Some(PadRoute::Keys),
            (Front::App, _) => None,
        }
    }

    /// Where the pad goes: to what wants it, unless View moved it to the second screen; to the second
    /// screen when nothing else wants it. A game keeps it (View is the game's button there).
    fn wanted_pad_route(&self) -> PadRoute {
        match self.pad_wanted_by() {
            None => PadRoute::Ui,
            Some(r @ (PadRoute::Game | PadRoute::Ui)) => r,
            Some(_) if self.pad_on_bottom => PadRoute::Ui,
            Some(r) => r,
        }
    }

    /// Whether the pad, on ClamShell's screens, may go to shelf's (shelf is in front and nothing else
    /// has the pad), or only to the second screen.
    pub fn pad_on_top(&self) -> bool {
        self.pad_wanted_by() == Some(PadRoute::Ui)
    }

    fn apply_event(&mut self, event: Event) -> (Dirty, Vec<Effect>) {
        match event {
            Event::Tick { unix_secs } => {
                // Once a minute: the bar's clock, and both screens' burn-in orbit step.
                let minute_changed = unix_secs / 60 != self.unix_secs / 60;
                // Starts systemd never listed failed: forget them.
                self.starting.retain(|(_, at)| unix_secs < at + STARTING_FOR);
                self.unix_secs = unix_secs;
                // The place, once moving has stopped for a moment.
                let mut effects = vec![];
                if self.resume_dirty.is_some_and(|since| unix_secs >= since + resume::RESUME_AFTER) {
                    self.resume_dirty = None;
                    effects.push(Effect::SaveResume(self.resume_text()));
                }
                if self.brightness_dirty.is_some_and(|since| unix_secs >= since + resume::RESUME_AFTER) {
                    self.brightness_dirty = None;
                    effects.push(Effect::SaveBrightness(self.brightness.text()));
                }
                (if minute_changed { Dirty::BOTH } else { Dirty::NONE }, effects)
            }
            Event::Status(status) => {
                let status = self.keep_dragged(status);
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
            // The list item the press came down on is gone.
            Event::Activate(id) | Event::LongPress(id) if self.resolve(&id).is_none() => (Dirty::BOTTOM, vec![]),
            Event::Activate(id) => {
                let id = self.resolve(&id).unwrap_or(id);
                self.focus = Some(id.clone());
                self.activate(id, false)
            }
            Event::LongPress(id) => {
                let id = self.resolve(&id).unwrap_or(id);
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
                self.game_started(&game);
                self.running = Some(game);
                self.woken = false;
                (Dirty::BOTH, vec![])
            }
            Event::Played { game, start, duration } => {
                self.relayout(|lib| {
                    if let Some(g) = lib.games.iter_mut().find(|g| g.id == game) {
                        g.plays.add(start, duration);
                    }
                    lib.rebuild_derived();
                });
                (Dirty::BOTH, self.game_gone(&game))
            }
            Event::Units(units) => {
                self.units = units;
                let units = &self.units;
                self.starting.retain(|(name, _)| !units.contains(name));
                (Dirty::BOTTOM, vec![])
            }
            Event::Windows(windows) => {
                self.set_windows(windows);
                let mut effects = self.follow_games();
                effects.extend(self.show_pending());
                (Dirty::BOTH, effects)
            }
            Event::LaunchFailed(game) => {
                self.launching.retain(|l| l.0 != game);
                if self.running_game(&game).is_some() {
                    return (Dirty::BOTH, self.game_gone(&game));
                }
                (Dirty::BOTH, vec![])
            }
            Event::Down(id) => {
                let dirty = screen_of(&id);
                if let Id::Key(pos) = id {
                    self.keyboard.down(pos);
                }
                self.press_key = self.item_key(&id).map(|key| (id.clone(), key));
                self.pressed = Some(id);
                (dirty, vec![])
            }
            Event::Up(id) => {
                if self.pressed.as_ref() == Some(&id) {
                    self.pressed = None;
                }
                if matches!(id, Id::Level(_)) {
                    self.dragging = None;
                }
                if self.press_key.as_ref().is_some_and(|(p, _)| *p == id) {
                    self.press_key = None;
                }
                if let Id::Key(pos) = id {
                    self.keyboard.up(pos);
                }
                (screen_of(&id), vec![])
            }
            Event::SlideEnd => {
                self.slide_end();
                (Dirty::BOTTOM, vec![])
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
            Event::Bluetooth(bluetooth) => {
                let changed = bluetooth != self.bluetooth;
                self.bluetooth = bluetooth;
                (if changed { Dirty::BOTTOM } else { Dirty::NONE }, vec![])
            }
            Event::Power(power) => {
                self.power = power;
                (if self.mode == Mode::Battery { Dirty::BOTTOM } else { Dirty::NONE }, vec![])
            }
            Event::Audio(audio) => {
                let changed = self.set_audio(audio);
                (if changed { Dirty::BOTTOM } else { Dirty::NONE }, vec![])
            }
            Event::Backlights(backlights) => {
                let changed = self.set_backlights(backlights);
                (if changed { Dirty::BOTTOM } else { Dirty::NONE }, vec![])
            }
            Event::SetLevel(level, value) => {
                let Some(level) = self.resolve_level(level) else {
                    return (Dirty::NONE, vec![]);
                };
                let before = self.level_of(level);
                let effects = self.set_level(level, value);
                (if self.level_of(level) == before { Dirty::NONE } else { Dirty::BOTTOM }, effects)
            }
            Event::LevelStep(level, direction) => (Dirty::BOTTOM, self.step_level(level, direction)),
            Event::Home => (Dirty::NONE, self.home()),
            Event::TouchpadProblem(problem) => {
                let changed = problem != self.touchpad_problem;
                self.touchpad_problem = problem;
                (if changed { Dirty::BOTTOM } else { Dirty::NONE }, vec![])
            }
            Event::Start => {
                let mut effects: Vec<Effect> = self.mode.watch(true).into_iter().collect();
                effects.extend(self.brightness_effect());
                effects.push(Effect::PadRoute(self.pad_route));
                (Dirty::BOTH, effects)
            }
            Event::PadSwitch => {
                if matches!(self.pad_wanted_by(), Some(PadRoute::Mouse | PadRoute::Keys)) {
                    self.pad_on_bottom = !self.pad_on_bottom;
                }
                (Dirty::NONE, vec![])
            }
            Event::TextInput(active) => {
                let before = self.mode;
                let effects = self.text_input(active);
                (if self.mode == before { Dirty::NONE } else { Dirty::BOTH }, effects)
            }
            Event::Pad(_) => (Dirty::NONE, vec![]),
            // Sleep may have reset what the settings set (frequency caps, cores): again.
            Event::Resumed => (Dirty::NONE, vec![Effect::Apply { game: self.running.clone(), settings: self.current() }]),
            Event::Quit => (Dirty::NONE, vec![Effect::SaveResume(self.resume_text()), Effect::Quit]),
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

    fn activate(&mut self, id: Id, long: bool) -> (Dirty, Vec<Effect>) {
        match id {
            Id::Game(game) => {
                // Activating the selected game again plays it (starts, resumes, or switches to it).
                if self.selected.as_ref() == Some(&game) {
                    return (Dirty::BOTH, self.play(game));
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
            Id::Mode(mode) => {
                self.raised_from = None;
                (Dirty::BOTH, self.set_mode(mode))
            }
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
                Some(game) => (Dirty::BOTH, self.play(game)),
                None => (Dirty::BOTH, vec![]),
            },
            Id::App(i) => (Dirty::BOTTOM, self.tap_app(usize::from(i), long)),
            Id::GameTile(i) => (Dirty::BOTTOM, self.tap_game(usize::from(i), long)),
            Id::StartGame => (Dirty::NONE, vec![Effect::ShowShelf]),
            Id::StartKeep => (Dirty::BOTH, self.start_beside(false)),
            Id::StartClose => (Dirty::BOTH, self.start_beside(true)),
            Id::NewInstance(i) => (Dirty::BOTTOM, self.new_instance(usize::from(i))),
            Id::PadMouse => {
                self.pad_mouse = !self.pad_mouse;
                self.pad_mouse_by_game = false;
                (Dirty::BOTTOM, vec![])
            }
            // A tap opens the tab (a slide is the UI's: it doesn't end in a tap).
            Id::Control(control) if !long => {
                self.raised_from = None;
                let mode = match control {
                    Control::Volume => Mode::Volume,
                    Control::Brightness => Mode::Brightness,
                };
                (Dirty::BOTH, self.set_mode(mode))
            }
            Id::Control(_) | Id::Level(_) => (Dirty::NONE, vec![]),
            Id::MuteOutput => (Dirty::BOTTOM, self.toggle_mute()),
            Id::StreamMute(i) => (Dirty::BOTTOM, self.toggle_stream_mute(usize::from(i))),
            Id::Output(i) => (Dirty::BOTTOM, self.choose_output(usize::from(i))),
            Id::WifiPower => (Dirty::BOTTOM, vec![Effect::Wifi(wifi::Command::Enable(!self.wifi.enabled))]),
            Id::Network(i) => (Dirty::BOTTOM, self.tap_network(usize::from(i), long)),
            Id::BluetoothPower => (Dirty::BOTTOM, vec![Effect::Bluetooth(bluetooth::Command::Power(!self.bluetooth.powered))]),
            Id::Device(i) => (Dirty::BOTTOM, self.tap_device(usize::from(i), long)),
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

    /// Changes the second screen's mode. The touchpad and the watches (Wi-Fi, Bluetooth, power) start
    /// and stop with their modes; an open editor closes.
    fn set_mode(&mut self, mode: Mode) -> Vec<Effect> {
        let before = self.mode;
        self.resume_dirty = Some(self.unix_secs);
        if mode == before {
            return vec![];
        }
        self.mode = mode;
        // What belonged to the tab left: an open editor, a level being set.
        self.editor = None;
        self.dragging = None;
        self.slider = None;
        before.watch(false).into_iter().chain(mode.watch(true)).collect()
    }

    /// The tab ClamShell starts in (a development option): set before [`Event::Start`].
    pub fn start_in(&mut self, mode: Mode) {
        self.mode = mode;
    }

    /// An app's text field wants typing (or is done): the keyboard comes up, and goes back to the mode it
    /// came up from when the field is done, unless the user chose a mode meanwhile. Not for a game (SDL
    /// asks for text input whether or not it needs it); the keyboard is a tap away there.
    fn text_input(&mut self, active: bool) -> Vec<Effect> {
        if active {
            // (Not over an open editor: what is typed there would be lost.)
            if self.mode == Mode::Keyboard || self.front() == game::Front::Game || self.editor.is_some() {
                return vec![];
            }
            self.raised_from = Some(self.mode);
            return self.set_mode(Mode::Keyboard);
        }
        match self.raised_from.take() {
            Some(mode) if self.mode == Mode::Keyboard => self.set_mode(mode),
            _ => vec![],
        }
    }

    /// The home button: from anything to shelf; from shelf back to what was in front before it (the
    /// game, an app's instance), or without that history (no compositor's word) to the running game.
    fn home(&mut self) -> Vec<Effect> {
        let front = self.windows.iter().find(|w| w.focused);
        let shelf_in_front = front.is_none_or(|w| w.app_id == apps::SHELF_APP_ID);
        if !shelf_in_front {
            return vec![Effect::ShowShelf];
        }
        let before = self.recent.iter().find(|ws| ws.as_str() != windows::SHELF_WORKSPACE).cloned();
        let game = self.running.as_deref().and_then(|id| self.running_game(id)).map(|g| g.workspace.clone());
        match before.or(game) {
            Some(workspace) => vec![Effect::FocusWorkspace(workspace)],
            None => vec![],
        }
    }

    /// Back to the global settings, the pad mouse off if the launch turned it on, and the game shelf
    /// was asked to switch to started.
    fn game_ended(&mut self) -> Vec<Effect> {
        vec![Effect::Apply { game: None, settings: self.settings.global }]
    }

    /// Launching `game` on a workspace of its own (unless it runs already, or is starting).
    pub(crate) fn launch_effects(&mut self, game: Arc<str>) -> Vec<Effect> {
        if self.running_game(&game).is_some() || self.launching.iter().any(|l| l.0 == game) {
            return vec![];
        }
        let Some(g) = self.library.games.iter().find(|g| g.id == game) else {
            return vec![];
        };
        let (launch, file) = (g.launch.clone(), g.file.clone());
        let appid = match launch {
            Launch::Steam { appid } => Some(appid),
            _ => None,
        };
        let workspace = format!("clamshell-game-{}-{}", self.unix_secs, self.started);
        self.started += 1;
        let settings = self.settings.of(&game);
        let env = self.settings.env_of(&game);
        self.launching.push((game.clone(), workspace.clone(), appid));
        // Shown once its first window appears.
        self.pending_show = Some(workspace.clone());
        vec![Effect::SaveResume(self.resume_text()), Effect::Launch { game, workspace, settings, env, launch, file }]
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
        assert!(
            matches!(fx.last(), Some(Effect::Launch { game, settings, env, launch: Launch::None, file: None, .. }) if &**game == "ff6" && *settings == Values::default() && env.is_empty()),
            "{fx:?}"
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
        let wifi_watch = |on| Effect::Wifi(wifi::Command::Watch(on));
        assert_eq!(s.apply(Event::Activate(Id::Mode(Mode::Wifi))).1, vec![Effect::Touchpad(false), wifi_watch(true)], "the old tab's off first");
        assert_eq!(s.apply(Event::Activate(Id::Mode(Mode::Touchpad))).1, vec![wifi_watch(false), Effect::Touchpad(true)]);
        let (_, fx) = s.apply(Event::Activate(Id::Mode(Mode::Keyboard)));
        assert_eq!(fx, vec![Effect::Touchpad(false)]);
        let (_, fx) = s.apply(Event::Activate(Id::Mode(Mode::Keyboard)));
        assert!(fx.is_empty());
    }

    #[test]
    fn a_text_field_brings_up_the_keyboard_and_back() {
        let mut s = state();
        s.apply(Event::Activate(Id::Mode(Mode::Touchpad)));
        s.apply(Event::Windows(vec![windows::Window { id: 1, app_id: "foot".into(), title: String::new(), focused: true, unit: None, workspace: "1".into() }]));
        let (dirty, fx) = s.apply(Event::TextInput(true));
        assert_eq!((s.mode, dirty, fx), (Mode::Keyboard, Dirty::BOTH, vec![Effect::Touchpad(false), Effect::PadRoute(PadRoute::Keys)]));
        assert_eq!(s.apply(Event::TextInput(false)).1, vec![Effect::Touchpad(true), Effect::PadRoute(PadRoute::Ui)]);
        assert_eq!(s.mode, Mode::Touchpad, "back where it came from");
        // The user chooses a tab meanwhile (even the keyboard's own): it stays.
        s.apply(Event::TextInput(true));
        s.apply(Event::Activate(Id::Mode(Mode::Keyboard)));
        s.apply(Event::TextInput(false));
        assert_eq!(s.mode, Mode::Keyboard);
        // Already on the keyboard: nothing to go back to.
        s.apply(Event::Activate(Id::Mode(Mode::Dock)));
        s.apply(Event::Activate(Id::Mode(Mode::Keyboard)));
        s.apply(Event::TextInput(true));
        s.apply(Event::TextInput(false));
        assert_eq!(s.mode, Mode::Keyboard);
    }

    #[test]
    fn review_fixes_hold() {
        let foot = || windows::Window { id: 1, app_id: "foot".into(), title: String::new(), focused: true, unit: None, workspace: "1".into() };
        // A text field doesn't take the screen from an open editor (its typing would be lost).
        let mut s = state();
        s.apply(Event::Windows(vec![foot()]));
        s.apply(Event::Activate(Id::Mode(Mode::Settings)));
        s.apply(Event::Activate(Id::Env));
        assert!(s.editor.is_some());
        s.apply(Event::TextInput(true));
        assert!(s.editor.is_some() && s.mode == Mode::Settings);
        // A keyboard a text field brought up isn't saved as the tab.
        let mut s = state();
        s.apply(Event::Windows(vec![foot()]));
        s.apply(Event::Activate(Id::Mode(Mode::Dock)));
        s.apply(Event::TextInput(true));
        assert_eq!(s.mode, Mode::Keyboard);
        assert!(s.resume_text().contains("@mode\tdock"));
        // Any tab but the touchpad comes back, its watch started with Start.
        s.apply(Event::Activate(Id::Mode(Mode::Wifi)));
        let mut again = state();
        again.restore(&s.resume_text());
        assert_eq!(again.mode, Mode::Wifi);
        assert!(again.apply(Event::Start).1.contains(&Effect::Wifi(wifi::Command::Watch(true))));
        // Holding the volume icon without moving makes up no volume.
        let mut s = state();
        s.apply(Event::SlideStart { control: Control::Volume, x: 300.0, width: 620.0 });
        s.apply(Event::SlideEnd);
        assert_eq!(s.status.volume, None);
        // Leaving a tab mid-drag lets readings in again.
        let mut s = state();
        s.apply(Event::Status(Status { volume: Some(40), ..Status::default() }));
        s.apply(Event::Activate(Id::Mode(Mode::Volume)));
        s.apply(Event::SetLevel(mixer::Level::Volume, 60));
        s.apply(Event::Activate(Id::Mode(Mode::Dock)));
        s.apply(Event::Status(Status { volume: Some(45), ..Status::default() }));
        assert_eq!(s.status.volume, Some(45));
    }

    #[test]
    fn the_pad_mouse_follows_the_game_in_front_unless_switched_by_hand() {
        let mut s = state();
        s.apply(Event::Activate(Id::Game("zelda-lttp".into())));
        s.apply(Event::Activate(Id::Scope(Scope::Game)));
        s.apply(Event::Activate(Id::Tile(Setting::PadMouse)));
        s.apply(Event::Started("zelda-lttp".into()));
        assert!(s.pad_mouse, "the game wants it");
        // Off and on again by hand: the user's now, it stays when the game ends.
        s.apply(Event::Activate(Id::PadMouse));
        s.apply(Event::Activate(Id::PadMouse));
        s.apply(Event::Played { game: "zelda-lttp".into(), start: 0, duration: 1 });
        assert!(s.pad_mouse);
    }

    #[test]
    fn pad_mouse_toggles_independently_of_the_mode() {
        let mut s = state();
        s.apply(Event::Activate(Id::Mode(Mode::Keyboard)));
        let (_, fx) = s.apply(Event::Activate(Id::PadMouse));
        assert_eq!(fx, vec![Effect::PadRoute(PadRoute::Mouse)]);
        assert_eq!(s.mode, Mode::Keyboard);
        let (_, fx) = s.apply(Event::Activate(Id::PadMouse));
        assert_eq!(fx, vec![Effect::PadRoute(PadRoute::Ui)]);
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
    fn a_game_runs_and_its_play_counts() {
        let mut s = state();
        let first = s.selected.clone().unwrap();
        s.apply(Event::Started(first.clone()));
        let (_, fx) = s.apply(Event::Activate(Id::Launch));
        assert!(!fx.iter().any(|e| matches!(e, Effect::Launch { .. })), "no second launch while one runs: {fx:?}");
        s.apply(Event::Played { game: first.clone(), start: 1000, duration: 600 });
        assert_eq!(s.running, None);
        let g = s.library.games.iter().find(|g| g.id == first).unwrap();
        assert_eq!((g.plays.count, g.plays.seconds), (1, 600));
        assert!(s.library.collections.iter().any(|c| c.name == "Recent"));
    }

    #[test]
    fn the_pad_goes_to_shelf_the_game_or_typing_by_what_is_in_front() {
        let mut s = state();
        let window = |app_id: &str, unit: Option<&str>| windows::Window {
            id: 1,
            app_id: app_id.into(),
            title: String::new(),
            focused: true,
            unit: unit.map(Into::into),
            workspace: String::new(),
        };
        let route = |s: &mut State, event: Event| {
            s.apply(event);
            s.pad_route
        };
        assert_eq!(route(&mut s, Event::Tick { unix_secs: 1 }), PadRoute::Ui, "nothing known, no game: shelf");
        assert_eq!(route(&mut s, Event::Started("zelda-lttp".into())), PadRoute::Game, "a game runs: in front");
        assert_eq!(route(&mut s, Event::Windows(vec![window(apps::SHELF_APP_ID, None)])), PadRoute::Ui, "shelf in front");
        assert_eq!(route(&mut s, Event::Windows(vec![window("foot", Some("clamshell-app-terminal-1-0.scope"))])), PadRoute::Ui, "an app: the second screen");
        assert_eq!(route(&mut s, Event::Activate(Id::Mode(Mode::Keyboard))), PadRoute::Keys, "an app and the keyboard: typing");
        assert_eq!(route(&mut s, Event::Windows(vec![window("retroarch", Some("clamshell-game-1-0.scope"))])), PadRoute::Game);
        let (_, fx) = s.apply(Event::Windows(vec![window(apps::SHELF_APP_ID, None)]));
        assert_eq!(fx, vec![Effect::PadRoute(PadRoute::Ui)]);
    }

    #[test]
    fn view_moves_the_pad_between_the_second_screen_and_what_wants_it() {
        let mut s = state();
        let foot = windows::Window { id: 1, app_id: "foot".into(), title: String::new(), focused: true, unit: None, workspace: "1".into() };
        s.apply(Event::Windows(vec![foot]));
        s.apply(Event::Activate(Id::Mode(Mode::Keyboard)));
        assert_eq!(s.pad_route, PadRoute::Keys);
        assert_eq!(s.apply(Event::PadSwitch).1, vec![Effect::PadRoute(PadRoute::Ui)], "to the keys on screen");
        assert!(!s.pad_on_top());
        assert_eq!(s.apply(Event::PadSwitch).1, vec![Effect::PadRoute(PadRoute::Keys)], "and back to typing");
        // The pad mouse takes it; View moves it to the second screen and back.
        assert_eq!(s.apply(Event::Activate(Id::PadMouse)).1, vec![Effect::PadRoute(PadRoute::Mouse)]);
        assert_eq!(s.apply(Event::PadSwitch).1, vec![Effect::PadRoute(PadRoute::Ui)]);
        assert_eq!(s.apply(Event::PadSwitch).1, vec![Effect::PadRoute(PadRoute::Mouse)]);
        // Nothing else wants it: View does nothing, the pad stays on the second screen.
        s.apply(Event::Activate(Id::PadMouse));
        s.apply(Event::Activate(Id::Mode(Mode::Dock)));
        assert_eq!(s.pad_route, PadRoute::Ui);
        assert_eq!(s.apply(Event::PadSwitch).1, vec![]);
        // Used there, it stays when the keyboard comes up under it.
        s.apply(Event::Focus(Id::Mode(Mode::Dock)));
        s.apply(Event::Activate(Id::Mode(Mode::Keyboard)));
        assert_eq!(s.pad_route, PadRoute::Ui);
    }

    #[test]
    fn home_goes_to_shelf_and_back_to_what_was_in_front() {
        let mut s = state();
        let window = |id: i64, app_id: &str, workspace: &str, focused: bool| windows::Window {
            id,
            app_id: app_id.into(),
            title: String::new(),
            focused,
            unit: None,
            workspace: workspace.into(),
        };
        let shelf = |focused| window(1, apps::SHELF_APP_ID, windows::SHELF_WORKSPACE, focused);
        assert_eq!(s.apply(Event::Home).1, vec![], "nothing before shelf: it stays");
        s.apply(Event::Started("zelda-lttp".into()));
        assert_eq!(s.apply(Event::Home).1, vec![Effect::FocusWorkspace(windows::GAME_WORKSPACE.into())], "no history: the game");
        // A terminal in front, then shelf: home goes back to the terminal.
        let terminal = "clamshell-app-terminal-1-0";
        s.apply(Event::Windows(vec![shelf(false), window(2, "foot", terminal, true)]));
        assert_eq!(s.apply(Event::Home).1, vec![Effect::ShowShelf]);
        s.apply(Event::Windows(vec![shelf(true), window(2, "foot", terminal, false)]));
        assert_eq!(s.apply(Event::Home).1, vec![Effect::FocusWorkspace(terminal.into())]);
        // The terminal closed: back to the game, which was in front before.
        s.apply(Event::Windows(vec![shelf(false), window(3, "retroarch", windows::GAME_WORKSPACE, true)]));
        s.apply(Event::Windows(vec![shelf(true), window(3, "retroarch", windows::GAME_WORKSPACE, false)]));
        assert_eq!(s.apply(Event::Home).1, vec![Effect::FocusWorkspace(windows::GAME_WORKSPACE.into())]);
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
