//! The compositor's thread at a higher priority than the apps': the most negative nice level rtkit gives,
//! asked of it over the system bus, so that the screens keep up under a game that takes every core. Not
//! real-time scheduling: the kernel kills a real-time thread that runs on past RLIMIT_RTTIME, which rtkit
//! requires, and the compositor draws ClamShell's screens on the CPU.
//!
//! A thread inherits its starter's nice level, so the system's threads go back to the ordinary level as
//! they start ([`crate::os::worker::start`]). What the compositor's thread starts directly keeps it:
//! Xwayland, which draws X11 games' frames, and short-lived commands.

use crate::os::dbus::Bus;

const RTKIT: &str = "org.freedesktop.RealtimeKit1";
const PATH: &str = "/org/freedesktop/RealtimeKit1";

/// Raises `thread` (a thread id of this process). A failure is logged: the thread runs on at its nice
/// level as it was.
pub fn raise(thread: u64) {
    let result = Bus::system().and_then(|mut bus| {
        let level: i32 = bus.get(RTKIT, PATH, RTKIT, "MinNiceLevel")?;
        bus.call::<()>(RTKIT, PATH, RTKIT, "MakeThreadHighPriority", (thread, level))?;
        Ok(level)
    });
    match result {
        Ok(level) => log::info!("priority: thread {thread} at nice level {level} (rtkit)"),
        Err(e) => log::warn!("priority: rtkit didn't raise thread {thread}: {e}; it runs at its nice level as it was"),
    }
}
