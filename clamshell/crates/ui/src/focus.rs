//! Where the pad's focus goes: moving it by direction over the screens' targets, pressing what it is
//! on, the shoulders and View; and, on a desktop, keys standing in for the pad. Pure functions of the
//! state and what the last frames drew ([`Drawn`]), answering with events.
//!
//! Where the pad may go ([`PadScope`]): with shelf in front, both screens, and in shelf's list up and
//! down step through the games; with anything else in front, only the second screen.

use clamshell_core::keyboard;
use clamshell_core::levels::Level;
use clamshell_core::{EditorId, Event, Id, LevelEvent, ListId, PadInput, PointerEvent, ShelfEvent, ShelfId, State, Tab};
use smithay_client_toolkit::seat::keyboard::Keysym;

use crate::draw::Rect;
use crate::nav::{self, Dir, NavTarget};
use crate::screens::{Screen, Screens};
use crate::toolkit::Target;

/// What the last frames drew on each screen, and the screens' sizes (in surface pixels).
pub(crate) trait Drawn {
    fn targets(&self, screen: Screen) -> &[Target];
    fn size(&self, screen: Screen) -> (f32, f32);
}

/// Where the pad's focus may go.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum PadScope {
    /// Shelf is in front and the focus is in its list or on its tabs: up and down step through the
    /// games, left and right through the collections.
    ShelfList,
    /// Shelf is in front: anywhere on both screens.
    BothScreens,
    /// Something else is in front: the second screen only.
    BottomOnly,
}

pub(crate) fn scope(s: &State) -> PadScope {
    let in_list = matches!(s.pointer().focus(), Some(Id::Shelf(ShelfId::Game(_) | ShelfId::Collection(_))));
    match (s.pad_on_top(), in_list) {
        (true, true) => PadScope::ShelfList,
        (true, false) => PadScope::BothScreens,
        (false, _) => PadScope::BottomOnly,
    }
}

/// Where the focus goes on `screen` when nothing better is known: shelf's selected game, or the open
/// tab's button.
pub(crate) fn default_focus(s: &State, screen: Screen) -> Option<Id> {
    match screen {
        Screen::Top => s.shelf().selected().cloned().map(|g| Id::Shelf(ShelfId::Game(g))),
        Screen::Bottom => Some(Id::Tab(s.bottom().tab())),
    }
}

/// Whether the last frame drew `id` (on `screen`, or on either).
pub(crate) fn drawn_on(d: &impl Drawn, screen: Option<Screen>, id: &Id) -> bool {
    Screen::BOTH.into_iter().filter(|s| screen.is_none_or(|on| on == *s)).any(|s| d.targets(s).iter().any(|t| &t.id == id))
}

/// The screen the focus is on (the top one when it isn't on anything drawn).
pub(crate) fn focus_screen(s: &State, d: &impl Drawn) -> Screen {
    match s.pointer().focus() {
        Some(focus) if drawn_on(d, Some(Screen::Bottom), focus) && !drawn_on(d, Some(Screen::Top), focus) => Screen::Bottom,
        _ => Screen::Top,
    }
}

/// Focus onto the second screen: where it is if that is drawn there, else where it last was there (in
/// `memory`) if still drawn, else the open tab.
pub(crate) fn focus_bottom(s: &State, d: &impl Drawn, memory: &Screens<Option<Id>>) -> Event {
    let bottom = Some(Screen::Bottom);
    let current = s.pointer().focus().cloned().filter(|id| drawn_on(d, bottom, id));
    let remembered = memory.bottom.clone().filter(|id| drawn_on(d, bottom, id));
    let target = current.or(remembered).or_else(|| default_focus(s, Screen::Bottom)).unwrap_or(Id::Tab(Tab::default()));
    PointerEvent::Focus(target).into()
}

/// Focus to the other screen (with shelf in front): where it last was there if still drawn, else the
/// default there, else the first thing drawn there. Remembers where it was in `memory`.
pub(crate) fn switch_screen(s: &State, d: &impl Drawn, memory: &mut Screens<Option<Id>>) -> Option<Event> {
    let from = focus_screen(s, d);
    let to = from.other();
    memory[from] = s.pointer().focus().cloned();
    let drawn = |id: &Id| drawn_on(d, Some(to), id);
    let remembered = memory[to].clone().filter(drawn);
    let first = d.targets(to).first().map(|t| t.id.clone());
    let target = remembered.or_else(|| default_focus(s, to).filter(drawn)).or(first)?;
    Some(PointerEvent::Focus(target).into())
}

