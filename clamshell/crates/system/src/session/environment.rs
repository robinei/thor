//! The session's displays, given to the user manager and the bus once ClamShell's compositor listens on
//! the device's screens, so that the session's services (udiskie) and what D-Bus starts (portals) reach
//! it: `WAYLAND_DISPLAY` and `DISPLAY`. thor-session takes them out again when the session ends.

use std::collections::HashMap;

use crate::os::dbus::Bus;

/// Puts `WAYLAND_DISPLAY` and `DISPLAY` in the user manager's environment and the bus's activation
/// environment. A failure is logged: the session runs on, without its services reaching the screens.
pub fn export(wayland: &str, x11: &str) {
    let result = Bus::session().and_then(|mut bus| {
        let assignments = vec![format!("WAYLAND_DISPLAY={wayland}"), format!("DISPLAY={x11}")];
        bus.call::<()>("org.freedesktop.systemd1", "/org/freedesktop/systemd1", "org.freedesktop.systemd1.Manager", "SetEnvironment", (assignments,))?;
        let variables: HashMap<&str, &str> = HashMap::from([("WAYLAND_DISPLAY", wayland), ("DISPLAY", x11)]);
        bus.call::<()>("org.freedesktop.DBus", "/org/freedesktop/DBus", "org.freedesktop.DBus", "UpdateActivationEnvironment", (variables,))
    });
    match result {
        Ok(()) => log::info!("session: WAYLAND_DISPLAY={wayland} and DISPLAY={x11} given to the user manager and the bus"),
        Err(e) => {
            log::error!("session: giving WAYLAND_DISPLAY and DISPLAY to the user manager and the bus: {e}: the session's services don't reach the screens")
        }
    }
}
