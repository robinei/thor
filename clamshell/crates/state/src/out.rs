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
    /// What went wrong and was worked around, for the journal (see [`crate::State::take_log`]).
    pub(crate) log: Vec<(LogLevel, String)>,
}

/// How much a line of the state's log matters, as the journal's levels: what is expected at debug, what
/// is worth knowing at info, what went wrong at warn.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LogLevel {
    Debug,
    Info,
    Warn,
}

impl Out {
    pub(crate) fn log(&mut self, level: LogLevel, line: String) {
        self.log.push((level, line));
    }
}
