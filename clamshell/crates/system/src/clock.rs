//! The clock: a `Tick` at the start of every second, and `Resumed` after the system slept (see
//! [`crate::sleep`]).

use std::io;
use std::thread;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use clamshell_core::Event;

use crate::{Closed, Sink, os::worker, sleep};

/// Seconds since the epoch on the wall clock at `t` (0 before the epoch).
pub fn unix_secs_at(t: SystemTime) -> u64 {
    t.duration_since(UNIX_EPOCH).map_or(0, |d| d.as_secs())
}

/// Seconds since the epoch on the wall clock now.
pub fn unix_secs() -> u64 {
    unix_secs_at(SystemTime::now())
}

/// Seconds on a monotonic clock, counted from the first call (the program's start); stopped while the
/// system sleeps.
pub fn mono_secs() -> u64 {
    static START: std::sync::OnceLock<std::time::Instant> = std::sync::OnceLock::new();
    START.get_or_init(std::time::Instant::now).elapsed().as_secs()
}

pub fn spawn(sink: impl Sink) -> io::Result<()> {
    worker::detached("clock", move || {
        let _ = tick(&sink);
    })
}

/// Ticks until the UI is gone.
fn tick(sink: &impl Sink) -> Result<(), Closed> {
    let mut asleep = sleep::Watch::new(sleep::slept());
    loop {
        let now = SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default();
        if asleep.woke(sleep::slept()) {
            log::info!("woke from sleep");
            sink.send(Event::Resumed)?;
        }
        sink.send(Event::Tick { unix_secs: now.as_secs(), mono_secs: mono_secs() })?;
        let into_second = Duration::from_nanos(u64::from(now.subsec_nanos()));
        thread::sleep(Duration::from_secs(1).saturating_sub(into_second));
    }
}
