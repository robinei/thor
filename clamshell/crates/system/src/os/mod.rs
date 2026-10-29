//! Operating system plumbing for any thread. Nothing here knows ClamShell's state or features.
//!
//! - threads that restart when they fail ([`worker`]);
//! - running programs ([`command`]);
//! - D-Bus ([`dbus`]);
//! - sysfs and procfs ([`sys`]);
//! - cgroups ([`cgroup`]);
//! - DRM's per-client GPU time ([`drm`]);
//! - evdev ([`evdev`]);
//! - QRTR sockets ([`qrtr`]);
//! - files written whole, and JSON ([`fs`]);
//! - HTTP through curl ([`http`]);
//! - escaped text ([`text`]);
//! - the XDG data directories and icon themes ([`xdg`]).

pub(crate) mod cgroup;
pub use cgroup::unit_of;
pub use sys::{launcher_game_of, lineage};
pub(crate) mod command;
pub mod dbus;
pub(crate) mod drm;
pub mod evdev;
pub(crate) mod fs;
pub(crate) mod http;
pub(crate) mod qrtr;
pub(crate) mod sys;
pub(crate) mod text;
pub mod worker;
pub(crate) mod xdg;

/// Logs that `path` couldn't be read, for `context` (a module's name, or none): at debug when it isn't
/// there, which is an ordinary case (a file not made yet, a program not installed), else as a warning.
pub(crate) fn log_unread(context: &str, path: &std::path::Path, e: &std::io::Error) {
    let context = if context.is_empty() { String::new() } else { format!("{context}: ") };
    if e.kind() == std::io::ErrorKind::NotFound {
        log::debug!("{context}{}: {e}", path.display());
    } else {
        log::warn!("{context}{}: {e}", path.display());
    }
}

/// What a non-blocking read gave: None when nothing was waiting (`WouldBlock`).
pub(crate) fn ready<T>(read: std::io::Result<T>) -> std::io::Result<Option<T>> {
    match read {
        Ok(value) => Ok(Some(value)),
        Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => Ok(None),
        Err(e) => Err(e),
    }
}

/// Makes reads and writes of `fd` return at once, with `WouldBlock`, rather than wait.
pub(crate) fn set_nonblocking(fd: impl std::os::fd::AsFd) -> std::io::Result<()> {
    let flags = rustix::fs::fcntl_getfl(&fd)?;
    rustix::fs::fcntl_setfl(&fd, flags | rustix::fs::OFlags::NONBLOCK)?;
    Ok(())
}

/// Locks `m`. Nothing here panics while holding a lock, since the workspace's lints rule panics out. A
/// lock that is poisoned anyway still holds usable data, so it is taken as it is.
pub(crate) fn lock<T>(m: &std::sync::Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
}
