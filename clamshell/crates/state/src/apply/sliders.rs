//! The sliders: the levels', a keymap's, a curve's or a light's parameter's, and a setting's in the
//! settings panel.

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

    /// A slider held, slid or let go. Each kind is started from, and sets, its own area's value: a level, a
    /// keymap's or a curve's parameter, or a setting; and when it is let go after moving, its area saves.
    pub(crate) fn on_slide(&mut self, slide: Slide, out: &mut Out) {
        match slide {
            Slide::Start { control, x, width } => {
                self.slider = match control {
                    Control::Param(param) => self.keymapping.param_slider(param, x, width),
                    Control::Curve(param) => self.curving.slider(param, x, width),
                    Control::Light(param) => self.lighting.slider(param, x, width),
                    Control::Setting(setting) => Some(self.setting_slider_at(setting, x, width)),
                    control => Some(self.levels.slider(control, x, width)),
                };
            }
            Slide::To { x } => {
                let Some(slider) = self.slider else { return };
                let value = slider.value_at(x);
                if value == slider.value {
                    return;
                }
                let value = match slider.control {
                    Control::Param(param) => self.keymapping.param_slid(param, value),
                    Control::Curve(param) => {
                        self.curving.slid(param, value);
                        Some(value)
                    }
                    Control::Light(param) => {
                        self.lighting.slid(param, value);
                        Some(value)
                    }
                    Control::Setting(setting) => {
                        self.setting_slid(setting, value);
                        Some(value)
                    }
                    control => {
                        self.levels.change(self.clock.mono(), |l| l.slid(control, value, out));
                        Some(value)
                    }
                };
                if let Some(value) = value.filter(|v| *v != slider.value) {
                    self.slider = Some(Slider { value, moved: true, ..slider });
                }
            }
            Slide::End => {
                let Some(slider) = self.slider.take().filter(|s| s.moved) else { return };
                match slider.control {
                    Control::Param(_) => out.effects.push(self.keymapping.save()),
                    Control::Curve(_) => out.effects.push(self.curving.save_effect()),
                    Control::Light(_) => out.effects.push(self.lighting.save_effect()),
                    Control::Setting(_) => out.effects.push(self.prefs.save_effect()),
                    // A level stays where it was set; the brightness saves itself a moment later.
                    _ => {}
                }
            }
        }
    }

    /// A setting's slider, on the stop of the value written at the level edited, or Default.
    fn setting_slider_at(&self, setting: Setting, x: f32, width: f32) -> Slider {
        let len = self.setting_stops(setting);
        let own = match self.panel().source(setting) {
            crate::settings::panel::Source::Written(_) => Some(self.panel().tile_value(setting)),
            _ => None,
        };
        let at = own.and_then(|v| (1..len).find(|i| self.setting_stop(setting, *i) == Some(Some(v)))).unwrap_or(0);
        let (at, hi) = (u8::try_from(at).unwrap_or(0), u8::try_from(len.saturating_sub(1)).unwrap_or(0));
        Slider::ranged(Control::Setting(setting), at, 0, hi, x, width)
    }

    /// A setting's slider moved to stop `position`: written where the panel edits at once (Default clears
    /// it there), and saved when the finger lifts.
    fn setting_slid(&mut self, setting: Setting, position: u8) {
        if let Some(stop) = self.setting_stop(setting, usize::from(position)) {
            let entry = self.panel().editing().map(|g| g.id.clone());
            self.prefs.set(entry.as_ref(), setting, stop);
        }
    }

    /// The slider held, of whichever kind: a level's, a keymap's or a curve's parameter's, or a setting's.
    pub fn held_slider(&self) -> Option<Slider> {
        self.slider
    }

    /// The level a bar control shows now: the slider's value while it is slid, otherwise the level's.
    pub fn control(&self, control: Control) -> Option<u8> {
        match self.slider {
            Some(s) if s.control == control => Some(s.value),
            _ => self.levels.level(control.level()?),
        }
    }

    /// How many stops `setting`'s slider has at the level edited (see [`State::setting_stop`]).
    pub(crate) fn setting_stops(&self, setting: Setting) -> usize {
        let at_all = self.panel().editing().is_none();
        1 + setting.choices().iter().filter(|v| !(at_all && **v == setting.default_value())).count()
    }

    /// Stop `i` of `setting`'s slider at the level edited: first Default (None), which writes nothing
    /// there, then its values. At All, Default is the stock value, so that value isn't a stop of its own.
    pub(crate) fn setting_stop(&self, setting: Setting, i: usize) -> Option<Option<u16>> {
        if i == 0 {
            return Some(None);
        }
        let at_all = self.panel().editing().is_none();
        setting.choices().iter().filter(|v| !(at_all && **v == setting.default_value())).nth(i - 1).map(|v| Some(*v))
    }
}
