//! The edit screen's Undo: the keymap as it was before each change saved since the screen opened.

use super::*;

/// The edit screen's Undo: the keymap being edited as last saved, and as it was before each change.
#[derive(Debug, Clone, Default)]
pub struct Undo {
    pub(crate) last: Option<Keymap>,
    pub(crate) steps: Vec<Keymap>,
}

/// How many changes Undo goes back over.
pub(super) const UNDO_MAX: usize = 50;

impl State {
    /// The keymaps saved; a change to the keymap being edited is a step Undo goes back over.
    pub(super) fn save_keymaps(&mut self) -> Effect {
        let undo = &mut self.bottom.keymap_undo;
        if let Some(last) = undo.last.as_ref()
            && let Some(now) = self.keymaps.get(last.id)
            && now != last
        {
            let before = undo.last.replace(now.clone());
            undo.steps.extend(before);
            if undo.steps.len() > UNDO_MAX {
                undo.steps.remove(0);
            }
        }
        Effect::Save(Save::Keymaps(self.keymaps.text()))
    }

    /// Whether the edit screen's Undo has a change to go back over.
    pub fn can_undo(&self) -> bool {
        !self.bottom.keymap_undo.steps.is_empty()
    }

    /// Undo: the keymap being edited as it was before its last change (the layer shown Base if that one
    /// is gone).
    pub(super) fn undo(&mut self, id: u16) -> Vec<Effect> {
        let undo = &mut self.bottom.keymap_undo;
        let Some(before) = undo.steps.pop().filter(|k| k.id == id) else {
            return vec![];
        };
        undo.last = Some(before.clone());
        if let Some(k) = self.keymaps.get_mut(id) {
            *k = before;
        }
        let view = &mut self.bottom.keymap_edit;
        if self.keymaps.get(id).is_none_or(|k| k.layer_name(view.layer).is_none()) {
            view.layer = Layer::BASE;
        }
        view.slider = None;
        vec![self.save_keymaps()]
    }

    /// The edit screen of keymap `id`, as first opened: Base, nothing selected.
    pub(super) fn open_edit(&mut self, id: u16) {
        self.bottom.keymaps = Some(Screen::Edit(id));
        self.bottom.keymap_edit = EditView::default();
        self.bottom.keymap_undo = Undo { last: self.keymaps.get(id).cloned(), steps: Vec::new() };
    }
}
