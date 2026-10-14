//! Threads that watch the system and send [`Event`]s: today only the clock. Later evdev and uinput (the
//! input loop, at raised priority), logind inhibitor locks, sway IPC, sysfs (power, backlight, charger).
//! Nothing here waits for the UI.

use std::thread::{self, JoinHandle};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use sysprog_core::Event;

/// Where policy threads deliver events. Returns false once the receiver is gone, and the thread stops.
pub trait Sink: Send + 'static {
    fn send(&self, event: Event) -> bool;
}

impl<F: Fn(Event) -> bool + Send + 'static> Sink for F {
    fn send(&self, event: Event) -> bool {
        self(event)
    }
}

/// Sends a `Tick` at the start of every second.
pub fn spawn_clock(sink: impl Sink) -> std::io::Result<JoinHandle<()>> {
    thread::Builder::new().name("clock".into()).spawn(move || {
        loop {
            let now = SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default();
            if !sink.send(Event::Tick { unix_secs: now.as_secs() }) {
                return;
            }
            let into_second = Duration::from_nanos(u64::from(now.subsec_nanos()));
            thread::sleep(Duration::from_secs(1).saturating_sub(into_second));
        }
    })
}
