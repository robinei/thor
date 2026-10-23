//! What one event produces. [`crate::State::apply`] makes one [`Out`] per event and passes it to the
//! rules, which push effects in the order things happen and queue the requests that State coordinates
//! across areas, such as a tab change, as events.

use crate::{Effect, Event};

#[derive(Debug, Default)]
pub(crate) struct Out {
    /// What must happen outside, in order.
    pub(crate) effects: Vec<Effect>,
    /// Requests for State, handled in order after the event that queued them, so that their effects
    /// follow the ones pushed before them.
    pub(crate) events: Vec<Event>,
}
