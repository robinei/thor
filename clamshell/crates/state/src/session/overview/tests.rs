use super::*;
use crate::testkit::*;
use crate::{DockId, Event, Id, PadRoute, Tab};

fn dock(i: u16) -> Id {
    Id::Dock(DockId::Entry(i))
}

fn top(s: &crate::State) -> Option<i64> {
    s.session().top_window().map(|w| w.id)
}

fn chosen(s: &crate::State) -> Option<i64> {
    s.session().overview().map(Overview::chosen)
}

/// Zelda running with windows `ids` (each with `parent` if given), the last in front, and the dock open.
fn running(windows: &[(i64, Option<i64>)]) -> crate::State {
    let mut s = state();
    tap(&mut s, Id::Tab(Tab::Dock));
    let zelda = launch(&mut s, "zelda-lttp");
    s.observed(windows.iter().map(|&(id, parent)| crate::session::windows::WindowFacts { parent, ..win_of(id, "zelda3", &zelda) }).collect());
    let last = windows.last().map(|w| w.0).unwrap_or_default();
    s.applied(crate::WindowEvent::Activate(last).into());
    s
}

#[test]
fn cells_share_the_screen_in_rows_and_a_short_last_row_is_centred() {
    assert_eq!([1, 2, 3, 4, 5, 9, 10].map(columns), [1, 2, 2, 2, 3, 3, 4]);
    let screen = (1000.0, 500.0);
    let gap = GAP * 500.0;
    let (x0, y0, w, h) = cell(0, 2, screen);
    let (x1, y1, ..) = cell(1, 2, screen);
    assert_eq!((x0, y0, y1), (gap, gap, gap), "two side by side");
    assert_eq!((w, h), ((1000.0 - 3.0 * gap) / 2.0, 500.0 - 2.0 * gap));
    assert_eq!(x1, 2.0 * gap + w);
    // Three: two above, one below in the middle.
    let (x2, y2, w2, _) = cell(2, 3, screen);
    assert_eq!(x2 + w2 / 2.0, 500.0, "centred");
    assert!(y2 > cell(0, 3, screen).1);
}

/// A tap on the dock entry in front opens the overview with the window in front chosen; the pad moves the
/// choice and A brings it in front, which closes the overview and gives the pad back to the game.
#[test]
fn a_tap_on_the_entry_in_front_shows_its_windows_and_the_pad_picks_one() {
    let mut s = running(&[(3, None), (4, None), (5, None)]);
    assert_eq!(s.pad().route(), PadRoute::Game);
    tap(&mut s, dock(0));
    assert_eq!(chosen(&s), Some(5), "the window in front");
    assert!(s.session().overview_windows().map(|w| w.id).eq([3, 4, 5]));
    assert_eq!(top(&s), Some(5), "still in front underneath");
    assert_eq!(s.pad().route(), PadRoute::Ui, "the pad moves the choice");
    // 3 and 4 above, 5 below: up goes to 3, right to 4, down from 4 to 5, down again nowhere.
    let step = |s: &mut crate::State, dir| s.applied(Event::Overview(OverviewEvent::Step(dir)));
    step(&mut s, Dir::Up);
    assert_eq!(chosen(&s), Some(3));
    step(&mut s, Dir::Left);
    assert_eq!(chosen(&s), Some(3), "the edge");
    step(&mut s, Dir::Right);
    step(&mut s, Dir::Down);
    assert_eq!(chosen(&s), Some(5), "the shorter last row's last");
    step(&mut s, Dir::Down);
    step(&mut s, Dir::Up);
    s.applied(Event::Overview(OverviewEvent::Choose));
    assert_eq!((top(&s), s.session().overview()), (Some(3), None));
    assert_eq!(s.session().keyboard_focus(), Some(3));
    assert_eq!(s.pad().route(), PadRoute::Game);
}

/// Another tap on the entry, or B, goes back to the window in front; a tap on a window brings it.
#[test]
fn the_entry_again_or_b_goes_back_and_a_tapped_window_comes_in_front() {
    let mut s = running(&[(3, None), (4, None)]);
    tap(&mut s, dock(0));
    tap(&mut s, dock(0));
    assert_eq!((top(&s), s.session().overview()), (Some(4), None));
    tap(&mut s, dock(0));
    s.applied(Event::Overview(OverviewEvent::Close));
    assert_eq!((top(&s), s.session().overview()), (Some(4), None));
    tap(&mut s, dock(0));
    s.applied(Event::Overview(OverviewEvent::Pick(3)));
    assert_eq!((top(&s), s.session().overview()), (Some(3), None));
}

/// With one window on the top screen the tap shows it as before; a dialog doesn't count, and is left out.
#[test]
fn one_window_and_its_dialog_open_no_overview() {
    let mut s = running(&[(3, None), (4, Some(3))]);
    tap(&mut s, dock(0));
    assert_eq!(s.session().overview(), None);
    assert_eq!(top(&s), Some(4));
    // With a second window, the dialog in front chooses its parent.
    let first = s.session().window(3).map(|w| w.facts.clone()).unwrap_or_default();
    s.applied(crate::WindowEvent::Opened(crate::session::windows::WindowFacts { id: 5, ..first }).into());
    s.applied(crate::WindowEvent::Activate(4).into());
    tap(&mut s, dock(0));
    assert_eq!(chosen(&s), Some(3));
    assert!(s.session().overview_windows().map(|w| w.id).eq([3, 5]));
}

/// The overview closes when another group comes in front, and when one window is left.
#[test]
fn it_closes_with_its_group_out_of_front_or_one_window_left() {
    let mut s = running(&[(3, None), (4, None)]);
    tap(&mut s, dock(0));
    s.applied(Event::Home);
    assert_eq!(s.session().overview(), None, "shelf in front");
    s.applied(crate::WindowEvent::Activate(4).into());
    tap(&mut s, dock(0));
    s.applied(crate::WindowEvent::Closed(3).into());
    assert_eq!(s.session().overview(), None, "one left");
    assert_eq!(top(&s), Some(4));
}
