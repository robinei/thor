//! uwsm, which runs ClamShell's compositor as the session's main unit (`uwsm start -- clamshell --drm`,
//! from thor-session), as it runs sway. Once ClamShell listens, `uwsm finalize` gives the user manager
//! WAYLAND_DISPLAY and DISPLAY, so that the session's services (udiskie) reach the compositor, and tells
//! systemd that the unit has started, which brings up graphical-session.target.

use std::process::Command;

/// Finalizes the session when ClamShell runs as a systemd unit that waits for it (NOTIFY_SOCKET, as
/// uwsm's is); started by hand, as in the bring-up image, there is nothing to finalize.
pub fn finalize(wayland: &str, x11: &str) {
    if std::env::var_os("NOTIFY_SOCKET").is_none() {
        log::info!("uwsm: not a unit waiting to be told it started: nothing to finalize");
        return;
    }
    let status = Command::new("uwsm").arg("finalize").env("WAYLAND_DISPLAY", wayland).env("DISPLAY", x11).status();
    match status {
        Ok(status) if status.success() => log::info!("uwsm: session finalized (WAYLAND_DISPLAY={wayland}, DISPLAY={x11})"),
        Ok(status) => log::error!("uwsm finalize: {status}: the session's services don't start"),
        Err(e) => log::error!("uwsm finalize: {e}: the session's services don't start"),
    }
}
