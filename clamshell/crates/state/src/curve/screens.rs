//! The HDR curves' screens, shown over the settings tab: the picker ([`crate::profiles::picker`]) and a
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

    /// A curve's name: "Own curve" for a game's own, and "Off" for none or for one that is gone.
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
mod tests;
