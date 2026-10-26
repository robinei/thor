use super::*;
use crate::Compositor;
use crate::HardwareKey;
use crate::Tab;
use crate::bottom::notice::NoticeKind;
use crate::effect::{Effect, Save};
use crate::session::below::WindowRule;
use crate::session::launch_unit::LaunchUnit;
use crate::session::running::{Origin, Running};
use crate::session::windows::WindowFacts;
use crate::settings::{Choice, Setting};
use crate::testkit::Applied;
use crate::testkit::{group_of, state, tap};
use crate::{Event, State, WindowPlace};

fn win(id: i64, unit: &str, title: &str) -> WindowFacts {
    WindowFacts { id, app_id: "emu".into(), title: title.into(), unit: Some(unit.into()), ..WindowFacts::default() }
}

const GAME: &str = "clamshell-run-melonds_1-99-1.scope";

/// The emulator's launch.
fn emu() -> LaunchUnit {
    LaunchUnit { key: "melonds_1".into(), secs: 99, n: 1 }
}

fn running(id: &str, key: &str) -> Running {
    Running { id: id.into(), key: key.into(), origin: Origin::Ours, app_id: None, start: 0, seen: false, tray: None, entry: None, told: None }
}

/// A game started by shelf, running with a top window and a second window, the second in front.
fn emulator() -> State {
    let mut s = state();
    s.add_run(running("melonds", "melonds_1"), emu());
    s.observed(vec![win(1, GAME, "Top"), win(2, GAME, "Bottom")]);
    s.applied(crate::WindowEvent::Activate(2).into());
    s
}

fn key(s: &mut State, k: HardwareKey) -> Vec<Effect> {
    s.applied(Event::HardwareKey(k))
}

#[test]
fn a_programs_only_window_is_remembered_by_its_app_id_alone() {
    let mut s = state();
    s.add_run(running("melonds", "melonds_1"), emu());
    s.observed(vec![win(2, GAME, "Page one")]);
    s.applied(crate::WindowEvent::Activate(2).into());
    let fx = key(&mut s, HardwareKey::WindowDown);
    let rule = WindowRule { owner: "melonds".into(), app_id: "emu".into(), title: None };
    assert!(matches!(&fx[0], Effect::Save(Save::WindowRules(r)) if r.list() == [rule.clone()]), "{fx:?}");
    // A different title next time, such as a page or a command, still matches.
    assert!(rule.matches("melonds", &win(3, GAME, "Page two")));
}

#[test]
fn a_window_moved_down_is_shown_there_and_remembered() {
    let mut s = emulator();
    // Nothing down: the windows' tab opens all the same, saying how to move one there.
    assert_eq!(key(&mut s, HardwareKey::BottomView), vec![Effect::Watch(crate::Watch::Metrics, false)]);
    assert!(!s.bottom().shows_windows(s.session()));
    let fx = key(&mut s, HardwareKey::WindowDown);
    assert_eq!(s.session().bottom_window().map(|w| w.id), Some(2));
    assert_eq!(s.session().top_window().map(|w| w.id), Some(1));
    // The title is included, since the emulator has another window with the same app id.
    let rule = WindowRule { owner: "melonds".into(), app_id: "emu".into(), title: Some("Bottom".into()) };
    assert!(matches!(&fx[0], Effect::Save(Save::WindowRules(r)) if r.list() == [rule.clone()]), "{fx:?}");
    assert_eq!(s.bottom().notice().map(|n| &n.what), Some(&NoticeKind::BottomRule(true)));
    assert!(s.bottom().shows_windows(s.session()));
    // Moving up acts even when ClamShell's own view covers the bottom window.
    key(&mut s, HardwareKey::BottomView);
    assert!(!s.bottom().shows_windows(s.session()));
    // Up moves it back to its group, forgets the rule, and shows ClamShell's view below.
    let fx = key(&mut s, HardwareKey::WindowUp);
    assert_eq!(s.session().place(2), Some(WindowPlace::Top(Some(&group_of(&emu())))));
    assert!(matches!(&fx[0], Effect::Save(Save::WindowRules(r)) if r.list().is_empty()), "{fx:?}");
    assert!(!s.bottom().shows_windows(s.session()), "none down: ClamShell's view");
}

