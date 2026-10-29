//! Where the pad's focus goes. This covers moving it by direction over the screens' targets, pressing
//! what it is on, the shoulders and View, and on a desktop, keys that stand in for the pad. These are pure
//! functions of the state and what the last frames drew ([`Drawn`]), and they answer with events.
//!
//! [`PadScope`] says where the pad may go. With the Library in front, it may go anywhere on both screens, and
//! in the Library's list up and down step through the games. With anything else in front, it stays on the
//! second screen.

use clamshell_state::{Event, Hold, Id, LevelEvent, PadInput, PointerEvent, RowPart, State};
use xkeysym::Keysym;

use crate::gfx::Rect;
use crate::input::nav::{self, Dir, NavTarget};
use crate::screens::{Screen, Screens};
use crate::toolkit::{Drag, PadMove, PadOut, PadUse, Target};

/// What the last frames drew on each screen, and the screens' sizes in surface pixels.
pub trait Drawn {
    fn targets(&self, screen: Screen) -> &[Target];
    fn size(&self, screen: Screen) -> (f32, f32);
    /// What the pad's buttons do there, as the screen's views declared (see [`PadOut`]).
    fn pad(&self, screen: Screen) -> &PadOut;
}

/// Where the pad's focus may go.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PadScope {
    /// The pad may use both screens and the focus is on a view's list, whose moves are its own (see
    /// [`PadOut::moves`]).
    List,
    /// One of ClamShell's apps is in front, and the focus may go anywhere on both screens.
    BothScreens,
    /// Something else is in front, and the focus stays on the second screen.
    BottomOnly,
}

/// Whether the pad is on targets reached by holding A, such as the Library's actions (see [`PadUse::Held`]).
/// Only while the pad may use the top screen: with anything else in front the Library isn't shown, and the
/// pad goes to the second screen even if the focus was left on an action, as it is after one that starts
/// a game.
fn on_actions(s: &State, d: &impl Drawn) -> bool {
    s.pad().on_top() && d.targets(Screen::Top).iter().any(|t| t.pad == PadUse::Held && s.pointer().focused(&t.id))
}

pub fn scope(s: &State, d: &impl Drawn) -> PadScope {
    let in_list = d.targets(Screen::Top).iter().any(|t| t.pad == PadUse::List && s.pointer().focus() == Some(&t.id));
    match (s.pad().on_top(), in_list) {
        (true, true) => PadScope::List,
        (true, false) => PadScope::BothScreens,
        (false, _) => PadScope::BottomOnly,
    }
}

/// Where the focus goes on `screen` when nothing better is known, as the screen declared: such as the
/// Library's selected game, or the open tab's button.
pub fn default_focus(d: &impl Drawn, screen: Screen) -> Option<Id> {
    d.pad(screen).start.clone()
}

/// The screen whose buttons the pad's B and Y, X and Start act on: the one the focus is on, or the second
/// screen while the pad may not use the top one.
fn pad_screen(s: &State, d: &impl Drawn) -> Screen {
    if s.pad().on_top() { focus_screen(s, d) } else { Screen::Bottom }
}

/// Whether the last frame drew `id` on `screen`, or on either screen if `screen` is `None`.
pub fn drawn_on(d: &impl Drawn, screen: Option<Screen>, id: &Id) -> bool {
    drawn_in(d, screen, id).is_some()
}

/// The target `id` as the last frame drew it on `screen`, or on either screen if `screen` is `None`.
fn drawn_in<'d>(d: &'d impl Drawn, screen: Option<Screen>, id: &Id) -> Option<&'d Target> {
    Screen::BOTH.into_iter().filter(|s| screen.is_none_or(|on| on == *s)).find_map(|s| d.targets(s).iter().find(|t| &t.id == id))
}

/// The screen the focus is on. It is the top one when the focus isn't on anything drawn.
pub fn focus_screen(s: &State, d: &impl Drawn) -> Screen {
    match s.pointer().focus() {
        Some(focus) if drawn_on(d, Some(Screen::Bottom), focus) && !drawn_on(d, Some(Screen::Top), focus) => Screen::Bottom,
        _ => Screen::Top,
    }
}

