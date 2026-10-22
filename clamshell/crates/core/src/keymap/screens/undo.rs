//! The edit screen's Undo: the keymap as it was before each change saved since the screen opened.

use std::collections::VecDeque;

use super::*;

/// The edit screen's Undo: the keymap being edited as last saved, and as it was before each change.
#[derive(Debug, Clone, Default)]
pub struct Undo {
    pub(crate) last: Option<Keymap>,
    pub(crate) steps: VecDeque<Keymap>,
}

/// How many changes Undo goes back over.
pub(super) const UNDO_MAX: usize = 50;

impl State {
    /// Saves the keymaps. A change to the keymap being edited becomes a step that Undo can go back over.
    pub(crate) fn save_keymaps(&mut self) -> Effect {
        let undo = &mut self.keymap_screens.undo;
        if let Some(last) = undo.last.as_ref()
            && let Some(now) = self.keymaps.get(last.id)
            && now != last
        {
            let before = undo.last.replace(now.clone());
            undo.steps.extend(before);
            if undo.steps.len() > UNDO_MAX {
                undo.steps.pop_front();
            }
        }
        Effect::Save(Save::Keymaps(self.keymaps.clone()))
    }

    /// Saves the keymaps if `changed` (see [`State::save_keymaps`]).
    pub(super) fn saved_if(&mut self, changed: bool) -> Vec<Effect> {
        if changed { vec![self.save_keymaps()] } else { vec![] }
    }

    /// Whether the edit screen's Undo has a change to go back over.
    pub fn can_undo(&self) -> bool {
        !self.keymap_screens.undo.steps.is_empty()
    }

    /// Undo: puts the keymap being edited back as it was before its last change. If the layer shown is
    /// gone after that, Base is shown.
    pub(super) fn undo(&mut self, id: u16) -> Vec<Effect> {
        let undo = &mut self.keymap_screens.undo;
        // Only the keymap being edited should have steps. A step for another keymap is left, not dropped.
        let Some(before) = undo.steps.pop_back_if(|k| k.id == id) else {
            return vec![];
        };
        undo.last = Some(before.clone());
        if let Some(k) = self.keymaps.get_mut(id) {
            *k = before;
        }
        let view = &mut self.keymap_screens.edit;
        if self.keymaps.get(id).is_none_or(|k| k.layer_name(view.layer).is_none()) {
            view.layer = Layer::BASE;
        }
        view.slider = None;
        vec![self.save_keymaps()]
    }

    /// Opens the edit screen of keymap `id` fresh: Base shown and nothing selected.
    pub(super) fn open_edit(&mut self, id: u16) {
        self.keymap_screens.open = Some(Screen::Edit(id));
        self.keymap_screens.edit = EditView::default();
        self.keymap_screens.undo = Undo { last: self.keymaps.get(id).cloned(), steps: VecDeque::new() };
    }
}