#[test]
fn a_rule_sends_the_window_down_when_its_program_opens_it_again() {
    let mut s = emulator();
    key(&mut s, HardwareKey::WindowDown);
    let rules = s.session().window_rules().clone();
    // A fresh start with the rules. The game's windows open on top, and the one the rule names goes
    // down, once.
    let mut s = crate::testkit::started(crate::Init { window_rules: rules.clone(), ..crate::testkit::init() });
    s.add_run(running("melonds", "melonds_1"), emu());
    s.observed(vec![win(5, GAME, "Top"), win(6, GAME, "Bottom")]);
    assert_eq!(s.session().bottom_window().map(|w| w.id), Some(6));
    assert_eq!(s.session().top_window().map(|w| w.id), Some(5));
    // Its window appears before its unit is listed, and is sent down when the unit appears.
    let mut s = crate::testkit::started(crate::Init { window_rules: rules.clone(), ..crate::testkit::init() });
    s.observed(vec![win(8, GAME, "Bottom")]);
    assert_eq!(s.session().bottom_window(), None, "no entry yet");
    s.add_run(running("melonds", "melonds_1"), emu());
    s.applied(Event::Units(vec![emu()]));
    assert_eq!(s.session().bottom_window().map(|w| w.id), Some(8));
    let mut s = emulator();
    key(&mut s, HardwareKey::WindowDown);
    // Another game's window with that title doesn't match the rule.
    s.add_run(running("other", "other_2"), LaunchUnit { key: "other_2".into(), secs: 5, n: 2 });
    s.observed(vec![win(7, "clamshell-run-other_2-5-2.scope", "Bottom")]);
    assert!(s.session().window(7).is_some() && !s.session().below(7));
}

#[test]
fn moving_another_window_down_swaps_the_single_bottom_slot() {
    let mut s = emulator();
    key(&mut s, HardwareKey::WindowDown);
    assert_eq!(s.session().bottom_window().map(|w| w.id), Some(2));
    key(&mut s, HardwareKey::WindowDown);
    assert_eq!(s.session().place(2), Some(WindowPlace::Top(Some(&group_of(&emu())))));
    assert_eq!(s.session().bottom_window().map(|w| w.id), Some(1));
    assert_eq!(s.session().top_window().map(|w| w.id), Some(2));
    key(&mut s, HardwareKey::BottomView);
    assert!(!s.bottom().shows_windows(s.session()));
    key(&mut s, HardwareKey::WindowUp);
    assert_eq!(s.session().place(1), Some(WindowPlace::Top(Some(&group_of(&emu())))));
    assert_eq!(s.session().bottom_window(), None);
    assert!(!s.bottom().shows_windows(s.session()));
    assert_eq!(s.session().top_window().map(|w| w.id), Some(1));
}

#[test]
fn the_windows_tab_shows_the_window_below_and_marks_one_hidden_by_another_tab() {
    let mut s = emulator();
    key(&mut s, HardwareKey::WindowDown);
    assert_eq!(s.bottom().tab(), Tab::Windows);
    assert_eq!(s.bottom().hidden_down(s.session()), None, "shown");
    // Home + right stick press goes back to the tab it came from: the window is then hidden there.
    key(&mut s, HardwareKey::BottomView);
    assert_eq!(s.bottom().tab(), Tab::Sysinfo);
    assert_eq!(s.bottom().hidden_down(s.session()), Some(2), "ClamShell's view over it");
    // Its tab shows it again.
    let fx = tap(&mut s, crate::Id::Tab(Tab::Windows));
    assert_eq!(fx, vec![Effect::Watch(crate::Watch::Metrics, false)], "the tab left isn't watched");
    assert!(s.bottom().shows_windows(s.session()));
    // With another program in front, its window below isn't marked as hidden.
    key(&mut s, HardwareKey::BottomView);
    s.observed(vec![win(1, GAME, "Top"), win(2, GAME, "Bottom"), win(3, "clamshell-run-x_1-1-0.scope", "x")]);
    s.applied(crate::WindowEvent::Activate(3).into());
    assert_eq!(s.bottom().hidden_down(s.session()), None);
}

/// A status reading that says the device is on battery or on the charger.
fn on_battery(s: &mut State, discharging: bool) -> Vec<Effect> {
    let status = crate::Status { battery: Some(50), charging: !discharging, discharging, ..crate::Status::default() };
    s.applied(Event::Reading(crate::Reading::Status(status)))
}

fn power(fx: &[Effect]) -> Option<bool> {
    fx.iter().find_map(|e| match e {
        Effect::Compositor(Compositor::Outputs { top: true, bottom }) => Some(*bottom),
        _ => None,
    })
}

