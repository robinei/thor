//! Removable filesystems: udisks2's mounts under `/run/media`.

use std::fs;
use std::path::PathBuf;

use std::time::Duration;

use crate::os::dbus::{self, Bus, PropMap, Props};
use crate::os::{sys, text};

/// Where udisks2 mounts removable filesystems.
pub(super) const MEDIA: &str = "/run/media/";

/// A removable filesystem mounted: its device, where, its type, label and UUID (empty if none).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Mount {
    pub device: String,
    pub dir: PathBuf,
    pub fs: String,
    pub label: String,
    pub uuid: String,
}

/// The removable filesystems mounted (under `/run/media`), in the order they were mounted.
pub fn media_mounts() -> Vec<Mount> {
    let named = |dir: &str| -> Vec<(String, PathBuf)> {
        fs::read_dir(dir)
            .map(|d| d.flatten().filter_map(|e| Some((text::unescape(&e.file_name().into_string().ok()?), fs::canonicalize(e.path()).ok()?))).collect())
            .unwrap_or_default()
    };
    let (labels, uuids) = (named("/dev/disk/by-label"), named("/dev/disk/by-uuid"));
    sys::mount_table()
        .into_iter()
        .filter(|m| m.dir.starts_with(MEDIA))
        .map(|m| {
            let real = fs::canonicalize(&m.device).ok();
            let find = |names: &[(String, PathBuf)]| names.iter().find(|(_, p)| Some(p) == real.as_ref()).map(|(n, _)| n.clone()).unwrap_or_default();
            let label = Some(find(&labels))
                .filter(|l| !l.is_empty())
                .unwrap_or_else(|| m.dir.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default());
            Mount { device: m.device, fs: m.fs, label, uuid: find(&uuids), dir: m.dir }
        })
        .collect()
}

const UDISKS: &str = "org.freedesktop.UDisks2";

/// Unmounts `device` through udisks2, then powers its drive off. Unmounting writes out what is pending,
/// which on a slow card can take a while, so it gets `limit`. Powering off is allowed to fail: an SD
/// card reader may not support it, and unmounted is enough.
pub(super) fn eject(device: &str, limit: Duration) -> dbus::Result<()> {
    let mut bus = Bus::system()?.with_timeout(limit);
    // No password prompt: polkit allows the session's user, or the call fails.
    let options = || PropMap::from([("auth.no_user_interaction".to_owned(), dbus::value(true))]);
    let spec = PropMap::from([("path".to_owned(), dbus::value(device.to_owned()))]);
    let (blocks,): (Vec<dbus::Path<'static>>,) =
        bus.call(UDISKS, "/org/freedesktop/UDisks2/Manager", "org.freedesktop.UDisks2.Manager", "ResolveDevice", (spec, options()))?;
    let block = blocks.first().ok_or_else(|| dbus::Error::Reply(format!("udisks2 doesn't know {device}")))?.to_string();
    bus.call::<()>(UDISKS, &block, "org.freedesktop.UDisks2.Filesystem", "Unmount", (options(),))?;
    let drive = bus.get_all(UDISKS, &block, "org.freedesktop.UDisks2.Block").map(|props| Props(&props).path("Drive"));
    match drive {
        Ok(Some(drive)) if drive != "/" => {
            if let Err(e) = bus.call::<()>(UDISKS, &drive, "org.freedesktop.UDisks2.Drive", "PowerOff", (options(),)) {
                log::info!("powering off {drive}: {e}");
            }
        }
        Ok(_) => {}
        Err(e) => log::info!("the drive of {device}: {e}"),
    }
    Ok(())
}
