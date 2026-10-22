//! Removable filesystems: udisks2's mounts under `/run/media`.

use std::fs;
use std::path::PathBuf;

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