/// Pad-style focus movement over the screens' targets (as if the second screen were right below the
/// main one).
pub(crate) fn navigate(s: &State, d: &impl Drawn, memory: &Screens<Option<Id>>, dir: Dir) -> Option<Event> {
    let scope = scope(s);
    // In shelf's list: up and down move through the games (scrolling), left and right switch
    // collections; down past the last game goes on to the second screen.
    if scope == PadScope::ShelfList {
        let len = s.shelf().list().len();
        let at_end = s.shelf().cursor().index + 1 >= len;
        let step = match dir {
            Dir::Up => Some(ShelfEvent::ListStep(-1)),
            Dir::Down if !at_end => Some(ShelfEvent::ListStep(1)),
            Dir::Left => Some(ShelfEvent::CollectionStep(-1)),
            Dir::Right => Some(ShelfEvent::CollectionStep(1)),
            Dir::Down => None,
        };
        if let Some(step) = step {
            return Some(step.into());
        }
    }
    // Away from shelf, only the second screen: to it first if the focus is elsewhere.
    if scope == PadScope::BottomOnly && focus_screen(s, d) != Screen::Bottom {
        return Some(focus_bottom(s, d, memory));
    }
    // On a level row (drawn), left and right step it.
    if let (Some(focus), Dir::Left | Dir::Right) = (s.pointer().focus(), dir)
        && let Some(level) = Level::of(focus)
        && drawn_on(d, None, focus)
    {
        let step = if dir == Dir::Left { -1 } else { 1 };
        return Some(LevelEvent::Step(level, step).into());
    }
    // In a scrolled list, up and down go to the row above or below, shown or not (the list scrolls to
    // it); past either end, on by direction.
    if let (Some(focus), Dir::Up | Dir::Down) = (s.pointer().focus(), dir)
        && let Some((list, i)) = ListId::of(focus)
        && drawn_on(d, None, focus)
    {
        let next = if dir == Dir::Up { i.checked_sub(1) } else { Some(i + 1).filter(|&n| n < s.list_len(list)) };
        if let Some(id) = next.and_then(|n| u16::try_from(n).ok()).and_then(|n| ListId::sibling(focus, n)) {
            return Some(PointerEvent::Focus(id).into());
        }
    }
    let screens: &[Screen] = if scope == PadScope::BottomOnly { &[Screen::Bottom] } else { &Screen::BOTH };
    let mut all = Vec::new();
    for &screen in screens {
        let (w, h) = d.size(screen);
        let (w, h) = (w.max(1.0), h.max(1.0));
        let y0 = match screen {
            Screen::Top => 0.0,
            Screen::Bottom => 1.0,
        };
        // (A list's area is for fingers: the pad goes to its rows.)
        all.extend(d.targets(screen).iter().filter(|t| !matches!(t.id, Id::List(_))).map(|t| NavTarget {
            screen,
            rect: Rect::new(t.rect.x / w, y0 + t.rect.y / h, t.rect.w / w, t.rect.h / h),
            id: t.id.clone(),
        }));
    }
    nav::step(&all, s.pointer().focus(), dir).map(|id| PointerEvent::Focus(id).into())
}

/// What the pad's A (or Enter) does: press the focused target if it is drawn where the pad may go; else
/// bring the focus back there first (a target a closed tab drew isn't pressed unseen).
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum PadPress {
    Press(Id),
    Focus(Event),
}

pub(crate) fn press(s: &State, d: &impl Drawn, memory: &Screens<Option<Id>>) -> Option<PadPress> {
    let screen = (scope(s) == PadScope::BottomOnly).then_some(Screen::Bottom);
    match s.pointer().focus() {
        Some(id) if drawn_on(d, screen, id) => Some(PadPress::Press(id.clone())),
        _ if s.pad_on_top() => default_focus(s, Screen::Top).map(|id| PadPress::Focus(PointerEvent::Focus(id).into())),
        _ => Some(PadPress::Focus(focus_bottom(s, d, memory))),
    }
}

