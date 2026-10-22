//! Fetching what the library's files don't say: covers, screenshots, a summary, the year an entry came
//! out and who made it.
//!
//! After the library is read, the entries it wants (see [`Wanted`]) are fetched with curl on a few
//! threads. Their images are kept in the art cache (~/.cache/clamshell/art). What was found, and also
//! what wasn't, is kept beside them (see [`crate::library::meta::cache::Cache`]) and sent to the state as
//! it arrives.
//!
//! - A content dir's games, with `"metadata": "libretro:<system>"` in library.json, where the system is
//!   named as libretro names its folder. Box art and a screenshot come from libretro's thumbnails, and the
//!   developer and year from libretro's database. The game is found by its file's CRC32 checksum in the
//!   database's No-Intro list. For a zip the checksum comes from its directory; other files up to 64 MB
//!   are read whole. Failing that, the game is found by name:
//!   - the entry's title as libretro names the game;
//!   - or the same after ignoring case, bracketed tags (`(USA)`), "the" and punctuation, treating `and`
//!     and `&` alike, and reading Roman numerals as numbers;
//!   - or, if only one game fits, the same but without libretro's subtitle after ` - `.
//!
//!   Of several releases, the USA one is preferred, then World, then Europe. A subtitle in the title
//!   itself isn't ignored, since "Front Mission - Gun Hazard" is a different game from "Front Mission".
//! - A content dir's films or series (`"tmdb:movie"`, `"tmdb:tv"`): poster, backdrop, overview and year
//!   from TMDB, by title. A `(1980)` at the end of the title is the year and is tried first, but a series
//!   may be dated from a different year than its pilot. Needs `keys.tmdb` in library.json.
//! - Steam's games: the store's description, developer, year and first screenshot. No key is needed.
//! - Games from desktop files: a cover from SteamGridDB, by name. Needs `keys.steamgriddb`.
//!
//! Art beside an entry's file comes first; what is fetched never replaces it.
//!
//! libretro's lists (a system's thumbnails and its database's files) are kept in
//! ~/.cache/clamshell/libretro and fetched again after [`LISTS_KEPT`]. An older copy is used while a
//! fetch fails.

pub(crate) mod cache;
mod libretro;
mod probe;
mod steam;
mod steamgriddb;
mod titles;
mod tmdb;

use std::collections::{HashMap, VecDeque};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use clamshell_core::{Event, Meta};
use serde::Deserialize;

use cache::Cache;
use libretro::Database;
use titles::Name;

use crate::Sink;
use crate::os::{http, worker};

/// How many fetches run at once.
const THREADS: usize = 4;
/// How long a libretro list kept on disk is used before it is fetched again.
const LISTS_KEPT: std::time::Duration = std::time::Duration::from_secs(30 * 86_400);

/// How long after a miss, when nothing was found, to try again, in seconds.
pub const RETRY_MISS: u64 = 30 * 86_400;

/// The services' keys, from library.json's `keys`.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Keys {
    #[serde(default)]
    pub tmdb: Option<String>,
    #[serde(default)]
    pub steamgriddb: Option<String>,
}

/// Where an entry's metadata comes from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum How {
    /// libretro's thumbnails of this system, named as its folder there.
    Libretro(String),
    /// TMDB, a film (`movie`) or a series (`tv`).
    Tmdb(String),
    /// Steam's store page of this app.
    Steam(u32),
    /// SteamGridDB, by name.
    SteamGridDb,
    /// A video's length from ffprobe, and a frame from a quarter of the way in from ffmpeg. These are
    /// stored apart from the rest (see [`crate::store`]).
    Duration,
}

impl How {
    /// Parses library.json's `metadata` field: `libretro:<system>`, `tmdb:movie`, `tmdb:tv` or
    /// `steamgriddb`. `steam`, a Steam game's store page, needs the app id and is handled by the library.
    pub fn parse(field: &str) -> Option<How> {
        if field == "steamgriddb" {
            return Some(How::SteamGridDb);
        }
        match field.split_once(':')? {
            ("libretro", system) if !system.is_empty() => Some(How::Libretro(system.to_owned())),
            ("tmdb", kind @ ("movie" | "tv")) => Some(How::Tmdb(kind.to_owned())),
            _ => None,
        }
    }

