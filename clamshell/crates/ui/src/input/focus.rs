//! Where the pad's focus goes. This covers moving it by direction over the screens' targets, pressing
//! what it is on, the shoulders and View, and on a desktop, keys that stand in for the pad. These are pure
//! functions of the state and what the last frames drew ([`Drawn`]), and they answer with events.
//!
//! [`PadScope`] says where the pad may go. With shelf in front, it may go anywhere on both screens, and
//! in shelf's list up and down step through the games. With anything else in front, it stays on the
//! second screen.

use clamshell_state::bottom::keyboard;
use clamshell_state::levels::Level;
use clamshell_state::{DockId, EditorId, Event, Id, LevelEvent, ListId, PadInput, PointerEvent, ShelfEvent, ShelfId, State, Tab};
use smithay_client_toolkit::seat::keyboard::Keysym;

use crate::gfx::Rect;
use crate::input::nav::{self, Dir, NavTarget};
use crate::screens::{Screen, Screens};
use crate::toolkit::Target;

/// What the last frames drew on each screen, and the screens' sizes in surface pixels.
pub(crate) trait Drawn {
    fn targets(&self, screen: Screen) -> &[Target];
    fn size(&self, screen: Screen) -> (f32, f32);
}

/// Where the pad's focus may go.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum PadScope {
    /// Shelf is in front and the focus is in its list or on its tabs. Up and down step through the
    /// games, and left and right through the collections.
    ShelfList,
    /// Shelf is in front, and the focus may go anywhere on both screens.
    BothScreens,
    /// Something else is in front, and the focus stays on the second screen.
    BottomOnly,
}

pub(crate) fn scope(s: &State) -> PadScope {
    let in_list = matches!(s.pointer().focus(), Some(Id::Shelf(ShelfId::Entry(_) | ShelfId::Collection(_))));
    match (s.pad().on_top(), in_list) {
        (true, true) => PadScope::ShelfList,
        (true, false) => PadScope::BothScreens,
        (false, _) => PadScope::BottomOnly,
    }
}

/// Where the focus goes on `screen` when nothing better is known: shelf's selected game on the top
/// screen, or the open tab's button on the bottom one.
pub(crate) fn default_focus(s: &State, screen: Screen) -> Option<Id> {
    match screen {
        Screen::Top => s.shelf().selected().cloned().map(|g| Id::Shelf(ShelfId::Entry(g))),
        Screen::Bottom => Some(Id::Tab(s.bottom().tab())),
    }
}

/// Whether the last frame drew `id` on `screen`, or on either screen if `screen` is `None`.
pub(crate) fn drawn_on(d: &impl Drawn, screen: Option<Screen>, id: &Id) -> bool {
    Screen::BOTH.into_iter().filter(|s| screen.is_none_or(|on| on == *s)).any(|s| d.targets(s).iter().any(|t| &t.id == id))
}

/// The screen the focus is on. It is the top one when the focus isn't on anything drawn.
pub(crate) fn focus_screen(s: &State, d: &impl Drawn) -> Screen {
    match s.pointer().focus() {
        Some(focus) if drawn_on(d, Some(Screen::Bottom), focus) && !drawn_on(d, Some(Screen::Top), focus) => Screen::Bottom,
        _ => Screen::Top,
    }
}

/// Moves the focus onto the second screen. It stays where it is if that is drawn there. Otherwise it
/// goes where it last was on that screen, as kept in `memory`, if that is still drawn, and failing that
/// to the open tab.
pub(crate) fn focus_bottom(s: &State, d: &impl Drawn, memory: &Screens<Option<Id>>) -> Event {
    let bottom = Some(Screen::Bottom);
    let current = s.pointer().focus().cloned().filter(|id| drawn_on(d, bottom, id));
    let remembered = memory.bottom.clone().filter(|id| drawn_on(d, bottom, id));
    let target = current.or(remembered).or_else(|| default_focus(s, Screen::Bottom)).unwrap_or(Id::Tab(Tab::default()));
    PointerEvent::Focus(target).into()
}

/// Moves the focus to the other screen, with shelf in front. It goes where it last was on that screen if
/// that is still drawn, else to the screen's default, else to the first thing drawn there. Where it was
/// is remembered in `memory`.
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