/// The shoulders: shelf's collections while the focus is on the main screen, else the bar's tabs, left
/// to right (opened and focused).
pub(crate) fn shoulder(s: &State, d: &impl Drawn, step: i32) -> Vec<Event> {
    // Shelf: a page (the rows on screen).
    if s.pad_on_top() && focus_screen(s, d) == Screen::Top {
        let page = i32::try_from(s.shelf().rows()).unwrap_or(1).max(1);
        return vec![ShelfEvent::ListStep(step * page).into()];
    }
    let tabs = Tab::ALL;
    let at = tabs.iter().position(|t| *t == s.bottom().tab()).unwrap_or(0) as i32;
    let next = (at + step).rem_euclid(tabs.len() as i32) as usize;
    match tabs.get(next) {
        Some(&tab) => vec![PointerEvent::Activate(Id::Tab(tab)).into(), PointerEvent::Focus(Id::Tab(tab)).into()],
        None => vec![],
    }
}

/// L2, R2: shelf's first or last entry.
pub(crate) fn trigger(s: &State, d: &impl Drawn, step: i32) -> Vec<Event> {
    if s.pad_on_top() && focus_screen(s, d) == Screen::Top {
        let len = i32::try_from(s.shelf().list().len()).unwrap_or(i32::MAX);
        return vec![ShelfEvent::ListStep(step * len).into()];
    }
    vec![]
}

/// Y, X, Start: the details' actions (see clamshell-core's `actions`: a favourite, on the dock,
/// Continue; a browsed file's Copy, Paste, Select), while the pad is on shelf. On the second screen with
/// the keyboard open (typing with the pad), X is Backspace; elsewhere nothing.
pub(crate) fn shortcut(s: &State, d: &impl Drawn, button: PadInput) -> Vec<Event> {
    let event = match button {
        PadInput::Y => ShelfEvent::ToggleFavourite,
        PadInput::X => ShelfEvent::TogglePin,
        PadInput::Start => ShelfEvent::Continue,
        _ => return vec![],
    };
    if s.pad_on_top() && focus_screen(s, d) == Screen::Top {
        return vec![event.into()];
    }
    let backspace = keyboard::find(keyboard::Key::Named(keyboard::Named::Backspace));
    match (button, backspace) {
        (PadInput::X, Some(pos)) if s.bottom().tab() == Tab::Keyboard => vec![PointerEvent::Activate(Id::Key(pos)).into()],
        _ => vec![],
    }
}

/// B: cancels the editor, if one is open, else goes back one of the keymaps' screens, else (shelf in
/// focus) out of shelf's open group.
pub(crate) fn back(s: &State) -> Option<Event> {
    if s.bottom().editor().is_some() {
        return Some(PointerEvent::Activate(Id::Editor(EditorId::Cancel)).into());
    }
    if s.bottom().tab() == Tab::Time && s.time_region().is_some() {
        return Some(PointerEvent::Activate(Id::TimeRegions).into());
    }
    if s.keymap_screen().is_some() {
        return Some(PointerEvent::Activate(Id::Keymap(clamshell_core::KeymapId::Back)).into());
    }
    // In shelf: what is typed to find an entry cleared, else out of the open group.
    let in_shelf = matches!(s.pointer().focus(), Some(Id::Shelf(_)));
    if in_shelf && !s.shelf().filter().is_empty() {
        return Some(clamshell_core::ShelfEvent::ClearFilter.into());
    }
    (in_shelf && s.shelf().group().is_some()).then(|| PointerEvent::Activate(Id::Shelf(ShelfId::Up)).into())
}

/// On a desktop, keys stand in for the pad: arrows move focus, Enter/Space is A (press, hold), Tab is
/// View, Backspace B, Q and E the shoulders, Home and End the triggers, F is Y, P is X, C is Start. (Not on the device: there what reaches shelf's
/// window is typing.)
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

/// The pad input a desktop key stands for (A as pressed; the caller knows up from down).
pub(crate) fn desktop_key(sym: Keysym) -> Option<PadInput> {
    DESKTOP_KEYS.iter().find(|(k, _)| *k == sym).map(|(_, input)| *input)
}

#[cfg(test)]
#[allow(clippy::indexing_slicing)]
mod tests {
    use super::*;
    use clamshell_core::settings::Setting;
    use clamshell_core::{Init, SettingsId};

    /// Targets on each screen, both screens 100 by 100.
    struct Fake(Screens<Vec<Target>>);

    impl Drawn for Fake {
        fn targets(&self, screen: Screen) -> &[Target] {
            &self.0[screen]
        }

