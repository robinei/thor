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
pub use sys::{lineage, steam_app_of};
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

/// Locks `m`. Nothing here panics while holding a lock, since the workspace's lints rule panics out. A
/// lock that is poisoned anyway still holds usable data, so it is taken as it is.
pub(crate) fn lock<T>(m: &std::sync::Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
}
