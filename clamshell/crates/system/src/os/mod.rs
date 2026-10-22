//! The operating system's plumbing, for any thread; nothing here knows ClamShell's state or features:
//!
//! - threads that restart what failed ([`worker`]); running programs ([`command`]);
//! - D-Bus ([`dbus`]); sysfs and procfs ([`sys`]); cgroups ([`cgroup`]); DRM's per-client GPU time
//!   ([`drm`]); evdev ([`evdev`]); QRTR sockets ([`qrtr`]);
//! - files written whole and JSON ([`fs`]); HTTP by curl ([`http`]); escaped text ([`text`]); the XDG
//!   data directories and icon themes ([`xdg`]).

pub mod cgroup;
pub mod command;
pub mod dbus;
pub mod drm;
pub mod evdev;
pub mod fs;
pub mod http;
pub(crate) mod qrtr;
pub mod sys;
pub mod text;
pub mod worker;
pub mod xdg;

/// Locks `m`. Nothing here panics while holding a lock (the workspace's lints rule panics out), so a lock
/// poisoned anyway still holds usable data: it is taken as it is.
pub(crate) fn lock<T>(m: &std::sync::Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
}
