//! The tweakable settings: a fixed table of settings, each with a short list of values that tapping its
//! tile steps through. [`Values`] holds one value per setting, either the global ones or a game's
//! effective ones. [`Overrides`] holds a game's own choices, each replacing the global one where set.
//! [`Settings`] holds all of them, along with the environment variables.

pub mod env;
pub mod fan;
pub(crate) mod panel;

use std::collections::HashMap;
use std::fmt::Write;
use std::sync::Arc;

use crate::kit::names::names;
use env::parse_env;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Setting {
    /// The power profile, which sets CPU and GPU frequency caps: 0 Battery, 1 Balanced, 2 Performance.
    Power,
    /// The most frames per second: the compositor paces the window's frames, or under sway MangoHud's
    /// limiter does. 0 means no cap.
    FpsCap,
    /// The main screen's refresh rate in Hz. 0 leaves it as it is.
    Refresh,
    /// The HUD over the game: 0 off, 1 the frame rate only, 2 the full HUD (the compositor's: the frame
    /// rate and frame time; under sway, MangoHud's overlay).
    Hud,
    /// The screen as the game sees it (see [`Resolution`]): the screen's own size or a fraction of it, which
    /// [`Setting::Scaling`] shows at the screen's size. The compositor's alone (`docs/picture.md`).
    Resolution,
    /// How the game's picture is shown when it is smaller than its screen (see [`Scaling`]).
    Scaling,
    /// Anti-aliasing of the game's picture by the compositor (see [`AntiAliasing`]).
    AntiAliasing,
    /// Sharpening of the game's picture by the compositor, in percent: 0 off, 25 to 100 its strength. CAS,
    /// or FSR's own with [`Scaling::Fsr`].
    Sharpen,
    /// The HDR curve by its id (see [`crate::curve`]): SDR shown with HDR's range. 0 is Off. It is chosen
    /// in the curve picker, and it works only under ClamShell's compositor.
    Curve,
    /// The second screen: 0 on but dimmed when untouched, 1 dark while a game runs until tapped, 2 off
    /// when idle. With 1 and 2 it is powered off on battery (see [`crate::bottom::screen`]).
    BottomScreen,
    /// The big CPU cores: 1 on, 0 off, leaving only the little ones.
    BigCores,
    /// The fan: 0 auto, 1 quiet, 2 max.
    Fan,
    /// The pad mouse at launch: 0 off, 1 on.
    PadMouse,
    /// The keymap by its id (see [`crate::keymap`]). 0 is Default, with no remapping. It is chosen in
    /// the keymap picker. Any id is kept, and one whose keymap is gone counts as Default.
    Keymap,
    /// Rumble strength, from 0 (off) up to 3 (full).
    Rumble,
    /// The CPU's frequency cap as a percent of its range. 0 uses the power profile's.
    CpuMax,
    /// The GPU's frequency cap as a percent of its range. 0 uses the power profile's.
    GpuMax,
    /// Charging stops at this percentage, and starts again 5 below it
    /// (`clamshell_system::power::tuning::Hold`). 0 means bypass: the battery is left alone while the
    /// charger powers the device. Global only, since it is for battery care.
    ChargeLimit,
    /// The screens go off after this many minutes without input. 0 means never. Global only.
    ScreenOff,
    /// The device sleeps after this many minutes without input. 0 means never. Global only.
    Sleep,
    /// What a program does while none of its windows is shown (see [`InBackground`]).
    Background,
}

/// A setting whose values are named choices. It is an enum, and each variant is stored as its value (see
/// [`Values`]). Defined by [`choices!`], which gives each variant its value, its label and what it does.
pub trait Choice: Copy + PartialEq + 'static {
    const SETTING: Setting;
    /// The values, labels and impacts, in the order a tap steps through them.
    const TABLE: Table;

    fn value(self) -> u16;

    /// The choice `value` stands for. A value that isn't offered gives the default.
    fn of(value: u16) -> Self;
}

