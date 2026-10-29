//! What the pad does on ClamShell's screens as their views draw them: B, Y, X and Start, the folders in a
//! pick, and where the focus starts (see clamshell-ui's `input::focus` and `toolkit::PadOut`).

use clamshell_state::settings::Setting;
use clamshell_state::{Event, Id, Init, LibraryViewId, PadInput, PointerEvent, SettingsId, State, Tab};
use clamshell_ui::Screen;
use clamshell_ui::input::focus::{back, default_focus, navigate, shortcut};
use clamshell_ui::input::nav::Dir;
use clamshell_ui::screens::Screens;

fn state() -> State {
    State::new(Init { library: Some(clamshell_library::demo::library()), ..Init::default() })
}

fn entry(s: &State, i: usize) -> Id {
    Id::LibraryView(LibraryViewId::Entry(s.library_view().entry_at(i).map(|g| g.id.clone()).unwrap_or_else(|| "?".into())))
}

fn focus(s: &mut State, id: Id) {
    s.apply(PointerEvent::Focus(id).into(), &mut Vec::new());
}

/// Both screens drawn for `s`, as the views draw them.
fn frames(s: &State) -> clamshell_ui::PadView {
    clamshell_ui::PadFrames::new(crate::VIEWS, (1280, 720), (1280, 720)).record(s)
}

#[test]
fn b_on_library_views_actions_goes_back_to_the_entry() {
    // (On the actions, their badges are faded: Y, X and Start do nothing.)
    let mut s = state();
    focus(&mut s, Id::LibraryView(LibraryViewId::Launch));
    let g = entry(&s, s.library_view().cursor().index);
    assert_eq!(back(&s, &frames(&s)), Some(PointerEvent::Focus(g).into()));
    assert_eq!(shortcut(&s, &frames(&s), PadInput::Y), vec![], "the badges faded: nothing");
}

#[test]
fn y_x_and_start_act_on_library_view_only_while_the_pad_is_there() {
    let mut s = state();
    let g = entry(&s, 0);
    focus(&mut s, g);
    let favourite = Id::LibraryView(LibraryViewId::Favourite);
    assert_eq!(shortcut(&s, &frames(&s), PadInput::Y), vec![PointerEvent::Activate(favourite).into()]);
    focus(&mut s, Id::Tab(Tab::Sysinfo));
    assert_eq!(shortcut(&s, &frames(&s), PadInput::Y), vec![], "the second screen: not the Library's");
    assert_eq!(shortcut(&s, &frames(&s), PadInput::X), vec![]);
    // Typing with the pad: X is Backspace.
    s.apply(PointerEvent::Activate(Id::Tab(Tab::Keyboard)).into(), &mut Vec::new());
    focus(&mut s, Id::Tab(Tab::Keyboard));
    let backspace = clamshell_state::bottom::keyboard::find(clamshell_state::bottom::keyboard::Key::Named(clamshell_state::bottom::keyboard::Named::Backspace));
    let d = frames(&s);
    assert_eq!(shortcut(&s, &d, PadInput::X), backspace.map(|b| PointerEvent::Activate(Id::Key(b)).into()).into_iter().collect::<Vec<_>>());
    assert_eq!(shortcut(&s, &d, PadInput::Y), vec![]);
}

/// B goes back on the screen the pad is on, as it shows: a keymap's screen left open on the settings
/// tab isn't gone back in from another tab.
#[test]
fn b_goes_back_only_in_what_is_shown() {
    use clamshell_state::profiles::picker::{Kind, ProfileId};
    let mut s = state();
    s.apply(PointerEvent::Activate(Id::Tab(Tab::Settings)).into(), &mut Vec::new());
    s.apply(PointerEvent::LongPress(Id::Settings(SettingsId::Tile(Setting::Keymap))).into(), &mut Vec::new());
    focus(&mut s, Id::Tab(Tab::Settings));
    let keymap_back = PointerEvent::Activate(Id::Profile(Kind::Keymap, ProfileId::Back)).into();
    assert_eq!(back(&s, &frames(&s)), Some(keymap_back), "the keymaps' screen shown");
    s.apply(PointerEvent::Activate(Id::Tab(Tab::Dock)).into(), &mut Vec::new());
    focus(&mut s, Id::Tab(Tab::Dock));
    assert_eq!(back(&s, &frames(&s)), None, "the dock shown: nothing to go back from");
}

