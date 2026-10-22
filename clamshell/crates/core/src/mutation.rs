//! What an area's rule asks of the state as a whole. A rule changes its own area directly, and another
//! area with one owner through a parameter. A change that State coordinates across several areas, such
//! as a tab change or something done to an entry, is returned instead, and
//! [`crate::State::settle`] applies it in order with the effects around it.

use std::sync::Arc;

use crate::{Effect, Tab};

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Mutation {
    /// An effect, added to what the event returns.
    Effect(Effect),
    /// The user chose a tab.
    Tab(Tab),
    /// Something done to an entry, by its id.
    Act(Arc<str>, EntryAct),
}

/// What is done to an entry.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum EntryAct {
    /// Quit, as from the dock.
    Quit,
    ForceKill,
}
