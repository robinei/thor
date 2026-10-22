//! The rows of the lists of layers, one to pick a layer to show and one to pick a layer as a target, and
//! the layers the edit screen shows.

use super::*;

/// The layers on for the layer the edit screen shows (see [`State::layers_shown`]).
pub(super) struct LayersShown {
    layer: Layer,
    base: bool,
}

impl LayersShown {
    pub(super) fn as_slice(&self) -> &[Layer] {
        if self.base { &[] } else { std::slice::from_ref(&self.layer) }
    }
}

impl State {
    /// The number of rows in the list of layers (see [`crate::ListId::Layers`]): the keymap's layers, then
    /// New layer. As a target, Base is left out. On other screens there are none.
    pub(crate) fn layer_rows(&self) -> usize {
        let (id, all) = match self.keymap_screens.open {
            Some(Screen::LayerPicker(id)) => (id, true),
            Some(Screen::Layers { id, .. }) => (id, false),
            _ => return 0,
        };
        debug_assert!(self.keymaps.get(id).is_some(), "keymap {id}, whose layers are listed, is there");
        let n = self.keymaps.get(id).map_or(0, |k| k.layers().count());
        if all { n + 1 } else { n }
    }

    /// The layer on row `i` of the list of layers, if it is one. The last row is New layer.
    pub fn layer_row(&self, i: usize) -> Option<Layer> {
        let (id, skip) = match self.keymap_screens.open? {
            Screen::LayerPicker(id) => (id, 0),
            Screen::Layers { id, .. } => (id, 1),
            _ => return None,
        };
        self.keymaps.get(id)?.layers().nth(i + skip).map(|(l, _)| l)
    }

    /// A row of the list of layers: a layer to show, or to choose as a target, or New layer. Its Edit
    /// button or a long press opens the layer's menu.
    pub(crate) fn on_layer_row(&mut self, i: usize, part: RowPart, long: bool) -> Vec<Effect> {
        let target = matches!(self.keymap_screens.open, Some(Screen::Layers { .. }));
        let id = match (self.layer_row(i), part, target) {
            (Some(l), RowPart::Edit, false) => KeymapId::EditLayer(l),
            (Some(l), _, false) => KeymapId::Layer(l),
            (Some(l), _, true) => KeymapId::ToLayer(l),
            (None, ..) if i + 1 == self.layer_rows() => KeymapId::NewLayer,
            (None, ..) => return vec![],
        };
        self.on_keymap(id, long)
    }

    /// The layers on for the layer shown: none for Base, which is beneath everything, or else that layer.
    /// It doesn't allocate, because the screens ask for it while drawing.
    pub(super) fn layers_shown(&self) -> LayersShown {
        let layer = self.keymap_screens.edit.layer;
        LayersShown { layer, base: layer == Layer::BASE }
    }
}