/// Moves the focus onto the second screen. It stays where it is if that is drawn there. Otherwise it
/// goes where it last was on that screen, as kept in `memory`, if that is still drawn, and failing that
/// to where the screen declared it starts, or the first target there the pad moves to. With nothing
/// there, it stays.
pub fn focus_bottom(s: &State, d: &impl Drawn, memory: &Screens<Option<Id>>) -> Option<Event> {
    let bottom = Some(Screen::Bottom);
    let current = s.pointer().focus().cloned().filter(|id| drawn_on(d, bottom, id));
    let remembered = memory.bottom.clone().filter(|id| drawn_on(d, bottom, id));
    let first = || first_moved_to(d, Screen::Bottom);
    let target = current.or(remembered).or_else(|| default_focus(d, Screen::Bottom)).or_else(first)?;
    Some(PointerEvent::Focus(target).into())
}

/// The first target on `screen` the pad moves to: not one only touch reaches, or one reached by holding A.
fn first_moved_to(d: &impl Drawn, screen: Screen) -> Option<Id> {
    d.targets(screen).iter().find(|t| matches!(t.pad, PadUse::Moves | PadUse::List)).map(|t| t.id.clone())
}

/// Moves the focus to the other screen, with the Library in front. It goes where it last was on that screen if
/// that is still drawn, else to the screen's default, else to the first thing drawn there. Where it was
/// is remembered in `memory`.
pub fn switch_screen(s: &State, d: &impl Drawn, memory: &mut Screens<Option<Id>>) -> Option<Event> {
    let from = focus_screen(s, d);
    let to = from.other();
    memory[from] = s.pointer().focus().cloned();
    let drawn = |id: &Id| drawn_on(d, Some(to), id);
    let remembered = memory[to].clone().filter(drawn);
    let target = remembered.or_else(|| default_focus(d, to).filter(drawn)).or_else(|| first_moved_to(d, to))?;
    Some(PointerEvent::Focus(target).into())
}

/// The targets the pad may move between, each with its screen, as the views marked them (see [`PadUse`]).
/// On targets reached by holding A, they are those alone. Otherwise they are the targets it moves to on
/// the screens the pad may use.
pub fn candidates<'d>(s: &State, d: &'d impl Drawn) -> Vec<(Screen, &'d Target)> {
    let (screens, pad): (&[Screen], PadUse) = if on_actions(s, d) {
        (&[Screen::Top], PadUse::Held)
    } else if scope(s, d) == PadScope::BottomOnly {
        (&[Screen::Bottom], PadUse::Moves)
    } else {
        (&Screen::BOTH, PadUse::Moves)
    };
    let reached = |t: &Target| t.pad == pad || (pad == PadUse::Moves && t.pad == PadUse::List);
    screens.iter().flat_map(|&screen| d.targets(screen).iter().map(move |t| (screen, t))).filter(|(_, t)| reached(t)).collect()
}

/// `t` on `screen` placed in the plane [`nav::step`] works in.
fn placed(d: &impl Drawn, screen: Screen, t: &Target) -> NavTarget {
    let (w, h) = d.size(screen);
    let (w, h) = (w.max(1.0), h.max(1.0));
    let y0 = match screen {
        Screen::Top => 0.0,
        Screen::Bottom => 1.0,
    };
    NavTarget { screen, rect: Rect::new(t.rect.x / w, y0 + t.rect.y / h, t.rect.w / w, t.rect.h / h), id: t.id.clone() }
}

