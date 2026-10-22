//! Notifications through the session's notification daemon (mako, on the main screen).

use crate::os::dbus;

/// A notification through the session bus' notification daemon, replacing `replaces` (0: none); its id.
pub fn notify(replaces: u32, summary: &str, body: &str, urgent: bool) -> dbus::Result<u32> {
    // Urgency: 1 normal, 2 critical (stays until dismissed). Timeout -1: the daemon's own.
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