        fn size(&self, _: Screen) -> (f32, f32) {
            (100.0, 100.0)
        }
    }

    fn t(x: f32, y: f32, id: Id) -> Target {
        Target { rect: Rect::new(x, y, 10.0, 10.0), id, track: None, list: None }
    }

    fn state() -> State {
        State::new(Init { library: clamshell_library::demo_library(), ..Init::default() })
    }

    fn game(s: &State, i: usize) -> Id {
        Id::Shelf(ShelfId::Game(s.shelf().game_at(i).map(|g| g.id.clone()).unwrap_or_else(|| "?".into())))
    }

    fn drawn(s: &State) -> Fake {
        Fake(Screens {
            top: vec![t(0.0, 0.0, game(s, 0)), t(0.0, 20.0, game(s, 1))],
            bottom: vec![
                t(0.0, 0.0, Id::Tab(Tab::Settings)),
                t(50.0, 0.0, Id::Tab(Tab::Sysinfo)),
                t(0.0, 50.0, Id::Settings(SettingsId::Tile(Setting::Power))),
            ],
        })
    }

    fn focus(s: &mut State, id: Id) {
        s.apply(PointerEvent::Focus(id).into());
    }

    #[test]
    fn view_switches_screens_and_comes_back_where_it_was() {
        let mut s = state();
        let d = drawn(&s);
        let mut memory = Screens::default();
        let g = game(&s, 1);
        focus(&mut s, g);
        // Nothing remembered below: the second screen's open tab.
        let to_bottom = switch_screen(&s, &d, &mut memory);
        assert_eq!(to_bottom, Some(PointerEvent::Focus(Id::Tab(s.bottom().tab())).into()));
        let tile = Id::Settings(SettingsId::Tile(Setting::Power));
        focus(&mut s, tile.clone());
        assert_eq!(scope(&s), PadScope::BothScreens);
        // Back up: the game it was on.
        assert_eq!(switch_screen(&s, &d, &mut memory), Some(PointerEvent::Focus(game(&s, 1)).into()));
        let g = game(&s, 0);
        focus(&mut s, g);
        // Down again: the tile it was on.
        assert_eq!(switch_screen(&s, &d, &mut memory), Some(PointerEvent::Focus(tile).into()));
        assert_eq!(memory.top, Some(game(&s, 0)));
    }

    #[test]
    fn a_focus_that_is_gone_is_recovered() {
        let mut s = state();
        let d = drawn(&s);
        let memory = Screens { top: None, bottom: Some(Id::Tab(Tab::Wifi)) };
        // Focused on something no longer drawn: A brings the focus back first (not drawn either:
        // remembered, so the open tab).
        focus(&mut s, Id::Settings(SettingsId::Env));
        assert_eq!(press(&s, &d, &memory), Some(PadPress::Focus(PointerEvent::Focus(game(&s, 0)).into())));
        assert_eq!(focus_bottom(&s, &d, &memory), PointerEvent::Focus(Id::Tab(s.bottom().tab())).into());
        // Drawn: pressed.
        focus(&mut s, Id::Tab(Tab::Sysinfo));
        assert_eq!(press(&s, &d, &memory), Some(PadPress::Press(Id::Tab(Tab::Sysinfo))));
        // Moving from a lost focus starts at the first target.
        focus(&mut s, Id::Settings(SettingsId::Env));
        assert_eq!(navigate(&s, &d, &memory, Dir::Right), Some(PointerEvent::Focus(game(&s, 0)).into()));
    }

    #[test]
    fn in_shelfs_list_up_and_down_step_and_the_end_goes_on_down() {
        let mut s = state();
        let d = drawn(&s);
        let memory = Screens::default();
        let g = game(&s, 0);
        focus(&mut s, g);
        assert_eq!(scope(&s), PadScope::ShelfList);
        assert_eq!(navigate(&s, &d, &memory, Dir::Down), Some(ShelfEvent::ListStep(1).into()));
        assert_eq!(navigate(&s, &d, &memory, Dir::Right), Some(ShelfEvent::CollectionStep(1).into()));
        let len = s.shelf().collection().map_or(0, |c| c.games.len());
        s.apply(ShelfEvent::ListStep(len as i32).into());
        let last = game(&s, len - 1);
        focus(&mut s, last.clone());
        // Past the last game: by direction, on to the second screen.
        let mut d = drawn(&s);
        d.0.top.push(t(0.0, 90.0, last));
        let below = navigate(&s, &d, &memory, Dir::Down);
        assert!(matches!(below, Some(Event::Pointer(PointerEvent::Focus(Id::Tab(_))))), "{below:?}");
    }