/// A choice setting's values as data, which [`Setting`] reads for it.
pub struct Table {
    pub values: &'static [u16],
    labels: &'static [&'static str],
    impacts: &'static [Impact],
    default: u16,
}

impl Table {
    pub const fn new(values: &'static [u16], labels: &'static [&'static str], impacts: &'static [Impact], default: u16) -> Table {
        Table { values, labels, impacts, default }
    }

    fn at(&self, value: u16) -> Option<usize> {
        self.values.iter().position(|&v| v == value)
    }

    fn label(&self, value: u16) -> &'static str {
        self.at(value).or_else(|| self.at(self.default)).and_then(|i| self.labels.get(i)).copied().unwrap_or_default()
    }

    fn impact(&self, value: u16) -> Impact {
        self.at(value).and_then(|i| self.impacts.get(i)).copied().unwrap_or(Impact::Stock)
    }
}

/// Defines a [`Choice`]: the enum, its default, and for each variant, in tap order, its stored value, its
/// label and its [`Impact`]. With `impl`, for an enum defined elsewhere (the fan's [`fan::Mode`]), it
/// gives only the [`Choice`]: the enum must be in scope by that name.
macro_rules! choices {
    ($(#[$doc:meta])* $ty:ident for $setting:ident, default $default:ident {
        $($(#[$vdoc:meta])* $v:ident = $value:literal, $label:literal, $impact:ident;)*
    }) => {
        $(#[$doc])*
        #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
        pub enum $ty {
            $($(#[$vdoc])* $v,)*
        }

        impl Default for $ty {
            fn default() -> Self {
                $ty::$default
            }
        }

        impl $ty {
            pub fn label(self) -> &'static str {
                match self {
                    $($ty::$v => $label,)*
                }
            }
        }

        $crate::settings::choices!(impl $ty for $setting, default $default {
            $($v = $value, $label, $impact;)*
        });
    };
    (impl $ty:ident for $setting:ident, default $default:ident {
        $($v:ident = $value:literal, $label:literal, $impact:ident;)*
    }) => {
        impl $crate::settings::Choice for $ty {
            const SETTING: $crate::settings::Setting = $crate::settings::Setting::$setting;
            const TABLE: $crate::settings::Table = $crate::settings::Table::new(
                &[$($value),*],
                &[$($label),*],
                &[$($crate::settings::Impact::$impact),*],
                match $ty::$default {
                    $($ty::$v => $value,)*
                },
            );

            fn value(self) -> u16 {
                match self {
                    $($ty::$v => $value,)*
                }
            }

            fn of(value: u16) -> Self {
                match value {
                    $($value => $ty::$v,)*
                    _ => $ty::$default,
                }
            }
        }
    };
}

pub(crate) use choices;

choices!(
    /// [`Setting::Power`]: how fast the CPU and GPU may run, through frequency caps. The Thor has no TDP
    /// setting.
    PowerProfile for Power, default Balanced {
        Battery = 0, "Battery", Saves;
        Balanced = 1, "Balanced", Stock;
        Performance = 2, "Performance", Costs;
    }
);

choices!(
    /// [`Setting::Hud`]: the HUD over the game.
    Hud for Hud, default Off {
        Off = 0, "HUD off", Stock;
        Fps = 1, "HUD FPS", Changes;
        Full = 2, "Full HUD", Changes;
    }
);

choices!(
    /// [`Setting::Resolution`]: the screen's size as the game sees it, as a fraction of the screen's own.
    /// Its tile shows the size it gives on the top screen.
    Resolution for Resolution, default Native {
        Native = 0, "Native", Stock;
        FiveSixths = 1, "5/6 size", Saves;
        TwoThirds = 2, "2/3 size", Saves;
        Half = 3, "1/2 size", Saves;
        Third = 4, "1/3 size", Saves;
    }
);

impl Resolution {
    /// The size it gives on a screen of `size`, rounded to even pixels.
    pub fn size_on(self, (w, h): (u32, u32)) -> (u32, u32) {
        let (num, den) = match self {
            Resolution::Native => (1, 1),
            Resolution::FiveSixths => (5, 6),
            Resolution::TwoThirds => (2, 3),
            Resolution::Half => (1, 2),
            Resolution::Third => (1, 3),
        };
        let scaled = |v: u32| (v * num / den) & !1;
        (scaled(w), scaled(h))
    }
}

choices!(
    /// [`Setting::Scaling`]: how a picture smaller than its screen is brought to the screen's size.
    /// Nearest at 1/2 or 1/3 size is exact integer scaling.
    Scaling for Scaling, default Bilinear {
        Bilinear = 0, "Bilinear", Stock;
        Nearest = 1, "Nearest", Changes;
        Fsr = 2, "FSR", Changes;
    }
);

choices!(
    /// [`Setting::AntiAliasing`]: anti-aliasing at the game's own resolution, before scaling.
    AntiAliasing for AntiAliasing, default Off {
        Off = 0, "AA off", Stock;
        /// One pass, softer: edges and fine detail alike.
        Fxaa = 2, "FXAA", Costs;
        /// Three passes, finding edges by their patterns and leaving detail.
        Smaa = 1, "SMAA", Costs;
    }
);

choices!(
    /// [`Setting::BottomScreen`]: what the second screen does while it isn't used (see
    /// [`crate::bottom::screen`]).
    BottomWhenIdle for BottomScreen, default OffWhenIdle {
        /// Off on battery when idle, after dimming first.
        OffWhenIdle = 2, "Off when idle", Stock;
        /// On, but dimmed when untouched.
        On = 0, "Screen on", Costs;
        /// Dark while a game is in force, until tapped, and off on battery.
        DarkInGame = 1, "Dark in game", Saves;
    }
);

choices!(
    /// [`Setting::BigCores`]: whether the big CPU cores are on.
    Cores for BigCores, default All {
        Little = 0, "Little cores", Saves;
        All = 1, "All cores", Stock;
    }
);

choices!(
    /// [`Setting::PadMouse`]: the pad mouse at launch.
    MouseAtLaunch for PadMouse, default Off {
        Off = 0, "Mouse off", Stock;
        On = 1, "Mouse on", Changes;
    }
);

choices!(
    /// [`Setting::Rumble`]: how strongly the controller rumbles.
    RumbleStrength for Rumble, default Full {
        Off = 0, "Rumble off", Changes;
        Low = 1, "Rumble low", Changes;
        Mid = 2, "Rumble mid", Changes;
        Full = 3, "Rumble full", Stock;
    }
);

choices!(
    /// [`Setting::Background`]: what a program does while none of its windows is shown (see
    /// [`crate::session::background`]).
    InBackground for Background, default PausesGames {
        /// The default. Pauses for a game, and Runs otherwise (see [`crate::Session::background_of`]).
        PausesGames = 3, "Pauses games", Stock;
        Runs = 0, "Runs hidden", Costs;
        /// Paused (frozen) once it has been hidden for the grace period.
        Pauses = 1, "Pauses hidden", Saves;
        /// Ended once its last window has closed.
        Quits = 2, "Quits closed", Saves;
    }
);

pub const COUNT: usize = 21;

/// What a value does, for the tile's colour. It is the stock value and does nothing, or it saves power
/// or eases the load, costs power, or changes how the game looks or plays.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Impact {
    Stock,
    Saves,
    Costs,
    Changes,
}

// In the enum's order, since a setting's index is its position. The name is its key in the settings
// file.
names!(Setting {
    Power: "power";
    FpsCap: "fps";
    Refresh: "refresh";
    Hud: "hud";
    Resolution: "resolution";
    Scaling: "scaling";
    AntiAliasing: "aa";
    Sharpen: "sharpen";
    Curve: "curve";
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

    /// Whether the setting takes `value`. It must be one of its choices, or any keymap id for the keymap.
    pub fn accepts(self, value: u16) -> bool {
        matches!(self, Setting::Keymap | Setting::Curve) || (self == Setting::Refresh && value <= 1000) || self.choices().contains(&value)
    }

    /// The setting's values as a [`Choice`]'s table, if they are named choices.
    fn table(self) -> Option<&'static Table> {
        match self {
            Setting::Power => Some(&PowerProfile::TABLE),
            Setting::Hud => Some(&Hud::TABLE),
            Setting::Resolution => Some(&Resolution::TABLE),
            Setting::Scaling => Some(&Scaling::TABLE),
            Setting::AntiAliasing => Some(&AntiAliasing::TABLE),
            Setting::BottomScreen => Some(&BottomWhenIdle::TABLE),
            Setting::BigCores => Some(&Cores::TABLE),
            Setting::Fan => Some(&fan::Mode::TABLE),
            Setting::PadMouse => Some(&MouseAtLaunch::TABLE),
            Setting::Rumble => Some(&RumbleStrength::TABLE),
            Setting::Background => Some(&InBackground::TABLE),
            Setting::FpsCap | Setting::Refresh | Setting::Keymap | Setting::Curve | Setting::CpuMax | Setting::GpuMax | Setting::Sharpen => None,
            Setting::ChargeLimit | Setting::ScreenOff | Setting::Sleep => None,
        }
    }

    /// The values a tap steps through, in order.
    pub fn choices(self) -> &'static [u16] {
        if let Some(table) = self.table() {
            return table.values;
        }
        match self {
            Setting::FpsCap => &[0, 30, 40, 45, 60, 90, 120],
            Setting::Refresh => &[0],
            // The other values are the existing keymaps and curves.
            Setting::Keymap | Setting::Curve => &[0],
            Setting::CpuMax | Setting::GpuMax => &[0, 40, 50, 60, 70, 80, 90, 100],
            Setting::Sharpen => &[0, 25, 50, 75, 100],
            Setting::ChargeLimit => &[100, 90, 80, 0],
            Setting::ScreenOff => &[1, 2, 5, 10, 0],
            Setting::Sleep => &[5, 15, 30, 60, 0],
            _ => &[],
        }
    }

    pub fn default_value(self) -> u16 {
        if let Some(table) = self.table() {
            return table.default;
        }
        match self {
            Setting::ChargeLimit => 100,
            Setting::ScreenOff => 5,
            Setting::Sleep => 15,
            _ => 0,
        }
    }

    /// Whether only ClamShell's own compositor does it: under sway it does nothing.
    pub fn compositor_only(self) -> bool {
        matches!(self, Setting::Curve | Setting::Resolution | Setting::Scaling | Setting::AntiAliasing | Setting::Sharpen)
    }

    /// Whether the setting works through the launch's own process, as MangoHud's cap and HUD do. An app
    /// that the launch hands over to another program (see [`crate::Panel::handed_over`]) doesn't get it.
    pub fn through_launch(self) -> bool {
        matches!(self, Setting::FpsCap | Setting::Hud)
    }

    /// Whether the setting applies only to all games, not per game.
    pub fn global_only(self) -> bool {
        matches!(self, Setting::ChargeLimit | Setting::ScreenOff | Setting::Sleep)
    }

    /// The setting's name, as its menu's title.
    pub fn title(self) -> &'static str {
        match self {
            Setting::Power => "Power",
            Setting::FpsCap => "Frame rate cap",
            Setting::Refresh => "Refresh rate",
            Setting::Hud => "HUD",
            Setting::Resolution => "Resolution",
            Setting::Scaling => "Scaling",
            Setting::AntiAliasing => "Anti-aliasing",
            Setting::Sharpen => "Sharpen",
            Setting::Curve => "HDR curve",
            Setting::BottomScreen => "Second screen",
            Setting::BigCores => "CPU cores",
            Setting::Fan => "Fan",
            Setting::PadMouse => "Pad mouse",
            Setting::Keymap => "Keymap",
            Setting::Rumble => "Rumble",
            Setting::CpuMax => "CPU limit",
            Setting::GpuMax => "GPU limit",
            Setting::ChargeLimit => "Charge limit",
            Setting::ScreenOff => "Screens off",
            Setting::Sleep => "Sleep",
            Setting::Background => "In the background",
        }
    }

    /// What `value` does.
    pub fn impact(self, value: u16) -> Impact {
        if let Some(table) = self.table() {
            return table.impact(value);
        }
        if value == self.default_value() {
            return Impact::Stock;
        }
        match (self, value) {
            (Setting::Refresh, 120) | (Setting::CpuMax | Setting::GpuMax, 100) => Impact::Costs,
            // Never, or later than the default, costs power. Sooner saves it.
            (Setting::ScreenOff | Setting::Sleep, v) if v == 0 || v > self.default_value() => Impact::Costs,
            (Setting::Keymap | Setting::Curve | Setting::Sharpen, _) => Impact::Changes,
            _ => Impact::Saves,
        }
    }

    /// The value after `value`, wrapping around.
    pub fn next(self, value: u16) -> u16 {
        let choices = self.choices();
        let i = choices.iter().position(|&c| c == value).map_or(0, |i| (i + 1) % choices.len());
        choices.get(i).copied().unwrap_or(value)
    }

    /// How `value` reads on its tile, written to `buf`.
    pub fn describe(self, value: u16, buf: &mut impl Write) {
        if let Some(table) = self.table() {
            let _ = buf.write_str(table.label(value));
            return;
        }
        let text = match (self, value) {
            (Setting::FpsCap, 0) => "No cap",
            (Setting::Refresh, 0) => "Hz as is",
            (Setting::Keymap, 0) => "Default",
            // The tile shows the keymap's name.
            (Setting::Keymap, v) => return number(buf, "Keymap ", v, ""),
            (Setting::Curve, 0) => "Off",
            // The tile shows the curve's name.
            (Setting::Curve, v) => return number(buf, "Curve ", v, ""),
            (Setting::CpuMax, 0) => "CPU auto",
            (Setting::GpuMax, 0) => "GPU auto",
            (Setting::Sharpen, 0) => "Sharpen off",
            (Setting::ChargeLimit, 100) => "Charge full",
            (Setting::ChargeLimit, 0) => "Bypass",
            (Setting::ScreenOff, 0) => "Always on",
            (Setting::Sleep, 0) => "No sleep",
            // Numbers: the value with its unit.
            (Setting::FpsCap, v) => return number(buf, "", v, " fps"),
            (Setting::Refresh, v) => return number(buf, "", v, " Hz"),
            (Setting::CpuMax, v) => return number(buf, "CPU ", v, " %"),
            (Setting::GpuMax, v) => return number(buf, "GPU ", v, " %"),
            (Setting::Sharpen, v) => return number(buf, "Sharpen ", v, " %"),
            (Setting::ChargeLimit, v) => return number(buf, "Charge to ", v, " %"),
            (Setting::ScreenOff, v) => return number(buf, "Off after ", v, " min"),
            (Setting::Sleep, v) => return number(buf, "Sleep after ", v, " min"),
            // Named choices come from their table, above.
            _ => "",
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

    /// A setting whose values are named choices, as its [`Choice`].
    pub fn choice<C: Choice>(&self) -> C {
        C::of(self.get(C::SETTING))
    }

    pub fn set(&mut self, s: Setting, value: u16) {
        if let Some(v) = self.0.get_mut(s.index()) {
            *v = value;
        }
    }

    /// Every value as overrides, to write them all.
    pub fn all(&self) -> Overrides {
        Overrides(self.0.map(Some))
    }
}

/// A game's own choices: a value per setting, or none to follow the global value.
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
/// environment. Stored as one file per settings screen (clamshell-system's `store`).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Settings {
    pub global: Values,
    pub overrides: HashMap<Arc<str>, Overrides>,
    /// Extra environment for every game, and per game on top of it, as `KEY=value` words.
    pub global_env: String,
    pub env: HashMap<Arc<str>, String>,
}

impl Settings {
    /// The settings `entry` runs with, below `levels`, such as its runner's (see [`crate::runner_key`]).
    /// These are the global ones, with each level's overrides on top, then the game's own.
    pub fn of_in(&self, entry: &str, levels: &[&str]) -> Values {
        let inherited = levels.iter().fold(self.global, |values, f| self.overrides_of(f).over(&values));
        self.overrides_of(entry).over(&inherited)
    }

    pub fn overrides_of(&self, entry: &str) -> Overrides {
        self.overrides.get(entry).copied().unwrap_or_default()
    }

    /// Sets `setting` for `entry` to `value`. With no game, or for a global-only setting, it is set for all
    /// games. With no value it goes back to the default: the global value for a game, or the stock value
    /// for all games.
    pub fn set(&mut self, entry: Option<&Arc<str>>, setting: Setting, value: Option<u16>) {
        match entry {
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

    /// A game's own environment text, or with `None`, the one for all games.
    pub fn env_text(&self, entry: Option<&str>) -> &str {
        match entry {
            Some(id) => self.env.get(id).map_or("", |e| e.as_str()),
            None => &self.global_env,
        }
    }

    pub fn set_env(&mut self, entry: Option<Arc<str>>, text: &str) {
        let text = text.trim().to_owned();
        match entry {
            Some(id) if text.is_empty() => {
                self.env.remove(&id);
            }
            Some(id) => {
                self.env.insert(id, text);
            }
            None => self.global_env = text,
        }
    }

    /// Appends the environment a game below `levels` gets to `vars`: the global variables, then each
    /// level's, then its own. Later ones win.
    pub fn env_in(&self, entry: &str, levels: &[&str], vars: &mut Vec<(String, String)>) {
        parse_env(&self.global_env, vars);
        for f in levels {
            parse_env(self.env_text(Some(f)), vars);
        }
        parse_env(self.env_text(Some(entry)), vars);
    }

    /// Drops a game's overrides and environment.
    pub fn reset(&mut self, entry: &str) {
        self.overrides.remove(entry);
        self.env.remove(entry);
    }

    /// Whether a game has anything of its own.
    pub fn has_own(&self, entry: &str) -> bool {
        self.overrides.contains_key(entry) || self.env.contains_key(entry)
    }

    /// Who uses profile `id` of the kind `setting` holds (a keymap): all games, if it is the global
    /// choice, and the games that chose it.
    pub fn users(&self, setting: Setting, id: u16) -> crate::profiles::picker::Users {
        let all_entries = self.global.get(setting) == id;
        let entries = self.overrides.values().filter(|o| o.get(setting) == Some(id)).count();
        crate::profiles::picker::Users { all_entries, entries }
    }

    /// Profile `id` of the kind `setting` holds is gone. All games, if it was theirs, go back to Default,
    /// and the games and levels that chose it choose Default. Their choice isn't cleared, since a level
    /// between them and all games, such as their runner, may choose another profile.
    pub fn forget(&mut self, setting: Setting, id: u16) {
        if self.global.get(setting) == id {
            self.global.set(setting, 0);
        }
        for o in self.overrides.values_mut().filter(|o| o.get(setting) == Some(id)) {
            o.set(setting, Some(0));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_resolution_is_its_fraction_of_the_screen_in_even_pixels() {
        let thor = (1920, 1080);
        assert_eq!(Resolution::Native.size_on(thor), thor);
        assert_eq!(Resolution::FiveSixths.size_on(thor), (1600, 900));
        assert_eq!(Resolution::TwoThirds.size_on(thor), (1280, 720));
        assert_eq!(Resolution::Half.size_on(thor), (960, 540));
        assert_eq!(Resolution::Third.size_on(thor), (640, 360));
        assert_eq!(Resolution::TwoThirds.size_on((960, 540)), (640, 360), "nested, half size");
        assert_eq!(Setting::Sharpen.next(100), 0);
        assert!(Setting::Scaling.compositor_only() && !Setting::FpsCap.compositor_only());
    }

    #[test]
    fn taps_step_through_the_values_and_wrap() {
        assert_eq!(Setting::FpsCap.next(0), 30);
        assert_eq!(Setting::FpsCap.next(120), 0);
        assert_eq!(Setting::FpsCap.next(33), 0, "a value not offered: the first");
        for s in Setting::ALL {
            assert!(s.choices().contains(&s.default_value()), "{s:?}");
            for &v in s.choices() {
                let mut buf = String::new();
                s.describe(v, &mut buf);
                assert!(!buf.is_empty(), "{s:?} {v}");
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
        assert_eq!(s.of_in("zelda", &[]).get(Setting::Power), 2);
        assert_eq!(s.global.get(Setting::ChargeLimit), 80, "global-only settings go global");
        let env_in = |s: &Settings, entry: &str, levels: &[&str]| {
            let mut vars = Vec::new();
            s.env_in(entry, levels, &mut vars);
            vars
        };
        assert_eq!(env_in(&s, "ff6", &[]), vec![("A".to_owned(), "1".to_owned())]);
        assert!(Setting::FpsCap.accepts(40) && !Setting::FpsCap.accepts(33) && Setting::Keymap.accepts(7));
        // Below levels: the first level's, then the next one's over it, then the game's own over both.
        s.set(Some(&"runner:SNES".into()), Setting::FpsCap, Some(30));
        s.set(Some(&"runner:SNES".into()), Setting::Keymap, Some(2));
        s.set(Some(&"runner:SNES RPG".into()), Setting::Keymap, Some(3));
        let ff6 = s.of_in("ff6", &["runner:SNES", "runner:SNES RPG"]);
        assert_eq!((ff6.get(Setting::FpsCap), ff6.get(Setting::Keymap)), (30, 3));
        s.set_env(Some("runner:SNES".into()), "A=2 B=1");
        let vars = |list: &[(&str, &str)]| list.iter().map(|(k, v)| ((*k).to_owned(), (*v).to_owned())).collect::<Vec<_>>();
        assert_eq!(env_in(&s, "ff6", &["runner:SNES"]), vars(&[("A", "2"), ("B", "1"), ("A", "1")]), "the level's, then its own");
        s.reset("zelda");
        assert!(!s.has_own("zelda"));
        assert_eq!(s.of_in("zelda", &[]), s.global);
    }

    #[test]
    fn forgetting_a_keymap_sets_who_chose_it_to_default() {
        let mut s = Settings::default();
        let (runner, game, other): (Arc<str>, Arc<str>, Arc<str>) = ("runner:SNES".into(), "ff6".into(), "zelda".into());
        s.set(None, Setting::Keymap, Some(5));
        s.set(Some(&runner), Setting::Keymap, Some(3));
        s.set(Some(&game), Setting::Keymap, Some(5));
        s.set(Some(&other), Setting::Keymap, Some(3));
        s.forget(Setting::Keymap, 5);
        assert_eq!(s.global.get(Setting::Keymap), 0);
        assert_eq!(s.of_in("ff6", &["runner:SNES"]).get(Setting::Keymap), 0, "Default, not its runner's");
        assert_eq!(s.of_in("snes-game", &["runner:SNES"]).get(Setting::Keymap), 3, "the runner's choice stays");
        s.forget(Setting::Keymap, 3);
        assert_eq!(s.of_in("snes-game", &["runner:SNES"]).get(Setting::Keymap), 0);
        assert_eq!(s.of_in("zelda", &[]).get(Setting::Keymap), 0);
    }
}
