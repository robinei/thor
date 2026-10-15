//! The tweakable settings: a fixed table of settings, each with a short list of values (tapping a tile
//! steps through them). [`Values`] holds one value per setting (the global ones, or a game's effective
//! ones); [`Overrides`] a game's own choices, each replacing the global one where set; [`Settings`] all
//! of them, with the environment variables, and their file format.

use std::collections::HashMap;
use std::fmt::Write;
use std::sync::Arc;

use crate::editor::parse_env;

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
    /// The pad's face buttons: 0 Xbox layout, 1 Nintendo layout (A/B and X/Y swapped).
    Buttons,
    /// The sticks' deadzone, in percent.
    Deadzone,
    /// Rumble: 0 off up to 3 full.
    Rumble,
    /// The CPU's frequency cap in percent of its range; 0: the power profile's.
    CpuMax,
    /// The GPU's frequency cap in percent of its range; 0: the power profile's.
    GpuMax,
    /// Charging stops at this percentage (global only: battery care).
    ChargeLimit,
    /// The screens go off after this many minutes without input; 0: never (global only).
    ScreenOff,
    /// The device sleeps after this many minutes without input; 0: never (global only).
    Sleep,
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

impl Setting {
    pub const ALL: [Setting; COUNT] = [
        Setting::Power,
        Setting::FpsCap,
        Setting::Refresh,
        Setting::Hud,
        Setting::Upscale,
        Setting::BottomScreen,
        Setting::BigCores,
        Setting::Fan,
        Setting::PadMouse,
        Setting::Buttons,
        Setting::Deadzone,
        Setting::Rumble,
        Setting::CpuMax,
        Setting::GpuMax,
        Setting::ChargeLimit,
        Setting::ScreenOff,
        Setting::Sleep,
    ];

    fn index(self) -> usize {
        self as usize
    }

    /// Its name in the settings file.
    pub fn key(self) -> &'static str {
        match self {
            Setting::Power => "power",
            Setting::FpsCap => "fps",
            Setting::Refresh => "refresh",
            Setting::Hud => "hud",
            Setting::Upscale => "upscale",
            Setting::BottomScreen => "bottom",
            Setting::BigCores => "bigcores",
            Setting::Fan => "fan",
            Setting::PadMouse => "padmouse",
            Setting::Buttons => "buttons",
            Setting::Deadzone => "deadzone",
            Setting::Rumble => "rumble",
            Setting::CpuMax => "cpumax",
            Setting::GpuMax => "gpumax",
            Setting::ChargeLimit => "charge",
            Setting::ScreenOff => "screenoff",
            Setting::Sleep => "sleep",
        }
    }

    /// The values a tap steps through, in order.
    pub fn choices(self) -> &'static [u16] {
        match self {
            Setting::Power | Setting::Hud | Setting::Fan => &[0, 1, 2],
            Setting::FpsCap => &[0, 30, 40, 45, 60, 90, 120],
            Setting::Refresh => &[0, 60, 90, 120],
            Setting::Upscale | Setting::Rumble => &[0, 1, 2, 3],
            Setting::BottomScreen | Setting::BigCores | Setting::PadMouse | Setting::Buttons => &[0, 1],
            Setting::Deadzone => &[0, 4, 8, 12, 16, 20],
            Setting::CpuMax | Setting::GpuMax => &[0, 40, 50, 60, 70, 80, 90, 100],
            Setting::ChargeLimit => &[100, 90, 80],
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
            _ => 0,
        }
    }

    /// Whether changing it does something yet (the rest are kept, and work once implemented).
    pub fn active(self) -> bool {
        !matches!(self, Setting::Upscale | Setting::Fan | Setting::Rumble)
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
            (Setting::Power, 2) | (Setting::Refresh, 120) | (Setting::Fan, 2) => Impact::Costs,
            (Setting::CpuMax | Setting::GpuMax, 100) => Impact::Costs,
            // Never, or later than by default, costs power; sooner saves it.
            (Setting::ScreenOff | Setting::Sleep, v) if v == 0 || v > self.default_value() => Impact::Costs,
            (Setting::Upscale, 3) | (Setting::Fan, _) => Impact::Changes,
            (Setting::Hud | Setting::PadMouse | Setting::Buttons | Setting::Deadzone | Setting::Rumble, _) => Impact::Changes,
            _ => Impact::Saves,
        }
    }

    /// The value after `value` (wrapping).
    pub fn next(self, value: u16) -> u16 {
        let choices = self.choices();
        let i = choices.iter().position(|&c| c == value).map_or(0, |i| (i + 1) % choices.len());
        choices.get(i).copied().unwrap_or(value)
    }

    /// How `value` reads on its tile.
    pub fn describe(self, value: u16, buf: &mut String) {
        buf.clear();
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
            (Setting::Buttons, 0) => "Xbox",
            (Setting::Buttons, _) => "Nintendo",
            (Setting::Deadzone, 0) => "No deadzone",
            (Setting::Rumble, 0) => "Rumble off",
            (Setting::Rumble, 1) => "Rumble low",
            (Setting::Rumble, 2) => "Rumble mid",
            (Setting::Rumble, _) => "Rumble full",
            (Setting::CpuMax, 0) => "CPU auto",
            (Setting::GpuMax, 0) => "GPU auto",
            (Setting::ChargeLimit, 100) => "Charge full",
            (Setting::ScreenOff, 0) => "Always on",
            (Setting::Sleep, 0) => "No sleep",
            // Numbers: the value with its unit.
            (Setting::FpsCap, v) => return number(buf, "", v, " fps"),
            (Setting::Refresh, v) => return number(buf, "", v, " Hz"),
            (Setting::Deadzone, v) => return number(buf, "Deadzone ", v, " %"),
            (Setting::CpuMax, v) => return number(buf, "CPU ", v, " %"),
            (Setting::GpuMax, v) => return number(buf, "GPU ", v, " %"),
            (Setting::ChargeLimit, v) => return number(buf, "Charge to ", v, " %"),
            (Setting::ScreenOff, v) => return number(buf, "Off after ", v, " min"),
            (Setting::Sleep, v) => return number(buf, "Sleep ", v, " min"),
        };
        buf.push_str(text);
    }
}