/// Picking, B cancels, Left goes out of the open folder and Right opens the selected one.
#[test]
fn picking_b_cancels_and_left_and_right_go_through_folders() {
    let mut s = state();
    let files = s.library_view().library().collections.iter().position(|c| c.name == "Files").unwrap_or(0);
    s.apply(PointerEvent::Activate(Id::LibraryView(LibraryViewId::Collection(files as u16))).into(), &mut Vec::new());
    s.apply(PointerEvent::Activate(Id::LibraryView(LibraryViewId::Launch)).into(), &mut Vec::new());
    assert!(s.library_view().group().is_some(), "in Home");
    s.apply(
        Event::Portal(clamshell_state::PortalEvent::Asked(clamshell_state::FileRequest {
            id: 1,
            title: String::new(),
            asks: clamshell_state::Asks::Open { folder: true },
            patterns: Vec::new(),
            asker: Vec::new(),
            folder: Some("/home/thor".into()),
        })),
        &mut Vec::new(),
    );
    assert!(s.library_view().pick().is_some());
    let g = entry(&s, s.library_view().cursor().index);
    focus(&mut s, g);
    let d = frames(&s);
    assert_eq!(back(&s, &d), Some(PointerEvent::Activate(Id::LibraryView(LibraryViewId::CancelPick)).into()));
    let memory = Screens::default();
    assert_eq!(navigate(&s, &d, &memory, Dir::Left), Some(PointerEvent::Activate(Id::LibraryView(LibraryViewId::Up)).into()));
}

/// The Device view's focus starts on its first row.
#[test]
fn the_device_views_focus_starts_on_its_first_row() {
    let account = clamshell_state::setup::account::Account::NoHelper;
    let mut s = State::new(Init { library: Some(clamshell_library::demo::library()), account, ..Init::default() });
    s.apply(Event::Entry("builtin:device".into(), clamshell_state::EntryAct::Play), &mut Vec::new());
    assert_eq!(s.top(), clamshell_state::Top::App { app: clamshell_state::BuiltinApp::Device, view: 0 });
    assert_eq!(default_focus(&frames(&s), Screen::Top), Some(Id::Device(clamshell_state::DeviceId::Wifi)));
}

/// When the main screen shows another view, the focus starts where that view says: the Device
/// view's first row, the Library's selection, the edit mode's row.
#[test]
#[allow(clippy::expect_used)]
fn the_focus_starts_where_the_view_shown_says() {
    use clamshell_state::library_view::edit::EditId;
    use clamshell_state::{DeviceId, EntryAct, Init};
    let account = clamshell_state::setup::account::Account::NoHelper;
    let state = State::new(Init { library: Some(clamshell_library::demo::library()), account, ..Init::default() });
    let mut core = clamshell_ui::Core::new(state, crate::VIEWS, || {}).expect("a core");
    for screen in Screen::BOTH {
        core.resize(screen, (1280, 720), 1);
    }
    let device = || Event::Entry("builtin:device".into(), EntryAct::Play);
    core.handle(device());
    assert_eq!(core.state().pointer().focus(), Some(&Id::Device(DeviceId::Wifi)));
    core.handle(Event::Home);
    let selection = core.state().library_view().selected().cloned().map(|id| Id::LibraryView(clamshell_state::LibraryViewId::Entry(id)));
    assert_eq!(core.state().pointer().focus(), selection.as_ref());
    core.handle(device());
    core.handle(PointerEvent::Activate(Id::Device(DeviceId::EditLibrary)).into());
    let row = core.state().library_view().edit().map(|e| Id::from(EditId::Row(e.cursor() as u16)));
    assert_eq!(core.state().pointer().focus(), row.as_ref());
}

