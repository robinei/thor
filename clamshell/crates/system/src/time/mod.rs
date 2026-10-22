//! Time: the clock's ticks, noticing a wake from sleep through the clocks, and setting the time zone and
//! clock through timedated.

pub mod clock;
pub(crate) mod timedated;
pub(crate) mod wake;
