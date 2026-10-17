//! Fetching what the library's files don't say: covers, screenshots, what an entry is about, the year it
//! came out, who made it. After the library is read, the entries it wants (see [`Wanted`]) are fetched
//! on a few threads (curl), their images kept in the art cache (~/.cache/clamshell/art), what was found
//! saved in the database (also what wasn't: see [`crate::store`]) and sent to the state as it arrives.
//!
//! - A content dir's games (`"metadata": "libretro:<system>"` in library.json, the system as libretro
//!   names its folder): box art and a screenshot from libretro's thumbnails, and the developer and year
//!   from libretro's database. The game is found by its file's checksum (CRC32: a zip's from its
//!   directory, a file up to 64 MB read whole) in the database's No-Intro list, else by name: the entry's title
//!   as libretro names the game, or the same once case, bracketed tags (`(USA)`), "the" and punctuation
//!   are left out, `and` and `&` alike, and Roman numerals read as numbers, or (where only one game fits) the same but for
//!   libretro's subtitle after ` - `; of several releases, the USA one, then World, then Europe. (A
//!   subtitle of the title's own isn't left out: "Front Mission - Gun Hazard" is another game than
//!   "Front Mission".)
//! - A content dir's films or series (`"tmdb:movie"`, `"tmdb:tv"`): poster, backdrop, overview and year
//!   from TMDB, by title (a `(1980)` at its end the year); needs `keys.tmdb` in library.json.
//! - Steam's games: the store's description, developer, year and first screenshot (no key).
//! - The desktop files' games: a cover from SteamGridDB, by name; needs `keys.steamgriddb`.
//!
//! Art beside an entry's file comes first: what is fetched never replaces it.

use std::collections::{HashMap, VecDeque};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::{Arc, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};

use clamshell_core::{Event, Meta};
use serde::Deserialize;
use serde_json::Value;

use crate::store::Store;
use crate::{Sink, worker};

/// Fetches at once.
const THREADS: usize = 4;
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
}

