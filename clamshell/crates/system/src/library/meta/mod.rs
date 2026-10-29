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

use clamshell_state::{Event, Meta};

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
pub(crate) const RETRY_MISS: u64 = 30 * 86_400;

/// The services' keys, from library.json's `keys`.
pub use clamshell_library::config::Keys;

/// Where an entry's metadata comes from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum How {
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
    pub(crate) fn parse(field: &str) -> Option<How> {
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
    pub(crate) fn source(&self) -> String {
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
    pub(crate) id: Arc<str>,
    /// The entry's key, which names its files in the art cache.
    pub(crate) key: String,
    pub(crate) title: String,
    /// The entry's file in its content dir, for the checksum.
    pub(crate) file: Option<PathBuf>,
    pub(crate) how: How,
    /// Whether it has art of its own already, so no cover is wanted.
    pub(crate) art: bool,
}

/// What a fetch needs.
pub struct Fetch {
    pub wanted: Vec<Wanted>,
    /// The services' keys, from library.json; `None` keeps those the queue has, as a fetch for a folder
    /// browsed (which knows none) does.
    pub keys: Option<Keys>,
    /// The art cache, which also keeps what was fetched (see [`Cache`]).
    pub art: PathBuf,
}

/// The entries waiting to be fetched for, in order, those being fetched, and the fetching threads at work
/// ([`THREADS`] at most, whoever asked).
#[derive(Clone, Default)]
pub struct Queue {
    inner: Arc<Mutex<Inner>>,
    shared: Arc<Mutex<Option<Arc<Shared>>>>,
}

#[derive(Default)]
struct Inner {
    waiting: VecDeque<Wanted>,
    /// Being fetched now: not added again until done.
    fetching: Vec<Arc<str>>,
    threads: usize,
}

impl Queue {
    /// Moves these entries to the front, in this order, if they are still waiting.
    pub(crate) fn first(&self, ids: &[Arc<str>]) {
        let mut q = crate::os::lock(&self.inner);
        for id in ids.iter().rev() {
            if let Some(at) = q.waiting.iter().position(|w| w.id == *id)
                && let Some(w) = q.waiting.remove(at)
            {
                q.waiting.push_front(w);
            }
        }
    }

    /// Adds `wanted` at the end, skipping entries already waiting or being fetched. Returns how many wait
    /// now, and how many more threads to start.
    fn add(&self, wanted: Vec<Wanted>) -> (usize, usize) {
        let mut q = crate::os::lock(&self.inner);
        for w in wanted {
            if !q.waiting.iter().any(|x| x.id == w.id) && !q.fetching.contains(&w.id) {
                q.waiting.push_back(w);
            }
        }
        let more = THREADS.saturating_sub(q.threads).min(q.waiting.len());
        q.threads += more;
        (q.waiting.len(), more)
    }

    /// The next entry to fetch for, now being fetched; or, with none waiting, `None`, and the thread
    /// asking is done, counted off under the same lock as [`Queue::add`] counts threads on, so none is
    /// left waiting with no thread.
    fn next(&self, done: Option<&Arc<str>>) -> Option<Wanted> {
        let mut q = crate::os::lock(&self.inner);
        if let Some(id) = done {
            q.fetching.retain(|f| f != id);
        }
        match q.waiting.pop_front() {
            Some(w) => {
                q.fetching.push(w.id.clone());
                Some(w)
            }
            None => {
                q.threads = q.threads.saturating_sub(1);
                None
            }
        }
    }

    /// Whether no thread fetches.
    fn idle(&self) -> bool {
        crate::os::lock(&self.inner).threads == 0
    }

    /// What the threads share, with `keys` if given: kept across fetches, so libretro's lists are read
    /// once and a fetch without keys doesn't drop them.
    fn shared(&self, keys: Option<Keys>, art: PathBuf) -> Arc<Shared> {
        let mut shared = crate::os::lock(&self.shared);
        match (shared.as_ref(), keys) {
            (Some(s), None) => s.clone(),
            (Some(s), Some(keys)) if s.keys == keys && s.art == art => s.clone(),
            (_, keys) => {
                let keys = keys.or_else(|| shared.as_ref().map(|s| s.keys.clone())).unwrap_or_default();
                let made = Arc::new(Shared { keys, art, listings: Mutex::new(HashMap::new()), databases: Mutex::new(HashMap::new()) });
                *shared = Some(made.clone());
                made
            }
        }
    }
}

/// Adds what `fetch` wants to `queue` and fetches on threads, up to [`THREADS`] at once in all, until the
/// queue is empty or the UI is gone. Sends `Fetching(true)` as they start and `Fetching(false)` when the
/// last thread is done. A thread that can't start is logged; the others fetch its share.
pub fn spawn(fetch: Fetch, queue: &Queue, sink: impl Sink + Clone) {
    let shared = queue.shared(fetch.keys, fetch.art);
    let (count, more) = queue.add(fetch.wanted);
    if count == 0 {
        return;
    }
    log::info!("meta: fetching for {count} entries");
    let _ = sink.send(Event::Fetching(true));
    for _ in 0..more {
        let (queue, sink) = (queue.clone(), sink.clone());
        let (waiting, told) = (queue.clone(), sink.clone());
        let shared = shared.clone();
        let started = worker::detached("meta", move || {
            let cache = Cache::open(&shared.art);
            let fetch_one = |w: &Wanted| {
                let failures = http::network_failures();
                let meta = shared.fetch(w);
                // Nothing found because the network failed isn't a miss: it is fetched again next time.
                if meta == Meta::default() && http::network_failures() != failures {
                    log::info!("meta: {}: the network failed: not kept as a miss", w.id);
                    return Ok(());
                }
                let saved = match &w.how {
                    How::Duration => cache.save_probe(&w.id, meta.duration, meta.details.shot.as_deref()),
                    _ => cache.save_meta(&w.id, &w.how.source(), crate::time::clock::unix_secs(), &meta),
                };
                if let Err(e) = saved {
                    log::warn!("meta: {}: {e}", w.id);
                }
                if meta == Meta::default() { Ok(()) } else { sink.send(Event::Meta { id: w.id.clone(), meta }) }
            };
            fetch_all(&queue, fetch_one, || {
                log::info!("meta: done");
                let _ = sink.send(Event::Fetching(false));
            });
        });
        if let Err(e) = started {
            log::warn!("meta: a fetching thread: {e}");
            // Not counted. If no other runs, the fetch is over: what waits is fetched with the next.
            let _ = waiting.next(None);
            if waiting.idle() {
                let _ = told.send(Event::Fetching(false));
            }
        }
    }
}

/// A fetching thread's work: fetches each entry taken from `queue` with `fetch_one`, until the queue is
/// empty or the UI is gone (`Err`). A panic fetching one entry (the panic hook has logged it) costs that
/// entry's metadata, not the rest. `done` is called by the last thread to finish.
fn fetch_all(queue: &Queue, mut fetch_one: impl FnMut(&Wanted) -> Result<(), crate::Closed>, done: impl FnOnce()) {
    let mut last: Option<Arc<str>> = None;
    while let Some(w) = queue.next(last.as_ref()) {
        let fetched = worker::or_on_panic(
            || fetch_one(&w),
            || {
                log::error!("meta: {}: fetching panicked; it has no metadata fetched", w.id);
                Ok(())
            },
        );
        last = Some(w.id.clone());
        if fetched.is_err() {
            // The UI is gone: this thread stops, counted off.
            let mut q = crate::os::lock(&queue.inner);
            q.fetching.retain(|f| Some(f) != last.as_ref());
            q.threads = q.threads.saturating_sub(1);
            return;
        }
    }
    if queue.idle() {
        done();
    }
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
    fn what_library_view_shows_comes_first() {
        let wanted = |id: &str| Wanted { id: id.into(), key: id.into(), title: id.into(), file: None, how: How::SteamGridDb, art: false };
        let queue = Queue::default();
        assert_eq!(queue.add(["a", "b", "c", "d"].map(wanted).to_vec()), (4, THREADS));
        assert_eq!(queue.add(vec![wanted("b")]), (4, 0), "not twice, and no more threads than THREADS");
        queue.first(&["c".into(), "x".into(), "b".into()]);
        let c = queue.next(None).unwrap();
        assert_eq!(queue.add(vec![wanted("c")]).0, 3, "not while being fetched");
        let order: Vec<String> = std::iter::from_fn(|| queue.next(Some(&c.id))).map(|w| w.id.to_string()).collect();
        assert_eq!(order, ["b", "a", "d"]);
    }

    /// A panic fetching one entry costs that entry: the rest are fetched, and the last thread still says
    /// the fetch is done, once.
    #[test]
    fn a_panic_fetching_one_entry_costs_only_that_entry() {
        let wanted = |id: &str| Wanted { id: id.into(), key: id.into(), title: id.into(), file: None, how: How::SteamGridDb, art: false };
        let queue = Queue::default();
        queue.add(["a", "b", "c"].map(wanted).to_vec());
        crate::os::lock(&queue.inner).threads = 1;
        let mut fetched = Vec::new();
        let mut done = 0;
        fetch_all(
            &queue,
            |w| {
                assert_ne!(&*w.id, "b", "b panics");
                fetched.push(w.id.to_string());
                Ok(())
            },
            || done += 1,
        );
        assert_eq!(fetched, ["a", "c"]);
        assert_eq!(done, 1);
        assert!(queue.idle());
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
