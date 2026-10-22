//! A binding's parameters on the edit screen's panel: what they start from, and their sliders.

use super::*;

impl State {
    /// A finger held (or slid) on a parameter's row of the panel: its slider, from its value now.
    pub(crate) fn param_slide_start(&mut self, param: Param, x: f32, width: f32) {
        let Some((_, _, b)) = self.param_binding(param) else {
            return;
        };
        // (Its value now under the finger, as the bar's sliders have it.)
        let range = param.range();
        self.keymap_screens.edit.slider = Some(Slider::ranged(Control::Param(param), param.get(&b), *range.start(), *range.end(), x, width));
    }

    /// What the selected input's parameters start from in the layer and actuation shown: its binding
    /// there, else the binding of the layers below it, else, for a press, the input as on the pad. A hold
    /// or double without a binding has none.
    pub fn param_base(&self, id: u16, input: Input) -> Option<Binding> {
        let k = self.keymaps.get(id);
        let act = self.keymap_screens.edit.act;
        let shown = self.layers_shown();
        let found = k.and_then(|k| k.lookup_of(input, act, shown.as_slice())).copied();
        match act {
            Actuation::Press => found.or_else(|| as_on_the_pad(input).map(|o| Binding::of(input, o))),
            _ => found,
        }
    }

    /// The selected input and what its parameters start from, if `param` applies to it (on the edit
    /// screen).
    pub(super) fn param_binding(&self, param: Param) -> Option<(u16, Input, Binding)> {
        let Some(Screen::Edit(id)) = self.keymap_screens.open else {
            return None;
        };
        let input = self.keymap_screens.edit.selected?;
        let b = self.param_base(id, input)?;
        param.applies_to(input, b.output, self.keymap_screens.edit.act).then_some((id, input, b))
    }

    /// Stores `input`'s changed parameters in the layer shown. In Base, parameters back to how the pad
    /// has them remove the binding instead. Returns whether anything changed.
    pub(super) fn set_tuned(&mut self, id: u16, input: Input, b: Binding) -> bool {
        let EditView { layer, act, .. } = self.keymap_screens.edit;
        let plain = layer == Layer::BASE && act == Actuation::Press && as_on_the_pad(input).map(|o| Binding::of(input, o)) == Some(b);
        self.keymaps.get_mut(id).is_some_and(|k| k.set_of(layer, input, act, (!plain).then_some(b)))
    }

    /// The parameter's slider moved: the binding follows at once (the pad gets the keymap as it changes),
    /// saved when the finger lifts. Whether it changed.
    pub(crate) fn param_slide_to(&mut self, x: f32) -> bool {
        let Some(slider) = self.keymap_screens.edit.slider else {
            return false;
        };
        let Control::Param(param) = slider.control else {
            return false;
        };
        let Some((id, input, mut b)) = self.param_binding(param) else {
            return false;
        };
        param.set(&mut b, slider.value_at(x));
        let value = param.get(&b);
        if value == slider.value {
            return false;
        }
        self.keymap_screens.edit.slider = Some(Slider { value, moved: true, ..slider });
        self.set_tuned(id, input, b)
    }

    /// The finger lifted: the keymaps saved, if the slider changed anything.
    pub(crate) fn param_slide_end(&mut self) -> Vec<Effect> {
        match self.keymap_screens.edit.slider.take() {
            Some(s) if s.moved => vec![self.save_keymaps()],
            _ => vec![],
        }
    }
}
