//! What one event produces. [`crate::State::apply`] makes one [`Out`] per event and passes it to the
//! rules, which push effects in the order things happen and mark the screens they changed.

use crate::{Dirty, Effect};

#[derive(Debug, Default)]
pub(crate) struct Out {
    /// What must happen outside, in order.
    pub(crate) effects: Vec<Effect>,
    /// The screens to redraw.
    pub(crate) dirty: Dirty,
}

impl Out {
    /// Adds `dirty` to the screens to redraw.
    pub(crate) fn mark(&mut self, dirty: Dirty) {
        self.dirty = self.dirty.or(dirty);
    }
}
