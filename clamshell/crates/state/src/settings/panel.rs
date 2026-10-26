//! The settings panel's rules. The panel is about an item chosen by what is in front. It edits one
//! level: All, the item's runner (Desktop for an app, Steam for a Steam game), or the item itself.
//! Changes are applied at once when they change the settings in force. The panel also opens the
//! environment editor.

use std::sync::Arc;

use crate::bottom::editor::{Editor, Editors, Purpose};
use crate::effect::{Effect, Save};
use crate::keymap::screens::Keymapping;
use crate::profiles::picker::{Owner, PickerRow};
use crate::session::running::Front;
use crate::settings::{Setting, Settings};
use crate::{Session, Shelf};

/// The level the settings panel edits: the global settings (All), the runner of the item in context, or the
/// item itself. The item can be a game, an app or a film, but not a browsed file or a folder, nor an
/// item of a runner without item settings. Content edits the runner's level when the item has no level
/// of its own.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum SettingsLevel {
    Global,
    Runner,
    #[default]
    Content,
}

/// Where a setting's value at the level edited is from (see [`Panel::source`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Source<'a> {
    /// The level edited sets it itself: the level's name.
    Written(&'a str),
    /// A level below sets it: that level's name.
    Read(&'a str),
    /// No level sets it: the stock value.
    Defaults,
}

/// Where a settings tile's value comes from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TileOrigin {
    /// The game's own choice.
    Own,
    /// The global value, which the game follows because it has no choice of its own.
    Inherited,
    /// The global settings are being edited, or the setting is global only.
    Global,
}

/// Every entry's settings, the level the settings panel edits, and the tile whose menu is open. The level
/// is global whenever there is no item in context.
#[derive(Debug, Clone, Default)]
pub struct Prefs {
    settings: Settings,
    level: SettingsLevel,
    /// The tile whose menu is open over the settings tab, by a long press: its values, and Reset.
    menu: Option<Setting>,
    /// A setting's slider held, if one is.
    slider: Option<crate::Slider>,
}

impl Prefs {
    pub(crate) fn new(settings: Settings) -> Prefs {
        Prefs { settings, ..Prefs::default() }
    }

    /// A setting's slider held, if one is.
    pub fn slider(&self) -> Option<crate::Slider> {
        self.slider
    }

    /// A setting's slider is held as `slider`, or let go with None.
    pub(crate) fn hold_slider(&mut self, slider: Option<crate::Slider>) {
        self.slider = slider;
    }

    pub fn settings(&self) -> &Settings {
        &self.settings
    }

    /// The level chosen in the settings panel (see [`Panel::level_shown`] for the one it shows).
    pub fn level(&self) -> SettingsLevel {
        self.level
    }

    pub(crate) fn set_level(&mut self, level: SettingsLevel) {
        self.level = level;
        self.menu = None;
    }

    /// The tile whose menu is open, if one is.
    pub fn menu(&self) -> Option<Setting> {
        self.menu
    }

    /// The menu open while the settings tab is the one shown (`tab`).
    pub(crate) fn menu_in(&self, tab: crate::Tab) -> Option<Setting> {
        self.menu.filter(|_| tab == crate::Tab::Settings)
    }

    pub(crate) fn open_menu(&mut self, setting: Setting) {
        self.menu = Some(setting);
    }

    pub(crate) fn close_menu(&mut self) {
        self.menu = None;
    }

    /// Sets `setting` for `entry`, or for all games (see [`Settings::set`]).
    pub(crate) fn set(&mut self, entry: Option<&Arc<str>>, setting: Setting, value: Option<u16>) {
        self.settings.set(entry, setting, value);
    }

    /// Chooses profile `id` of the kind `setting` holds for `entry`, or for all entries. Every choice of a
    /// profile is set here: the picker's, an entry's own profile just made, and a runner's own keymap from
    /// the library.
    pub(crate) fn choose(&mut self, entry: Option<&Arc<str>>, setting: Setting, id: u16) {
        self.settings.set(entry, setting, Some(id));
    }

    /// Drops a game's overrides and environment (see [`Settings::reset`]).
    pub(crate) fn reset(&mut self, entry: &str) {
        self.settings.reset(entry);
    }

    pub(crate) fn set_env(&mut self, entry: Option<Arc<str>>, text: &str) {
        self.settings.set_env(entry, text);
    }

    /// Profile `id` of the kind `setting` holds is gone (see [`Settings::forget`]).
    pub(crate) fn forget(&mut self, setting: Setting, id: u16) {
        self.settings.forget(setting, id);
    }

