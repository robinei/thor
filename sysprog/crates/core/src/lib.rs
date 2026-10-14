//! The program's state and its rules. No I/O: every change comes in as an [`Event`], and what the outside
//! world must do comes back as [`Effect`]s. The UI and the policy threads only read the state and send
//! events, so all of this is unit-testable.

pub mod keyboard;
pub mod metrics;
pub mod touch;

use std::collections::HashMap;
use std::sync::Arc;

use keyboard::{Key, KeyPos, Keyboard, Stroke};

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

    fn from_label(label: &str) -> Option<Power> {
        Power::ALL.into_iter().find(|p| p.label() == label)
    }
}

/// Frame rate caps offered; 0 is no cap.
pub const FPS_CAPS: [u16; 6] = [0, 30, 40, 60, 90, 120];

/// The settings a game runs with: the global ones, with the game's overrides on top. Applied at launch
/// and live while it runs.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct GameSettings {
    pub power: Power,
    /// Frames per second at most (MangoHud's limiter); 0: no cap.
    pub fps_cap: u16,
}

/// A game's own choices, each overriding the global one where set.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Overrides {
    pub power: Option<Power>,
    pub fps_cap: Option<u16>,
}

impl Overrides {
    pub fn over(self, global: GameSettings) -> GameSettings {
        GameSettings { power: self.power.unwrap_or(global.power), fps_cap: self.fps_cap.unwrap_or(global.fps_cap) }
    }