    #[test]
    fn the_shoulders_step_the_tabs_from_the_second_screen_and_page_through_shelf() {
        let mut s = state();
        let d = drawn(&s);
        focus(&mut s, Id::Tab(Tab::Sysinfo));
        let open = s.bottom().tab();
        let at = Tab::ALL.iter().position(|t| *t == open).unwrap_or(0);
        let next = Tab::ALL[(at + 1) % Tab::ALL.len()];
        assert_eq!(shoulder(&s, &d, 1), vec![PointerEvent::Activate(Id::Tab(next)).into(), PointerEvent::Focus(Id::Tab(next)).into()]);
        let g = game(&s, 0);
        focus(&mut s, g);
        let page = s.shelf().rows() as i32;
        assert_eq!(shoulder(&s, &d, -1), vec![ShelfEvent::ListStep(-page).into()], "in shelf: a page");
        let len = s.shelf().list().len() as i32;
        assert_eq!(trigger(&s, &d, 1), vec![ShelfEvent::ListStep(len).into()], "the last entry");
    }

    #[test]
    fn y_x_and_start_act_on_shelf_only_while_the_pad_is_there() {
        let mut s = state();
        let d = drawn(&s);
        let g = game(&s, 0);
        focus(&mut s, g);
        assert_eq!(shortcut(&s, &d, PadInput::Y), vec![ShelfEvent::ToggleFavourite.into()]);
        focus(&mut s, Id::Tab(Tab::Sysinfo));
        assert_eq!(shortcut(&s, &d, PadInput::Y), vec![], "the second screen: not shelf's");
        assert_eq!(shortcut(&s, &d, PadInput::X), vec![]);
        // Typing with the pad: X is Backspace.
        s.apply(PointerEvent::Activate(Id::Tab(Tab::Keyboard)).into());
        focus(&mut s, Id::Tab(Tab::Sysinfo));
        let backspace = keyboard::find(keyboard::Key::Named(keyboard::Named::Backspace)).map(Id::Key);
        assert_eq!(shortcut(&s, &d, PadInput::X), backspace.map(|b| PointerEvent::Activate(b).into()).into_iter().collect::<Vec<_>>());
        assert_eq!(shortcut(&s, &d, PadInput::Y), vec![]);
    }

    #[test]
    fn up_and_down_in_a_list_go_to_the_next_row_shown_or_not() {
        use clamshell_core::Reading;
        use clamshell_core::wifi::{Network, Wifi};
        let mut s = state();
        let networks = (0..10).map(|i| Network { ssid: format!("n{i}"), ..Network::default() }).collect();
        s.apply(Reading::Wifi(Wifi { device: true, enabled: true, networks, ..Wifi::default() }).into());
        s.apply(PointerEvent::Activate(Id::Tab(Tab::Wifi)).into());
        let row = |i| ListId::Wifi.row(i);
        // Rows 3 and 4 drawn, the list's area under them.
        let d = Fake(Screens { top: vec![], bottom: vec![t(0.0, 0.0, Id::List(ListId::Wifi)), t(0.0, 30.0, row(3)), t(0.0, 40.0, row(4))] });
        let memory = Screens::default();
        focus(&mut s, row(4));
        assert_eq!(navigate(&s, &d, &memory, Dir::Down), Some(PointerEvent::Focus(row(5)).into()), "not drawn: the list scrolls to it");
        focus(&mut s, row(3));
        assert_eq!(navigate(&s, &d, &memory, Dir::Up), Some(PointerEvent::Focus(row(2)).into()));
        // Sideways and past the ends: by direction, never onto the list's area.
        assert_ne!(navigate(&s, &d, &memory, Dir::Left), Some(PointerEvent::Focus(Id::List(ListId::Wifi)).into()));
    }

    #[test]
    fn desktop_keys_stand_for_the_pad() {
        assert_eq!(desktop_key(Keysym::Tab), Some(PadInput::Switch));
        assert_eq!(desktop_key(Keysym::space), Some(PadInput::A(true)));
        assert_eq!(desktop_key(Keysym::q), Some(PadInput::Shoulder(-1)));
        assert_eq!(desktop_key(Keysym::x), None);
    }
}
