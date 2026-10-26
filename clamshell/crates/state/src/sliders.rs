//! The sliders: the levels', a keymap's or a curve's parameter's, and a setting's in the settings panel.

use crate::out::Out;
use crate::settings::Setting;
use crate::{Control, LevelEvent, Slide, Slider, State};

impl State {
    /// A level event. A level row set is the one the finger landed on, and nothing once it is gone.
    pub(crate) fn on_level(&mut self, event: LevelEvent, out: &mut Out) {
        let event = match event {
            LevelEvent::Set(level, value) => match self.lists().resolve_level(level) {
                Some(level) => LevelEvent::Set(level, value),
                None => return,
            },
            e => e,
        };
        self.levels.on_event(event, self.clock.mono(), out);
    }

    pub(crate) fn on_slide(&mut self, slide: Slide, out: &mut Out) {
        match slide {
            Slide::Start { control: Control::Param(param), x, width } => {
                self.keymapping.param_slide_start(param, x, width);
            }
            Slide::Start { control: Control::Curve(param), x, width } => {
                self.curving.slide_start(param, x, width);
            }
            Slide::Start { control: control @ Control::Setting(setting), x, width } => {
                let len = self.setting_stops(setting);
                let own = match self.panel().source(setting) {
                    crate::settings::panel::Source::Written(_) => Some(self.panel().tile_value(setting)),
                    _ => None,
                };
                let at = own.and_then(|v| (1..len).find(|i| self.setting_stop(setting, *i) == Some(Some(v)))).unwrap_or(0);
                let (at, hi) = (u8::try_from(at).unwrap_or(0), u8::try_from(len.saturating_sub(1)).unwrap_or(0));
                self.setting_slider = Some(Slider::ranged(control, at, 0, hi, x, width));
            }
            Slide::Start { control, x, width } => {
                self.levels.slide_start(control, x, width);
            }
            Slide::To { x } if self.levels.slider().is_none() => {
                self.keymapping.param_slide_to(x);
                self.curving.slide_to(x);
                self.setting_slide_to(x);
            }
            Slide::To { x } => {
                self.levels.change(self.clock.mono(), |l| l.slide_to(x, out));
            }
            Slide::End => {
                self.levels.slide_end();
                self.keymapping.param_slide_end(out);
                self.curving.slide_end(out);
                if self.setting_slider.take().is_some_and(|s| s.moved) {
                    out.effects.push(self.prefs.save_effect());
                }
            }
        }
    }

    /// A setting's slider moved: the stop under the finger is written where the panel edits at once (Default
    /// clears it there), and saved when the finger lifts.
    fn setting_slide_to(&mut self, x: f32) {
        let Some(slider) = self.setting_slider else { return };
        let Control::Setting(setting) = slider.control else { return };
        let position = slider.value_at(x);
        if position == slider.value {
            return;
        }
        self.setting_slider = Some(Slider { value: position, moved: true, ..slider });
        if let Some(stop) = self.setting_stop(setting, usize::from(position)) {
            let entry = self.panel().editing().map(|g| g.id.clone());
            self.prefs.set(entry.as_ref(), setting, stop);
        }
    }

    /// A setting's slider held, if one is.
    pub fn setting_slider(&self) -> Option<Slider> {
        self.setting_slider
    }

    /// How many stops `setting`'s slider has at the level edited (see [`State::setting_stop`]).
    pub fn setting_stops(&self, setting: Setting) -> usize {
        let at_all = self.panel().editing().is_none();
        1 + setting.choices().iter().filter(|v| !(at_all && **v == setting.default_value())).count()
    }

    /// Stop `i` of `setting`'s slider at the level edited: first Default (None), which writes nothing
    /// there, then its values. At All, Default is the stock value, so that value isn't a stop of its own.
    pub fn setting_stop(&self, setting: Setting, i: usize) -> Option<Option<u16>> {
        if i == 0 {
            return Some(None);
        }
        let at_all = self.panel().editing().is_none();
        setting.choices().iter().filter(|v| !(at_all && **v == setting.default_value())).nth(i - 1).map(|v| Some(*v))
    }
}
