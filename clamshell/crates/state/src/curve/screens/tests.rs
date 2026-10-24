use super::*;
use crate::profiles::picker::{Kind as ProfileKind, ProfileId};
use crate::testkit::*;
use crate::*;

fn row(i: u16) -> Id {
    ListId::Curves.row(i)
}

fn picker(s: &mut State) {
    tap(s, Id::Tab(Tab::Settings));
    tap(s, Id::Settings(SettingsId::Tile(Setting::Curve)));
}

fn profile(s: &mut State, id: ProfileId) -> Vec<Effect> {
    tap(s, Id::Profile(ProfileKind::Curve, id))
}

fn chosen(s: &State) -> Option<u16> {
    s.prefs().settings().overrides_of("zelda-lttp").get(Setting::Curve)
}

fn saved(fx: &[Effect]) -> Option<&Curves> {
    fx.iter().find_map(|e| if let Effect::Save(Save::Curves(c)) = e { Some(c) } else { None })
}

fn new_state() -> State {
    State::new(Init { curves: Curves::initial(), ..init() })
}

#[test]
fn the_tile_opens_the_picker_and_a_row_chooses_a_curve_for_the_game() {
    let mut s = new_state();
    picker(&mut s);
    assert_eq!(s.curving().screen(), Some(CurveScreen::Picker(PickerScreen::List)));
    assert_eq!(s.lists().len(ListId::Curves), 5, "the game's own (made from it), Off, then the three to start from");
    assert_eq!(s.panel().profile_row(ProfileKind::Curve, 1), Some(PickerRow::Default));
    assert_eq!(s.panel().profile_row(ProfileKind::Curve, 2), Some(PickerRow::Named(1, "True black")));
    tap(&mut s, row(2));
    assert_eq!(s.curving().screen(), None, "chosen: the picker closes");
    assert_eq!(chosen(&s), Some(1));
    assert_eq!(s.prefs().settings().users(Setting::Curve, 1).entries, 1);
    // Off for the game.
    picker(&mut s);
    tap(&mut s, row(1));
    assert_eq!(chosen(&s), Some(0));
    assert_eq!(s.curving().name(0), "Off");
}

#[test]
fn a_games_own_curve_is_made_from_the_one_in_force_and_opens_on_its_sliders() {
    let mut s = new_state();
    picker(&mut s);
    tap(&mut s, row(3));
    picker(&mut s);
    let fx = tap(&mut s, row(0));
    // The own curve copies Punchy, opens on its edit screen and is chosen; the settings are saved before
    // the curves.
    let own = s.curving().curves().own_of("zelda-lttp").map(|p| p.id).unwrap();
    assert_eq!(s.curving().screen(), Some(CurveScreen::Edit(own)));
    assert_eq!(chosen(&s), Some(own));
    assert_eq!(s.curving().in_force(own), s.curving().in_force(2));
    let settings = fx.iter().position(|e| matches!(e, Effect::Save(Save::Settings(_)))).unwrap();
    let curves = fx.iter().position(|e| matches!(e, Effect::Save(Save::Curves(_)))).unwrap();
    assert!(settings < curves, "{fx:?}");
    assert_eq!(s.curving().name(own), "Own curve");
}

#[test]
fn a_slider_changes_the_curve_as_it_moves_and_saves_once_when_lifted() {
    let mut s = new_state();
    picker(&mut s);
    tap(&mut s, Id::Row { list: ListId::Curves, i: 2, part: RowPart::Edit });
    assert_eq!(s.curving().screen(), Some(CurveScreen::Edit(1)));
    let before = *s.curving().in_force(1).unwrap();
    assert!(s.applied(Slide::Start { control: Control::Curve(CurveParam::Peak), x: 300.0, width: 620.0 }.into()).is_empty());
    let slider = s.curving().slider().unwrap();
    assert_eq!(slider.start, CurveParam::Peak.position(before.peak));
    // Moving right raises the peak at once, without saving yet.
    let fx = s.applied(Slide::To { x: 360.0 }.into());
    assert!(saved(&fx).is_none(), "{fx:?}");
    let moved = *s.curving().in_force(1).unwrap();
    assert!(moved.peak > before.peak, "{moved:?}");
    assert_eq!(CurveParam::Peak.position(moved.peak), s.curving().slider().unwrap().value);
    let fx = s.applied(Slide::End.into());
    assert_eq!(saved(&fx).and_then(|c| c.get(1)).map(|p| p.body), Some(moved));
    assert_eq!(s.curving().slider(), None);
    // A touch that doesn't move saves nothing.
    s.applied(Slide::Start { control: Control::Curve(CurveParam::Black), x: 300.0, width: 620.0 }.into());
    assert!(saved(&s.applied(Slide::End.into())).is_none());
}

