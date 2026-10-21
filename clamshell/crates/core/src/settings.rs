//! The tweakable settings: a fixed table of settings, each with a short list of values (tapping a tile
//! steps through them). [`Values`] holds one value per setting (the global ones, or a game's effective
//! ones); [`Overrides`] a game's own choices, each replacing the global one where set; [`Settings`] all
//! of them, with the environment variables.

use std::collections::HashMap;
use std::fmt::Write;
use std::sync::Arc;

use crate::editor::parse_env;
use crate::names::names;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Setting {
    /// The power profile: 0 Battery, 1 Balanced, 2 Performance (CPU and GPU frequency caps).
    Power,
    /// Frames per second at most (MangoHud's limiter); 0: no cap.
    FpsCap,
    /// The main screen's refresh rate in Hz; 0: as it is.
    Refresh,
    /// MangoHud's overlay: 0 off, 1 the frame rate only, 2 the full overlay.
    Hud,
    /// Rendering below the screen's resolution through gamescope: 0 native, 1 FSR from 720p, 2 FSR from
    /// 540p, 3 integer scaling.
    Upscale,
    /// The second screen while a game runs: 0 on, 1 dark (until tapped).
    BottomScreen,
    /// The big CPU cores: 1 on, 0 off (the little ones only).
    BigCores,
    /// The fan: 0 auto, 1 quiet, 2 max.
    Fan,
    /// The pad mouse at launch: 0 off, 1 on.
    PadMouse,
    /// The keymap (see [`crate::keymap`]), by its id; 0: Default, no remapping. Chosen in its picker
    /// (any id is kept; one that is gone counts as Default).
    Keymap,
    /// Rumble: 0 off up to 3 full.
    Rumble,
    /// The CPU's frequency cap in percent of its range; 0: the power profile's.
    CpuMax,
    /// The GPU's frequency cap in percent of its range; 0: the power profile's.
    GpuMax,
    /// Charging stops at this percentage (again 5 below it: `clamshell_system::tuning::Hold`); 0: bypass,
    /// the battery left alone while the charger powers the device (global only: battery care).
    ChargeLimit,
    /// The screens go off after this many minutes without input; 0: never (global only).
    ScreenOff,
    /// The device sleeps after this many minutes without input; 0: never (global only).
    Sleep,
    /// What a program does while none of its windows is shown: see [`InBackground`].
    Background,
}

/// [`Setting::Background`]'s values (see [`crate::background`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InBackground {
    Runs,
    /// Paused (frozen) once hidden for the grace.
    Pauses,
    /// Ended once its last window has closed.
    Quits,
    /// The default: Pauses for a game, else Runs (see [`crate::State::background_of`]).
    PausesGames,
}

impl InBackground {
    pub const fn value(self) -> u16 {
        match self {
            InBackground::Runs => 0,
            InBackground::Pauses => 1,
            InBackground::Quits => 2,
            InBackground::PausesGames => 3,
        }
    }

    pub fn of(value: u16) -> InBackground {
        match value {
            1 => InBackground::Pauses,
            2 => InBackground::Quits,
            3 => InBackground::PausesGames,
            _ => InBackground::Runs,
        }
    }

    fn label(self) -> &'static str {
        match self {
            InBackground::Runs => "Runs hidden",
            InBackground::Pauses => "Pauses hidden",
            InBackground::Quits => "Quits closed",
            InBackground::PausesGames => "Pauses games",
        }
    }
}

pub const COUNT: usize = 17;

/// What a value does, for the tile's colour: nothing (the stock value), saves power or eases the load,
/// costs power, or changes how the game looks or plays.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Impact {
    Stock,
    Saves,
    Costs,
    Changes,
}

