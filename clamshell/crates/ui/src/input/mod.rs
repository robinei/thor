//! Input as pure state machines, told what happens and answering with what to do: presses from touch,
//! the mouse and keys ([`press`]), a list's fling ([`fling`]), the pad's focus ([`focus`]) and the
//! direction it steps in ([`nav`]).

pub(crate) mod fling;
pub(crate) mod focus;
pub(crate) mod nav;
pub(crate) mod press;

pub(crate) use press::{Action, End, EventClock, Input, Source, Timer, points};
