//! What was fetched about an entry, and a video's length. Each is a file per entry in the art cache,
//! beside its images: `<key>-meta.json` and `<key>-probe.json`, with the entry's id inside. Each file is
//! written whole by one thread, either the metadata fetcher or the folder browser.

use std::collections::HashMap;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use clamshell_library::key_of;
use clamshell_state::Meta;
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};

use crate::os::fs::{read_json, write_json};

/// A video's length and a frame of it, as probed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Probe {
    pub(crate) seconds: Option<u32>,
    pub(crate) frame: Option<PathBuf>,
}

/// What was fetched about an entry, from where (as [`crate::library::meta::How::source`] gives it) and
/// when.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Fetched {
    pub(crate) source: String,
    pub(crate) at: u64,
    pub(crate) meta: Meta,
}

/// The cache's files.
pub(crate) struct Cache {
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
    pub(crate) fn open(dir: &Path) -> Cache {
        Cache { dir: dir.to_path_buf() }
    }

    /// Reads the files whose names end in `suffix`.
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

    /// What was fetched about each entry, from where and when.
    pub(crate) fn metas(&self) -> HashMap<String, Fetched> {
        self.read_all::<MetaFile>("-meta.json").into_iter().map(|(_, m)| (m.id, Fetched { source: m.source, at: m.fetched, meta: m.meta })).collect()
    }

    /// Saves what was fetched about `id` from `source` at `at`. An empty `meta` records a miss.
    pub(crate) fn save_meta(&self, id: &str, source: &str, at: u64, meta: &Meta) -> io::Result<()> {
        let file = MetaFile { id: id.to_owned(), source: source.to_owned(), fetched: at, meta: meta.clone() };
        write_json(&self.dir.join(format!("{}-meta.json", key_of(id))), &file)
    }

    /// The videos' lengths and frames, by id. A length is none if ffprobe couldn't tell.
    pub(crate) fn probes(&self) -> HashMap<String, Probe> {
        self.read_all::<ProbeFile>("-probe.json").into_iter().map(|(_, p)| (p.id, Probe { seconds: p.seconds, frame: p.frame })).collect()
    }

    pub(crate) fn save_probe(&self, id: &str, seconds: Option<u32>, frame: Option<&Path>) -> io::Result<()> {
        write_json(&self.dir.join(format!("{}-probe.json", key_of(id))), &ProbeFile { id: id.to_owned(), seconds, frame: frame.map(Path::to_path_buf) })
    }

    /// Forgets the misses, where nothing was found, so they are fetched again. Returns how many.
    pub(crate) fn forget_misses(&self) -> usize {
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
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path();
        let cache = Cache::open(dir);
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
