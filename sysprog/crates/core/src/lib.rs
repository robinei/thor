//! The program's state and its rules. No I/O: every change comes in as an [`Event`], and what the outside
//! world must do comes back as [`Effect`]s. The UI and the policy threads only read the state and send
//! events, so all of this is unit-testable.

pub mod keyboard;
pub mod editor;
pub mod metrics;
mod panel;
mod resume;
pub mod settings;
mod shelf;
pub mod touch;

use std::sync::Arc;

use keyboard::{Key, KeyPos, Keyboard, Stroke};
use settings::{Setting, Settings, Values};

pub use editor::{Editor, count_env, parse_env};

pub use sysprog_library::{Collection, Game, Launch, Library};

/// Second-screen modes (docs/plans.md, "Second screen"), chosen from the bar; the last one chosen stays.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum Mode {
    Settings,
    Launcher,
    Keyboard,
    Touchpad,
    #[default]
    Sysinfo,
}

impl Mode {
    /// The modes in the bar, in order.
    pub const BAR: [Mode; 5] = [Mode::Settings, Mode::Launcher, Mode::Keyboard, Mode::Touchpad, Mode::Sysinfo];

    pub fn name(self) -> &'static str {
        match self {
            Mode::Settings => "settings",
            Mode::Launcher => "launcher",
            Mode::Keyboard => "keyboard",
            Mode::Touchpad => "touchpad",
            Mode::Sysinfo => "sysinfo",
        }
    }

    pub fn from_name(name: &str) -> Option<Mode> {
        Mode::BAR.into_iter().find(|m| m.name() == name)
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
    EnvSave,
    EnvCancel,
    /// The dark second screen (while a game runs): a tap anywhere lights it.
    Wake,
    /// The settings panel's scope tabs.
    Scope(Scope),
    /// Drops the game in context's overrides.
    Reset,
    /// An on-screen keyboard key, by position.
    Key(KeyPos),
    /// The pad mouse toggle.
    PadMouse,
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
    /// The pad mouse was turned on by a launch (Setting::PadMouse), to turn off when the game ends.
    pad_mouse_by_launch: bool,
    /// What the settings panel edits (global whenever there is no game in context).
    pub scope: Scope,
    pub mode: Mode,
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
        Id::Mode(_) | Id::Tile(_) | Id::Env | Id::EnvSave | Id::EnvCancel | Id::Wake | Id::Scope(_) | Id::Reset | Id::Launch | Id::Key(_) | Id::PadMouse => Dirty::BOTTOM,
    }
}

impl State {
    pub fn new(library: Library) -> Self {
        let cursors = vec![Cursor::default(); library.collections.len()];
        let mut s = State { library, cursors, rows: 1, ..State::default() };
        s.sync();
        s.focus = s.selected.clone().map(Id::Game);
        s
    }

    /// Applies one event; returns which screens changed and what must happen outside.
    pub fn apply(&mut self, event: Event) -> (Dirty, Vec<Effect>) {
        match event {
            Event::Tick { unix_secs } => {
                // Once a minute: the bar's clock, and both screens' burn-in orbit step.
                let minute_changed = unix_secs / 60 != self.unix_secs / 60;
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
                // Only the sysinfo mode shows them.
                (if self.mode == Mode::Sysinfo { Dirty::BOTTOM } else { Dirty::NONE }, vec![])
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
                (screen_of(&id), vec![])
            }
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
            Id::Mode(mode) => {
                let before = self.mode;
                self.mode = mode;
                self.resume_dirty = Some(self.unix_secs);
                let touchpad = |m: Mode| m == Mode::Touchpad;
                let effects = if touchpad(before) != touchpad(self.mode) {
                    vec![Effect::Touchpad(touchpad(self.mode))]
                } else {
                    vec![]
                };
                (Dirty::BOTH, effects)
            }
            Id::Tile(setting) => {
                let value = if long { None } else { Some(setting.next(self.tile_value(setting))) };
                (Dirty::BOTH, self.change(setting, value))
            }
            Id::Env => {
                self.open_editor();
                (Dirty::BOTTOM, vec![])
            }
            Id::EnvSave => (Dirty::BOTTOM, self.save_env()),
            Id::EnvCancel => {
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
            Id::PadMouse => {
                self.pad_mouse = !self.pad_mouse;
                (Dirty::BOTTOM, vec![Effect::PadMouse(self.pad_mouse)])
            }
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
    fn launch_effects(&self, game: Arc<str>) -> Vec<Effect> {
        if self.running.is_some() {
            return vec![];
        }
        let Some(g) = self.library.games.iter().find(|g| g.id == game) else {
            return vec![];
        };
        let settings = self.settings.of(&game);
        let env = self.settings.env_of(&game);
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
        State::new(sysprog_library::demo_library())
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
    fn tick_redraws_once_a_minute() {
        let mut s = state();
        let (dirty, _) = s.apply(Event::Tick { unix_secs: 60 });
        assert_eq!(dirty, Dirty::BOTH);
        let (dirty, _) = s.apply(Event::Tick { unix_secs: 119 });
        assert_eq!(dirty, Dirty::NONE);
    }
}
