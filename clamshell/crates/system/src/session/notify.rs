//! Notifications through the session's notification daemon, which is mako on the main screen.

use crate::os::dbus;

/// Sends a notification through the session bus' notification daemon, replacing notification `replaces`
/// unless it is 0. Returns its id.
pub fn notify(replaces: u32, summary: &str, body: &str, urgent: bool) -> dbus::Result<u32> {
    // Urgency 1 is normal and 2 is critical, which stays until dismissed. Timeout -1 uses the daemon's own.
    let hints = dbus::PropMap::from([("urgency".to_owned(), dbus::value(if urgent { 2u8 } else { 1u8 }))]);
    let actions: Vec<&str> = Vec::new();
    let (id,): (u32,) = dbus::Bus::session()?.call(
        "org.freedesktop.Notifications",
        "/org/freedesktop/Notifications",
        "org.freedesktop.Notifications",
        "Notify",
        ("ClamShell", replaces, "", summary, body, actions, hints, -1i32),
    )?;
    Ok(id)
}