    /// Opens the editor on `entry`'s environment variables, or with none, those for all games.
    pub(crate) fn open_env_editor(&self, entry: Option<Arc<str>>, editors: &mut Editors) {
        let text = self.settings.env_text(entry.as_deref()).to_owned();
        editors.open(Editor { purpose: Purpose::Env { entry }, text });
    }

    pub(crate) fn save_effect(&self) -> Effect {
        Effect::Save(Save::Settings(self.settings.clone()))
    }

    /// The settings, for a test to set up.
    #[cfg(test)]
    pub(crate) fn settings_mut(&mut self) -> &mut Settings {
        &mut self.settings
    }
}

/// A program as the settings panel shows it: its entry's id and name.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Program<'a> {
    pub id: Arc<str>,
    pub title: &'a str,
}

/// How many tiles a row of the settings grid has.
pub const GRID_COLUMNS: usize = 5;

/// The settings grid's tiles, the most used first, in rows of [`GRID_COLUMNS`]: four rows show at once and
/// the grid scrolls. On a program's tab Reset comes before the last, CPU cores, to show with the others.
/// The settings for the device rather than a program, the charge limit and idle, are on the battery tab.
const GRID: [crate::id::SettingsId; 20] = {
    use crate::id::SettingsId::{Env, Tile};
    [
        Tile(Setting::Power),
        Tile(Setting::FpsCap),
        Tile(Setting::Refresh),
        Tile(Setting::Hud),
        Tile(Setting::Curve),
        Tile(Setting::Resolution),
        Tile(Setting::Scaling),
        Tile(Setting::Size),
        Tile(Setting::AntiAliasing),
        Tile(Setting::Sharpen),
        Tile(Setting::Keymap),
        Tile(Setting::PadMouse),
        Tile(Setting::BottomScreen),
        Tile(Setting::Background),
        Env,
        Tile(Setting::Fan),
        Tile(Setting::CpuMax),
        Tile(Setting::GpuMax),
        Tile(Setting::Rumble),
        Tile(Setting::BigCores),
    ]
};