/// In the Library's list, up and down step through the games, counted from the focused one; left and right
/// switch tabs; past the last game, down goes on by direction to the second screen. A shoulder moves a
/// page and a trigger to the end.
#[test]
fn the_librarys_list_takes_the_pads_moves() {
    use clamshell_state::LibraryViewEvent;
    let mut s = state();
    let memory = Screens::default();
    let g = entry(&s, 0);
    focus(&mut s, g);
    let d = frames(&s);
    assert_eq!(navigate(&s, &d, &memory, Dir::Down), Some(LibraryViewEvent::ListStep(1).into()));
    assert_eq!(navigate(&s, &d, &memory, Dir::Right), Some(LibraryViewEvent::CollectionStep(1).into()));
    let page = s.library_view().rows() as i32;
    assert_eq!(clamshell_ui::input::focus::shoulder(&s, &d, -1), vec![LibraryViewEvent::ListStep(-page).into()], "a page");
    let len = s.library_view().list().len() as i32;
    assert_eq!(clamshell_ui::input::focus::trigger(&s, &d, 1), vec![LibraryViewEvent::ListStep(len).into()], "the last entry");
    // The focus came up from the second screen onto entry 1, while entry 0 is selected.
    let g = entry(&s, 1);
    focus(&mut s, g);
    let d = frames(&s);
    assert_eq!(navigate(&s, &d, &memory, Dir::Down), Some(LibraryViewEvent::ListStep(2).into()), "to entry 2");
    assert_eq!(navigate(&s, &d, &memory, Dir::Up), Some(LibraryViewEvent::ListStep(0).into()), "to entry 0");
    // The last game: down goes on to the second screen.
    s.apply(LibraryViewEvent::ListStep(len).into(), &mut Vec::new());
    let last = entry(&s, len as usize - 1);
    focus(&mut s, last);
    let d = frames(&s);
    let below = navigate(&s, &d, &memory, Dir::Down);
    assert!(matches!(&below, Some(Event::Pointer(PointerEvent::Focus(id))) if !matches!(id, Id::LibraryView(_))), "{below:?}");
}

/// While a dock tile is moved, the mode the dock draws takes the pad's buttons: Back ends the move as B
/// does, and on a desktop so does Enter, rather than pressing what is focused underneath.
#[test]
fn a_dock_tiles_move_takes_back_and_enter() {
    use clamshell_state::DockId;
    let account = clamshell_state::setup::account::Account::NoHelper;
    let state = State::new(Init { library: Some(clamshell_library::demo::library()), account, ..Init::default() });
    let mut core = clamshell_ui::Core::new(state, crate::VIEWS, || {}).expect("a core");
    for screen in Screen::BOTH {
        core.resize(screen, (1280, 720), 1);
    }
    let start_moving = |core: &mut clamshell_ui::Core| {
        core.handle(PointerEvent::Activate(Id::Tab(Tab::Dock)).into());
        core.handle(PointerEvent::LongPress(Id::Dock(DockId::Entry(0))).into());
        core.handle(PointerEvent::Activate(Id::Dock(DockId::Move)).into());
        assert!(core.state().dock().moving().is_some());
    };
    start_moving(&mut core);
    core.handle(Event::Pad(PadInput::Back));
    assert!(core.state().dock().moving().is_none(), "Back: done");
    start_moving(&mut core);
    core.key(xkeysym::Keysym::Return, true);
    core.key(xkeysym::Keysym::Return, false);
    assert!(core.state().dock().moving().is_none(), "Enter: done");
}

/// A held on a drawn key repeats it, and on a modifier long-presses it, as the keyboard declares.
#[test]
fn a_held_on_a_key_repeats_and_on_a_modifier_long_presses() {
    use clamshell_state::bottom::keyboard::{self, Key, Modifier};
    use clamshell_ui::input::focus::{PadPress, press};
    use clamshell_ui::input::{Action, Input, Source, Timer};
    let mut s = state();
    s.apply(PointerEvent::Activate(Id::Tab(Tab::Keyboard)).into(), &mut Vec::new());
    for (key, repeats) in [(Key::Char('a'), true), (Key::Mod(Modifier::Shift), false)] {
        let id = Id::Key(keyboard::find(key).expect("on the keyboard"));
        focus(&mut s, id.clone());
        let pressed = press(&s, &frames(&s), &Screens::default());
        assert!(matches!(pressed, Some(PadPress::Press(..))), "{key:?}: {pressed:?}");
        let Some(PadPress::Press(pressed, hold)) = pressed else { continue };
        let mut input = Input::default();
        let now = std::time::Instant::now();
        input.press(Source::PadA, pressed, hold, now);
        let held = input.timer(Source::PadA, Timer::Hold, &s, now);
        let expected = if repeats { PointerEvent::Activate(id) } else { PointerEvent::LongPress(id) };
        assert!(held.contains(&Action::Emit(expected.into())), "{key:?}: {held:?}");
    }
}