/// Pad-style focus movement over the screens' targets, as if the second screen were right below the
/// main one.
pub fn navigate(s: &State, d: &impl Drawn, memory: &Screens<Option<Id>>, dir: Dir) -> Option<Event> {
    // On the Library's actions, reached by holding A, up and down move through them only.
    if on_actions(s, d) {
        let rows: Vec<NavTarget> = candidates(s, d).into_iter().map(|(screen, t)| placed(d, screen, t)).collect();
        return match dir {
            Dir::Up | Dir::Down => nav::step(&rows, s.pointer().focus(), dir).map(|id| PointerEvent::Focus(id).into()),
            Dir::Left | Dir::Right => None,
        };
    }
    let scope = scope(s, d);
    // On a view's list, its moves are its own; one it doesn't take goes by direction, as down past the
    // Library's last game goes on to the second screen.
    if scope == PadScope::List
        && let Some(taken) = d.pad(Screen::Top).moves.and_then(|moves| moves(s, PadMove::Dir(dir)))
    {
        return taken;
    }
    // Away from the Library, only the second screen is used. If the focus is elsewhere, it moves there first.
    if scope == PadScope::BottomOnly && focus_screen(s, d) != Screen::Bottom {
        return focus_bottom(s, d, memory);
    }
    // On a drawn target that sets a level, left and right step the level.
    if let (Some(focus), Dir::Left | Dir::Right) = (s.pointer().focus(), dir)
        && let Some(Drag::Level(level, _)) = drawn_in(d, None, focus).and_then(|t| t.drag)
    {
        let step = if dir == Dir::Left { -1 } else { 1 };
        return Some(LevelEvent::Step(level, step).into());
    }
    // In a scrolled list, up and down go to the row above or below, whether it is shown or not; the
    // list scrolls to it. Past either end, the focus moves on by direction.
    if let (Some(focus), Dir::Up | Dir::Down) = (s.pointer().focus(), dir)
        && let Some((list, i)) = s.lists().of(focus)
        && drawn_on(d, None, focus)
    {
        let next = if dir == Dir::Up { i.checked_sub(1) } else { Some(i + 1).filter(|&n| n < s.lists().len(list)) };
        if let Some(id) = next.and_then(|n| s.lists().sibling(focus, n)) {
            // A row that is drawn without that part, such as a keymap without Edit, gets the focus on
            // the row itself.
            let id = match id {
                Id::Row { list, i, part } if part != RowPart::Main && !drawn_on(d, None, &id) && drawn_on(d, None, &list.row(i)) => {
                    Id::Row { list, i, part: RowPart::Main }
                }
                id => id,
            };
            return Some(PointerEvent::Focus(id).into());
        }
    }
    let all: Vec<NavTarget> = candidates(s, d).into_iter().map(|(screen, t)| placed(d, screen, t)).collect();
    nav::step(&all, s.pointer().focus(), dir).map(|id| PointerEvent::Focus(id).into())
}

/// What the pad's A, or Enter, does. It presses the focused target if that is drawn where the pad may
/// go. Otherwise it brings the focus back there first, so that a target drawn by a closed tab isn't
/// pressed unseen.
#[derive(Debug, Clone, PartialEq)]
pub enum PadPress {
    /// Presses the target, which holding does as it was drawn to.
    Press(Id, Hold),
    Focus(Event),
}

pub fn press(s: &State, d: &impl Drawn, memory: &Screens<Option<Id>>) -> Option<PadPress> {
    let screen = (scope(s, d) == PadScope::BottomOnly).then_some(Screen::Bottom);
    match s.pointer().focus() {
        Some(id) if let Some(t) = drawn_in(d, screen, id) => Some(PadPress::Press(id.clone(), t.hold)),
        _ if s.pad().on_top() => default_focus(d, Screen::Top).map(|id| PadPress::Focus(PointerEvent::Focus(id).into())),
        _ => focus_bottom(s, d, memory).map(PadPress::Focus),
    }
}

/// The shoulders. On the Library they move a page through the list. Elsewhere they step through the tabs
/// the second screen declared (see [`PadOut::tabs`]), from the open one, opening and focusing each.
pub fn shoulder(s: &State, d: &impl Drawn, step: i32) -> Vec<Event> {
    // On the Library's actions, the Library stays where it is.
    if on_actions(s, d) {
        return vec![];
    }
    // On a screen with a list that pages, a page.
    if let Some(taken) = list_moves(s, d, PadMove::Page(step)) {
        return taken.into_iter().collect();
    }
    let tabs = &d.pad(Screen::Bottom).tabs;
    let n = tabs.iter().flatten().count() as i32;
    if n == 0 {
        return vec![];
    }
    let at = tabs.iter().flatten().position(|t| t.current).unwrap_or(0) as i32;
    let next = (at + step).rem_euclid(n) as usize;
    match tabs.iter().flatten().nth(next) {
        Some(tab) => vec![PointerEvent::Activate(tab.open.clone()).into(), PointerEvent::Focus(tab.focus.clone()).into()],
        None => vec![],
    }
}