/// The settings panel as it is now: the item in context, the levels it offers, what it edits, and the
/// keymaps' picker for it. Built by [`Panel::new`]; it doesn't allocate, since the UI builds it while
/// drawing.
#[derive(Debug, Clone)]
pub struct Panel<'a> {
    prefs: &'a Prefs,
    keymapping: &'a Keymapping,
    curving: &'a crate::curve::screens::Curving,
    context: Option<Program<'a>>,
    levels: [Option<(SettingsLevel, Program<'a>)>; 2],
    editing: Option<Program<'a>>,
    /// The level the edited one takes its settings from before its own, which is its runner's.
    runner: Option<&'a str>,
    handover: bool,
}

impl<'a> Panel<'a> {
    /// The settings panel as it is now. Its item is chosen by what is in front: the entry whose window is
    /// in front, or shelf's selection, where a browsed file that belongs to a library entry stands for
    /// that entry. There is none for a window of no program.
    pub(crate) fn new(
        session: &'a Session,
        shelf: &'a Shelf,
        prefs: &'a Prefs,
        keymapping: &'a Keymapping,
        curving: &'a crate::curve::screens::Curving,
    ) -> Panel<'a> {
        let entry_of = |id: &str| session.entry(shelf.library(), id);
        let id = match session.front() {
            Front::Program => session.top_window().and_then(|w| session.owner_of(w)).or_else(|| session.program().cloned()),
            Front::Shelf => shelf.selected().cloned().map(|id| shelf.entry_of_file(&id).unwrap_or(id)),
            Front::Other => None,
        };
        let context = id.and_then(|id| entry_of(&id).map(|g| Program { id, title: &g.title }));
        let entry = context.as_ref().and_then(|item| entry_of(&item.id));
        let levels = match (&context, entry) {
            (Some(item), Some(g)) => match &g.runner {
                Some(r) => {
                    let runner = Some((SettingsLevel::Runner, Program { id: r.key.clone(), title: &r.name }));
                    let own = (r.item_settings && g.browsed.is_none() && !g.is_group()).then(|| (SettingsLevel::Content, item.clone()));
                    [runner, own]
                }
                None => [Some((SettingsLevel::Runner, item.clone())), None],
            },
            _ => [None, None],
        };
        let level = prefs.level;
        let editing = match &levels {
            _ if level == SettingsLevel::Global => None,
            [Some((s, p)), _] if *s == level => Some(p.clone()),
            [runner, own] => own.as_ref().or(runner.as_ref()).map(|(_, p)| p.clone()),
        };
        let runner = editing.as_ref().and_then(|p| entry_of(&p.id)).and_then(|g| g.runner.as_ref()).map(|r| &*r.key);
        let handover = editing.is_some() && entry.and_then(|g| g.runner.as_ref()).is_some_and(|r| r.handover);
        Panel { prefs, keymapping, curving, context, levels, editing, runner, handover }
    }

    /// The item the settings panel is about, chosen by what is in front (see [`Panel::new`]). It is
    /// none for a window of no program, and then the panel edits the defaults.
    pub fn context(&self) -> Option<&Program<'a>> {
        self.context.as_ref()
    }

    /// The levels below All that the panel offers for the item in context, with their titles. Normally
    /// these are its runner's and its own (`All | SNES | Zelda`, `All | Desktop | Spotify`). There is only
    /// its runner's for a browsed file, a folder, or a runner's item without item settings (`All |
    /// Text`). There is only its own for an item without a runner, such as ClamShell's tools.
    pub fn levels(&self) -> &[Option<(SettingsLevel, Program<'a>)>; 2] {
        &self.levels
    }

    /// The settings grid's tiles, in order (see [`GRID`]).
    pub fn grid(&self) -> impl Iterator<Item = crate::id::SettingsId> + Clone + use<> {
        let (most, last) = GRID.split_at(GRID.len() - 1);
        let reset = self.editing().is_some().then_some(crate::id::SettingsId::Reset);
        most.iter().copied().chain(reset).chain(last.iter().copied())
    }

    /// What the panel edits now: the program at the chosen level, or none for All. Content edits the
    /// runner's level when the item has none of its own.
    pub fn editing(&self) -> Option<&Program<'a>> {
        self.editing.as_ref()
    }

    /// The level chosen.
    pub fn level(&self) -> SettingsLevel {
        self.prefs.level
    }

    /// The level the panel shows as chosen: the one chosen, or the level that stands in for it.
    pub fn level_shown(&self) -> SettingsLevel {
        match &self.editing {
            None => SettingsLevel::Global,
            Some(p) => self.levels.iter().flatten().find(|(_, l)| l.id == p.id).map_or(SettingsLevel::Global, |(level, _)| *level),
        }
    }

    /// Whether the panel edits a runner whose launches hand their app over to another program, or an item
    /// of such a runner. Android is one: Waydroid's session runs its apps. Anything that works through
    /// the launch's process, such as [`Setting::through_launch`] settings and the environment, has no
    /// effect there.
    pub fn handed_over(&self) -> bool {
        self.handover
    }

    /// The value a tile shows and steps from: the edited level's effective value, or the global one.
    pub fn tile_value(&self, setting: Setting) -> u16 {
        match &self.editing {
            Some(g) if !setting.global_only() => self.prefs.settings.of_in(&g.id, self.runner.as_slice()).get(setting),
            _ => self.prefs.settings.global.get(setting),
        }
    }

    /// Where a tile's value comes from.
    pub fn tile_origin(&self, setting: Setting) -> TileOrigin {
        match &self.editing {
            Some(g) if !setting.global_only() => match self.prefs.settings.overrides_of(&g.id).get(setting) {
                Some(_) => TileOrigin::Own,
                None => TileOrigin::Inherited,
            },
            _ => TileOrigin::Global,
        }
    }

    /// The value `setting` has at the level edited without a choice of its own there: the level's below
    /// (the runner's for an item, All's for a runner), or for All the stock value.
    pub fn inherited_value(&self, setting: Setting) -> u16 {
        match &self.editing {
            Some(g) if !setting.global_only() => match self.runner.filter(|r| *r != &*g.id) {
                Some(runner) => self.prefs.settings.of_in(runner, &[]).get(setting),
                None => self.prefs.settings.global.get(setting),
            },
            _ => setting.default_value(),
        }
    }

    /// What the level below the edited one is called, which Reset goes back to: the runner's title, or
    /// All; none when All is edited, where Reset goes back to the stock value.
    pub fn below_name(&self) -> Option<&'a str> {
        let g = self.editing.as_ref()?;
        let runner = self.levels.iter().flatten().find(|(level, l)| *level == SettingsLevel::Runner && l.id != g.id).map(|(_, l)| l.title);
        Some(runner.unwrap_or("All"))
    }

    /// Where `setting`'s value at the level edited is from: written there (its own), or read from the
    /// level below that sets it, or from the defaults when none does.
    pub fn source(&self, setting: Setting) -> Source<'a> {
        let level = self.editing.as_ref().map_or("All", |g| g.title);
        match self.tile_origin(setting) {
            TileOrigin::Own => Source::Written(level),
            TileOrigin::Global if self.tile_value(setting) != setting.default_value() => Source::Written(level),
            TileOrigin::Global => Source::Defaults,
            TileOrigin::Inherited => {
                let set_by = |id: &str| self.prefs.settings.overrides_of(id).get(setting).is_some();
                let runner = self
                    .editing
                    .as_ref()
                    .and_then(|g| self.levels.iter().flatten().find(|(level, l)| *level == SettingsLevel::Runner && l.id != g.id && set_by(&l.id)));
                match runner {
                    Some((_, l)) => Source::Read(l.title),
                    None if self.prefs.settings.global.get(setting) != setting.default_value() => Source::Read("All"),
                    None => Source::Defaults,
                }
            }
        }
    }

    /// Whether Reset has something to do for `setting`: the level edited chooses it itself, or All differs
    /// from the stock value.
    pub(crate) fn resettable(&self, setting: Setting) -> bool {
        match self.tile_origin(setting) {
            TileOrigin::Own => true,
            TileOrigin::Global => self.tile_value(setting) != setting.default_value(),
            TileOrigin::Inherited => false,
        }
    }

    fn entry(&self) -> Option<&str> {
        self.editing.as_ref().map(|g| &*g.id)
    }

    /// Row `i` of the keymaps' picker, for what the panel edits.
    pub(crate) fn keymap_row(&self, i: usize) -> Option<PickerRow<'a>> {
        self.keymapping.row(i, self.entry())
    }

    /// Row `i` of `kind`'s picker, for what the panel edits.
    pub fn profile_row(&self, kind: crate::profiles::picker::Kind, i: usize) -> Option<PickerRow<'a>> {
        let row = match kind {
            crate::profiles::picker::Kind::Keymap => self.keymap_row(i),
            crate::profiles::picker::Kind::Curve => self.curving.row(i, self.entry()),
        };
        row.or_else(|| (i == self.profile_rows_len(kind) && self.picker_resettable(kind)).then_some(PickerRow::Reset))
    }

    /// The picker's rows of `kind`, without its Reset row.
    fn profile_rows_len(&self, kind: crate::profiles::picker::Kind) -> usize {
        match kind {
            crate::profiles::picker::Kind::Keymap => self.keymapping.rows_len(self.entry()),
            crate::profiles::picker::Kind::Curve => self.curving.rows_len(self.entry()),
        }
    }

    /// Whether the picker of `kind` ends with Reset: on its list of profiles, while the level edited
    /// chooses one itself (see [`Panel::resettable`]).
    fn picker_resettable(&self, kind: crate::profiles::picker::Kind) -> bool {
        use crate::profiles::picker::{Kind, PickerScreen};
        let list = match kind {
            Kind::Keymap => self.keymapping.picker() == Some(PickerScreen::List),
            Kind::Curve => self.curving.screen() == Some(crate::curve::screens::CurveScreen::Picker(PickerScreen::List)),
        };
        list && self.resettable(kind.setting())
    }

    /// Whether row `i` of the picker of `kind` is its Reset row.
    pub(crate) fn is_profile_reset(&self, kind: crate::profiles::picker::Kind, i: usize) -> bool {
        matches!(self.profile_row(kind, i), Some(PickerRow::Reset))
    }

    /// The profile of `kind` chosen for the level edited; 0 is Default.
    pub fn profile_chosen(&self, kind: crate::profiles::picker::Kind) -> u16 {
        self.tile_value(kind.setting())
    }

    pub(crate) fn keymap_rows_len(&self) -> usize {
        let kind = crate::profiles::picker::Kind::Keymap;
        self.profile_rows_len(kind) + usize::from(self.picker_resettable(kind))
    }

    pub(crate) fn curve_rows_len(&self) -> usize {
        let kind = crate::profiles::picker::Kind::Curve;
        self.profile_rows_len(kind) + usize::from(self.picker_resettable(kind))
    }

    /// Whether the panel edits a game that has no keymap of its own and so can make one.
    pub fn keymap_can_make_own(&self) -> bool {
        self.keymapping.can_make_own(self.entry())
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::panic)]
mod tests;
