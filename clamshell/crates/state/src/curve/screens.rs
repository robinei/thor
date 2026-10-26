//! The tone curves' screens, shown over the settings tab: the picker ([`crate::profiles::picker`]) and a
//! curve's edit screen, with a slider for each parameter ([`CurveParam`]). A slider changes the curve as
//! it moves, so the game on the top screen shows each change at once, and the curves are saved when the
//! finger lifts.

use super::{Curve, CurveParam, Curves};
use crate::Tab;
use crate::bottom::editor::Editors;
use crate::effect::{Effect, Save};
use crate::levels::slider::{Control, Slider};
use crate::nav::lists::RowPart;
use crate::out::Out;
use crate::profiles::Profiles;
use crate::profiles::picker::{self, Kind, Owner, PickerCx, PickerRow, PickerScreen, ProfileId, ProfileName};
use crate::settings::{Setting, Settings};

/// Which of the curves' screens is open.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CurveScreen {
    /// One of the picker's screens.
    Picker(PickerScreen),
    /// A curve's sliders.
    Edit(u16),
}

/// The curves and their screens: every curve, the screen open over the settings tab, and a parameter's
/// slider while a finger is on it.
#[derive(Debug, Clone, Default)]
pub struct Curving {
    curves: Curves,
    open: Option<CurveScreen>,
    slider: Option<Slider>,
}

impl Curving {
    pub(crate) fn new(curves: Curves) -> Curving {
        Curving { curves, ..Curving::default() }
    }

    pub fn curves(&self) -> &Curves {
        &self.curves
    }

    /// The curves' screen open over the settings tab, if one is.
    pub fn screen(&self) -> Option<CurveScreen> {
        self.open
    }

    /// The curves' screen that is open, while the settings tab is the one shown (`tab`).
    pub(crate) fn screen_in(&self, tab: Tab) -> Option<CurveScreen> {
        self.open.filter(|_| tab == Tab::Settings)
    }

    /// A parameter's slider, while a finger is on it.
    pub fn slider(&self) -> Option<Slider> {
        self.slider
    }

    fn picker_screen(&self) -> Option<PickerScreen> {
        match self.open {
            Some(CurveScreen::Picker(p)) => Some(p),
            _ => None,
        }
    }

    pub(crate) fn rows_len(&self, entry: Option<&str>) -> usize {
        self.curves.rows_len(self.picker_screen(), entry)
    }

    /// Row `i` of the picker, with the settings panel editing `entry`.
    pub(crate) fn row(&self, i: usize, entry: Option<&str>) -> Option<PickerRow<'_>> {
        self.curves.row(self.picker_screen(), i, entry)
    }

    /// A curve's name: "Own curve" for a game's own, and "Default colours" for none or for one that is gone.
    pub fn name(&self, id: u16) -> &str {
        self.curves.shown_name(id, Kind::Curve)
    }

    /// Curve `id`, the one chosen, if it is there. Off (0), and a curve that is gone, have none: SDR is
    /// shown as it is.
    pub fn in_force(&self, id: u16) -> Option<&Curve> {
        self.curves.get(id).map(|p| &p.body)
    }

    /// The curve tile opens the picker.
    pub(crate) fn open_picker(&mut self) {
        self.open = Some(CurveScreen::Picker(PickerScreen::List));
    }

    /// A row of the picker (see [`picker::on_row`]).
    pub(crate) fn on_row(&mut self, i: usize, part: RowPart, long: bool, cx: &PickerCx, editors: &mut Editors, out: &mut Out) {
        picker::on_row(self, i, part, long, cx, editors, out);
    }

    /// The picker's buttons, but Back (see [`picker::on`]).
    pub(crate) fn on_picker(&mut self, id: ProfileId, settings: &Settings, editors: &mut Editors, out: &mut Out) {
        picker::on(self, id, |c| settings.users(Setting::Curve, c), editors, out);
    }

    /// The editor saved a curve's name (see [`picker::named`]).
    pub(crate) fn named_profile(&mut self, action: ProfileName, name: &str, out: &mut Out) {
        picker::named(self, action, name, out);
    }

    /// Goes back one screen: from the edit screen to the picker, within the picker, and from the picker to
    /// the settings.
    pub(crate) fn back(&mut self) {
        self.slider = None;
        self.open = match self.open {
            None => None,
            Some(CurveScreen::Picker(p)) => picker::back(p).map(CurveScreen::Picker),
            Some(CurveScreen::Edit(_)) => Some(CurveScreen::Picker(PickerScreen::List)),
        };
    }

    /// A finger is held or slid on a parameter's row of the edit screen. Its slider starts from the value
    /// now.
    pub(crate) fn slide_start(&mut self, param: CurveParam, x: f32, width: f32) {
        let Some(CurveScreen::Edit(id)) = self.open else {
            return;
        };
        let Some(curve) = self.in_force(id) else {
            return;
        };
        let (lo, hi, _) = param.range();
        self.slider = Some(Slider::ranged(Control::Curve(param), param.position(param.get(curve)), param.position(lo), param.position(hi), x, width));
    }

    /// The slider moved. The curve follows at once, so the game shows it as it changes.
    pub(crate) fn slide_to(&mut self, x: f32) {
        let (Some(slider), Some(CurveScreen::Edit(id))) = (self.slider, self.open) else {
            return;
        };
        let Control::Curve(param) = slider.control else {
            return;
        };
        let position = slider.value_at(x);
        if position == slider.value {
            return;
        }
        self.slider = Some(Slider { value: position, moved: true, ..slider });
        self.curves.edit(id, |p| {
            let before = p.body;
            param.set(&mut p.body, param.value(position));
            p.body != before
        });
    }

    /// The finger lifted. The curves are saved if the slider changed anything.
    pub(crate) fn slide_end(&mut self, out: &mut Out) {
        if self.slider.take().is_some_and(|s| s.moved) {
            out.effects.push(self.save_effect());
        }
    }

    pub(crate) fn save_effect(&self) -> Effect {
        Effect::Save(Save::Curves(self.curves.clone()))
    }
}