#[test]
fn on_battery_the_untouched_bottom_screen_goes_off_and_a_touch_brings_it_back() {
    let mut s = state();
    on_battery(&mut s, true);
    s.applied(Event::tick(1000));
    assert_eq!(power(&s.applied(Event::tick(1000 + DIM_AFTER + OFF_AFTER - 1))), None, "dimmed, not yet off");
    assert_eq!(power(&s.applied(Event::tick(1000 + DIM_AFTER + OFF_AFTER))), Some(false), "off by itself");
    assert!(s.bottom().off());
    assert_eq!(power(&s.applied(Event::BottomTouched)), Some(true), "a touch: on again");
    assert!(!s.bottom().off());
}

#[test]
fn off_by_itself_as_the_screens_go_off_it_goes_off_with_them_and_stays_off() {
    use crate::idle::IdleEvent;
    let mut s = state();
    on_battery(&mut s, true);
    s.applied(Event::tick(1000));
    s.applied(Event::Idle(IdleEvent::Watching(true)));
    s.applied(Event::tick(1060));
    s.applied(Event::BottomTouched);
    // The screens go off for idle after 5 minutes, and the bottom one by itself 4 minutes after the
    // touch, at the same time.
    let fx = s.applied(Event::tick(1060 + DIM_AFTER + OFF_AFTER));
    assert!(s.bottom().off());
    assert!(fx.contains(&Effect::Compositor(Compositor::Outputs { top: false, bottom: false })), "{fx:?}");
    let fx = s.applied(Event::Idle(IdleEvent::DeviceInput));
    assert!(fx.contains(&Effect::Compositor(Compositor::Outputs { top: true, bottom: false })), "{fx:?}");
}

#[test]
fn the_wall_clock_set_back_does_not_keep_it_on() {
    let mut s = state();
    on_battery(&mut s, true);
    let year = 365 * 86_400;
    s.applied(Event::Tick { unix_secs: 2 * year, mono_secs: 1000 });
    let at = |mono: u64| Event::Tick { unix_secs: year + mono, mono_secs: mono };
    assert_eq!(power(&s.applied(at(1000 + DIM_AFTER + OFF_AFTER - 1))), None);
    assert!(s.bottom().dimmed(), "dimmed on time");
    assert_eq!(power(&s.applied(at(1000 + DIM_AFTER + OFF_AFTER))), Some(false), "off on time");
}

#[test]
fn on_the_charger_or_set_to_stay_on_it_only_dims() {
    let mut s = state();
    on_battery(&mut s, false);
    s.applied(Event::tick(1000));
    assert_eq!(power(&s.applied(Event::tick(1000 + DIM_AFTER + OFF_AFTER))), None, "on the charger: only dimmed");
    // Unplugged now, but set to Screen on, so it is only dimmed.
    s.settings_mut().global.set(Setting::BottomScreen, BottomWhenIdle::On.value());
    on_battery(&mut s, true);
    assert_eq!(power(&s.applied(Event::tick(1001 + DIM_AFTER + OFF_AFTER))), None);
    // Back to the default, it turns off. With the charger plugged in, it turns on.
    s.settings_mut().global.set(Setting::BottomScreen, BottomWhenIdle::OffWhenIdle.value());
    assert_eq!(power(&s.applied(Event::tick(1002 + DIM_AFTER + OFF_AFTER))), Some(false));
    assert_eq!(power(&on_battery(&mut s, false)), Some(true), "the charger: on again");
}

#[test]
fn dark_for_a_game_on_battery_it_goes_off_and_a_tap_wakes_it() {
    let mut s = state();
    on_battery(&mut s, true);
    s.settings_mut().global.set(Setting::BottomScreen, BottomWhenIdle::DarkInGame.value());
    crate::testkit::start(&mut s, "zelda-lttp");
    assert!(s.bottom_dark());
    assert!(s.bottom().off(), "dark on battery: off at once");
    assert_eq!(power(&s.applied(Event::Pointer(crate::PointerEvent::Activate(crate::Id::Wake)))), Some(true), "tapped: on");
}

/// Home+Start on a screen dark for a game turns it on as a tap does, and it stays on: the game no
/// longer darkens it until the next game starts.
#[test]
fn home_start_wakes_a_screen_dark_for_a_game() {
    let mut s = state();
    on_battery(&mut s, true);
    s.settings_mut().global.set(Setting::BottomScreen, BottomWhenIdle::DarkInGame.value());
    s.applied(Event::tick(1000));
    crate::testkit::start(&mut s, "zelda-lttp");
    assert!(s.bottom().off());
    assert_eq!(power(&key(&mut s, HardwareKey::BottomPower)), Some(true));
    assert!(!s.bottom_dark() && !s.bottom().off());
    assert_eq!(power(&s.applied(Event::tick(1001))), None, "it stays on");
    assert!(!s.bottom().off());
}