/// What the top screen's list does with `m` while the pad is on that screen, if it takes it.
fn list_moves(s: &State, d: &impl Drawn, m: PadMove) -> Option<Option<Event>> {
    let on_top = s.pad().on_top() && focus_screen(s, d) == Screen::Top;
    d.pad(Screen::Top).moves.filter(|_| on_top).and_then(|moves| moves(s, m))
}

/// L2 and R2 go to the Library's first or last entry.
pub fn trigger(s: &State, d: &impl Drawn, step: i32) -> Vec<Event> {
    if on_actions(s, d) {
        return vec![];
    }
    if let Some(taken) = list_moves(s, d, PadMove::End(step)) {
        return taken.into_iter().collect();
    }
    vec![]
}

/// Y, X and Start press what their badges show on the screen the pad is on (see [`pad_screen`]): on the
/// Library the details' actions (favourite, on the dock and Continue; for a browsed file Copy, Paste and
/// Select), picking Save here and Save as; with a keyboard drawn, X is Backspace. Elsewhere they do nothing.
pub fn shortcut(s: &State, d: &impl Drawn, button: PadInput) -> Vec<Event> {
    let i = match button {
        PadInput::Y => 0,
        PadInput::X => 1,
        PadInput::Start => 2,
        _ => return vec![],
    };
    let keys = &d.pad(pad_screen(s, d)).keys;
    keys.get(i).cloned().flatten().map(|id| PointerEvent::Activate(id).into()).into_iter().collect()
}

/// B, and the Back button, go back as the screens declared: an editor's Cancel first, on either screen
/// the pad may use; otherwise the back of the screen the pad is on (see [`pad_screen`]), such as a tab's
/// page or menu, the edit mode's form or folder, a pick's Cancel, Setup's step, the Device view's Close,
/// or the Library's actions, typed search or open group.
pub fn back(s: &State, d: &impl Drawn) -> Option<Event> {
    let usable = |screen: &Screen| *screen == Screen::Bottom || s.pad().on_top();
    let over = Screen::BOTH.into_iter().filter(usable).find_map(|screen| d.pad(screen).back.as_ref().filter(|(_, over)| *over));
    over.or(d.pad(pad_screen(s, d)).back.as_ref()).map(|(event, _)| event.clone())
}

/// On a desktop, keys stand in for the pad. The arrows move the focus, Enter or Space is A (press and
/// hold), Tab is View, Backspace is B, Q and E are the shoulders, Home and End the triggers, F is Y, P is
/// X and C is Start. This is not done on the device, where keys reaching the Library's window are typing.
const DESKTOP_KEYS: [(Keysym, PadInput); 16] = [
    (Keysym::Left, PadInput::Left),
    (Keysym::Right, PadInput::Right),
    (Keysym::Up, PadInput::Up),
    (Keysym::Down, PadInput::Down),
    (Keysym::Return, PadInput::A(true)),
    (Keysym::KP_Enter, PadInput::A(true)),
    (Keysym::space, PadInput::A(true)),
    (Keysym::f, PadInput::Y),
    (Keysym::p, PadInput::X),
    (Keysym::c, PadInput::Start),
    (Keysym::Home, PadInput::Trigger(-1)),
    (Keysym::End, PadInput::Trigger(1)),
    (Keysym::Tab, PadInput::Switch),
    (Keysym::BackSpace, PadInput::B),
    (Keysym::q, PadInput::Shoulder(-1)),
    (Keysym::e, PadInput::Shoulder(1)),
];

/// The pad input a desktop key stands for. A is always given as pressed; the caller knows key up from
/// key down.
pub fn desktop_key(sym: Keysym) -> Option<PadInput> {
    DESKTOP_KEYS.iter().find(|(k, _)| *k == sym).map(|(_, input)| *input)
}

#[cfg(test)]
#[allow(clippy::indexing_slicing)]
mod tests {
    use super::*;
    use clamshell_state::LibraryViewId;
    use clamshell_state::settings::Setting;
    use clamshell_state::{Init, ListId, SettingsId, Tab};

    /// Targets on each screen, with both screens 100 by 100, and what the pad's buttons do there.
    struct Fake(Screens<Vec<Target>>, Screens<PadOut>);

