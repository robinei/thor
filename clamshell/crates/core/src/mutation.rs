//! What an area's rule asks of the rest of the state. A rule changes its own area directly and returns
//! the changes it needs elsewhere, which [`crate::State::settle`] applies in order.

use crate::Effect;
use crate::bottom::editor::Editor;
use crate::bottom::notice::NoticeKind;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Mutation {
    /// An effect, added to what the event returns.
    Effect(Effect),
    /// A notice under the bar.
    Notify(NoticeKind),
    /// An editor opened in its tab.
    OpenEditor(Editor),
}