#[test]
fn windows_moved_down_keep_it_on() {
    let mut s = emulator();
    on_battery(&mut s, true);
    key(&mut s, HardwareKey::WindowDown);
    s.observed(vec![win(1, GAME, "Top"), win(2, GAME, "Bottom")]);
    s.applied(Event::tick(1000));
    assert_eq!(power(&s.applied(Event::tick(1000 + DIM_AFTER + OFF_AFTER))), None, "a window shown there");
}

#[test]
fn home_start_brings_back_a_screen_off_by_hand_and_by_itself_at_once() {
    let mut s = state();
    on_battery(&mut s, true);
    s.applied(Event::tick(1000));
    key(&mut s, HardwareKey::BottomPower);
    s.applied(Event::tick(1000 + DIM_AFTER + OFF_AFTER));
    assert_eq!(power(&key(&mut s, HardwareKey::BottomPower)), Some(true), "one press");
    assert!(!s.bottom().off(), "on, and not off by itself either");
}

#[test]
fn home_start_brings_back_a_screen_off_by_itself() {
    let mut s = state();
    on_battery(&mut s, true);
    s.applied(Event::tick(1000));
    s.applied(Event::tick(1000 + DIM_AFTER + OFF_AFTER));
    assert_eq!(power(&key(&mut s, HardwareKey::BottomPower)), Some(true), "on, as a touch would");
    assert!(!s.bottom().off());
}

#[test]
fn the_bottom_screen_dims_untouched_and_lights_when_used() {
    let mut s = state();
    s.applied(Event::tick(1000));
    s.applied(Event::tick(1000 + DIM_AFTER - 1));
    assert!(!s.bottom().dimmed());
    s.applied(Event::tick(1000 + DIM_AFTER));
    assert!(s.bottom().dimmed(), "untouched a while: dimmed");
    // A touch on it lights it.
    s.applied(Event::BottomTouched);
    assert!(!s.bottom().dimmed());
    // The pad's focus coming to one of its targets lights it too, but not shelf's targets on the main
    // screen.
    s.applied(Event::tick(1000 + 3 * DIM_AFTER));
    assert!(s.bottom().dimmed());
    s.applied(Event::Pointer(crate::PointerEvent::Focus(crate::Id::Shelf(crate::ShelfId::Launch))));
    assert!(s.bottom().dimmed(), "the main screen's");
    s.applied(Event::Pointer(crate::PointerEvent::Focus(crate::Id::Tab(crate::Tab::Dock))));
    assert!(!s.bottom().dimmed());
    // Windows moved there are watched without touching, so they are never dimmed.
    let mut s = emulator();
    key(&mut s, HardwareKey::WindowDown);
    s.observed(vec![win(1, GAME, "Top"), win(2, GAME, "Bottom")]);
    s.applied(Event::tick(5000));
    s.applied(Event::tick(5000 + 2 * DIM_AFTER));
    assert!(s.bottom().shows_windows(s.session()) && !s.bottom().dimmed());
}

#[test]
fn the_bottom_screen_turns_off_and_on() {
    let mut s = emulator();
    let off = vec![Effect::Compositor(Compositor::Outputs { top: true, bottom: false }), Effect::Watch(crate::Watch::Metrics, false)];
    assert_eq!(key(&mut s, HardwareKey::BottomPower), off, "nothing seen: not watched");
    assert!(s.bottom().off());
    // A window moved down turns it on again.
    let fx = key(&mut s, HardwareKey::WindowDown);
    assert!(fx.contains(&Effect::Compositor(Compositor::Outputs { top: true, bottom: true })), "{fx:?}");
    assert!(!s.bottom().off());
}

/// Home + right stick down with no window of a program in front, shelf's or none, does nothing.
#[test]
fn window_down_with_no_window_in_front_does_nothing() {
    let mut s = state();
    assert_eq!(key(&mut s, HardwareKey::WindowDown), vec![], "no windows");
    s.observed(vec![crate::testkit::win_of(2, "foot", &crate::testkit::launch_of("foot_1"))]);
    assert_eq!(key(&mut s, HardwareKey::WindowDown), vec![], "shelf in front");
    assert!(s.session().window_rules().list().is_empty());
}

/// Home + right stick up says the window's rule is forgotten.
#[test]
fn a_window_moved_up_says_its_rule_is_forgotten() {
    let mut s = emulator();
    key(&mut s, HardwareKey::WindowDown);
    s.observed(vec![win(1, GAME, "Top"), win(2, GAME, "Bottom")]);
    s.applied(Event::tick(100));
    key(&mut s, HardwareKey::WindowUp);
    assert_eq!(s.bottom().notice().map(|n| &n.what), Some(&NoticeKind::BottomRule(false)));
}

