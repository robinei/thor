//! Time: the clock's ticks, waking from sleep seen from the clocks, and the time zone and clock as
//! timedated sets them.

pub mod clock;
pub(crate) mod timedated;
pub(crate) mod wake;
