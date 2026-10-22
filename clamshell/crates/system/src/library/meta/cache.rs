//! What was fetched about an entry, and a video's length: a file per entry in the art cache, beside its
//! images (`<key>-meta.json`, `<key>-probe.json`; the entry's id in the file). Each is written whole by
//! one thread (the metadata's, the folder browser's).

use std::collections::HashMap;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use clamshell_core::Meta;
use clamshell_library::key_of;
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};

use crate::os::fs::{read_json, write_json};

/// A video's length and a frame of it, as read.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Probe {
    pub seconds: Option<u32>,
    pub frame: Option<PathBuf>,
}

/// What was fetched about an entry, from where (as [`crate::library::meta::How::source`] says) and when.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Fetched {
    pub source: String,
    pub at: u64,
    pub meta: Meta,
}

/// The cache's files.
pub struct Cache {
    dir: PathBuf,
}

/// A `<key>-meta.json`: what was fetched about an entry, from where and when.
#[derive(Serialize, Deserialize)]
struct MetaFile {
    id: String,
    source: String,
    fetched: u64,
    #[serde(flatten)]
    meta: Meta,
}

#[derive(Serialize, Deserialize)]
struct ProbeFile {
    id: String,
    #[serde(default)]
    seconds: Option<u32>,
    #[serde(default)]
    frame: Option<PathBuf>,
}

impl Cache {
    pub fn open(dir: &Path) -> Cache {
        Cache { dir: dir.to_path_buf() }
    }

    /// The files whose names end in `suffix`, read.
    fn read_all<T: DeserializeOwned>(&self, suffix: &str) -> Vec<(PathBuf, T)> {
        let entries = fs::read_dir(&self.dir)
            .inspect_err(|e| {
                if e.kind() == io::ErrorKind::NotFound {
                    log::debug!("meta: {}: {e}", self.dir.display());
                } else {
                    log::warn!("meta: {}: {e}", self.dir.display());
                }
            })
            .into_iter()
            .flatten()
            .flatten();
        entries.map(|e| e.path()).filter(|p| p.to_str().is_some_and(|p| p.ends_with(suffix))).filter_map(|p| Some((p.clone(), read_json(&p)?))).collect()
    }

    /// What was fetched about each entry: from where, when, and what.
    pub fn metas(&self) -> HashMap<String, Fetched> {
        self.read_all::<MetaFile>("-meta.json").into_iter().map(|(_, m)| (m.id, Fetched { source: m.source, at: m.fetched, meta: m.meta })).collect()
    }

    /// What was fetched about `id` (nothing: a miss), from `source`, at `at`.
    pub fn save_meta(&self, id: &str, source: &str, at: u64, meta: &Meta) -> io::Result<()> {
        let file = MetaFile { id: id.to_owned(), source: source.to_owned(), fetched: at, meta: meta.clone() };
        write_json(&self.dir.join(format!("{}-meta.json", key_of(id))), &file)
    }

    /// The videos' lengths (none: ffprobe couldn't tell) and frames, by id.
    pub fn probes(&self) -> HashMap<String, Probe> {
        self.read_all::<ProbeFile>("-probe.json").into_iter().map(|(_, p)| (p.id, Probe { seconds: p.seconds, frame: p.frame })).collect()
    }

    pub fn save_probe(&self, id: &str, seconds: Option<u32>, frame: Option<&Path>) -> io::Result<()> {
        write_json(&self.dir.join(format!("{}-probe.json", key_of(id))), &ProbeFile { id: id.to_owned(), seconds, frame: frame.map(Path::to_path_buf) })
    }

    /// The misses (fetched, nothing found) forgotten: fetched again. How many.
    pub fn forget_misses(&self) -> usize {
        let misses = self.read_all::<MetaFile>("-meta.json").into_iter().filter(|(_, m)| m.meta == Meta::default());
        misses.filter(|(path, _)| fs::remove_file(path).inspect_err(|e| log::warn!("meta: removing {}: {e}", path.display())).is_ok()).count()
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    #[test]
    fn the_cache_keeps_a_file_per_entry() {
        let dir = clamshell_core::kit::testdir::TempDir::new("cache");
        let cache = Cache::open(&dir);
        let details = clamshell_library::Details { year: Some(1994), summary: "S".into(), ..Default::default() };
        let meta = Meta { cover: Some("/c/x-cover.png".into()), details, ..Meta::default() };
        cache.save_meta("x", "libretro:SNES", 5, &meta).unwrap();
        cache.save_meta("y", "steam:1", 6, &Meta::default()).unwrap();
        let metas = cache.metas();
        assert_eq!(metas.get("x"), Some(&Fetched { source: "libretro:SNES".into(), at: 5, meta }));
        assert_eq!(metas.get("y").map(|f| &f.meta), Some(&Meta::default()), "a miss");
        assert_eq!(cache.forget_misses(), 1);
        assert!(!cache.metas().contains_key("y") && cache.metas().contains_key("x"));
        cache.save_probe("ep", Some(2700), Some(Path::new("/c/ep-frame.jpg"))).unwrap();
        cache.save_probe("odd", None, None).unwrap();
        let probes = cache.probes();
        assert_eq!(probes.get("ep"), Some(&Probe { seconds: Some(2700), frame: Some("/c/ep-frame.jpg".into()) }));
        assert_eq!(probes.get("odd").map(|p| p.seconds), Some(None));
        assert!(dir.join(format!("{}-meta.json", key_of("x"))).exists());
    }
}