// (In the enum's order: a setting's index is its place. The name is its key in the settings file.)
names!(Setting {
    Power: "power";
    FpsCap: "fps";
    Refresh: "refresh";
    Hud: "hud";
    Upscale: "upscale";
    BottomScreen: "bottom";
    BigCores: "bigcores";
    Fan: "fan";
    PadMouse: "padmouse";
    Keymap: "keymap";
    Rumble: "rumble";
    CpuMax: "cpumax";
    GpuMax: "gpumax";
    ChargeLimit: "charge";
    ScreenOff: "screenoff";
    Sleep: "sleep";
    Background: "background";
});

const _: () = assert!(Setting::ALL.len() == COUNT, "every setting in the list");

impl Setting {
    fn index(self) -> usize {
        self as usize
    }

    /// Whether `value` is one it takes: one of its choices (any keymap's id, for the keymap).
    pub fn accepts(self, value: u16) -> bool {
        self == Setting::Keymap || self.choices().contains(&value)
    }

    /// The values a tap steps through, in order.
    pub fn choices(self) -> &'static [u16] {
        match self {
            Setting::Power | Setting::Hud | Setting::Fan => &[0, 1, 2],
            Setting::Background => {
                &const { [InBackground::PausesGames.value(), InBackground::Runs.value(), InBackground::Pauses.value(), InBackground::Quits.value()] }
            }
            Setting::FpsCap => &[0, 30, 40, 45, 60, 90, 120],
            Setting::Refresh => &[0, 60, 90, 120],
            Setting::Upscale | Setting::Rumble => &[0, 1, 2, 3],
            Setting::BottomScreen | Setting::BigCores | Setting::PadMouse => &[0, 1],
            // (The others are the keymaps there are.)
            Setting::Keymap => &[0],
            Setting::CpuMax | Setting::GpuMax => &[0, 40, 50, 60, 70, 80, 90, 100],
            Setting::ChargeLimit => &[100, 90, 80, 0],
            Setting::ScreenOff => &[1, 2, 5, 10, 0],
            Setting::Sleep => &[5, 15, 30, 60, 0],
        }
    }

    pub fn default_value(self) -> u16 {
        match self {
            Setting::Power | Setting::BigCores => 1,
            Setting::Rumble => 3,
            Setting::ChargeLimit => 100,
            Setting::ScreenOff => 5,
            Setting::Sleep => 15,
            Setting::Background => InBackground::PausesGames.value(),
            _ => 0,
        }
    }

    /// Whether changing it does something yet (the rest are kept, and work once implemented).
    pub fn active(self) -> bool {
        !matches!(self, Setting::Upscale)
    }

    /// Whether it works through the launch's own process (MangoHud's cap and HUD): an app the launch
    /// hands over to another program (see [`crate::State::handed_over`]) doesn't get it.
    pub fn through_launch(self) -> bool {
        matches!(self, Setting::FpsCap | Setting::Hud)
    }

    /// Only for all games (not per game).
    pub fn global_only(self) -> bool {
        matches!(self, Setting::ChargeLimit | Setting::ScreenOff | Setting::Sleep)
    }

    /// What `value` does.
    pub fn impact(self, value: u16) -> Impact {
        if value == self.default_value() {
            return Impact::Stock;
        }
        match (self, value) {
            (Setting::Power, 2) | (Setting::Refresh, 120) | (Setting::Fan, 2) | (Setting::Background, 0) => Impact::Costs,
            (Setting::CpuMax | Setting::GpuMax, 100) => Impact::Costs,
            // Never, or later than by default, costs power; sooner saves it.
            (Setting::ScreenOff | Setting::Sleep, v) if v == 0 || v > self.default_value() => Impact::Costs,
            (Setting::Upscale, 3) | (Setting::Fan, _) => Impact::Changes,
            (Setting::Hud | Setting::PadMouse | Setting::Keymap | Setting::Rumble, _) => Impact::Changes,
            _ => Impact::Saves,
        }
    }

    /// The value after `value` (wrapping).
    pub fn next(self, value: u16) -> u16 {
        let choices = self.choices();
        let i = choices.iter().position(|&c| c == value).map_or(0, |i| (i + 1) % choices.len());
        choices.get(i).copied().unwrap_or(value)
    }

    /// How `value` reads on its tile, written to `buf`.
    pub fn describe(self, value: u16, buf: &mut impl Write) {
        let text = match (self, value) {
            (Setting::Power, 0) => "Battery",
            (Setting::Power, 1) => "Balanced",
            (Setting::Power, _) => "Performance",
            (Setting::FpsCap, 0) => "No cap",
            (Setting::Refresh, 0) => "Hz as is",
            (Setting::Hud, 0) => "HUD off",
            (Setting::Hud, 1) => "HUD FPS",
            (Setting::Hud, _) => "Full HUD",
            (Setting::Upscale, 0) => "Native",
            (Setting::Upscale, 1) => "FSR 720p",
            (Setting::Upscale, 2) => "FSR 540p",
            (Setting::Upscale, _) => "Integer",
            (Setting::BottomScreen, 0) => "Screen on",
            (Setting::BottomScreen, _) => "Dark in game",
            (Setting::BigCores, 0) => "Little cores",
            (Setting::BigCores, _) => "All cores",
            (Setting::Fan, 0) => "Fan auto",
            (Setting::Fan, 1) => "Fan quiet",
            (Setting::Fan, _) => "Fan max",
            (Setting::PadMouse, 0) => "Mouse off",
            (Setting::PadMouse, _) => "Mouse on",
            (Setting::Keymap, 0) => "Default",
            // (The tile shows the keymap's name.)
            (Setting::Keymap, v) => return number(buf, "Keymap ", v, ""),
            (Setting::Rumble, 0) => "Rumble off",
            (Setting::Rumble, 1) => "Rumble low",
            (Setting::Rumble, 2) => "Rumble mid",
            (Setting::Rumble, _) => "Rumble full",
            (Setting::CpuMax, 0) => "CPU auto",
            (Setting::GpuMax, 0) => "GPU auto",
            (Setting::ChargeLimit, 100) => "Charge full",
            (Setting::ChargeLimit, 0) => "Bypass",
            (Setting::ScreenOff, 0) => "Always on",
            (Setting::Sleep, 0) => "No sleep",
            (Setting::Background, v) => InBackground::of(v).label(),
            // Numbers: the value with its unit.
            (Setting::FpsCap, v) => return number(buf, "", v, " fps"),
            (Setting::Refresh, v) => return number(buf, "", v, " Hz"),
            (Setting::CpuMax, v) => return number(buf, "CPU ", v, " %"),
            (Setting::GpuMax, v) => return number(buf, "GPU ", v, " %"),
            (Setting::ChargeLimit, v) => return number(buf, "Charge to ", v, " %"),
            (Setting::ScreenOff, v) => return number(buf, "Off after ", v, " min"),
            (Setting::Sleep, v) => return number(buf, "Sleep after ", v, " min"),
        };
        let _ = buf.write_str(text);
    }
}