impl How {
    /// From library.json's `metadata` field: `libretro:<system>`, `tmdb:movie` or `tmdb:tv`.
    pub fn parse(field: &str) -> Option<How> {
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
    /// The art cache.
    pub art: PathBuf,
    /// The database.
    pub db: PathBuf,
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
        self.waiting.lock().ok()?.pop_front()
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
    let shared =
        Arc::new(Shared { keys: fetch.keys, art: fetch.art, db: fetch.db, listings: Mutex::new(HashMap::new()), databases: Mutex::new(HashMap::new()) });
    for _ in 0..THREADS {
        let (queue, shared, sink) = (queue.clone(), shared.clone(), sink.clone());
        queue.threads.fetch_add(1, Ordering::SeqCst);
        worker::detached("meta", move || {
            let store = Store::open(&shared.db).map_err(|e| log::warn!("meta: {e}")).ok();
            while let Some(w) = queue.next() {
                let meta = shared.fetch(&w);
                if let Some(store) = &store
                    && let Err(e) = store.save_meta(&w.id, &w.how.source(), now(), &meta)
                {
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

pub fn now() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_secs())
}

/// What the threads share: the keys, where images go, and libretro's file lists (fetched once per
/// system and kind, `None` if that failed).
struct Shared {
    keys: Keys,
    art: PathBuf,
    db: PathBuf,
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
        };
        log::info!("meta: {}: {}", w.id, if meta == Meta::default() { "nothing found" } else { "found" });
        meta
    }

    /// The image at `url` into the art cache as `<key>-<what>.<its extension>`.
    fn image(&self, w: &Wanted, what: &str, url: &str) -> Option<PathBuf> {
        let file = self.art.join(format!("{}-{what}.{}", w.key, extension(url)));
        download(url, &[], &file).then_some(file)
    }

    fn libretro(&self, w: &Wanted, system: &str) -> Meta {
        // By checksum: the game's name in the database, as its thumbnails are named.
        let database = self.database(system);
        let crc = w.file.as_deref().and_then(checksum);
        let known = crc.and_then(|c| database.names.get(&c));
        let by_name = || self.listing(system, "Named_Boxarts").and_then(|names| best_match(&w.title, &names).map(str::to_owned));
        let get = |c: Option<u32>, of: &HashMap<u32, String>| c.and_then(|c| of.get(&c)).cloned().unwrap_or_default();
        let mut meta = Meta { year: crc.and_then(|c| database.years.get(&c)).copied(), developer: get(crc, &database.developers), ..Meta::default() };
        // The checksum's name, else (none, or no pictures under it) the title's.
        let mut tried = None;
        for name in [known.map(|n| thumbnail_name(n)), None] {
            let Some(name) = name.or_else(|| by_name().filter(|n| tried.as_ref() != Some(n))) else {
                continue;
            };
            let url = |kind: &str| format!("{LIBRETRO}/{}/{kind}/{}.png", encode(system), encode(&name));
            meta.cover = (!w.art).then(|| self.image(w, "cover", &url("Named_Boxarts"))).flatten();
            meta.shot = self.image(w, "shot", &url("Named_Snaps"));
            if meta.cover.is_some() || meta.shot.is_some() {
                break;
            }
            tried = Some(name);
        }
        meta
    }

    /// libretro's database of `system`: its No-Intro list's names, and the developers and years, by
    /// checksum; fetched once (empty where it couldn't be).
    fn database(&self, system: &str) -> Arc<Database> {
        let Ok(mut databases) = self.databases.lock() else {
            return Arc::default();
        };
        if let Some(d) = databases.get(system) {
            return d.clone();
        }
        let dat = |dir: &str| {
            let url = format!("{DATABASE}/{dir}/{}.dat", encode(system));
            get(&url, &[]).map(|b| String::from_utf8_lossy(&b).into_owned()).unwrap_or_default()
        };
        let mut d = Database::default();
        for (crc, fields) in dat_games(&dat("no-intro")) {
            if let Some(name) = fields.get("name") {
                d.names.insert(crc, name.clone());
            }
        }
        for (crc, fields) in dat_games(&dat("developer")) {
            if let Some(dev) = fields.get("developer") {
                d.developers.insert(crc, dev.clone());
            }
        }
        for (crc, fields) in dat_games(&dat("releaseyear")) {
            if let Some(year) = fields.get("releaseyear").and_then(|y| y.parse().ok()) {
                d.years.insert(crc, year);
            }
        }
        log::info!("meta: libretro's database of {system}: {} games", d.names.len());
        let d = Arc::new(d);
        databases.insert(system.to_owned(), d.clone());
        d
    }

    /// The names (without `.png`) in a system's folder of a kind, fetched once.
    fn listing(&self, system: &str, kind: &str) -> Option<Arc<Vec<Name>>> {
        let id = format!("{system}/{kind}");
        let mut listings = self.listings.lock().ok()?;
        if let Some(l) = listings.get(&id) {
            return l.clone();
        }
        let page = get(&format!("{LIBRETRO}/{}/{kind}/", encode(system)), &[]).map(|b| String::from_utf8_lossy(&b).into_owned());
        let names = page.map(|p| Arc::new(hrefs(&p).filter_map(|h| h.strip_suffix(".png").map(Name::new)).collect::<Vec<_>>()));
        if names.is_none() {
            log::warn!("meta: libretro has no {id}");
        }
        listings.insert(id, names.clone());
        names
    }

    fn tmdb(&self, w: &Wanted, kind: &str) -> Meta {
        let Some(key) = self.keys.tmdb.as_deref() else {
            return Meta::default();
        };
        let (title, year) = title_and_year(&w.title);
        // A read access token (a JWT) goes in a header; an API key in the query.
        let bearer = key.starts_with("eyJ").then(|| format!("Authorization: Bearer {key}"));
        let mut url = format!("https://api.themoviedb.org/3/search/{kind}?include_adult=false&query={}", encode(title));
        if let Some(y) = year {
            url.push_str(&format!("&{}={y}", if kind == "tv" { "first_air_date_year" } else { "year" }));
        }
        if bearer.is_none() {
            url.push_str(&format!("&api_key={}", encode(key)));
        }
        let headers: Vec<&str> = bearer.iter().map(String::as_str).collect();
        let Some(hit) = get_json(&url, &headers).and_then(|v| v.get("results")?.get(0).cloned()) else {
            return Meta::default();
        };
        let path = |field: &str| hit.get(field).and_then(Value::as_str).filter(|p| !p.is_empty()).map(str::to_owned);
        let date = path("first_air_date").or_else(|| path("release_date")).unwrap_or_default();
        Meta {
            cover: path("poster_path").filter(|_| !w.art).and_then(|p| self.image(w, "cover", &format!("https://image.tmdb.org/t/p/w500{p}"))),
            shot: path("backdrop_path").and_then(|p| self.image(w, "shot", &format!("https://image.tmdb.org/t/p/w780{p}"))),
            summary: path("overview").unwrap_or_default(),
            year: year_in(&date),
            developer: String::new(),
        }
    }

    fn steam(&self, w: &Wanted, appid: u32) -> Meta {
        let url = format!("https://store.steampowered.com/api/appdetails?appids={appid}&l=english");
        let Some(data) = get_json(&url, &[]).and_then(|v| v.get(appid.to_string())?.get("data").cloned()) else {
            return Meta::default();
        };
        let text = |v: Option<&Value>| v.and_then(Value::as_str).map(entities).unwrap_or_default();
        let shot = data.get("screenshots").and_then(|s| s.get(0)?.get("path_full")?.as_str().map(str::to_owned));
        let cover = format!("https://shared.steamstatic.com/store_item_assets/steam/apps/{appid}/library_600x900.jpg");
        Meta {
            cover: (!w.art).then(|| self.image(w, "cover", &cover)).flatten(),
            shot: shot.and_then(|url| self.image(w, "shot", &url)),
            summary: text(data.get("short_description")),
            year: year_in(&text(data.get("release_date").and_then(|r| r.get("date")))),
            developer: text(data.get("developers").and_then(|d| d.get(0))),
        }
    }

    fn steamgriddb(&self, w: &Wanted) -> Meta {
        let (Some(key), false) = (self.keys.steamgriddb.as_deref(), w.art) else {
            return Meta::default();
        };
        let auth = format!("Authorization: Bearer {key}");
        let api = "https://www.steamgriddb.com/api/v2";
        let game = get_json(&format!("{api}/search/autocomplete/{}", encode(&w.title)), &[&auth]).and_then(|v| v.get("data")?.get(0)?.get("id")?.as_u64());
        let grid = game.and_then(|g| get_json(&format!("{api}/grids/game/{g}?dimensions=600x900"), &[&auth]));
        let url = grid.and_then(|v| v.get("data")?.get(0)?.get("url")?.as_str().map(str::to_owned));
        Meta { cover: url.and_then(|u| self.image(w, "cover", &u)), ..Meta::default() }
    }
}

const LIBRETRO: &str = "https://thumbnails.libretro.com";
const DATABASE: &str = "https://raw.githubusercontent.com/libretro/libretro-database/master/metadat";

/// A system's games in libretro's database, by their files' checksums.
#[derive(Debug, Default)]
struct Database {
    names: HashMap<u32, String>,
    developers: HashMap<u32, String>,
    years: HashMap<u32, u16>,
}

/// A clrmamepro DAT's games: each file's checksum (`rom ( ... crc XXXXXXXX ... )`) with its game's
/// fields (`name "..."`, `developer "..."`, ...; the first of each).
fn dat_games(text: &str) -> Vec<(u32, HashMap<String, String>)> {
    let mut out = Vec::new();
    let mut fields: HashMap<String, String> = HashMap::new();
    let mut crcs: Vec<u32> = Vec::new();
    let mut flush = |fields: &mut HashMap<String, String>, crcs: &mut Vec<u32>| {
        for crc in crcs.drain(..) {
            out.push((crc, fields.clone()));
        }
        fields.clear();
    };
    for line in text.lines().map(str::trim) {
        if line.starts_with("game (") {
            flush(&mut fields, &mut crcs);
        } else if let Some(rom) = line.strip_prefix("rom (") {
            let mut words = rom.split_whitespace();
            while let Some(w) = words.next() {
                if w == "crc"
                    && let Some(crc) = words.next().and_then(|c| u32::from_str_radix(c, 16).ok())
                {
                    crcs.push(crc);
                }
            }
        } else if let Some((key, rest)) = line.split_once(' ')
            && let Some(value) = rest.trim().strip_prefix('"').and_then(|v| v.strip_suffix('"'))
        {
            fields.entry(key.to_owned()).or_insert_with(|| value.to_owned());
        }
    }
    flush(&mut fields, &mut crcs);
    out
}

/// A game's name as libretro's thumbnails name it: the characters a file name can't have as `_`.
fn thumbnail_name(name: &str) -> String {
    name.chars().map(|c| if "&*/:`<>?\\|\"".contains(c) { '_' } else { c }).collect()
}

/// The largest a file read whole for its checksum may be (bytes): a cartridge's, not a disc's.
const CHECKSUM_MAX: u64 = 64 << 20;

/// A game file's checksum (CRC32): a zip's largest file's, from the zip's directory; a file up to
/// [`CHECKSUM_MAX`], read whole.
fn checksum(file: &Path) -> Option<u32> {
    if file.extension().is_some_and(|e| e.eq_ignore_ascii_case("zip")) {
        return zip_tail(file).and_then(|(tail, base)| zip_crc(&tail, base));
    }
    if std::fs::metadata(file).ok()?.len() > CHECKSUM_MAX || !file.is_file() {
        return None;
    }
    Some(crc32(&std::fs::read(file).ok()?))
}

/// The end of a zip (its last MiB: the central directory of a few files is there), and where in the file
/// it starts.
fn zip_tail(file: &Path) -> Option<(Vec<u8>, usize)> {
    use std::io::{Read, Seek, SeekFrom};
    let mut f = std::fs::File::open(file).ok()?;
    let len = f.metadata().ok()?.len();
    let base = len.saturating_sub(1 << 20);
    f.seek(SeekFrom::Start(base)).ok()?;
    let mut tail = Vec::new();
    f.read_to_end(&mut tail).ok()?;
    Some((tail, usize::try_from(base).ok()?))
}

/// The CRC32 of a zip's largest file, from its central directory: `zip` the end of the file, from
/// `base` in it.
fn zip_crc(zip: &[u8], base: usize) -> Option<u32> {
    let u16_at = |at: usize| zip.get(at..at + 2).and_then(|b| <[u8; 2]>::try_from(b).ok()).map(u16::from_le_bytes);
    let u32_at = |at: usize| zip.get(at..at + 4).and_then(|b| <[u8; 4]>::try_from(b).ok()).map(u32::from_le_bytes);
    // The end of the central directory: its signature, within the last 64 KiB (after it, a comment).
    let from = zip.len().saturating_sub(65_557);
    let end = (from..zip.len().saturating_sub(21)).rev().find(|&i| u32_at(i) == Some(0x0605_4b50))?;
    let count = usize::from(u16_at(end + 10)?);
    let mut at = usize::try_from(u32_at(end + 16)?).ok()?.checked_sub(base)?;
    let mut best: Option<(u32, u32)> = None;
    for _ in 0..count {
        if u32_at(at)? != 0x0201_4b50 {
            return None;
        }
        let (crc, size) = (u32_at(at + 16)?, u32_at(at + 24)?);
        if best.is_none_or(|(_, s)| size > s) {
            best = Some((crc, size));
        }
        at += 46 + usize::from(u16_at(at + 28)?) + usize::from(u16_at(at + 30)?) + usize::from(u16_at(at + 32)?);
    }
    best.map(|(crc, _)| crc)
}

/// CRC32 (as zip and No-Intro have it).
fn crc32(data: &[u8]) -> u32 {
    let mut table = [0u32; 256];
    for (i, t) in (0u32..).zip(table.iter_mut()) {
        *t = (0..8).fold(i, |c, _| if c & 1 == 1 { 0xEDB8_8320 ^ (c >> 1) } else { c >> 1 });
    }
    !data.iter().fold(!0u32, |c, &b| table.get(usize::from((c as u8) ^ b)).copied().unwrap_or(0) ^ (c >> 8))
}

/// The body of `url` (a GET, redirects followed), if it answered 2xx.
fn get(url: &str, headers: &[&str]) -> Option<Vec<u8>> {
    let mut curl = Command::new("curl");
    curl.args(["-sfL", "--max-time", "30", "-A", "ClamShell"]);
    for h in headers {
        curl.args(["-H", h]);
    }
    let out = curl.arg(url).stdin(Stdio::null()).stderr(Stdio::null()).output().ok()?;
    out.status.success().then_some(out.stdout)
}

fn get_json(url: &str, headers: &[&str]) -> Option<Value> {
    serde_json::from_slice(&get(url, headers)?).ok()
}

/// `url` into `file` (written whole, then renamed into place); whether it worked.
fn download(url: &str, headers: &[&str], file: &Path) -> bool {
    let Some(body) = get(url, headers).filter(|b| !b.is_empty()) else {
        return false;
    };
    if let Some(dir) = file.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    let part = file.with_extension("part");
    std::fs::write(&part, body).and_then(|()| std::fs::rename(&part, file)).map_err(|e| log::warn!("meta: {}: {e}", file.display())).is_ok()
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

/// `text` for a URL's path or query: everything but letters, digits and `-._~` percent-encoded.
fn encode(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for b in text.bytes() {
        if b.is_ascii_alphanumeric() || b"-._~".contains(&b) {
            out.push(char::from(b));
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

/// Percent-encoding undone.
fn decode(text: &str) -> String {
    let bytes = text.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while let Some(&b) = bytes.get(i) {
        let hex = bytes.get(i + 1..i + 3).and_then(|h| std::str::from_utf8(h).ok()).and_then(|h| u8::from_str_radix(h, 16).ok());
        match (b, hex) {
            (b'%', Some(v)) => {
                out.push(v);
                i += 3;
            }
            _ => {
                out.push(b);
                i += 1;
            }
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// The links of a directory listing's page, decoded.
fn hrefs(page: &str) -> impl Iterator<Item = String> + '_ {
    page.split("href=\"").skip(1).filter_map(|rest| rest.split('"').next()).map(decode)
}

/// A title as compared with libretro's names: bracketed parts, a ` # ...` tail, "the" and "and", case and
/// anything but letters and digits left out, Roman numerals (II to X) as numbers.
fn normal(title: &str) -> String {
    // (A ` # ...` tail: a translation's, "Star Ocean # SNES".)
    let title = title.split_once(" # ").map_or(title, |(t, _)| t);
    let mut kept = String::with_capacity(title.len());
    let mut depth = 0u32;
    for c in title.chars() {
        match c {
            '(' | '[' => depth += 1,
            ')' | ']' => depth = depth.saturating_sub(1),
            c if depth == 0 => kept.push(c),
            _ => {}
        }
    }
    let numeral = |w: &str| ["ii", "iii", "iv", "v", "vi", "vii", "viii", "ix", "x"].iter().position(|n| *n == w).map(|i| (i + 2).to_string());
    kept.to_lowercase()
        .split(|c: char| !c.is_alphanumeric())
        .filter(|w| !w.is_empty() && *w != "the" && *w != "and")
        .map(|w| numeral(w).unwrap_or_else(|| w.to_owned()))
        .collect()
}

/// A title without its subtitle (what follows ` - `), if it has one.
fn main_title(title: &str) -> Option<&str> {
    title.split_once(" - ").map(|(main, _)| main)
}

/// A libretro name, and it as [`normal`] compares it, whole and without its subtitle.
#[derive(Debug, Clone)]
struct Name {
    name: String,
    normal: String,
    main: Option<String>,
}

impl Name {
    fn new(name: &str) -> Name {
        Name { name: name.to_owned(), normal: normal(name), main: main_title(name).map(normal) }
    }
}

/// libretro's name for `title`: the same name (its characters libretro can't have in a file name as
/// `_`); else of the releases whose name is the same once [`normal`] (or failing that, where one game
/// alone fits, the same but for libretro's subtitle): USA, then World, then Europe, then any other, a
/// prototype or demo last.
fn best_match<'a>(title: &str, names: &'a [Name]) -> Option<&'a str> {
    let exact = thumbnail_name(title);
    if let Some(n) = names.iter().find(|n| n.name == exact) {
        return Some(&n.name);
    }
    let wanted = normal(title);
    if wanted.is_empty() {
        return None;
    }
    let rank = |n: &&Name| {
        let n = &n.name;
        let unfinished = ["(Beta", "(Proto", "(Demo", "(Sample", "(Pirate", "(Unl"].iter().any(|t| n.contains(t));
        let region = ["(USA", "(World", "(Europe"].iter().position(|r| n.contains(r)).unwrap_or(3);
        (unfinished, region, n.len())
    };
    let best = |found: Vec<&'a Name>| found.into_iter().min_by_key(rank).map(|n| n.name.as_str());
    let same: Vec<&Name> = names.iter().filter(|n| n.normal == wanted).collect();
    if !same.is_empty() {
        return best(same);
    }
    // libretro's with a subtitle the title doesn't have, and only one game it can be.
    let loose: Vec<&Name> = names.iter().filter(|n| n.main.as_ref() == Some(&wanted)).collect();
    let games: std::collections::HashSet<&str> = loose.iter().map(|n| n.normal.as_str()).collect();
    if games.len() == 1 { best(loose) } else { None }
}

/// A title and the year at its end in brackets, if any: "Cosmos (1980)".
fn title_and_year(title: &str) -> (&str, Option<u16>) {
    let t = title.trim_end();
    if let Some(open) = t.strip_suffix(')').and_then(|t| t.rfind('(')) {
        let year = t.get(open + 1..t.len() - 1).and_then(|y| y.parse::<u16>().ok()).filter(|y| (1870..2200).contains(y));
        if year.is_some() {
            return (t.get(..open).unwrap_or(t).trim_end(), year);
        }
    }
    (t, None)
}

/// The first four-digit year in `text` ("2020-09-17", "17 Sep, 2020").
fn year_in(text: &str) -> Option<u16> {
    text.split(|c: char| !c.is_ascii_digit()).find(|w| w.len() == 4).and_then(|w| w.parse().ok())
}

/// HTML's common entities undone (Steam's texts have them).
fn entities(text: &str) -> String {
    text.replace("&quot;", "\"").replace("&#39;", "'").replace("&apos;", "'").replace("&lt;", "<").replace("&gt;", ">").replace("&amp;", "&")
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::indexing_slicing)]
mod tests {
    use super::*;

    fn names(list: &[&str]) -> Vec<Name> {
        list.iter().map(|s| Name::new(s)).collect()
    }

    #[test]
    fn libretro_names_by_title() {
        let list = names(&[
            "Legend of Zelda, The - A Link to the Past (Europe)",
            "Legend of Zelda, The - A Link to the Past (USA)",
            "Legend of Zelda, The - A Link to the Past (USA) (Beta)",
            "Mario _ Luigi (USA)",
            "Super Metroid (Japan, USA) (En,Ja)",
        ]);
        assert_eq!(
            best_match("Legend of Zelda, The - A Link to the Past (Europe)", &list),
            Some("Legend of Zelda, The - A Link to the Past (Europe)"),
            "the same name"
        );
        assert_eq!(
            best_match("The Legend of Zelda - A Link to the Past", &list),
            Some("Legend of Zelda, The - A Link to the Past (USA)"),
            "USA first, no beta"
        );
        assert_eq!(best_match("Mario & Luigi", &list), Some("Mario _ Luigi (USA)"));
        assert_eq!(best_match("super metroid", &list), Some("Super Metroid (Japan, USA) (En,Ja)"));
        assert_eq!(best_match("Zelda", &list), None, "nothing loosely");
        assert_eq!(best_match("(USA)", &list), None);
        let list = names(&[
            "Mega Man 7 (USA)",
            "Musya - The Classic Japanese Tale of Horror (USA)",
            "Street Fighter II (USA)",
            "Final Fantasy - Mystic Quest (USA)",
            "Final Fantasy II (USA)",
            "Final Fantasy III (USA)",
            "Super Mario World (USA)",
            "Super Mario World 2 - Yoshi's Island (USA)",
        ]);
        assert_eq!(best_match("Mega Man VII (U)", &list), Some("Mega Man 7 (USA)"), "a numeral as a number");
        assert_eq!(best_match("Musya (U) [!]", &list), Some("Musya - The Classic Japanese Tale of Horror (USA)"), "its subtitle left out");
        assert_eq!(best_match("Street Fighter II - The World Warrior (U) [!]", &list), None, "a subtitle of its own: maybe another game");
        assert_eq!(best_match("Final Fantasy (U)", &list), Some("Final Fantasy - Mystic Quest (USA)"), "the one game it can be");
        assert_eq!(best_match("Rocky and Bullwinkle", &names(&["Rocky & Bullwinkle (USA)"])), Some("Rocky & Bullwinkle (USA)"), "and, &");
        assert_eq!(best_match("Star Ocean # SNES", &names(&["Star Ocean (Japan)"])), Some("Star Ocean (Japan)"), "a translation's tail");
        assert_eq!(best_match("Super Mario World 2 (U)", &list), Some("Super Mario World 2 - Yoshi's Island (USA)"));
        let two = names(&["Toys - Let the Toy Wars Begin! (USA)", "Toys - Something Else (USA)"]);
        assert_eq!(best_match("Toys", &two), None, "two games it could be: neither");
    }

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
    fn checksums_and_databases() {
        assert_eq!(crc32(b"123456789"), 0xCBF4_3926);
        // A zip of two files (stored), made by hand: its directory's checksum of the larger.
        let mut zip = Vec::new();
        let mut directory = Vec::new();
        for (name, data) in [("a.txt", &b"hi"[..]), ("game.sfc", &b"123456789"[..])] {
            let offset = zip.len() as u32;
            let crc = crc32(data);
            let header = |sig: u32, extra: &[u8]| {
                let mut h = sig.to_le_bytes().to_vec();
                h.extend_from_slice(extra);
                h
            };
            let mut local = header(0x0403_4b50, &[20, 0, 0, 0, 0, 0, 0, 0, 0, 0]);
            local.extend_from_slice(&crc.to_le_bytes());
            local.extend_from_slice(&(data.len() as u32).to_le_bytes());
            local.extend_from_slice(&(data.len() as u32).to_le_bytes());
            local.extend_from_slice(&(name.len() as u16).to_le_bytes());
            local.extend_from_slice(&[0, 0]);
            local.extend_from_slice(name.as_bytes());
            local.extend_from_slice(data);
            zip.extend_from_slice(&local);
            let mut central = header(0x0201_4b50, &[20, 0, 20, 0, 0, 0, 0, 0, 0, 0, 0, 0]);
            central.extend_from_slice(&crc.to_le_bytes());
            central.extend_from_slice(&(data.len() as u32).to_le_bytes());
            central.extend_from_slice(&(data.len() as u32).to_le_bytes());
            central.extend_from_slice(&(name.len() as u16).to_le_bytes());
            central.extend_from_slice(&[0; 12]);
            central.extend_from_slice(&offset.to_le_bytes());
            central.extend_from_slice(name.as_bytes());
            directory.extend_from_slice(&central);
        }
        let start = zip.len() as u32;
        zip.extend_from_slice(&directory);
        zip.extend_from_slice(&0x0605_4b50u32.to_le_bytes());
        zip.extend_from_slice(&[0, 0, 0, 0, 2, 0, 2, 0]);
        zip.extend_from_slice(&(directory.len() as u32).to_le_bytes());
        zip.extend_from_slice(&start.to_le_bytes());
        zip.extend_from_slice(&[0, 0]);
        assert_eq!(zip_crc(&zip, 0), Some(0xCBF4_3926));
        let tail = zip.get(10..).unwrap().to_vec();
        assert_eq!(zip_crc(&tail, 10), Some(0xCBF4_3926), "its end only");
        assert_eq!(zip_crc(b"not a zip", 0), None);

        let dat = "clrmamepro (\n\tname \"x\"\n)\n\ngame (\n\tname \"Legend of Zelda, The (USA)\"\n\trom ( name \"z.sfc\" size 1 crc 777E4BC0 md5 00 )\n)\n\ngame (\n\tcomment \"Ys (USA)\"\n\tdeveloper \"Falcom\"\n\trom ( crc 0000000A )\n)\n";
        let games = dat_games(dat);
        assert_eq!(games.len(), 2);
        assert_eq!(games[0].0, 0x777E_4BC0);
        assert_eq!(games[0].1.get("name").map(String::as_str), Some("Legend of Zelda, The (USA)"));
        assert_eq!(games[1].1.get("developer").map(String::as_str), Some("Falcom"));
        assert_eq!(thumbnail_name("Battletoads & Double Dragon (USA)"), "Battletoads _ Double Dragon (USA)");
    }

    #[test]
    fn urls_titles_and_years() {
        assert_eq!(encode("Nintendo - SNES/A&B.png"), "Nintendo%20-%20SNES%2FA%26B.png");
        assert_eq!(decode("A%20B%26C.png"), "A B&C.png");
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
