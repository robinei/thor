//! Input handling as pure state machines. Each is told what happened and answers with what to do. They
//! cover presses from touch, the mouse and keys ([`press`]), flinging a list ([`fling`]), the pad's focus
//! ([`focus`]) and the direction the focus steps in ([`nav`]).

pub(crate) mod fling;
pub(crate) mod focus;
pub(crate) mod nav;
pub(crate) mod press;

pub(crate) use press::{Action, End, EventClock, Input, Source, Timer, points};