    impl Drawn for Fake {
        fn targets(&self, screen: Screen) -> &[Target] {
            &self.0[screen]
        }

        fn size(&self, _: Screen) -> (f32, f32) {
            (100.0, 100.0)
        }

        fn pad(&self, screen: Screen) -> &PadOut {
            &self.1[screen]
        }
    }

    fn t(x: f32, y: f32, id: Id) -> Target {
        Target { rect: Rect::new(x, y, 10.0, 10.0), id, drag: None, list: None, item: None, pad: PadUse::Moves, hold: Hold::Long }
    }

    /// `t`, a list's (see [`PadUse::List`]).
    fn list(t: Target) -> Target {
        Target { pad: PadUse::List, ..t }
    }

    /// `t`, reached only by holding A.
    fn held(t: Target) -> Target {
        Target { pad: PadUse::Held, ..t }
    }

    fn state() -> State {
        State::new(Init { library: Some(clamshell_library::demo::library()), ..Init::default() })
    }

    fn entry(s: &State, i: usize) -> Id {
        Id::LibraryView(LibraryViewId::Entry(s.library_view().entry_at(i).map(|g| g.id.clone()).unwrap_or_else(|| "?".into())))
    }

    fn drawn(s: &State) -> Fake {
        let start = |id: Id| PadOut { start: Some(id), ..PadOut::default() };
        Fake(
            Screens {
                top: vec![list(t(0.0, 0.0, entry(s, 0))), list(t(0.0, 20.0, entry(s, 1)))],
                bottom: vec![
                    t(0.0, 0.0, Id::Tab(Tab::Settings)),
                    t(50.0, 0.0, Id::Tab(Tab::Sysinfo)),
                    t(0.0, 50.0, Id::Settings(SettingsId::Tile(Setting::Power))),
                ],
            },
            Screens { top: start(entry(s, s.library_view().cursor().index)), bottom: PadOut { tabs: tab_stops(s), ..start(Id::Tab(s.bottom().tab())) } },
        )
    }

    /// Every tab, each opened and focused by its button, as the bar declares them.
    fn tab_stops(s: &State) -> [Option<crate::toolkit::TabStop>; crate::toolkit::MAX_TABS] {
        let mut tabs = <[Option<crate::toolkit::TabStop>; crate::toolkit::MAX_TABS]>::default();
        for (slot, tab) in tabs.iter_mut().zip(Tab::ALL) {
            *slot = Some(crate::toolkit::TabStop { open: Id::Tab(tab), focus: Id::Tab(tab), current: tab == s.bottom().tab() });
        }
        tabs
    }

    fn focus(s: &mut State, id: Id) {
        s.apply(PointerEvent::Focus(id).into(), &mut Vec::new());
    }

    #[test]
    fn view_switches_screens_and_comes_back_where_it_was() {
        let mut s = state();
        let d = drawn(&s);
        let mut memory = Screens::default();
        let g = entry(&s, 1);
        focus(&mut s, g);
        // Nothing is remembered below, so the focus goes to the second screen's open tab.
        let to_bottom = switch_screen(&s, &d, &mut memory);
        assert_eq!(to_bottom, Some(PointerEvent::Focus(Id::Tab(s.bottom().tab())).into()));
        let tile = Id::Settings(SettingsId::Tile(Setting::Power));
        focus(&mut s, tile.clone());
        assert_eq!(scope(&s, &d), PadScope::BothScreens);
        // Back up, to the game it was on.
        assert_eq!(switch_screen(&s, &d, &mut memory), Some(PointerEvent::Focus(entry(&s, 1)).into()));
        let g = entry(&s, 0);
        focus(&mut s, g);
        // Down again, to the tile it was on.
        assert_eq!(switch_screen(&s, &d, &mut memory), Some(PointerEvent::Focus(tile).into()));
        assert_eq!(memory.top, Some(entry(&s, 0)));
    }

    /// With nothing remembered or declared to start at, View goes to the first target the pad moves to,
    /// past ones only touch reaches.
    #[test]
    fn view_goes_to_the_first_target_the_pad_moves_to() {
        let mut s = state();
        let mut d = drawn(&s);
        d.1.bottom.start = None;
        let touch = Target { pad: PadUse::Touch, ..t(0.0, 0.0, Id::Tab(Tab::Wifi)) };
        d.0.bottom.insert(0, touch);
        let g = entry(&s, 0);
        focus(&mut s, g);
        assert_eq!(switch_screen(&s, &d, &mut Screens::default()), Some(PointerEvent::Focus(Id::Tab(Tab::Settings)).into()));
    }

