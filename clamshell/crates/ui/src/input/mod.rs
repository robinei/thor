//! Input handling as pure state machines. Each is told what happened and answers with what to do. They
//! cover presses from touch, the mouse and keys ([`press`]), flinging a list ([`fling`]), the pad's focus
//! ([`focus`]) and the direction the focus steps in ([`nav`]).

pub mod fling;
pub mod focus;
pub mod nav;
pub mod press;

pub use press::{Action, EventClock, Input, Timer, points};
pub use press::{End, Source};