    /// The form saved with what was found. The entry is fetched again when this changes.
    pub fn source(&self) -> String {
        match self {
            How::Libretro(system) => format!("libretro:{system}"),
            How::Tmdb(kind) => format!("tmdb:{kind}"),
            How::Steam(appid) => format!("steam:{appid}"),
            How::SteamGridDb => "steamgriddb".into(),
            How::Duration => "duration".into(),
        }
    }
}

/// An entry to fetch for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Wanted {
    pub id: Arc<str>,
    /// The entry's key, which names its files in the art cache.
    pub key: String,
    pub title: String,
    /// The entry's file in its content dir, for the checksum.
    pub file: Option<PathBuf>,
    pub how: How,
    /// Whether it has art of its own already, so no cover is wanted.
    pub art: bool,
}

/// What a fetch needs.
pub struct Fetch {
    pub wanted: Vec<Wanted>,
    pub keys: Keys,
    /// The art cache, which also keeps what was fetched (see [`Cache`]).
    pub art: PathBuf,
}

/// The entries waiting to be fetched for, in order, and the fetching threads at work.
#[derive(Clone, Default)]
pub struct Queue {
    waiting: Arc<Mutex<VecDeque<Wanted>>>,
    threads: Arc<std::sync::atomic::AtomicUsize>,
}

impl Queue {
    /// Moves these entries to the front, in this order, if they are still waiting.
    pub fn first(&self, ids: &[Arc<str>]) {
        let mut q = crate::os::lock(&self.waiting);
        for id in ids.iter().rev() {
            if let Some(at) = q.iter().position(|w| w.id == *id)
                && let Some(w) = q.remove(at)
            {
                q.push_front(w);
            }
        }
    }

    /// Adds `wanted` at the end, skipping entries already waiting. Returns how many wait now.
    fn add(&self, wanted: Vec<Wanted>) -> usize {
        let mut q = crate::os::lock(&self.waiting);
        for w in wanted {
            if !q.iter().any(|x| x.id == w.id) {
                q.push_back(w);
            }
        }
        q.len()
    }

    fn next(&self) -> Option<Wanted> {
        crate::os::lock(&self.waiting).pop_front()
    }
}

/// Adds what `fetch` wants to `queue` and fetches on [`THREADS`] more threads until the queue is empty or
/// the UI is gone. Sends `Fetching(true)` as they start and `Fetching(false)` when the last thread is
/// done.
pub fn spawn(fetch: Fetch, queue: &Queue, sink: impl Sink + Clone) -> std::io::Result<()> {
    use std::sync::atomic::Ordering;
    let count = queue.add(fetch.wanted);
    if count == 0 {
        return Ok(());
    }
    log::info!("meta: fetching for {count} entries");
    let _ = sink.send(Event::Fetching(true));
    let shared = Arc::new(Shared { keys: fetch.keys, art: fetch.art, listings: Mutex::new(HashMap::new()), databases: Mutex::new(HashMap::new()) });
    for _ in 0..THREADS {
        let (queue, shared, sink) = (queue.clone(), shared.clone(), sink.clone());
        queue.threads.fetch_add(1, Ordering::SeqCst);
        worker::detached("meta", move || {
            let cache = Cache::open(&shared.art);
            while let Some(w) = queue.next() {
                let meta = shared.fetch(&w);
                let saved = match &w.how {
                    How::Duration => cache.save_probe(&w.id, meta.duration, meta.details.shot.as_deref()),
                    _ => cache.save_meta(&w.id, &w.how.source(), crate::time::clock::unix_secs(), &meta),
                };
                if let Err(e) = saved {
                    log::warn!("meta: {}: {e}", w.id);
                }
                if meta != Meta::default() && sink.send(Event::Meta { id: w.id.clone(), meta }).is_err() {
                    break;
                }
            }
            if queue.threads.fetch_sub(1, Ordering::SeqCst) == 1 {
                log::info!("meta: done");
                let _ = sink.send(Event::Fetching(false));
            }
        })?;
    }
    Ok(())
}