    /// With nothing remembered or declared to start at, the focus comes to the second screen's first
    /// target the pad moves to; with nothing there, it stays.
    #[test]
    fn focus_bottom_goes_to_the_first_target_without_a_start() {
        let mut s = state();
        let mut d = drawn(&s);
        d.1.bottom.start = None;
        let g = entry(&s, 0);
        focus(&mut s, g);
        assert_eq!(focus_bottom(&s, &d, &Screens::default()), Some(PointerEvent::Focus(Id::Tab(Tab::Settings)).into()));
        d.0.bottom.clear();
        assert_eq!(focus_bottom(&s, &d, &Screens::default()), None);
    }

    #[test]
    fn a_focus_that_is_gone_is_recovered() {
        let mut s = state();
        let d = drawn(&s);
        let memory = Screens { top: None, bottom: Some(Id::Tab(Tab::Wifi)) };
        // Focused on something no longer drawn, A brings the focus back first. The remembered tab isn't
        // drawn either, so the focus goes to the open tab.
        focus(&mut s, Id::Settings(SettingsId::Env));
        assert_eq!(press(&s, &d, &memory), Some(PadPress::Focus(PointerEvent::Focus(entry(&s, 0)).into())));
        assert_eq!(focus_bottom(&s, &d, &memory), Some(PointerEvent::Focus(Id::Tab(s.bottom().tab())).into()));
        // A drawn target is pressed.
        focus(&mut s, Id::Tab(Tab::Sysinfo));
        assert_eq!(press(&s, &d, &memory), Some(PadPress::Press(Id::Tab(Tab::Sysinfo), Hold::Long)));
        // Moving from a lost focus starts at the first target.
        focus(&mut s, Id::Settings(SettingsId::Env));
        assert_eq!(navigate(&s, &d, &memory, Dir::Right), Some(PointerEvent::Focus(entry(&s, 0)).into()));
    }

    #[test]
    fn with_a_game_in_front_the_pad_leaves_library_views_actions_for_the_second_screen() {
        let mut s = state();
        // A game started from the Library's actions is in front, with the focus left on an action.
        let key = clamshell_library::key_of(&s.library_view().entry_at(0).map(|e| e.id.to_string()).unwrap_or_default());
        let launch = clamshell_state::LaunchUnit { key, secs: 1, n: 0 };
        s.apply(Event::Units(vec![launch.clone()]), &mut Vec::new());
        let window = clamshell_state::session::windows::WindowFacts { id: 1, app_id: "game".into(), unit: Some(launch.service()), ..Default::default() };
        s.apply(clamshell_state::WindowEvent::Opened(window).into(), &mut Vec::new());
        s.apply(clamshell_state::WindowEvent::Activate(1).into(), &mut Vec::new());
        let actions: Vec<Id> = s.library_view().actions().as_slice().iter().map(|a| Id::LibraryView(a.id())).collect();
        let mut d = drawn(&s);
        d.0.top.extend(actions.iter().enumerate().map(|(i, id)| t(60.0, 15.0 * i as f32, id.clone())));
        focus(&mut s, actions[0].clone());
        assert!(s.library_view().in_actions(s.pointer()));
        assert!(!s.pad().on_top(), "a game is in front");
        let memory = Screens::default();
        assert_eq!(navigate(&s, &d, &memory, Dir::Down), Some(PointerEvent::Focus(Id::Tab(s.bottom().tab())).into()));
        assert!(candidates(&s, &d).iter().all(|(screen, _)| *screen == Screen::Bottom));
        assert!(!shoulder(&s, &d, 1).is_empty(), "the shoulders step the tabs");
    }