fn number(buf: &mut String, before: &str, v: u16, after: &str) {
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

/// `key=value` fields, tab-separated, for the settings file.
pub fn fields(o: &Overrides, out: &mut String) {
    for s in Setting::ALL {
        if let Some(v) = o.get(s) {
            let _ = write!(out, "\t{}={v}", s.key());
        }
    }
}

/// Reads `key=value` fields back (unknown keys and values not offered are skipped).
pub fn parse<'a>(fields: impl Iterator<Item = &'a str>) -> Overrides {
    let mut o = Overrides::default();
    for field in fields {
        let Some((key, value)) = field.split_once('=') else {
            continue;
        };
        let Some(s) = Setting::ALL.into_iter().find(|s| s.key() == key) else {
            continue;
        };
        if let Ok(v) = value.parse::<u16>()
            && s.choices().contains(&v)
        {
            o.set(s, Some(v));
        }
    }
    o
}

/// Every game's settings: the global values and environment, and each game's overrides and
/// environment. Saved as text ([`Settings::text`]).
#[derive(Debug, Clone, Default)]
pub struct Settings {
    pub global: Values,
    pub overrides: HashMap<Arc<str>, Overrides>,
    /// Extra environment for every game, and per game (on top), as `KEY=value` words.
    pub global_env: String,
    pub env: HashMap<Arc<str>, String>,
}

impl Settings {
    /// The settings `game` runs with: the global ones and its overrides.
    pub fn of(&self, game: &str) -> Values {
        self.overrides_of(game).over(&self.global)
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

    /// The environment a game gets: the global variables, then its own.
    pub fn env_of(&self, game: &str) -> Vec<(String, String)> {
        let mut vars = parse_env(&self.global_env);
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

    /// As text: a `*` line with the global values, then a line per game with its overrides, as
    /// tab-separated `key=value` fields; environment as an `env=` field (last on the line).
    pub fn text(&self) -> String {
        let mut text = String::from("*");
        fields(&self.global.all(), &mut text);
        push_env(&mut text, &self.global_env);
        text.push('\n');
        let mut ids: Vec<&Arc<str>> = self.overrides.keys().chain(self.env.keys()).collect();
        ids.sort();
        ids.dedup();
        for id in ids {
            text.push_str(id);
            fields(&self.overrides_of(id), &mut text);
            push_env(&mut text, self.env_text(Some(id)));
            text.push('\n');
        }
        text
    }

    /// Reads [`Settings::text`] back (unknown fields are skipped).
    pub fn load(text: &str) -> Settings {
        let mut s = Settings::default();
        for line in text.lines() {
            let (line, env) = line.split_once("\tenv=").unwrap_or((line, ""));
            let mut fields = line.split('\t');
            let Some(id) = fields.next() else {
                continue;
            };
            let o = parse(fields);
            if id == "*" {
                s.global = o.over(&Values::default());
                s.global_env = env.to_owned();
            } else {
                if !o.is_empty() {
                    s.overrides.insert(id.into(), o);
                }
                s.set_env(Some(id.into()), env);
            }
        }
        s
    }
}

/// Appends `env` as the line's last field (`env=...`), if any.
fn push_env(text: &mut String, env: &str) {
    if !env.trim().is_empty() {
        text.push_str("\tenv=");
        text.push_str(env.trim());
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
        assert_eq!(Setting::Buttons.impact(1), Impact::Changes);
        assert_eq!(Setting::Fan.impact(1), Impact::Changes);
        assert_eq!(Setting::ChargeLimit.impact(80), Impact::Saves);
    }

    #[test]
    fn settings_round_trip_as_text() {
        let mut s = Settings::default();
        let zelda: Arc<str> = "zelda".into();
        s.set(None, Setting::FpsCap, Some(60));
        s.set(Some(&zelda), Setting::Power, Some(2));
        s.set(Some(&zelda), Setting::ChargeLimit, Some(80));
        s.set_env(Some("ff6".into()), " A=1 ");
        let text = s.text();
        assert!(text.ends_with("\tcharge=80\tscreenoff=5\tsleep=15\nff6\tenv=A=1\nzelda\tpower=2\n"), "{text}");
        let back = Settings::load(&text);
        assert_eq!(back.of("zelda"), s.of("zelda"));
        assert_eq!(back.global, s.global, "global-only settings go global");
        assert_eq!(back.env_of("ff6"), vec![("A".to_owned(), "1".to_owned())]);
        s.reset("zelda");
        assert!(!s.has_own("zelda"));
        assert_eq!(s.of("zelda"), s.global);
    }

    #[test]
    fn overrides_round_trip_as_fields() {
        let mut o = Overrides::default();
        o.set(Setting::FpsCap, Some(40));
        o.set(Setting::Buttons, Some(1));
        let mut text = String::new();
        fields(&o, &mut text);
        assert_eq!(text, "\tfps=40\tbuttons=1");
        assert_eq!(parse(text.split('\t')), o);
        assert_eq!(o.over(&Values::default()).get(Setting::FpsCap), 40);
        assert_eq!(o.over(&Values::default()).get(Setting::Power), 1);
    }
}