    pub fn is_empty(self) -> bool {
        self == Overrides::default()
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
    Power(Power),
    Launch,
    /// A frame rate cap choice (0: none).
    FpsCap(u16),
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
    /// Start this game (its launch command or Steam id, and its file) with these settings.
    Launch { game: Arc<str>, settings: GameSettings, launch: Launch, file: Option<std::path::PathBuf> },
    /// The running game's settings changed: apply them now.
    Apply { game: Arc<str>, settings: GameSettings },
    /// The game ended: back to normal (power).
    GameEnded,
    /// The games' settings changed: write them (see [`State::settings_text`]).
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
    /// The settings every game runs with unless it overrides them.
    pub global: GameSettings,
    /// Per-game overrides, by game id.
    pub overrides: HashMap<Arc<str>, Overrides>,
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

/// Seconds after the last move before the place is saved.
const RESUME_AFTER: u64 = 2;

/// Which screen a target is drawn on.
fn screen_of(id: &Id) -> Dirty {
    match id {
        Id::Game(_) | Id::Tab(_) => Dirty::BOTH,
        Id::Mode(_) | Id::Power(_) | Id::FpsCap(_) | Id::Scope(_) | Id::Reset | Id::Launch | Id::Key(_) | Id::PadMouse => Dirty::BOTTOM,
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

    pub fn collection(&self) -> Option<&Collection> {
        self.library.collections.get(self.tab)
    }

    pub fn cursor(&self) -> Cursor {
        self.cursors.get(self.tab).copied().unwrap_or_default()
    }

    /// The game at `position` in the current tab.
    pub fn game_at(&self, position: usize) -> Option<&Game> {
        let index = *self.collection()?.games.get(position)?;
        self.library.games.get(index)
    }

    pub fn selected_game(&self) -> Option<&Game> {
        self.game_at(self.cursor().index)
    }

    /// Where shelf is, to come back to: the current tab's name, then each tab's name and selected
    /// game's id, tab-separated.
    pub fn resume_text(&self) -> String {
        let mut text = self.collection().map(|c| c.name.clone()).unwrap_or_default();
        text.push('\n');
        // The second screen's mode.
        text.push_str("@mode\t");
        text.push_str(self.mode.name());
        text.push('\n');
        for (i, c) in self.library.collections.iter().enumerate() {
            let cursor = self.cursors.get(i).copied().unwrap_or_default();
            if let Some(g) = c.games.get(cursor.index).and_then(|&g| self.library.games.get(g)) {
                text.push_str(&c.name);
                text.push('\t');
                text.push_str(&g.id);
                text.push('\n');
            }
        }
        text
    }

    /// Goes back to the place `resume_text` saved (tabs and games that are gone are skipped).
    pub fn restore(&mut self, text: &str) {
        let mut lines = text.lines();
        let current = lines.next().unwrap_or("");
        for line in lines {
            let Some((name, id)) = line.split_once('\t') else {
                continue;
            };
            if name == "@mode" {
                // Not into touchpad (grabs the touchscreen; only on request).
                if let Some(mode) = Mode::from_name(id).filter(|m| *m != Mode::Touchpad) {
                    self.mode = mode;
                }
                continue;
            }
            let Some(tab) = self.library.collections.iter().position(|c| c.name == name) else {
                continue;
            };
            let position = self.library.collections.get(tab).and_then(|c| {
                c.games.iter().position(|&g| self.library.games.get(g).is_some_and(|g| &*g.id == id))
            });
            if let (Some(p), Some(cursor)) = (position, self.cursors.get_mut(tab)) {
                cursor.index = p;
            }
        }
        if let Some(tab) = self.library.collections.iter().position(|c| c.name == current) {
            self.tab = tab;
        }
        self.sync();
        self.focus = self.selected.clone().map(Id::Game);
        self.resume_dirty = None;
    }

    /// Keeps the cursor in range and on screen, and `selected` in step with it.
    fn sync(&mut self) {
        let before = self.selected.clone();
        let len = self.collection().map_or(0, |c| c.games.len());
        let rows = self.rows.max(1);
        if let Some(c) = self.cursors.get_mut(self.tab) {
            c.index = c.index.min(len.saturating_sub(1));
            if c.index < c.top {
                c.top = c.index;
            } else if c.index >= c.top + rows {
                c.top = c.index + 1 - rows;
            }
            c.top = c.top.min(len.saturating_sub(rows));
        }
        self.selected = self.selected_game().map(|g| g.id.clone());
        if self.selected != before {
            self.resume_dirty = Some(self.unix_secs);
        }
    }

    /// Moves the cursor by `step` games (clamped at the ends), selecting and focusing that game.
    fn step_list(&mut self, step: i32) {
        let len = self.collection().map_or(0, |c| c.games.len());
        if let Some(c) = self.cursors.get_mut(self.tab) {
            let target = c.index as i64 + i64::from(step);
            c.index = target.clamp(0, len.saturating_sub(1) as i64) as usize;
        }
        self.sync();
        self.focus = self.selected.clone().map(Id::Game);
    }

    /// Changes the library (favourites, plays) keeping the current tab, each tab's place and the
    /// selected game, all by name, since derived tabs may come or go.
    fn relayout(&mut self, change: impl FnOnce(&mut Library)) {
        let names: Vec<String> = self.library.collections.iter().map(|c| c.name.clone()).collect();
        let tab_name = names.get(self.tab).cloned();
        let selected = self.selected.clone();
        change(&mut self.library);
        let old = std::mem::take(&mut self.cursors);
        self.cursors = self
            .library
            .collections
            .iter()
            .map(|c| names.iter().position(|n| *n == c.name).and_then(|i| old.get(i).copied()).unwrap_or_default())
            .collect();
        self.tab = tab_name.and_then(|n| self.library.collections.iter().position(|c| c.name == n)).unwrap_or(0);
        if let Some(id) = selected {
            let position = self.collection().and_then(|c| {
                c.games.iter().position(|&i| self.library.games.get(i).is_some_and(|g| g.id == id))
            });
            if let (Some(p), Some(c)) = (position, self.cursors.get_mut(self.tab)) {
                c.index = p;
            }
        }
        self.sync();
    }

    fn set_tab(&mut self, tab: usize) {
        if tab < self.library.collections.len() {
            self.tab = tab;
            self.resume_dirty = Some(self.unix_secs);
            self.sync();
            if matches!(self.focus, Some(Id::Game(_)) | Some(Id::Tab(_))) {
                self.focus = self.selected.clone().map(Id::Game);
            }
        }
    }

    /// The settings `game` runs with: the global ones and its overrides.
    pub fn settings_of(&self, game: &str) -> GameSettings {
        self.overrides_of(game).over(self.global)
    }

    pub fn overrides_of(&self, game: &str) -> Overrides {
        self.overrides.get(game).copied().unwrap_or_default()
    }

    /// The game the settings panel is about: the running one, else shelf's selection.
    pub fn context(&self) -> Option<&Game> {
        let id = self.running.as_ref().or(self.selected.as_ref())?;
        self.library.games.iter().find(|g| &g.id == id)
    }

    /// What the settings panel edits now: the game in context (if the scope is the game's), or none
    /// for the global settings.
    pub fn editing(&self) -> Option<&Game> {
        self.context().filter(|_| self.scope == Scope::Game)
    }

    /// Changes what the panel edits: the game's overrides or the global settings. Saved, and applied
    /// at once to a running game whose settings that changed.
    fn change_settings(&mut self, change: impl FnOnce(&mut Overrides)) -> Vec<Effect> {
        let before = self.running.as_ref().map(|id| self.settings_of(id));
        match self.editing().map(|g| g.id.clone()) {
            Some(id) => {
                let mut o = self.overrides_of(&id);
                change(&mut o);
                if o.is_empty() {
                    self.overrides.remove(&id);
                } else {
                    self.overrides.insert(id, o);
                }
            }
            None => {
                let mut o = Overrides::default();
                change(&mut o);
                self.global = o.over(self.global);
            }
        }
        let mut effects = vec![Effect::SaveSettings(self.settings_text())];
        if let Some(id) = self.running.clone() {
            let settings = self.settings_of(&id);
            if before != Some(settings) {
                effects.push(Effect::Apply { game: id, settings });
            }
        }
        effects
    }

    /// The settings as text: a `*` line with the global power and fps cap, then one line per game with
    /// overrides (empty fields: not overridden), tab-separated.
    pub fn settings_text(&self) -> String {
        let field = |v: Option<String>| v.unwrap_or_default();
        let mut lines: Vec<String> = self
            .overrides
            .iter()
            .filter(|(_, o)| !o.is_empty())
            .map(|(id, o)| {
                format!("{id}\t{}\t{}", field(o.power.map(|p| p.label().to_owned())), field(o.fps_cap.map(|c| c.to_string())))
            })
            .collect();
        lines.sort();
        let mut text = format!("*\t{}\t{}\n", self.global.power.label(), self.global.fps_cap);
        for l in lines {
            text.push_str(&l);
            text.push('\n');
        }
        text
    }

    pub fn load_settings(&mut self, text: &str) {
        for line in text.lines() {
            let mut fields = line.split('\t');
            let (Some(id), Some(power), Some(fps)) = (fields.next(), fields.next(), fields.next()) else {
                continue;
            };
            let o = Overrides { power: Power::from_label(power), fps_cap: fps.parse().ok() };
            if id == "*" {
                self.global = o.over(GameSettings::default());
            } else if !o.is_empty() {
                self.overrides.insert(id.into(), o);
            }
        }
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
                    Some(since) if unix_secs >= since + RESUME_AFTER => {
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
                (Dirty::BOTH, vec![Effect::GameEnded])
            }
            Event::LaunchFailed(game) => {
                if self.running.as_ref() == Some(&game) {
                    self.running = None;
                }
                (Dirty::BOTH, vec![Effect::GameEnded])
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
            Id::Power(power) => (Dirty::BOTH, self.change_settings(|o| o.power = Some(power))),
            Id::FpsCap(cap) => (Dirty::BOTH, self.change_settings(|o| o.fps_cap = Some(cap))),
            Id::Scope(scope) => {
                self.scope = scope;
                (Dirty::BOTTOM, vec![])
            }
            Id::Reset => (Dirty::BOTH, self.change_settings(|o| *o = Overrides::default())),
            Id::Launch => match self.selected.clone() {
                Some(game) => (Dirty::BOTH, self.launch_effects(game)),
                None => (Dirty::BOTH, vec![]),
            },
            Id::PadMouse => {
                self.pad_mouse = !self.pad_mouse;
                (Dirty::BOTTOM, vec![Effect::PadMouse(self.pad_mouse)])
            }
            Id::Key(pos) => {
                let effects = self.keyboard.press(pos, long).map(Effect::Key).into_iter().collect();
                (Dirty::BOTTOM, effects)
            }
        }
    }

    /// Launching `game`, unless one is already running.
    fn launch_effects(&self, game: Arc<str>) -> Vec<Effect> {
        if self.running.is_some() {
            return vec![];
        }
        let Some(g) = self.library.games.iter().find(|g| g.id == game) else {
            return vec![];
        };
        let settings = self.settings_of(&game);
        vec![
            Effect::SaveResume(self.resume_text()),
            Effect::Launch { game, settings, launch: g.launch.clone(), file: g.file.clone() },
        ]
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::indexing_slicing)]
mod tests {
    use super::*;

    fn state() -> State {
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
        assert_eq!(fx.last(), Some(&Effect::Launch { game: "ff6".into(), settings: GameSettings::default(), launch: Launch::None, file: None }));
    }

    #[test]
    fn games_override_the_global_settings() {
        let mut s = state();
        s.apply(Event::Activate(Id::Scope(Scope::Global)));
        s.apply(Event::Activate(Id::FpsCap(60)));
        s.apply(Event::Activate(Id::Scope(Scope::Game)));
        let (_, fx) = s.apply(Event::Activate(Id::Power(Power::Performance)));
        let text = "*\tBalanced\t60\nzelda-lttp\tPerformance\t\n";
        assert!(matches!(fx.first(), Some(Effect::SaveSettings(t)) if t == text), "{fx:?}");
        assert_eq!(s.settings_of("zelda-lttp"), GameSettings { power: Power::Performance, fps_cap: 60 });
        assert_eq!(s.settings_of("ff6"), GameSettings { power: Power::Balanced, fps_cap: 60 });
        let mut fresh = state();
        fresh.load_settings(text);
        assert_eq!(fresh.settings_of("zelda-lttp"), s.settings_of("zelda-lttp"));
        assert_eq!(fresh.global, s.global);
        // Reset drops the game's overrides: back to the global settings.
        s.apply(Event::Activate(Id::Reset));
        assert!(s.overrides.is_empty());
        assert_eq!(s.settings_of("zelda-lttp"), s.global);
    }

    #[test]
    fn the_launch_carries_the_games_settings() {
        let mut s = state();
        s.apply(Event::Activate(Id::FpsCap(40)));
        let (_, fx) = s.apply(Event::Activate(Id::Launch));
        assert!(matches!(fx.last(), Some(Effect::Launch { settings, .. }) if settings.fps_cap == 40));
    }

    #[test]
    fn a_running_game_takes_changes_at_once() {
        let mut s = state();
        s.apply(Event::Started("zelda-lttp".into()));
        s.apply(Event::Activate(Id::Game("ff6".into())));
        // The running game is the context, not the selection.
        let (_, fx) = s.apply(Event::Activate(Id::FpsCap(30)));
        assert_eq!(fx.last(), Some(&Effect::Apply { game: "zelda-lttp".into(), settings: GameSettings { fps_cap: 30, ..GameSettings::default() } }));
        // A global change reaches it only where it doesn't override.
        s.apply(Event::Activate(Id::Scope(Scope::Global)));
        let (_, fx) = s.apply(Event::Activate(Id::FpsCap(90)));
        assert_eq!(fx.len(), 1, "its own cap stays: nothing to apply");
        let (_, fx) = s.apply(Event::Activate(Id::Power(Power::Battery)));
        assert!(matches!(fx.last(), Some(Effect::Apply { settings, .. }) if settings.power == Power::Battery && settings.fps_cap == 30));
        let (_, fx) = s.apply(Event::Played { game: "zelda-lttp".into(), start: 0, duration: 1 });
        assert_eq!(fx, vec![Effect::GameEnded]);
    }

    #[test]
    fn without_a_game_the_panel_edits_the_global_settings() {
        let mut empty = State::new(Library::default());
        empty.apply(Event::Activate(Id::Mode(Mode::Settings)));
        assert_eq!(empty.mode, Mode::Settings, "always available");
        empty.apply(Event::Activate(Id::FpsCap(40)));
        assert_eq!(empty.global.fps_cap, 40);
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
    fn the_list_scrolls_to_keep_the_selection_on_screen() {
        let mut s = state();
        s.apply(Event::Rows(3));
        s.apply(Event::ListStep(4));
        assert_eq!(s.cursor(), Cursor { index: 4, top: 2 });
        assert_eq!(s.selected.as_deref(), Some("dark-souls"));
        assert_eq!(s.focus, Some(Id::Game("dark-souls".into())));
        s.apply(Event::ListStep(-100));
        assert_eq!(s.cursor(), Cursor { index: 0, top: 0 });
        s.apply(Event::ListStep(100));
        assert_eq!(s.cursor().index, 6);
    }

    #[test]
    fn tabs_keep_their_own_place() {
        let mut s = state();
        s.apply(Event::ListStep(2));
        s.apply(Event::TabStep(1));
        assert_eq!(s.tab, 1);
        assert_eq!(s.cursor().index, 0);
        s.apply(Event::TabStep(1));
        assert_eq!(s.tab, 0, "wraps around");
        assert_eq!(s.cursor().index, 2);
    }

    #[test]
    fn a_favourite_shows_in_its_tab_and_is_saved() {
        let mut s = state();
        s.apply(Event::ListStep(2));
        let tab_name = s.collection().map(|c| c.name.clone());
        let (_, fx) = s.apply(Event::ToggleFavourite);
        assert_eq!(fx, vec![Effect::SaveFavourites(vec!["mk-ds".into()])]);
        assert!(s.library.collections.iter().any(|c| c.name == "Favourites" && c.games == vec![2]));
        assert_eq!(s.collection().map(|c| c.name.clone()), tab_name, "the tab stays");
        assert_eq!(s.selected.as_deref(), Some("mk-ds"), "so does the selection");
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
    fn the_place_is_saved_after_a_pause_and_restored() {
        let mut s = state();
        s.apply(Event::Tick { unix_secs: 100 });
        s.apply(Event::ListStep(3));
        s.apply(Event::TabStep(1));
        assert!(s.apply(Event::Tick { unix_secs: 101 }).1.is_empty(), "not while moving");
        let (_, fx) = s.apply(Event::Tick { unix_secs: 103 });
        let text = match fx.first() {
            Some(Effect::SaveResume(text)) => text.clone(),
            other => format!("{other:?}"),
        };
        assert_eq!(text, "More\n@mode\tsysinfo\nGames\tff6\nMore\tff6\n");
        let mut fresh = state();
        fresh.restore(&text);
        assert_eq!(fresh.collection().map(|c| c.name.as_str()), Some("More"));
        fresh.apply(Event::TabStep(-1));
        assert_eq!(fresh.selected.as_deref(), Some("ff6"));
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