/// The targets the pad may move between, each with its screen. On shelf's actions, reached by holding A,
/// they are the actions alone. Otherwise they are the targets on the screens the pad may use, except
/// those for fingers: a list's area (the pad goes to its rows instead) and a dock tile's tray mark (the
/// pad uses the Menu entry in the tile's long-press menu). Shelf's actions are left out, as they are
/// reached only by holding A.
pub(crate) fn candidates<'d>(s: &State, d: &'d impl Drawn) -> Vec<(Screen, &'d Target)> {
    if s.shelf().in_actions(s.pointer()) {
        let actions: Vec<Id> = s.shelf().actions().as_slice().iter().map(|a| Id::Shelf(a.id())).collect();
        return d.targets(Screen::Top).iter().filter(|t| actions.contains(&t.id)).map(|t| (Screen::Top, t)).collect();
    }
    let screens: &[Screen] = if scope(s) == PadScope::BottomOnly { &[Screen::Bottom] } else { &Screen::BOTH };
    let actions = |id: &Id| matches!(id, Id::Shelf(s) if !matches!(s, ShelfId::Entry(_) | ShelfId::Collection(_) | ShelfId::Up));
    let fingers = |id: &Id| matches!(id, Id::List(_) | Id::Dock(DockId::Tray(_)));
    screens.iter().flat_map(|&screen| d.targets(screen).iter().map(move |t| (screen, t))).filter(|(_, t)| !fingers(&t.id) && !actions(&t.id)).collect()
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
pub(crate) fn navigate(s: &State, d: &impl Drawn, memory: &Screens<Option<Id>>, dir: Dir) -> Option<Event> {
    // On shelf's actions, reached by holding A, up and down move through them only.
    if s.shelf().in_actions(s.pointer()) {
        let rows: Vec<NavTarget> = candidates(s, d).into_iter().map(|(screen, t)| placed(d, screen, t)).collect();
        return match dir {
            Dir::Up | Dir::Down => nav::step(&rows, s.pointer().focus(), dir).map(|id| PointerEvent::Focus(id).into()),
            Dir::Left | Dir::Right => None,
        };
    }
    let scope = scope(s);
    // In shelf's list, up and down move through the games, scrolling it, and left and right switch
    // collections. Down past the last game goes on to the second screen.
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
    // Away from shelf, only the second screen is used. If the focus is elsewhere, it moves there first.
    if scope == PadScope::BottomOnly && focus_screen(s, d) != Screen::Bottom {
        return Some(focus_bottom(s, d, memory));
    }
    // On a drawn level row, left and right step the level.
    if let (Some(focus), Dir::Left | Dir::Right) = (s.pointer().focus(), dir)
        && let Some(level) = Level::of(focus)
        && drawn_on(d, None, focus)
    {
        let step = if dir == Dir::Left { -1 } else { 1 };
        return Some(LevelEvent::Step(level, step).into());
    }
    // In a scrolled list, up and down go to the row above or below, whether it is shown or not; the
    // list scrolls to it. Past either end, the focus moves on by direction.
    if let (Some(focus), Dir::Up | Dir::Down) = (s.pointer().focus(), dir)
        && let Some((list, i)) = ListId::of(focus)
        && drawn_on(d, None, focus)
    {
        let next = if dir == Dir::Up { i.checked_sub(1) } else { Some(i + 1).filter(|&n| n < s.lists().len(list)) };
        if let Some(id) = next.and_then(|n| u16::try_from(n).ok()).and_then(|n| ListId::sibling(focus, n, s.dock().items().len())) {
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
pub(crate) enum PadPress {
    Press(Id),
    Focus(Event),
}

pub(crate) fn press(s: &State, d: &impl Drawn, memory: &Screens<Option<Id>>) -> Option<PadPress> {
    let screen = (scope(s) == PadScope::BottomOnly).then_some(Screen::Bottom);
    match s.pointer().focus() {
        Some(id) if drawn_on(d, screen, id) => Some(PadPress::Press(id.clone())),
        _ if s.pad().on_top() => default_focus(s, Screen::Top).map(|id| PadPress::Focus(PointerEvent::Focus(id).into())),
        _ => Some(PadPress::Focus(focus_bottom(s, d, memory))),
    }
}

/// The shoulders. On shelf they move a page through the list. Elsewhere they step through the bar's
/// tabs from left to right, opening and focusing each.
pub(crate) fn shoulder(s: &State, d: &impl Drawn, step: i32) -> Vec<Event> {
    // On shelf's actions, shelf stays where it is.
    if s.shelf().in_actions(s.pointer()) {
        return vec![];
    }
    // On shelf, move a page, which is the rows on screen.
    if on_shelf(s, d) {
        let page = i32::try_from(s.shelf().rows()).unwrap_or(1).max(1);
        return vec![ShelfEvent::ListStep(step * page).into()];
    }
    let tabs: Vec<Tab> = Tab::ALL.to_vec();
    let at = tabs.iter().position(|t| *t == s.bottom().tab()).unwrap_or(0) as i32;
    let next = (at + step).rem_euclid(tabs.len() as i32) as usize;
    match tabs.get(next) {
        // Focus the tab's icon. For volume and brightness, the icon's target is their control.
        Some(&tab) => vec![PointerEvent::Activate(Id::Tab(tab)).into(), PointerEvent::Focus(crate::views::bar::tab_target(tab)).into()],
        None => vec![],
    }
}

/// Whether the pad is on shelf, rather than on a view shown in its place.
fn on_shelf(s: &State, d: &impl Drawn) -> bool {
    s.pad().on_top() && focus_screen(s, d) == Screen::Top && s.device().top_view() == clamshell_state::device::TopView::Shelf
}

/// L2 and R2 go to shelf's first or last entry.
pub(crate) fn trigger(s: &State, d: &impl Drawn, step: i32) -> Vec<Event> {
    if s.shelf().in_actions(s.pointer()) {
        return vec![];
    }
    if on_shelf(s, d) {
        let len = i32::try_from(s.shelf().list().len()).unwrap_or(i32::MAX);
        return vec![ShelfEvent::ListStep(step * len).into()];
    }
    vec![]
}

/// Y, X and Start run the details' actions while the pad is on shelf (see clamshell-state's `actions`).
/// For an entry these are favourite, on the dock and Continue; for a browsed file, Copy, Paste and
/// Select. On the second screen with the keyboard open, for typing with the pad, X is Backspace.
/// Elsewhere they do nothing.
pub(crate) fn shortcut(s: &State, d: &impl Drawn, button: PadInput) -> Vec<Event> {
    let event = match button {
        PadInput::Y => ShelfEvent::ToggleFavourite,
        PadInput::X => ShelfEvent::TogglePin,
        PadInput::Start => ShelfEvent::Continue,
        _ => return vec![],
    };
    if on_shelf(s, d) {
        return vec![event.into()];
    }
    let backspace = keyboard::find(keyboard::Key::Named(keyboard::Named::Backspace));
    match (button, backspace) {
        (PadInput::X, Some(pos)) if s.bottom().tab() == Tab::Keyboard => vec![PointerEvent::Activate(Id::Key(pos)).into()],
        _ => vec![],
    }
}

/// B cancels the editor if one is open. Otherwise it goes back a page in the Time tab or one of the
/// keymaps' screens, closes the Device view, or, with shelf in focus, goes back from shelf's actions,
/// clears the search, or leaves shelf's open group.
pub(crate) fn back(s: &State) -> Option<Event> {
    if s.bottom().editor().is_some() {
        return Some(PointerEvent::Activate(Id::Editor(EditorId::Cancel)).into());
    }
    if s.bottom().tab() == Tab::Time && s.sys().time_page() != clamshell_state::TimePage::Main {
        return Some(PointerEvent::Activate(Id::Time(clamshell_state::TimeId::Back)).into());
    }
    if s.keymapping().screen().is_some() {
        return Some(PointerEvent::Activate(Id::Keymap(clamshell_state::KeymapId::Back)).into());
    }
    // The Device view in shelf's place goes back to shelf.
    if s.device().top_view() == clamshell_state::device::TopView::Device {
        return Some(PointerEvent::Activate(Id::Device(clamshell_state::DeviceId::Close)).into());
    }
    // On shelf's actions, which holding A led to, go back to the entry in the list.
    if matches!(s.pointer().focus(), Some(Id::Shelf(id)) if !matches!(id, ShelfId::Entry(_) | ShelfId::Collection(_) | ShelfId::Up)) {
        return default_focus(s, Screen::Top).map(|id| PointerEvent::Focus(id).into());
    }
    // In shelf, clear what was typed to find an entry, or else leave the open group.
    let in_shelf = matches!(s.pointer().focus(), Some(Id::Shelf(_)));
    if in_shelf && !s.shelf().filter().is_empty() {
        return Some(clamshell_state::ShelfEvent::ClearFilter.into());
    }
    (in_shelf && s.shelf().group().is_some()).then(|| PointerEvent::Activate(Id::Shelf(ShelfId::Up)).into())
}

/// On a desktop, keys stand in for the pad. The arrows move the focus, Enter or Space is A (press and
/// hold), Tab is View, Backspace is B, Q and E are the shoulders, Home and End the triggers, F is Y, P is
/// X and C is Start. This is not done on the device, where keys reaching shelf's window are typing.
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
pub(crate) fn desktop_key(sym: Keysym) -> Option<PadInput> {
    DESKTOP_KEYS.iter().find(|(k, _)| *k == sym).map(|(_, input)| *input)
}

#[cfg(test)]
#[allow(clippy::indexing_slicing)]
mod tests {
    use super::*;
    use clamshell_state::settings::Setting;
    use clamshell_state::{Init, SettingsId};

    /// Targets on each screen, with both screens 100 by 100.
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
        Target { rect: Rect::new(x, y, 10.0, 10.0), id, track: None, list: None, item: None }
    }

    fn state() -> State {
        State::new(Init { library: clamshell_library::demo::library(), ..Init::default() })
    }

    fn entry(s: &State, i: usize) -> Id {
        Id::Shelf(ShelfId::Entry(s.shelf().entry_at(i).map(|g| g.id.clone()).unwrap_or_else(|| "?".into())))
    }

    fn drawn(s: &State) -> Fake {
        Fake(Screens {
            top: vec![t(0.0, 0.0, entry(s, 0)), t(0.0, 20.0, entry(s, 1))],
            bottom: vec![
                t(0.0, 0.0, Id::Tab(Tab::Settings)),
                t(50.0, 0.0, Id::Tab(Tab::Sysinfo)),
                t(0.0, 50.0, Id::Settings(SettingsId::Tile(Setting::Power))),
            ],
        })
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
        assert_eq!(scope(&s), PadScope::BothScreens);
        // Back up, to the game it was on.
        assert_eq!(switch_screen(&s, &d, &mut memory), Some(PointerEvent::Focus(entry(&s, 1)).into()));
        let g = entry(&s, 0);
        focus(&mut s, g);
        // Down again, to the tile it was on.
        assert_eq!(switch_screen(&s, &d, &mut memory), Some(PointerEvent::Focus(tile).into()));
        assert_eq!(memory.top, Some(entry(&s, 0)));
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
        assert_eq!(focus_bottom(&s, &d, &memory), PointerEvent::Focus(Id::Tab(s.bottom().tab())).into());
        // A drawn target is pressed.
        focus(&mut s, Id::Tab(Tab::Sysinfo));
        assert_eq!(press(&s, &d, &memory), Some(PadPress::Press(Id::Tab(Tab::Sysinfo))));
        // Moving from a lost focus starts at the first target.
        focus(&mut s, Id::Settings(SettingsId::Env));
        assert_eq!(navigate(&s, &d, &memory, Dir::Right), Some(PointerEvent::Focus(entry(&s, 0)).into()));
    }

    #[test]
    fn in_shelfs_list_up_and_down_step_and_the_end_goes_on_down() {
        let mut s = state();
        let d = drawn(&s);
        let memory = Screens::default();
        let g = entry(&s, 0);
        focus(&mut s, g);
        assert_eq!(scope(&s), PadScope::ShelfList);
        assert_eq!(navigate(&s, &d, &memory, Dir::Down), Some(ShelfEvent::ListStep(1).into()));
        assert_eq!(navigate(&s, &d, &memory, Dir::Right), Some(ShelfEvent::CollectionStep(1).into()));
        let len = s.shelf().collection().map_or(0, |c| c.entries.len());
        s.apply(ShelfEvent::ListStep(len as i32).into(), &mut Vec::new());
        let last = entry(&s, len - 1);
        focus(&mut s, last.clone());
        // Past the last game, the focus moves by direction on to the second screen.
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
        let icon = crate::views::bar::tab_target(next);
        assert_eq!(shoulder(&s, &d, 1), vec![PointerEvent::Activate(Id::Tab(next)).into(), PointerEvent::Focus(icon).into()], "its icon in the bar focused");
        let g = entry(&s, 0);
        focus(&mut s, g);
        let page = s.shelf().rows() as i32;
        assert_eq!(shoulder(&s, &d, -1), vec![ShelfEvent::ListStep(-page).into()], "in shelf: a page");
        let len = s.shelf().list().len() as i32;
        assert_eq!(trigger(&s, &d, 1), vec![ShelfEvent::ListStep(len).into()], "the last entry");
    }

    #[test]
    fn on_shelfs_actions_the_pad_stays_on_them() {
        let mut s = state();
        let mut d = drawn(&s);
        d.0.top.extend([t(60.0, 0.0, Id::Shelf(ShelfId::Launch)), t(60.0, 15.0, Id::Shelf(ShelfId::Favourite)), t(60.0, 30.0, Id::Shelf(ShelfId::Pin))]);
        focus(&mut s, Id::Shelf(ShelfId::Launch));
        assert!(s.shelf().in_actions(s.pointer()));
        let memory = Screens::default();
        assert_eq!(navigate(&s, &d, &memory, Dir::Down), Some(PointerEvent::Focus(Id::Shelf(ShelfId::Favourite)).into()));
        assert_eq!(navigate(&s, &d, &memory, Dir::Left), None, "not out to the list");
        focus(&mut s, Id::Shelf(ShelfId::Pin));
        assert_eq!(navigate(&s, &d, &memory, Dir::Down), Some(PointerEvent::Focus(Id::Shelf(ShelfId::Pin)).into()), "not down to the second screen");
        assert_eq!(shoulder(&s, &d, 1), vec![]);
        assert_eq!(shortcut(&s, &d, PadInput::Y), vec![ShelfEvent::ToggleFavourite.into()], "sent, but the state has no shortcut there");
        assert_eq!(s.shelf().shortcut(clamshell_state::shelf::actions::Shortcut::Y, s.pointer()), None);
    }

    #[test]
    fn the_pad_reaches_shelfs_actions_only_by_a_held() {
        let mut s = state();
        let mut d = drawn(&s);
        // Shelf's actions, right beside and below the last game shown.
        d.0.top.extend([t(60.0, 20.0, Id::Shelf(ShelfId::Launch)), t(60.0, 35.0, Id::Shelf(ShelfId::Favourite))]);
        let last = entry(&s, 1);
        focus(&mut s, last.clone());
        let memory = Screens::default();
        for dir in [Dir::Down, Dir::Right] {
            let to = navigate(&s, &d, &memory, dir);
            assert!(!matches!(&to, Some(Event::Pointer(PointerEvent::Focus(Id::Shelf(ShelfId::Launch | ShelfId::Favourite))))), "{dir:?}: {to:?}");
        }
    }

    #[test]
    fn b_on_shelfs_actions_goes_back_to_the_entry() {
        let mut s = state();
        focus(&mut s, Id::Shelf(ShelfId::Launch));
        let g = entry(&s, s.shelf().cursor().index);
        assert_eq!(back(&s), Some(PointerEvent::Focus(g).into()));
    }

    #[test]
    fn y_x_and_start_act_on_shelf_only_while_the_pad_is_there() {
        let mut s = state();
        let d = drawn(&s);
        let g = entry(&s, 0);
        focus(&mut s, g);
        assert_eq!(shortcut(&s, &d, PadInput::Y), vec![ShelfEvent::ToggleFavourite.into()]);
        focus(&mut s, Id::Tab(Tab::Sysinfo));
        assert_eq!(shortcut(&s, &d, PadInput::Y), vec![], "the second screen: not shelf's");
        assert_eq!(shortcut(&s, &d, PadInput::X), vec![]);
        // Typing with the pad: X is Backspace.
        s.apply(PointerEvent::Activate(Id::Tab(Tab::Keyboard)).into(), &mut Vec::new());
        focus(&mut s, Id::Tab(Tab::Sysinfo));
        let backspace = keyboard::find(keyboard::Key::Named(keyboard::Named::Backspace)).map(Id::Key);
        assert_eq!(shortcut(&s, &d, PadInput::X), backspace.map(|b| PointerEvent::Activate(b).into()).into_iter().collect::<Vec<_>>());
        assert_eq!(shortcut(&s, &d, PadInput::Y), vec![]);
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
        let d = Fake(Screens { top: vec![], bottom: vec![t(0.0, 0.0, Id::List(ListId::Wifi)), t(0.0, 30.0, row(3)), t(0.0, 40.0, row(4))] });
        let memory = Screens::default();
        focus(&mut s, row(4));
        assert_eq!(navigate(&s, &d, &memory, Dir::Down), Some(PointerEvent::Focus(row(5)).into()), "not drawn: the list scrolls to it");
        focus(&mut s, row(3));
        assert_eq!(navigate(&s, &d, &memory, Dir::Up), Some(PointerEvent::Focus(row(2)).into()));
        // Sideways and past the ends, the focus moves by direction but never onto the list's area.
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