fn number(buf: &mut impl Write, before: &str, v: u16, after: &str) {
    let _ = write!(buf, "{before}{v}{after}");
}

/// One value per setting.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Values([u16; COUNT]);

impl Default for Values {
    fn default() -> Self {
        Values(Setting::ALL.map(Setting::default_value))
    }
}

impl Values {
    pub fn get(&self, s: Setting) -> u16 {
        self.0.get(s.index()).copied().unwrap_or_else(|| s.default_value())
    }

    /// The settings that are a choice of two, or of named values, by what they mean.
    pub fn background(&self) -> InBackground {
        InBackground::of(self.get(Setting::Background))
    }

    /// The bottom screen dark while a game is in force.
    pub fn bottom_dark(&self) -> bool {
        self.get(Setting::BottomScreen) == 1
    }

    pub fn pad_mouse(&self) -> bool {
        self.get(Setting::PadMouse) == 1
    }

    /// All cores on (else the big ones off).
    pub fn big_cores(&self) -> bool {
        self.get(Setting::BigCores) != 0
    }

    pub fn set(&mut self, s: Setting, value: u16) {
        if let Some(v) = self.0.get_mut(s.index()) {
            *v = value;
        }
    }

    /// Every value, as overrides (to write them all).
    pub fn all(&self) -> Overrides {
        Overrides(self.0.map(Some))
    }
}

