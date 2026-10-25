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

/// What the settings panel edits: the global settings (All), the runner of the item in context, or the
/// item itself. The item can be a game, an app or a film, but not a browsed file or a folder, nor an
/// item of a runner without item settings. Content edits the runner's level when the item has no level
/// of its own.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum Scope {
    Global,
    Runner,
    #[default]
    Content,
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

/// Every entry's settings, the scope the settings panel edits, and the tile whose menu is open. The scope
/// is global whenever there is no item in context.
#[derive(Debug, Clone, Default)]
pub struct Prefs {
    settings: Settings,
    scope: Scope,
    /// The tile whose menu is open over the settings tab, by a long press: its values, and Reset.
    menu: Option<Setting>,
}

impl Prefs {
    pub(crate) fn new(settings: Settings) -> Prefs {
        Prefs { settings, ..Prefs::default() }
    }

    pub fn settings(&self) -> &Settings {
        &self.settings
    }

    /// The scope chosen in the settings panel (see [`Panel::scope_shown`] for the one it shows).
    pub fn scope(&self) -> Scope {
        self.scope
    }

    pub(crate) fn set_scope(&mut self, scope: Scope) {
        self.scope = scope;
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

/// The settings panel as it is now: the item in context, the levels it offers, what it edits, and the
/// keymaps' picker for it. Built by [`Panel::new`]; it doesn't allocate, since the UI builds it while
/// drawing.
#[derive(Debug, Clone)]
pub struct Panel<'a> {
    prefs: &'a Prefs,
    keymapping: &'a Keymapping,
    curving: &'a crate::curve::screens::Curving,
    context: Option<Program<'a>>,
    levels: [Option<(Scope, Program<'a>)>; 2],
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
            Front::Program => session.windows().iter().find(|w| w.focused && !w.down).and_then(|w| session.owner_of(w)).or_else(|| session.program().cloned()),
            Front::Shelf => shelf.selected().cloned().map(|id| shelf.entry_of_file(&id).unwrap_or(id)),
            Front::Other => None,
        };
        let context = id.and_then(|id| entry_of(&id).map(|g| Program { id, title: &g.title }));
        let entry = context.as_ref().and_then(|item| entry_of(&item.id));
        let levels = match (&context, entry) {
            (Some(item), Some(g)) => match &g.runner {
                Some(r) => {
                    let runner = Some((Scope::Runner, Program { id: r.key.clone(), title: &r.name }));
                    let own = (r.item_settings && g.browsed.is_none() && !g.is_group()).then(|| (Scope::Content, item.clone()));
                    [runner, own]
                }
                None => [Some((Scope::Runner, item.clone())), None],
            },
            _ => [None, None],
        };
        let scope = prefs.scope;
        let editing = match &levels {
            _ if scope == Scope::Global => None,
            [Some((s, p)), _] if *s == scope => Some(p.clone()),
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
    pub fn levels(&self) -> &[Option<(Scope, Program<'a>)>; 2] {
        &self.levels
    }

    /// What the panel edits now: the level of the chosen scope, or none for All. Content edits the
    /// runner's level when the item has none of its own.
    pub fn editing(&self) -> Option<&Program<'a>> {
        self.editing.as_ref()
    }

    /// The scope chosen.
    pub fn scope(&self) -> Scope {
        self.prefs.scope
    }

    /// The scope the panel shows as chosen: the one chosen, or the level that stands in for it.
    pub fn scope_shown(&self) -> Scope {
        match &self.editing {
            None => Scope::Global,
            Some(p) => self.levels.iter().flatten().find(|(_, l)| l.id == p.id).map_or(Scope::Global, |(scope, _)| *scope),
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
        let runner = self.levels.iter().flatten().find(|(scope, l)| *scope == Scope::Runner && l.id != g.id).map(|(_, l)| l.title);
        Some(runner.unwrap_or("All"))
    }

    /// Whether Reset has something to do for `setting`: the level edited chooses it itself, or All differs
    /// from the stock value.
    pub fn resettable(&self, setting: Setting) -> bool {
        match self.tile_origin(setting) {
            TileOrigin::Own => true,
            TileOrigin::Global => self.tile_value(setting) != setting.default_value(),
            TileOrigin::Inherited => false,
        }
    }

    /// The keymap chosen for the scope edited; 0 is Default.
    pub fn keymap_chosen(&self) -> u16 {
        self.tile_value(Setting::Keymap)
    }

    fn entry(&self) -> Option<&str> {
        self.editing.as_ref().map(|g| &*g.id)
    }

    /// Row `i` of the keymaps' picker, for what the panel edits.
    pub fn keymap_row(&self, i: usize) -> Option<PickerRow<'a>> {
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

    /// The profile of `kind` chosen for the scope edited; 0 is Default.
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
#[allow(clippy::unwrap_used)]
mod tests {
    use crate::bottom::keyboard::{Key, Named, find};
    use crate::settings::{self, Choice, PowerProfile, Setting};
    use crate::testkit::*;
    use crate::*;
    use clamshell_library::runner_key;

    #[test]
    fn refresh_taps_follow_announced_modes_and_reports_never_change_saved_choices() {
        let mut s = started(Init::default());
        let mut modes = crate::device::display::DisplayModes::default();
        for hz in [75000, 59940, 60000] {
            modes.add(hz);
        }
        assert!(s.applied(Event::DisplayModes(modes)).is_empty());
        assert!(s.applied(Event::DisplayModes(modes)).is_empty(), "a repeated report has no effects");
        let effects = taps(&mut s, Setting::Refresh, 1);
        assert_eq!(s.prefs().settings().global.get(Setting::Refresh), 60);
        // Persist the requested choice before applying it; the output request follows hardware settings.
        assert!(
            matches!(effects.as_slice(), [Effect::Save(Save::Settings(_)), Effect::Apply { entry: None, settings }, Effect::Refresh(60)] if settings.get(Setting::Refresh) == 60),
            "{effects:?}"
        );
        let effects = taps(&mut s, Setting::Refresh, 1);
        assert_eq!(s.prefs().settings().global.get(Setting::Refresh), 75);
        assert!(effects.iter().any(|effect| matches!(effect, Effect::Refresh(75))));
        assert!(s.applied(Event::DisplayModes(Default::default())).is_empty());
        assert_eq!(s.prefs().settings().global.get(Setting::Refresh), 75, "hotplug never overwrites a saved choice");
        let effects = taps(&mut s, Setting::Refresh, 1);
        assert_eq!(s.prefs().settings().global.get(Setting::Refresh), 0);
        assert!(effects.iter().any(|effect| matches!(effect, Effect::Refresh(0))));
        let mut replacement = crate::device::display::DisplayModes::default();
        replacement.add(144000);
        assert!(s.applied(Event::DisplayModes(replacement)).is_empty());
        taps(&mut s, Setting::Refresh, 1);
        assert_eq!(s.prefs().settings().global.get(Setting::Refresh), 144);
    }

    /// Taps a tile `n` times.
    fn taps(s: &mut State, setting: Setting, n: usize) -> Vec<Effect> {
        let mut fx = vec![];
        for _ in 0..n {
            fx = tap(s, Id::Settings(SettingsId::Tile(setting)));
        }
        fx
    }

    fn scope(s: &mut State, scope: Scope) {
        tap(s, Id::Settings(SettingsId::Scope(scope)));
    }

    fn state_with(settings: &settings::Settings) -> State {
        State::new(Init { settings: settings.clone(), ..init() })
    }

    #[test]
    fn an_entry_takes_its_runners_settings_not_its_folders() {
        let mut library = clamshell_library::demo::library();
        for g in library.entries.iter_mut().filter(|g| g.id.starts_with("tv:")) {
            g.runner = Some(clamshell_library::RunnerOf::new("video", "Video", true));
        }
        let mut settings = settings::Settings::default();
        settings.set(Some(&runner_key("video")), Setting::FpsCap, Some(30));
        settings.set(Some(&"tv:Cosmos".into()), Setting::Power, Some(PowerProfile::Battery.value()));
        settings.set_env(Some(runner_key("video")), "MPV=1");
        let mut s = State::new(Init { library, settings, ..init() });
        let episode: std::sync::Arc<str> = "tv:Cosmos/Season 1/2. One Voice in the Cosmic Fugue.mkv".into();
        let fx = launch_now(&mut s, &episode);
        let launch = fx.iter().find_map(|e| match e {
            Effect::Launch { settings, env, .. } => Some((settings.get(Setting::FpsCap), settings.get(Setting::Power), env.clone())),
            _ => None,
        });
        assert_eq!(launch, Some((30, 1, vec![("MPV".to_owned(), "1".to_owned())])), "the runner's, not the series folder's");
    }

    #[test]
    fn the_panel_offers_the_items_levels() {
        let mut library = clamshell_library::demo::library();
        for g in library.entries.iter_mut().filter(|g| g.id.starts_with("tv:")) {
            g.runner = Some(clamshell_library::RunnerOf::new("video", "Video", true));
        }
        let mut s = State::new(Init { library, ..init() });
        let levels = |s: &State| s.panel().levels().iter().flatten().map(|(scope, l)| (*scope, l.title.to_owned())).collect::<Vec<_>>();
        assert_eq!(levels(&s), [(Scope::Runner, "A Link to the Past".to_owned())], "a game with no runner: its own, as its runner");
        tap(&mut s, Id::Shelf(ShelfId::Collection(2)));
        assert_eq!(levels(&s), [(Scope::Runner, "Video".to_owned())], "a series: its runner's only");
        tap(&mut s, Id::Shelf(ShelfId::Launch));
        tap(&mut s, Id::Shelf(ShelfId::Launch));
        let title = "1. The Shores of the Cosmic Ocean".to_owned();
        assert_eq!(levels(&s), [(Scope::Runner, "Video".to_owned()), (Scope::Content, title.clone())]);
        assert_eq!(s.panel().editing().map(|p| p.title), Some(title.as_str()), "its own by default");
        scope(&mut s, Scope::Runner);
        assert_eq!(s.panel().editing().map(|p| p.id.clone()), Some(runner_key("video")));
        taps(&mut s, Setting::Power, 1);
        assert_eq!(s.prefs().settings().overrides_of("runner:video").get(Setting::Power), Some(2), "kept by the runner");
        assert_eq!(s.settings_of("tv:Cosmos/Season 2/1. Standing Up in the Milky Way.mkv").get(Setting::Power), 2, "every video's");
    }

    #[test]
    fn a_runner_can_leave_its_items_without_settings_of_their_own() {
        let mut library = clamshell_library::demo::library();
        for g in library.entries.iter_mut() {
            g.runner = Some(clamshell_library::RunnerOf::new("snes", "SNES", false));
        }
        let s = State::new(Init { library, ..init() });
        let levels = s.panel().levels().iter().flatten().map(|(scope, l)| (*scope, l.title)).collect::<Vec<_>>();
        assert_eq!(levels, [(Scope::Runner, "SNES")]);
        assert_eq!(s.panel().editing().map(|p| p.id.clone()), Some(runner_key("snes")), "the item's tab: its runner's");
    }

    #[test]
    fn a_runner_gets_its_keymap_once() {
        let mut library = clamshell_library::demo::library();
        library.runner_keymaps = vec![(runner_key("text"), "viewer".into())];
        let mut s = State::new(Init { library, ..init() });
        let fx = s.applied(Event::Start);
        assert!(fx.iter().any(|e| matches!(e, Effect::Save(Save::Keymaps(_)))), "{fx:?}");
        let own = s.keymapping().keymaps().own_of("runner:text").map(|k| k.id);
        assert!(own.is_some());
        assert_eq!(s.prefs().settings().overrides_of("runner:text").get(Setting::Keymap), own);
        // With Default chosen for it instead, it is left that way, also when the library is read again.
        s.settings_mut().set(Some(&runner_key("text")), Setting::Keymap, Some(0));
        let fx = s.applied(Event::Library(Box::new(s.shelf().library().clone())));
        assert!(!fx.iter().any(|e| matches!(e, Effect::Save(Save::Keymaps(_)))));
    }

    /// The settings in force changed by a runner's keymap given to it, not by the panel, are applied too.
    #[test]
    fn a_runners_keymap_given_while_its_item_runs_is_applied() {
        let mut library = clamshell_library::demo::library();
        for g in library.entries.iter_mut() {
            g.runner = Some(clamshell_library::RunnerOf::new("text", "Text", false));
        }
        let mut s = started(Init { library, ..init() });
        let zelda = launch(&mut s, "zelda-lttp").workspace();
        s.observed(vec![win(3, "zelda3", &zelda, true)]);
        assert_eq!(s.session().program().map(|p| &**p), Some("zelda-lttp"));
        let mut library = s.shelf().library().clone();
        library.runner_keymaps = vec![(runner_key("text"), "viewer".into())];
        let fx = s.applied(Event::Library(Box::new(library)));
        let own = s.keymapping().keymaps().own_of("runner:text").map(|k| k.id);
        assert!(own.is_some());
        let applied = fx.iter().find_map(|e| match e {
            Effect::Apply { entry, settings } => Some((entry.as_deref(), settings.get(Setting::Keymap))),
            _ => None,
        });
        assert_eq!(applied, Some((Some("zelda-lttp"), own.unwrap_or_default())), "{fx:?}");
    }

    #[test]
    fn a_tiles_menu_picks_a_value_and_resets_to_the_level_below() {
        let mut s = state();
        let fps = Id::Settings(SettingsId::Tile(Setting::FpsCap));
        tap(&mut s, Id::Tab(crate::Tab::Settings));
        long(&mut s, fps.clone());
        assert_eq!(s.prefs().menu(), Some(Setting::FpsCap));
        assert_eq!(s.pad_back(), Some(Id::Settings(SettingsId::Back)));
        // The cap's values, and no Reset while the game follows All's.
        assert_eq!(s.lists().len(ListId::Values), Setting::FpsCap.choices().len());
        assert_eq!(s.menu_row(3), Some(Some(45)));
        tap(&mut s, ListId::Values.row(3));
        assert_eq!((s.prefs().menu(), s.panel().tile_value(Setting::FpsCap)), (None, 45), "set, and closed");
        assert_eq!(s.panel().tile_origin(Setting::FpsCap), TileOrigin::Own);
        // Now with Reset, back to All's.
        long(&mut s, fps.clone());
        let reset = Setting::FpsCap.choices().len();
        assert_eq!((s.lists().len(ListId::Values), s.menu_row(reset)), (reset + 1, Some(None)));
        assert_eq!((s.panel().below_name(), s.panel().inherited_value(Setting::FpsCap)), (Some("All"), 0));
        tap(&mut s, ListId::Values.row(reset as u16));
        assert_eq!(s.panel().tile_origin(Setting::FpsCap), TileOrigin::Inherited);
        // Back closes it.
        long(&mut s, fps);
        tap(&mut s, Id::Settings(SettingsId::Back));
        assert_eq!(s.prefs().menu(), None);
    }

    #[test]
    fn a_profile_tile_steps_through_its_profiles_and_its_picker_resets() {
        let mut s = State::new(Init { curves: crate::curve::Curves::initial(), ..init() });
        let tile = Id::Settings(SettingsId::Tile(Setting::Curve));
        tap(&mut s, Id::Tab(crate::Tab::Settings));
        // Off, then the three named ones, then Off again.
        for want in [1, 2, 3, 0] {
            tap(&mut s, tile.clone());
            assert_eq!(s.panel().tile_value(Setting::Curve), want);
        }
        assert_eq!(s.panel().tile_origin(Setting::Curve), TileOrigin::Own, "Off chosen for the game");
        // The picker (a long press) ends with Reset while the game chooses one itself.
        long(&mut s, tile);
        let rows = s.lists().len(ListId::Curves);
        assert_eq!(s.panel().profile_row(crate::profiles::picker::Kind::Curve, rows - 1), Some(crate::profiles::picker::PickerRow::Reset));
        tap(&mut s, ListId::Curves.row(rows as u16 - 1));
        assert_eq!(s.panel().tile_origin(Setting::Curve), TileOrigin::Inherited);
        assert_eq!(s.lists().len(ListId::Curves), rows - 1, "nothing to reset");
    }

    #[test]
    fn games_override_the_global_settings() {
        let mut s = state();
        scope(&mut s, Scope::Global);
        taps(&mut s, Setting::FpsCap, 4); // 30, 40, 45, 60
        scope(&mut s, Scope::Content);
        let fx = taps(&mut s, Setting::Power, 1); // Balanced -> Performance
        let Some(Effect::Save(Save::Settings(saved))) = fx.first() else {
            unreachable!("{fx:?}");
        };
        assert_eq!(saved, s.prefs().settings(), "saved whole");
        assert_eq!(saved.overrides_of("zelda-lttp").get(Setting::Power), Some(2));
        let zelda = s.settings_of("zelda-lttp");
        assert_eq!((zelda.get(Setting::Power), zelda.get(Setting::FpsCap)), (2, 60));
        assert_eq!(s.settings_of("ff6").get(Setting::Power), 1);
        assert_eq!((s.panel().tile_origin(Setting::Power), s.panel().tile_origin(Setting::FpsCap)), (TileOrigin::Own, TileOrigin::Inherited));
        let fresh = state_with(saved);
        assert_eq!(fresh.settings_of("zelda-lttp"), zelda);
        assert_eq!(fresh.prefs().settings().global, s.prefs().settings().global);
        // The tile's menu (a long press) drops one override with its Reset row, last, and Reset drops all
        // of them.
        taps(&mut s, Setting::Rumble, 1);
        long(&mut s, Id::Settings(SettingsId::Tile(Setting::Power)));
        assert_eq!(s.prefs().menu(), Some(Setting::Power));
        assert_eq!(s.menu_row(3), Some(None), "after the three power profiles");
        tap(&mut s, ListId::Values.row(3));
        assert_eq!(s.prefs().menu(), None, "closed");
        assert_eq!(s.prefs().settings().overrides_of("zelda-lttp").get(Setting::Power), None);
        assert_eq!(s.prefs().settings().overrides_of("zelda-lttp").get(Setting::Rumble), Some(0));
        tap(&mut s, Id::Settings(SettingsId::Reset));
        assert!(s.prefs().settings().overrides.is_empty());
        // Global-only settings change the global value even on a game's tab.
        taps(&mut s, Setting::ChargeLimit, 1);
        assert_eq!(s.prefs().settings().global.get(Setting::ChargeLimit), 90);
        assert_eq!(s.panel().tile_origin(Setting::ChargeLimit), TileOrigin::Global);
    }

    #[test]
    fn the_launch_carries_the_games_settings_and_environment() {
        let mut settings = settings::Settings::default();
        settings.set_env(None, "A=1 B=2");
        settings.set_env(Some("zelda-lttp".into()), "B=3 'C=x y'");
        let mut s = state_with(&settings);
        taps(&mut s, Setting::FpsCap, 2);
        let fx = tap(&mut s, Id::Shelf(ShelfId::Launch));
        let Some(Effect::Launch { settings, env, .. }) = fx.iter().find(|e| matches!(e, Effect::Launch { .. })) else { unreachable!("{fx:?}") };
        assert_eq!(settings.get(Setting::FpsCap), 40);
        let pairs: Vec<(&str, &str)> = env.iter().map(|(k, v)| (k.as_str(), v.as_str())).collect();
        assert_eq!(pairs, [("A", "1"), ("B", "2"), ("B", "3"), ("C", "x y")], "the game's last, so they win");
    }

    #[test]
    fn a_running_game_takes_changes_at_once() {
        let mut s = state();
        crate::testkit::start(&mut s, "zelda-lttp");
        show_window(&mut s, "zelda-lttp", 7);
        tap(&mut s, entry("ff6"));
        // The running game is the context, not the selection.
        let fx = taps(&mut s, Setting::FpsCap, 1);
        assert!(fx.iter().any(|e| matches!(e, Effect::Apply { entry: Some(g), settings } if &**g == "zelda-lttp" && settings.get(Setting::FpsCap) == 30)));
        // A global change reaches it only where it doesn't override.
        scope(&mut s, Scope::Global);
        assert!(!taps(&mut s, Setting::FpsCap, 1).iter().any(|e| matches!(e, Effect::Apply { .. })), "its own cap stays: nothing to apply");
        let fx = taps(&mut s, Setting::Power, 1);
        assert!(fx.iter().any(|e| matches!(e, Effect::Apply { settings, .. } if settings.get(Setting::Power) == 2)));
        let mut fx = end(&mut s, "zelda-lttp");
        fx.extend(s.observed(vec![]));
        let fx: Vec<Effect> = fx.into_iter().filter(|e| !matches!(e, Effect::RecordPlay { .. } | Effect::Window(_))).collect();
        assert_eq!(fx, vec![Effect::Apply { entry: None, settings: s.prefs().settings().global }, Effect::PadRoute(PadRoute::Ui)], "and the pad back to shelf");
    }

    #[test]
    fn a_game_can_start_the_pad_mouse_and_darken_the_second_screen() {
        let mut s = state();
        taps(&mut s, Setting::PadMouse, 1);
        taps(&mut s, Setting::BottomScreen, 2);
        let fx = crate::testkit::start(&mut s, "zelda-lttp");
        assert!(fx.contains(&Effect::PadRoute(PadRoute::Mouse)), "{fx:?}");
        assert!(s.bottom_dark());
        tap(&mut s, Id::Wake);
        assert!(!s.bottom_dark());
        let fx = end(&mut s, "zelda-lttp");
        assert!(fx.contains(&Effect::PadRoute(PadRoute::Ui)), "{fx:?}");
        assert!(!s.pad().mouse_on());
    }

    #[test]
    fn without_a_game_the_panel_edits_the_global_settings() {
        let mut empty = State::new(Init { library: Library::default(), ..init() });
        tap(&mut empty, Id::Tab(Tab::Settings));
        assert_eq!(empty.bottom().tab(), Tab::Settings, "always available");
        taps(&mut empty, Setting::FpsCap, 2);
        assert_eq!(empty.prefs().settings().global.get(Setting::FpsCap), 40);
    }

    #[test]
    fn the_editor_pastes_the_clipboards_text_as_a_line() {
        let mut s = state();
        tap(&mut s, Id::Tab(Tab::Settings));
        tap(&mut s, Id::Settings(SettingsId::Env));
        let key = |s: &mut State, k| tap(s, Id::Key(find(k).unwrap()));
        let ctrl_v = |s: &mut State| {
            key(s, Key::Mod(crate::bottom::keyboard::Modifier::Ctrl));
            key(s, Key::Char('v'))
        };
        ctrl_v(&mut s);
        assert_eq!(s.bottom().editor().map(|e| e.text.as_str()), Some(""), "nothing on the clipboard");
        s.applied(Event::Reading(Reading::Clipboard(Some("A=1\nB=2\n".into()))));
        ctrl_v(&mut s);
        assert_eq!(s.bottom().editor().map(|e| e.text.as_str()), Some("A=1 B=2"), "its line breaks spaces, its end trimmed");
        // A right click does the same.
        s.applied(Event::Paste);
        assert_eq!(s.bottom().editor().map(|e| e.text.as_str()), Some("A=1 B=2A=1 B=2"));
    }

    #[test]
    fn the_environment_editor_takes_the_keyboard() {
        let mut s = state();
        tap(&mut s, Id::Tab(Tab::Settings));
        tap(&mut s, Id::Settings(SettingsId::Env));
        let key = |s: &mut State, k| tap(s, Id::Key(find(k).unwrap()));
        for c in ['d', 'x', 'v', 'k'] {
            assert_eq!(key(&mut s, Key::Char(c)), vec![], "typed into the editor, not sent");
        }
        key(&mut s, Key::Named(Named::Backspace));
        key(&mut s, Key::Char('='));
        key(&mut s, Key::Char('1'));
        let fx = key(&mut s, Key::Named(Named::Enter));
        assert!(matches!(fx.first(), Some(Effect::Save(Save::Settings(t))) if t.env_text(Some("zelda-lttp")) == "dxv=1"), "{fx:?}");
        assert_eq!(s.bottom().editor(), None);
        // What a launch of it gets: the variables of all games, its runner's and its own.
        let env_of = |s: &State| {
            let runner = s.entry("zelda-lttp").and_then(|g| g.runner.as_ref()).map(|r| &*r.key);
            let mut vars = Vec::new();
            s.prefs().settings().env_in("zelda-lttp", runner.as_slice(), &mut vars);
            vars
        };
        assert_eq!(env_of(&s), vec![("dxv".to_owned(), "1".to_owned())]);
        let fresh = state_with(s.prefs().settings());
        assert_eq!(env_of(&fresh), env_of(&s));
    }

    #[test]
    fn a_text_field_doesnt_take_the_screen_from_an_open_editor() {
        let mut s = state();
        s.observed(vec![foot()]);
        tap(&mut s, Id::Tab(Tab::Settings));
        tap(&mut s, Id::Settings(SettingsId::Env));
        assert!(s.bottom().editor().is_some());
        s.applied(Event::TextInput(true));
        assert!(s.bottom().editor().is_some() && s.bottom().tab() == Tab::Settings, "its typing would be lost");
    }

    #[test]
    fn an_editor_stays_with_its_tab() {
        let mut s = state();
        // An app in front, so the keyboard types into it.
        s.observed(vec![foot()]);
        tap(&mut s, Id::Tab(Tab::Settings));
        tap(&mut s, Id::Settings(SettingsId::Env));
        let key = |s: &mut State, k| tap(s, Id::Key(find(k).unwrap()));
        key(&mut s, Key::Char('a'));
        // In the keyboard tab, keys go to the app, not to the editor left open in the settings.
        tap(&mut s, Id::Tab(Tab::Keyboard));
        assert_eq!(s.bottom().editor(), None);
        assert_eq!(key(&mut s, Key::Char('b')).len(), 1, "sent");
        tap(&mut s, Id::Tab(Tab::Settings));
        assert_eq!(s.bottom().editor().map(|e| e.text.as_str()), Some("a"), "as left");
    }
}
