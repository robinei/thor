//! Files written whole, so that a crash or power loss mid-write leaves the last complete one, and JSON
//! read and written that way.

use std::fs;
use std::io::{self, Write as _};
use std::path::{Path, PathBuf};

use serde::Serialize;
use serde::de::DeserializeOwned;

/// Writes `bytes` to `path` whole: to a file beside it, flushed to the disk, then renamed over it (and
/// the rename made to last).
pub fn write_file(path: &Path, bytes: &[u8]) -> io::Result<()> {
    let dir = path.parent().ok_or_else(|| io::Error::other("no folder"))?;
    fs::create_dir_all(dir)?;
    let mut part = path.as_os_str().to_owned();
    part.push(".part");
    let part = PathBuf::from(part);
    let mut file = fs::File::create(&part)?;
    file.write_all(bytes)?;
    file.sync_all()?;
    fs::rename(&part, path)?;
    fs::File::open(dir)?.sync_all()
}

/// `value` as pretty JSON, written whole ([`write_file`]).
pub fn write_json(path: &Path, value: &impl Serialize) -> io::Result<()> {
    let mut text = serde_json::to_string_pretty(value).map_err(io::Error::other)?;
    text.push('\n');
    write_file(path, text.as_bytes())
}

/// A file read as `T`: none if it isn't there, or doesn't read (logged).
pub fn read_json<T: DeserializeOwned>(path: &Path) -> Option<T> {
    let text = fs::read_to_string(path)
        .map_err(|e| {
            if e.kind() != io::ErrorKind::NotFound {
                log::warn!("{}: {e}", path.display())
            }
        })
        .ok()?;
    serde_json::from_str(&text).map_err(|e| log::warn!("{}: {e}", path.display())).ok()
}