/// A game's own choices: a value per setting, or none to follow the global one.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Overrides([Option<u16>; COUNT]);

impl Overrides {
    pub fn get(&self, s: Setting) -> Option<u16> {
        self.0.get(s.index()).copied().flatten()
    }

    pub fn set(&mut self, s: Setting, value: Option<u16>) {
        if let Some(v) = self.0.get_mut(s.index()) {
            *v = value;
        }
    }

    /// The global values with these on top.
    pub fn over(&self, global: &Values) -> Values {
        let mut values = *global;
        for s in Setting::ALL {
            if let Some(v) = self.get(s) {
                values.set(s, v);
            }
        }
        values
    }

    pub fn is_empty(&self) -> bool {
        self.0.iter().all(Option::is_none)
    }
}

/// Every entry's settings: the global values and environment, and each entry's overrides and
/// environment. Kept a file per settings screen (clamshell-system's `store`).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Settings {
    pub global: Values,
    pub overrides: HashMap<Arc<str>, Overrides>,
    /// Extra environment for every game, and per game (on top), as `KEY=value` words.
    pub global_env: String,
    pub env: HashMap<Arc<str>, String>,
}

impl Settings {
    /// The settings `game` (an entry, or a level: a runner's key) runs with alone: the global ones and its
    /// overrides.
    pub fn of(&self, game: &str) -> Values {
        self.of_in(game, &[])
    }

    /// The settings `game` runs with below `levels` (its runner's: see [`crate::runner_key`]): the global
    /// ones, each level's overrides over them, then its own.
    pub fn of_in(&self, game: &str, levels: &[&str]) -> Values {
        let inherited = levels.iter().fold(self.global, |values, f| self.overrides_of(f).over(&values));
        self.overrides_of(game).over(&inherited)
    }

    pub fn overrides_of(&self, game: &str) -> Overrides {
        self.overrides.get(game).copied().unwrap_or_default()
    }

    /// Sets `setting` for `game` (none: for all games, as are global-only settings) to `value`, or
    /// back to the default (none: the global value for a game, the stock value for all games).
    pub fn set(&mut self, game: Option<&Arc<str>>, setting: Setting, value: Option<u16>) {
        match game {
            Some(id) if !setting.global_only() => {
                let mut o = self.overrides_of(id);
                o.set(setting, value);
                if o.is_empty() {
                    self.overrides.remove(id);
                } else {
                    self.overrides.insert(id.clone(), o);
                }
            }
            _ => self.global.set(setting, value.unwrap_or_else(|| setting.default_value())),
        }
    }

    /// A game's own environment text, or all games' (none).
    pub fn env_text(&self, game: Option<&str>) -> &str {
        match game {
            Some(id) => self.env.get(id).map_or("", |e| e.as_str()),
            None => &self.global_env,
        }
    }

    pub fn set_env(&mut self, game: Option<Arc<str>>, text: &str) {
        let text = text.trim().to_owned();
        match game {
            Some(id) if text.is_empty() => {
                self.env.remove(&id);
            }
            Some(id) => {
                self.env.insert(id, text);
            }
            None => self.global_env = text,
        }
    }

    /// The environment a game gets alone: the global variables, then its own.
    pub fn env_of(&self, game: &str) -> Vec<(String, String)> {
        self.env_in(game, &[])
    }

    /// The environment a game below `levels` gets: the global variables, each level's, then its own
    /// (later ones win).
    pub fn env_in(&self, game: &str, levels: &[&str]) -> Vec<(String, String)> {
        let mut vars = parse_env(&self.global_env);
        for f in levels {
            vars.extend(parse_env(self.env_text(Some(f))));
        }
        vars.extend(parse_env(self.env_text(Some(game))));
        vars
    }

