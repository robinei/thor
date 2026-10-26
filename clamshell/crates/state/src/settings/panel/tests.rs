use crate::Compositor;
use crate::bottom::keyboard::{Key, Named, find};
use crate::settings::{self, Choice, PowerProfile, Setting};
use crate::testkit::*;
use crate::*;
use clamshell_library::runner_key;

#[test]
fn refresh_taps_follow_announced_modes_and_reports_never_change_saved_choices() {
    let mut s = started(Init::default());
    let mut modes = crate::presentation::modes::DisplayModes::default();
    for hz in [75000, 59940, 60000] {
        modes.add(hz);
    }
    assert!(s.applied(Event::Reading(Reading::DisplayModes(modes))).is_empty());
    assert!(s.applied(Event::Reading(Reading::DisplayModes(modes))).is_empty(), "a repeated report has no effects");
    let effects = taps(&mut s, Setting::Refresh, 1);
    assert_eq!(s.prefs().settings().global.get(Setting::Refresh), 60);
    // Persist the requested choice before applying it; the output request follows hardware settings.
    assert!(
        matches!(effects.as_slice(), [Effect::Save(Save::Settings(_)), Effect::Apply { entry: None, settings }, Effect::Compositor(Compositor::Refresh(60))] if settings.get(Setting::Refresh) == 60),
        "{effects:?}"
    );
    let effects = taps(&mut s, Setting::Refresh, 1);
    assert_eq!(s.prefs().settings().global.get(Setting::Refresh), 75);
    assert!(effects.iter().any(|effect| matches!(effect, Effect::Compositor(Compositor::Refresh(75)))));
    assert!(s.applied(Event::Reading(Reading::DisplayModes(Default::default()))).is_empty());
    assert_eq!(s.prefs().settings().global.get(Setting::Refresh), 75, "hotplug never overwrites a saved choice");
    let effects = taps(&mut s, Setting::Refresh, 1);
    assert_eq!(s.prefs().settings().global.get(Setting::Refresh), 0);
    assert!(effects.iter().any(|effect| matches!(effect, Effect::Compositor(Compositor::Refresh(0)))));
    let mut replacement = crate::presentation::modes::DisplayModes::default();
    replacement.add(144000);
    assert!(s.applied(Event::Reading(Reading::DisplayModes(replacement))).is_empty());
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

fn level(s: &mut State, level: SettingsLevel) {
    tap(s, Id::Settings(SettingsId::Scope(level)));
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
    let mut s = State::new(Init { library: Some(library), settings, ..init() });
    let episode: std::sync::Arc<str> = "tv:Cosmos/Season 1/2. One Voice in the Cosmic Fugue.mkv".into();
    let fx = launch_now(&mut s, &episode);
    let launch = fx.iter().find_map(|e| match e {
        Effect::Launch { settings, env, .. } => Some((settings.get(Setting::FpsCap), settings.get(Setting::Power), env.clone())),
        _ => None,
    });
    let env = [("MESA_VK_WSI_PRESENT_MODE", "fifo"), ("vblank_mode", "3"), ("MPV", "1")].map(|(k, v)| (k.to_owned(), v.to_owned())).to_vec();
    assert_eq!(launch, Some((30, 1, env)), "the runner's, not the series folder's, after the cap's");
}

#[test]
fn the_panel_offers_the_items_levels() {
    let mut library = clamshell_library::demo::library();
    for g in library.entries.iter_mut().filter(|g| g.id.starts_with("tv:")) {
        g.runner = Some(clamshell_library::RunnerOf::new("video", "Video", true));
    }
    let mut s = State::new(Init { library: Some(library), ..init() });
    let levels = |s: &State| s.panel().levels().iter().flatten().map(|(level, l)| (*level, l.title.to_owned())).collect::<Vec<_>>();
    assert_eq!(levels(&s), [(SettingsLevel::Runner, "A Link to the Past".to_owned())], "a game with no runner: its own, as its runner");
    tap(&mut s, Id::Shelf(ShelfId::Collection(2)));
    assert_eq!(levels(&s), [(SettingsLevel::Runner, "Video".to_owned())], "a series: its runner's only");
    tap(&mut s, Id::Shelf(ShelfId::Launch));
    tap(&mut s, Id::Shelf(ShelfId::Launch));
    let title = "1. The Shores of the Cosmic Ocean".to_owned();
    assert_eq!(levels(&s), [(SettingsLevel::Runner, "Video".to_owned()), (SettingsLevel::Content, title.clone())]);
    assert_eq!(s.panel().editing().map(|p| p.title), Some(title.as_str()), "its own by default");
    level(&mut s, SettingsLevel::Runner);
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
    let s = State::new(Init { library: Some(library), ..init() });
    let levels = s.panel().levels().iter().flatten().map(|(level, l)| (*level, l.title)).collect::<Vec<_>>();
    assert_eq!(levels, [(SettingsLevel::Runner, "SNES")]);
    assert_eq!(s.panel().editing().map(|p| p.id.clone()), Some(runner_key("snes")), "the item's tab: its runner's");
}

#[test]
fn a_runner_gets_its_keymap_once() {
    let mut library = clamshell_library::demo::library();
    library.runner_keymaps = vec![(runner_key("text"), "viewer".into())];
    let mut s = State::new(Init { library: Some(library), ..init() });
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
    let mut s = started(Init { library: Some(library), ..init() });
    let zelda = launch(&mut s, "zelda-lttp");
    s.observed(vec![win_of(3, "zelda3", &zelda)]);
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
    level(&mut s, SettingsLevel::Global);
    taps(&mut s, Setting::FpsCap, 4); // 30, 40, 45, 60
    level(&mut s, SettingsLevel::Content);
    let fx = taps(&mut s, Setting::Power, 1); // Balanced -> Performance
    let Some(Effect::Save(Save::Settings(saved))) = fx.first() else {
        panic!("{fx:?}");
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
    let Some(Effect::Launch { settings, env, .. }) = fx.iter().find(|e| matches!(e, Effect::Launch { .. })) else { panic!("{fx:?}") };
    assert_eq!(settings.get(Setting::FpsCap), 40);
    let pairs: Vec<(&str, &str)> = env.iter().map(|(k, v)| (k.as_str(), v.as_str())).collect();
    let cap = [("MESA_VK_WSI_PRESENT_MODE", "fifo"), ("vblank_mode", "3")];
    assert_eq!(pairs, [cap[0], cap[1], ("A", "1"), ("B", "2"), ("B", "3"), ("C", "x y")], "the cap's first, the game's last, so they win");
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
    level(&mut s, SettingsLevel::Global);
    assert!(!taps(&mut s, Setting::FpsCap, 1).iter().any(|e| matches!(e, Effect::Apply { .. })), "its own cap stays: nothing to apply");
    let fx = taps(&mut s, Setting::Power, 1);
    assert!(fx.iter().any(|e| matches!(e, Effect::Apply { settings, .. } if settings.get(Setting::Power) == 2)));
    let mut fx = end(&mut s, "zelda-lttp");
    fx.extend(s.observed(vec![]));
    let fx: Vec<Effect> = fx.into_iter().filter(|e| !matches!(e, Effect::RecordPlay { .. })).collect();
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
    let mut empty = State::new(Init { library: Some(Library::default()), ..init() });
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