#[test]
fn back_goes_from_the_sliders_to_the_picker_and_out() {
    let mut s = new_state();
    picker(&mut s);
    tap(&mut s, Id::Row { list: ListId::Curves, i: 2, part: RowPart::Edit });
    assert_eq!(s.pad_back(), Some(Id::Profile(ProfileKind::Curve, ProfileId::Back)));
    profile(&mut s, ProfileId::Back);
    assert_eq!(s.curving().screen(), Some(CurveScreen::Picker(PickerScreen::List)));
    profile(&mut s, ProfileId::Back);
    assert_eq!(s.curving().screen(), None);
}

#[test]
fn deleting_a_curve_in_use_asks_and_puts_its_games_back_on_off() {
    let mut s = new_state();
    picker(&mut s);
    tap(&mut s, row(2));
    picker(&mut s);
    s.applied(Event::Pointer(PointerEvent::LongPress(row(2))));
    assert_eq!(s.curving().screen(), Some(CurveScreen::Picker(PickerScreen::Menu(1))));
    profile(&mut s, ProfileId::Delete);
    assert_eq!(s.curving().screen(), Some(CurveScreen::Picker(PickerScreen::Delete(1))));
    let fx = profile(&mut s, ProfileId::ConfirmDelete);
    assert!(saved(&fx).is_some_and(|c| c.get(1).is_none()), "{fx:?}");
    assert_eq!(chosen(&s), Some(0));
    assert_eq!(s.curving().in_force(1), None);
}

#[test]
fn a_new_curve_is_named_and_changes_nothing_until_edited() {
    let mut s = new_state();
    picker(&mut s);
    profile(&mut s, ProfileId::New);
    assert!(matches!(s.bottom().editor(), Some(Editor { purpose: Purpose::ProfileName(ProfileKind::Curve, crate::profiles::picker::ProfileName::New), .. })));
    for c in "mine".chars() {
        tap(&mut s, Id::Key(crate::bottom::keyboard::find(crate::bottom::keyboard::Key::Char(c)).unwrap()));
    }
    let fx = tap(&mut s, Id::Editor(EditorId::Save));
    let id = saved(&fx).and_then(|c| c.named().find(|p| p.name == "mine")).map(|p| p.id).unwrap();
    assert_eq!(s.curving().screen(), Some(CurveScreen::Edit(id)));
    assert_eq!(s.curving().in_force(id), Some(&Curve::default()));
}

#[test]
fn a_windows_curve_is_the_one_chosen_for_the_game_running_it_and_follows_its_slider() {
    let mut s = new_state();
    launch(&mut s, "zelda-lttp");
    show_window(&mut s, "zelda-lttp", 7);
    assert_eq!(s.window_curve(7), None, "Off");
    picker(&mut s);
    tap(&mut s, row(2));
    assert_eq!(s.window_curve(7).as_ref(), s.curving().in_force(1));
    // A slider's change reaches the window as it moves, before the finger lifts.
    picker(&mut s);
    tap(&mut s, Id::Row { list: ListId::Curves, i: 2, part: RowPart::Edit });
    s.applied(Slide::Start { control: Control::Curve(CurveParam::Black), x: 300.0, width: 620.0 }.into());
    s.applied(Slide::To { x: 200.0 }.into());
    assert_eq!(s.window_curve(7).map(|c| c.black), Some(0), "slid all the way down");
    assert_eq!(s.window_curve(99), None, "no such window");
}