    /// Drops a game's overrides and environment.
    pub fn reset(&mut self, game: &str) {
        self.overrides.remove(game);
        self.env.remove(game);
    }

    /// Whether a game has anything of its own.
    pub fn has_own(&self, game: &str) -> bool {
        self.overrides.contains_key(game) || self.env.contains_key(game)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn taps_step_through_the_values_and_wrap() {
        assert_eq!(Setting::FpsCap.next(0), 30);
        assert_eq!(Setting::FpsCap.next(120), 0);
        assert_eq!(Setting::FpsCap.next(33), 0, "a value not offered: the first");
        for s in Setting::ALL {
            assert!(s.choices().contains(&s.default_value()), "{s:?}");
            let mut buf = String::new();
            for &v in s.choices() {
                s.describe(v, &mut buf);
                assert!(!buf.is_empty());
            }
        }
    }

    #[test]
    fn values_say_what_they_do() {
        assert_eq!(Setting::Power.impact(1), Impact::Stock);
        assert_eq!(Setting::Power.impact(0), Impact::Saves);
        assert_eq!(Setting::Power.impact(2), Impact::Costs);
        assert_eq!(Setting::FpsCap.impact(40), Impact::Saves);
        assert_eq!(Setting::BigCores.impact(0), Impact::Saves);
        assert_eq!(Setting::CpuMax.impact(60), Impact::Saves);
        assert_eq!(Setting::CpuMax.impact(100), Impact::Costs);
        assert_eq!(Setting::Keymap.impact(1), Impact::Changes);
        assert_eq!(Setting::Fan.impact(1), Impact::Changes);
        assert_eq!(Setting::ChargeLimit.impact(80), Impact::Saves);
        assert_eq!(Setting::ChargeLimit.impact(0), Impact::Saves, "bypass");
    }

    #[test]
    fn an_entrys_own_settings_and_environment() {
        let mut s = Settings::default();
        let zelda: Arc<str> = "zelda".into();
        s.set(None, Setting::FpsCap, Some(60));
        s.set(Some(&zelda), Setting::Power, Some(2));
        s.set(Some(&zelda), Setting::ChargeLimit, Some(80));
        s.set_env(Some("ff6".into()), " A=1 ");
        assert_eq!(s.of("zelda").get(Setting::Power), 2);
        assert_eq!(s.global.get(Setting::ChargeLimit), 80, "global-only settings go global");
        assert_eq!(s.env_of("ff6"), vec![("A".to_owned(), "1".to_owned())]);
        assert!(Setting::FpsCap.accepts(40) && !Setting::FpsCap.accepts(33) && Setting::Keymap.accepts(7));
        // Below levels: the first one's, the next one's over it, its own over both.
        s.set(Some(&"runner:SNES".into()), Setting::FpsCap, Some(30));
        s.set(Some(&"runner:SNES".into()), Setting::Keymap, Some(2));
        s.set(Some(&"runner:SNES RPG".into()), Setting::Keymap, Some(3));
        let ff6 = s.of_in("ff6", &["runner:SNES", "runner:SNES RPG"]);
        assert_eq!((ff6.get(Setting::FpsCap), ff6.get(Setting::Keymap)), (30, 3));
        s.set_env(Some("runner:SNES".into()), "A=2 B=1");
        let vars = |list: &[(&str, &str)]| list.iter().map(|(k, v)| ((*k).to_owned(), (*v).to_owned())).collect::<Vec<_>>();
        assert_eq!(s.env_in("ff6", &["runner:SNES"]), vars(&[("A", "2"), ("B", "1"), ("A", "1")]), "the level's, then its own");
        s.reset("zelda");
        assert!(!s.has_own("zelda"));
        assert_eq!(s.of("zelda"), s.global);
    }
}
