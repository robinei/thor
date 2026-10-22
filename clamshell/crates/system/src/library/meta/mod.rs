//! Fetching what the library's files don't say: covers, screenshots, what an entry is about, the year it
//! came out, who made it. After the library is read, the entries it wants (see [`Wanted`]) are fetched
//! on a few threads (curl), their images kept in the art cache (~/.cache/clamshell/art), what was found
//! kept beside them (also what wasn't: see [`crate::library::meta::cache::Cache`]) and sent to the state as it arrives.
//!
//! - A content dir's games (`"metadata": "libretro:<system>"` in library.json, the system as libretro names
//!   its folder): box art and a screenshot from libretro's thumbnails, and the developer and year from
//!   libretro's database. The game is found by its file's checksum (CRC32: a zip's from its directory, a file
//!   up to 64 MB read whole) in the database's No-Intro list, else by name: the entry's title as libretro
//!   names the game, or the same once case, bracketed tags (`(USA)`), "the" and punctuation are left out,
//!   `and` and `&` alike, and Roman numerals read as numbers, or (where only one game fits) the same but for
//!   libretro's subtitle after ` - `; of several releases, the USA one, then World, then Europe. (A subtitle
//!   of the title's own isn't left out: "Front Mission - Gun Hazard" is another game than "Front Mission".)
//! - A content dir's films or series (`"tmdb:movie"`, `"tmdb:tv"`): poster, backdrop, overview and year
//!   from TMDB, by title (a `(1980)` at its end the year, tried first: a series may be dated from another
//!   year than its pilot's); needs `keys.tmdb` in library.json.
//! - Steam's games: the store's description, developer, year and first screenshot (no key).
//! - The desktop files' games: a cover from SteamGridDB, by name; needs `keys.steamgriddb`.
//!
//! Art beside an entry's file comes first: what is fetched never replaces it.
//!
//! libretro's lists (a system's thumbnails, its database's files) are kept in ~/.cache/clamshell/libretro
//! and fetched again after [`LISTS_KEPT`]; an older copy serves while a fetch fails.

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

/// Fetches at once.
const THREADS: usize = 4;
/// How long a libretro list kept on disk is used before it is fetched again.
const LISTS_KEPT: std::time::Duration = std::time::Duration::from_secs(30 * 86_400);

/// A miss (nothing found) is tried again after this long (seconds).
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
    /// libretro's thumbnails of this system (its folder's name there).
    Libretro(String),
    /// TMDB, a film (`movie`) or a series (`tv`).
    Tmdb(String),
    /// Steam's store page of this app.
    Steam(u32),
    /// SteamGridDB, by name.
    SteamGridDb,
    /// A video's length (ffprobe's) and a frame of it (ffmpeg's, a quarter in): kept apart from the rest
    /// (see [`crate::store`]).
    Duration,
}

impl How {
    /// From library.json's `metadata` field: `libretro:<system>`, `tmdb:movie`, `tmdb:tv`, `steamgriddb`
    /// (`steam`, a Steam game's store page, is by its app id: see the library).
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

    /// As saved with what was found: fetched again when it changes.
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
    /// Its key: the art cache's files are named by it.
    pub key: String,
    pub title: String,
    /// Its file in its content dir, for its checksum.
    pub file: Option<PathBuf>,
    pub how: How,
    /// It has art of its own already (no cover wanted).
    pub art: bool,
}

/// What a fetch needs.
pub struct Fetch {
    pub wanted: Vec<Wanted>,
    pub keys: Keys,
    /// The art cache (what was fetched kept there too, see [`Cache`]).
    pub art: PathBuf,
}

/// The entries waiting to be fetched for, in order, and the fetching threads at work.
#[derive(Clone, Default)]
pub struct Queue {
    waiting: Arc<Mutex<VecDeque<Wanted>>>,
    threads: Arc<std::sync::atomic::AtomicUsize>,
}

impl Queue {
    /// These entries (those still waiting) first, in this order.
    pub fn first(&self, ids: &[Arc<str>]) {
        let Ok(mut q) = self.waiting.lock() else {
            return;
        };
        for id in ids.iter().rev() {
            if let Some(at) = q.iter().position(|w| w.id == *id)
                && let Some(w) = q.remove(at)
            {
                q.push_front(w);
            }
        }
    }

    /// `wanted` added at the end (those not waiting already); how many wait now.
    fn add(&self, wanted: Vec<Wanted>) -> usize {
        let Ok(mut q) = self.waiting.lock() else {
            return 0;
        };
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

/// Adds what `fetch` wants to `queue` and fetches on [`THREADS`] more threads until it is empty (or the
/// UI is gone): `Fetching(true)` as they start, `Fetching(false)` when the last thread is done.
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

/// What the threads share: the keys, where images go, and libretro's file lists (fetched once per
/// system and kind, `None` if that failed).
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

    /// One of libretro's lists (`name` in the lists' folder, next to the art cache), from `url`: kept on
    /// disk (see [`http::kept`]).
    fn list(&self, name: &str, url: &str) -> Option<String> {
        let dir = self.art.with_file_name("libretro");
        let file = dir.join(name.replace('/', "_"));
        http::kept(&file, LISTS_KEPT, || http::get(url, &[])).map(|b| String::from_utf8_lossy(&b).into_owned())
    }

    /// The image at `url` into the art cache as `<key>-<what>.<its extension>`.
    fn image(&self, w: &Wanted, what: &str, url: &str) -> Option<PathBuf> {
        let file = self.art.join(format!("{}-{what}.{}", w.key, extension(url)));
        http::download(url, &[], &file).then_some(file)
    }
}

/// An image URL's extension (jpg, png or webp), jpg where it says none of them.
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