impl Owner for Curving {
    type Body = Curve;
    const KIND: Kind = Kind::Curve;
    fn profiles(&self) -> &Profiles<Curve> {
        &self.curves
    }
    fn profiles_mut(&mut self) -> &mut Profiles<Curve> {
        &mut self.curves
    }
    fn picker(&self) -> Option<PickerScreen> {
        self.picker_screen()
    }
    fn set_picker(&mut self, screen: Option<PickerScreen>) {
        self.open = screen.map(CurveScreen::Picker);
    }
    fn open_edit(&mut self, id: u16) {
        self.slider = None;
        self.open = Some(CurveScreen::Edit(id));
    }
    fn save(&mut self) -> Effect {
        self.save_effect()
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    use crate::bottom::editor::Purpose;
    use crate::profiles::picker::{Kind as ProfileKind, ProfileId};
    use crate::testkit::*;
    use crate::*;

    fn row(i: u16) -> Id {
        ListId::Curves.row(i)
    }

    fn picker(s: &mut State) {
        tap(s, Id::Tab(Tab::Settings));
        long(s, Id::Settings(SettingsId::Tile(Setting::Curve)));
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
        assert_eq!(s.curving().name(0), "Default colours");
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
        assert!(s.applied(Slide::Start { control: Control::Curve(CurveParam::Highlights), x: 300.0, width: 620.0 }.into()).is_empty());
        let slider = s.curving().slider().unwrap();
        assert_eq!(slider.start, CurveParam::Highlights.position(before.highlights));
        // Moving right raises the peak at once, without saving yet.
        let fx = s.applied(Slide::To { x: 360.0 }.into());
        assert!(saved(&fx).is_none(), "{fx:?}");
        let moved = *s.curving().in_force(1).unwrap();
        assert!(moved.highlights > before.highlights, "{moved:?}");
        assert_eq!(CurveParam::Highlights.position(moved.highlights), s.curving().slider().unwrap().value);
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
        assert!(matches!(
            s.bottom().editor(),
            Some(Editor { purpose: Purpose::ProfileName(ProfileKind::Curve, crate::profiles::picker::ProfileName::New), .. })
        ));
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

    /// HDR content's peak is the program's setting, not a curve's. Its slider on the edit screen starts at
    /// Default, which writes nothing at the level edited; a value is written there, saved when the finger
    /// lifts; and choosing another curve keeps it. Where the value is from follows: read from the defaults,
    /// written to the game, read from All.
    #[test]
    fn the_hdr_content_peak_is_the_games_setting_slid_from_default_and_outlives_its_curve() {
        let mut s = new_state();
        picker(&mut s);
        tap(&mut s, Id::Row { list: ListId::Curves, i: 2, part: RowPart::Edit });
        assert_eq!(s.curving().screen(), Some(CurveScreen::Edit(1)));
        let set = |s: &State| s.prefs().settings().overrides_of("zelda-lttp").get(Setting::HdrPeak);
        let slide = |s: &mut State, to: f32| {
            s.applied(Slide::Start { control: Control::Setting(Setting::HdrPeak), x: 300.0, width: 620.0 }.into());
            let moved = s.applied(Slide::To { x: to }.into());
            assert!(moved.iter().all(|e| !matches!(e, Effect::Save(_))), "saved only when lifted: {moved:?}");
            s.applied(Slide::End.into())
        };
        assert_eq!(s.panel().source(Setting::HdrPeak), Source::Defaults);
        // A game's stops: Default, As declared, then the values.
        assert_eq!(s.setting_stops(Setting::HdrPeak), 9);
        assert_eq!((s.setting_stop(Setting::HdrPeak, 0), s.setting_stop(Setting::HdrPeak, 1)), (Some(None), Some(Some(0))));
        let fx = slide(&mut s, 5000.0);
        assert!(fx.iter().any(|e| matches!(e, Effect::Save(_))), "{fx:?}");
        assert_eq!(set(&s), Some(4000));
        assert!(matches!(s.panel().source(Setting::HdrPeak), Source::Written(_)));
        assert_eq!(s.curving().in_force(1).copied(), Curves::initial().get(1).map(|p| p.body), "the curve itself is unchanged");
        // Another curve for the game: the peak stays.
        profile(&mut s, ProfileId::Back);
        tap(&mut s, row(3));
        assert_eq!((chosen(&s), set(&s)), (Some(2), Some(4000)));
        // Back to Default: nothing written for the game.
        picker(&mut s);
        tap(&mut s, Id::Row { list: ListId::Curves, i: 3, part: RowPart::Edit });
        slide(&mut s, -5000.0);
        assert_eq!(set(&s), None);
        // All's value, read by the game; at All, Default is the stock value, not a stop of its own.
        tap(&mut s, Id::Settings(SettingsId::Scope(Scope::Global)));
        assert_eq!(s.setting_stops(Setting::HdrPeak), 8);
        slide(&mut s, 5000.0);
        assert_eq!(s.prefs().settings().global.get(Setting::HdrPeak), 4000);
        tap(&mut s, Id::Settings(SettingsId::Scope(Scope::Content)));
        assert_eq!(s.panel().source(Setting::HdrPeak), Source::Read("All"));
    }
}