    #[test]
    fn up_and_down_from_a_rows_part_go_to_the_next_row_itself_where_it_has_no_such_part() {
        use clamshell_state::Reading;
        use clamshell_state::radios::wifi::{Network, Wifi};
        let mut s = state();
        let networks = (0..10).map(|i| Network { ssid: format!("n{i}"), ..Network::default() }).collect();
        s.apply(Reading::Wifi(Wifi { present: true, on: true, networks, ..Wifi::default() }).into(), &mut Vec::new());
        s.apply(PointerEvent::Activate(Id::Tab(Tab::Wifi)).into(), &mut Vec::new());
        let part = |i, part| Id::Row { list: ListId::Wifi, i, part };
        // Row 3 has an Edit part, row 4 doesn't, and row 2 isn't drawn.
        let d = Fake(
            Screens {
                top: vec![],
                bottom: vec![t(0.0, 30.0, part(3, RowPart::Main)), t(50.0, 30.0, part(3, RowPart::Edit)), t(0.0, 40.0, part(4, RowPart::Main))],
            },
            Screens::default(),
        );
        let memory = Screens::default();
        focus(&mut s, part(3, RowPart::Edit));
        assert_eq!(navigate(&s, &d, &memory, Dir::Down), Some(PointerEvent::Focus(part(4, RowPart::Main)).into()));
        assert_eq!(navigate(&s, &d, &memory, Dir::Up), Some(PointerEvent::Focus(part(2, RowPart::Edit)).into()), "not drawn: the same part");
    }

    #[test]
    fn the_shoulders_step_the_tabs_from_the_second_screen_and_page_through_library_view() {
        let mut s = state();
        let d = drawn(&s);
        focus(&mut s, Id::Tab(Tab::Sysinfo));
        let open = s.bottom().tab();
        let at = Tab::ALL.iter().position(|t| *t == open).unwrap_or(0);
        let next = Tab::ALL[(at + 1) % Tab::ALL.len()];
        assert_eq!(
            shoulder(&s, &d, 1),
            vec![PointerEvent::Activate(Id::Tab(next)).into(), PointerEvent::Focus(Id::Tab(next)).into()],
            "its button, as declared"
        );
    }

    /// On a list's targets, the list's moves are its own; a move it doesn't take goes by direction. On its
    /// screen, the shoulders and triggers are its pages and ends.
    #[test]
    fn a_lists_moves_are_its_own_and_one_it_declines_goes_by_direction() {
        let mut s = state();
        let mut d = drawn(&s);
        d.1.top.moves = Some(|_, m| match m {
            PadMove::Dir(Dir::Up) => Some(Some(Event::Home)),
            PadMove::Dir(_) => None,
            PadMove::Page(_) | PadMove::End(_) | PadMove::Scroll(_) => Some(Some(Event::HomeTap)),
        });
        let g = entry(&s, 0);
        focus(&mut s, g);
        let memory = Screens::default();
        assert_eq!(scope(&s, &d), PadScope::List);
        assert_eq!(navigate(&s, &d, &memory, Dir::Up), Some(Event::Home), "taken");
        assert_eq!(navigate(&s, &d, &memory, Dir::Down), Some(PointerEvent::Focus(entry(&s, 1)).into()), "declined: by direction");
        assert_eq!((shoulder(&s, &d, 1), trigger(&s, &d, 1)), (vec![Event::HomeTap], vec![Event::HomeTap]));
    }

    #[test]
    fn on_library_views_actions_the_pad_stays_on_them() {
        let mut s = state();
        let mut d = drawn(&s);
        d.0.top.extend([
            held(t(60.0, 0.0, Id::LibraryView(LibraryViewId::Launch))),
            held(t(60.0, 15.0, Id::LibraryView(LibraryViewId::Favourite))),
            held(t(60.0, 30.0, Id::LibraryView(LibraryViewId::Pin))),
        ]);
        focus(&mut s, Id::LibraryView(LibraryViewId::Launch));
        assert!(s.library_view().in_actions(s.pointer()));
        let memory = Screens::default();
        assert_eq!(navigate(&s, &d, &memory, Dir::Down), Some(PointerEvent::Focus(Id::LibraryView(LibraryViewId::Favourite)).into()));
        assert_eq!(navigate(&s, &d, &memory, Dir::Left), None, "not out to the list");
        focus(&mut s, Id::LibraryView(LibraryViewId::Pin));
        assert_eq!(
            navigate(&s, &d, &memory, Dir::Down),
            Some(PointerEvent::Focus(Id::LibraryView(LibraryViewId::Pin)).into()),
            "not down to the second screen"
        );
        assert_eq!(shoulder(&s, &d, 1), vec![]);
    }