/// What the threads share: the keys, where images go, and libretro's file lists. Each list is fetched
/// once per system and kind, and is `None` if that failed.
struct Shared {
    keys: Keys,
    art: PathBuf,
    listings: Mutex<HashMap<String, Option<Arc<Vec<Name>>>>>,
    /// libretro's database of each system, fetched once.
    databases: Mutex<HashMap<String, Arc<Database>>>,
}

impl Shared {
    fn fetch(&self, w: &Wanted) -> Meta {
        let meta = match &w.how {
            How::Libretro(system) => self.libretro(w, system),
            How::Tmdb(kind) => self.tmdb(w, kind),
            How::Steam(appid) => self.steam(w, *appid),
            How::SteamGridDb => self.steamgriddb(w),
            How::Duration => self.probe(w),
        };
        log::debug!("meta: {}: {}", w.id, if meta == Meta::default() { "nothing found" } else { "found" });
        meta
    }

    /// One of libretro's lists, fetched from `url` and kept on disk as `name` in the lists' folder next to
    /// the art cache (see [`http::kept`]).
    fn list(&self, name: &str, url: &str) -> Option<String> {
        let dir = self.art.with_file_name("libretro");
        let file = dir.join(name.replace('/', "_"));
        http::kept(&file, LISTS_KEPT, || http::get(url, &[])).map(|b| String::from_utf8_lossy(&b).into_owned())
    }

    /// Downloads the image at `url` into the art cache as `<key>-<what>.<extension>`.
    fn image(&self, w: &Wanted, what: &str, url: &str) -> Option<PathBuf> {
        let file = self.art.join(format!("{}-{what}.{}", w.key, extension(url)));
        http::download(url, &[], &file).then_some(file)
    }
}

/// An image URL's extension: png or webp if it says so, otherwise jpg.
fn extension(url: &str) -> &'static str {
    let path = url.split(['?', '#']).next().unwrap_or(url).to_ascii_lowercase();
    match path.rsplit('.').next() {
        Some("png") => "png",
        Some("webp") => "webp",
        _ => "jpg",
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::indexing_slicing, clippy::panic)]
mod tests {
    use super::libretro::hrefs;
    use super::titles::{entities, title_and_year, year_in};
    use super::*;

    #[test]
    fn what_shelf_shows_comes_first() {
        let wanted = |id: &str| Wanted { id: id.into(), key: id.into(), title: id.into(), file: None, how: How::SteamGridDb, art: false };
        let queue = Queue::default();
        queue.add(["a", "b", "c", "d"].map(wanted).to_vec());
        assert_eq!(queue.add(vec![wanted("b")]), 4, "not twice");
        queue.first(&["c".into(), "x".into(), "b".into()]);
        let order: Vec<String> = std::iter::from_fn(|| queue.next()).map(|w| w.id.to_string()).collect();
        assert_eq!(order, ["c", "b", "a", "d"]);
    }

    #[test]
    fn urls_titles_and_years() {
        let page = r#"<a href="?C=N;O=D">Name</a><a href="Mario%20_%20Luigi%20(USA).png">x</a><a href="Zelda.png">y</a>"#;
        assert_eq!(hrefs(page).collect::<Vec<_>>(), vec!["?C=N;O=D", "Mario _ Luigi (USA).png", "Zelda.png"]);
        assert_eq!(title_and_year("Cosmos (1980)"), ("Cosmos", Some(1980)));
        assert_eq!(title_and_year("Blade Runner 2049"), ("Blade Runner 2049", None));
        assert_eq!(title_and_year("Doom (Final)"), ("Doom (Final)", None));
        assert_eq!(year_in("17 Sep, 2020"), Some(2020));
        assert_eq!(year_in("2019-03-01"), Some(2019));
        assert_eq!(extension("https://x/a.PNG?t=1"), "png");
        assert_eq!(extension("https://x/a"), "jpg");
        assert_eq!(entities("Tom &amp; Jerry&#39;s"), "Tom & Jerry's");
        assert_eq!(How::parse("libretro:Sega - Mega Drive - Genesis").map(|h| h.source()).as_deref(), Some("libretro:Sega - Mega Drive - Genesis"));
        assert_eq!(How::parse("tmdb:anime"), None);
    }
}