/// On battery and untouched, it doesn't go off by itself while the refresh rate is calibrated (the
/// calibration has it off meanwhile), only once that is done, nor while something is typed there.
#[test]
fn a_refresh_calibration_or_an_editor_keeps_it_on() {
    let mut s = state();
    on_battery(&mut s, true);
    s.applied(Event::tick(1000));
    let fx = s.applied(Event::Pointer(crate::PointerEvent::Activate(crate::Id::Device(crate::DeviceId::Refresh))));
    assert_eq!(power(&fx), Some(false), "off while calibrating");
    assert_eq!(power(&s.applied(Event::tick(1000 + DIM_AFTER + OFF_AFTER))), None, "calibrating");
    assert!(!s.bottom().off());
    assert_eq!(power(&s.applied(Event::RefreshCalibrated(Ok(String::new())))), None, "done: off by itself, so it stays off");
    assert!(s.bottom().off());
    let mut s = state();
    on_battery(&mut s, true);
    s.applied(Event::tick(1000));
    let step = crate::setup::account::PasswordStep::Current;
    let editor = crate::Editor { purpose: crate::bottom::editor::Purpose::AccountPassword(step), text: String::new() };
    s.applied(Event::EditOnKeyboard(editor));
    assert_eq!(power(&s.applied(Event::tick(1000 + DIM_AFTER + OFF_AFTER))), None, "typing");
    let fx = s.applied(Event::Pointer(crate::PointerEvent::Activate(crate::Id::Editor(crate::EditorId::Cancel))));
    assert_eq!(power(&fx), None, "the cancel is a touch: in use again");
    assert_eq!(power(&s.applied(Event::tick(1000 + 2 * (DIM_AFTER + OFF_AFTER)))), Some(false), "then off by itself");
}

/// The Wake tap lasts until the next game: a game starting lets the screen go dark again.
#[test]
fn a_game_starting_lets_a_woken_screen_go_dark_again() {
    let mut s = state();
    s.settings_mut().global.set(Setting::BottomScreen, BottomWhenIdle::DarkInGame.value());
    crate::testkit::launch(&mut s, "zelda-lttp");
    assert!(s.bottom_dark());
    s.applied(Event::Pointer(crate::PointerEvent::Activate(crate::Id::Wake)));
    assert!(!s.bottom_dark(), "woken");
    crate::testkit::launch(&mut s, "ff6");
    assert!(s.bottom_dark(), "another game: dark again");
}

#[test]
fn moving_a_window_down_from_another_tab_opens_the_windows_tab_with_the_screen_on() {
    let mut s = emulator();
    tap(&mut s, crate::Id::Tab(Tab::Settings));
    key(&mut s, HardwareKey::BottomPower);
    assert!(s.bottom().off());
    let fx = key(&mut s, HardwareKey::WindowDown);
    assert_eq!(s.bottom().tab(), Tab::Windows, "{fx:?}");
    assert!(!s.bottom().off(), "not moved to a dark panel");
    assert!(s.bottom().shows_windows(s.session()));
    // Back to the settings, where it was moved from.
    key(&mut s, HardwareKey::BottomView);
    assert_eq!(s.bottom().tab(), Tab::Settings);
}

#[test]
fn the_keyboard_doesnt_come_up_over_a_window_shown_below() {
    let mut s = emulator();
    key(&mut s, HardwareKey::WindowDown);
    s.observed(vec![win(1, GAME, "Top"), win(2, GAME, "Bottom")]);
    assert!(s.bottom().shows_windows(s.session()));
    s.applied(Event::TextInput(true));
    assert_eq!(s.bottom().tab(), Tab::Windows, "the window keeps the screen");
    // With nothing below, the windows' tab gives way to it.
    let mut s = emulator();
    tap(&mut s, crate::Id::Tab(Tab::Windows));
    s.applied(Event::TextInput(true));
    assert_eq!(s.bottom().tab(), Tab::Keyboard);
}

#[test]
fn the_windows_tab_with_nothing_below_shows_no_window() {
    let mut s = emulator();
    key(&mut s, HardwareKey::BottomView);
    assert_eq!(s.bottom().tab(), Tab::Windows, "opened even with nothing below: it says how to move one");
    assert!(!s.bottom().shows_windows(s.session()));
    key(&mut s, HardwareKey::BottomView);
    assert_eq!(s.bottom().tab(), Tab::Sysinfo);
}