    #[test]
    fn the_pad_reaches_library_views_actions_only_by_a_held() {
        let mut s = state();
        let mut d = drawn(&s);
        // The Library's actions, right beside and below the last game shown.
        d.0.top.extend([held(t(60.0, 20.0, Id::LibraryView(LibraryViewId::Launch))), held(t(60.0, 35.0, Id::LibraryView(LibraryViewId::Favourite)))]);
        let last = entry(&s, 1);
        focus(&mut s, last.clone());
        let memory = Screens::default();
        for dir in [Dir::Down, Dir::Right] {
            let to = navigate(&s, &d, &memory, dir);
            assert!(
                !matches!(&to, Some(Event::Pointer(PointerEvent::Focus(Id::LibraryView(LibraryViewId::Launch | LibraryViewId::Favourite))))),
                "{dir:?}: {to:?}"
            );
        }
    }

    #[test]
    fn up_and_down_in_a_list_go_to_the_next_row_shown_or_not() {
        use clamshell_state::Reading;
        use clamshell_state::radios::wifi::{Network, Wifi};
        let mut s = state();
        let networks = (0..10).map(|i| Network { ssid: format!("n{i}"), ..Network::default() }).collect();
        s.apply(Reading::Wifi(Wifi { present: true, on: true, networks, ..Wifi::default() }).into(), &mut Vec::new());
        s.apply(PointerEvent::Activate(Id::Tab(Tab::Wifi)).into(), &mut Vec::new());
        let row = |i| ListId::Wifi.row(i);
        // Rows 3 and 4 are drawn, with the list's area under them.
        let d =
            Fake(Screens { top: vec![], bottom: vec![t(0.0, 0.0, Id::List(ListId::Wifi)), t(0.0, 30.0, row(3)), t(0.0, 40.0, row(4))] }, Screens::default());
        let memory = Screens::default();
        focus(&mut s, row(4));
        assert_eq!(navigate(&s, &d, &memory, Dir::Down), Some(PointerEvent::Focus(row(5)).into()), "not drawn: the list scrolls to it");
        focus(&mut s, row(3));
        assert_eq!(navigate(&s, &d, &memory, Dir::Up), Some(PointerEvent::Focus(row(2)).into()));
        // Sideways and past the ends, the focus moves by direction but never onto the list's area.
        assert_ne!(navigate(&s, &d, &memory, Dir::Left), Some(PointerEvent::Focus(Id::List(ListId::Wifi)).into()));
    }

    /// Left and right step the level a focused target sets, as its drag declares; on one not drawn they
    /// move the focus.
    #[test]
    fn left_and_right_on_a_drawn_level_step_it() {
        use crate::toolkit::Track;
        use clamshell_state::levels::Level;
        let mut s = state();
        let mut d = drawn(&s);
        let track = Track { x0: 0.0, w: 50.0 };
        d.0.bottom.push(Target { drag: Some(Drag::Level(Level::Stream(2), track)), ..t(0.0, 70.0, Level::Stream(2).id()) });
        let memory = Screens::default();
        focus(&mut s, Level::Stream(2).id());
        assert_eq!(navigate(&s, &d, &memory, Dir::Left), Some(LevelEvent::Step(Level::Stream(2), -1).into()));
        assert_eq!(navigate(&s, &d, &memory, Dir::Right), Some(LevelEvent::Step(Level::Stream(2), 1).into()));
        // Not drawn: the focus moves.
        focus(&mut s, Level::Stream(3).id());
        assert!(matches!(navigate(&s, &d, &memory, Dir::Right), Some(Event::Pointer(PointerEvent::Focus(_)))));
    }

    #[test]
    fn desktop_keys_stand_for_the_pad() {
        assert_eq!(desktop_key(Keysym::Tab), Some(PadInput::Switch));
        assert_eq!(desktop_key(Keysym::space), Some(PadInput::A(true)));
        assert_eq!(desktop_key(Keysym::q), Some(PadInput::Shoulder(-1)));
        assert_eq!(desktop_key(Keysym::x), None);
    }
}
